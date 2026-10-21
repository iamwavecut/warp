//! Protocol negotiation for user-configured MCP endpoints. No Warp service dependency.
use futures::{StreamExt as _, TryStreamExt as _};
use rmcp::model::{
    ClientConfig, ClientJsonRpcMessage, ClientRequest, DiscoverRequest, DiscoverRequestParams,
    ErrorCode, InitializeRequest, ProtocolVersion, RequestId, RequestMetaObject,
    ServerJsonRpcMessage, ServerResult,
};
use rmcp::service::{ClientInitializeError, ClientLifecycleMode, select_protocol_version};
use std::time::Duration;
type ReqwestHttpTransport = rmcp::transport::StreamableHttpClientTransport<reqwest::Client>;
pub fn supported_versions(legacy_only: bool) -> Vec<ProtocolVersion> {
    ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .rev()
        .filter(|version| !legacy_only || version.has_initialize())
        .cloned()
        .collect()
}

#[derive(Clone)]
pub struct NegotiatedProtocol {
    pub version: ProtocolVersion,
    discover: bool,
}

impl NegotiatedProtocol {
    pub fn lifecycle(&self) -> ClientLifecycleMode {
        if self.discover {
            ClientLifecycleMode::Discover {
                preferred_versions: vec![self.version.clone()],
            }
        } else {
            ClientLifecycleMode::Initialize
        }
    }
}
pub struct PreflightResponse {
    pub status: reqwest::StatusCode,
    pub protocol: NegotiatedProtocol,
    /// The `WWW-Authenticate` challenges the server sent. Retained because
    /// they are the only reliable way to tell an OAuth-protected resource apart
    /// from a server that merely rejected the credential we sent.
    pub www_authenticate: Vec<String>,
}

#[allow(clippy::result_large_err)]
pub async fn negotiate_protocol(
    url: &str,
    client: &reqwest::Client,
    info: ClientConfig,
    bearer_token: Option<&str>,
) -> Result<PreflightResponse, rmcp::RmcpError> {
    let mut protocol = NegotiatedProtocol {
        version: ProtocolVersion::LATEST,
        discover: true,
    };
    let mut attempted = Vec::new();
    loop {
        attempted.push(protocol.version.clone());
        let (mut preflight, message) =
            send_preflight_request(url, client, &info, bearer_token, protocol.clone()).await?;
        if (!protocol.discover
            && matches!(
                preflight.status,
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED
            ))
            || preflight.status == reqwest::StatusCode::UNAUTHORIZED
            || preflight.status == reqwest::StatusCode::FORBIDDEN
            || preflight.status.is_server_error()
        {
            return Ok(preflight);
        }
        match message {
            Some(ServerJsonRpcMessage::Response(response)) => {
                if response.id != RequestId::Number(0) {
                    return Err(ClientInitializeError::ConflictInitResponseId(
                        RequestId::Number(0),
                        response.id,
                    )
                    .into());
                }
                if !preflight.status.is_success() {
                    return Ok(preflight);
                }
                let server_supported = match response.result {
                    ServerResult::InitializeResult(result) if !protocol.discover => {
                        vec![result.protocol_version]
                    }
                    ServerResult::DiscoverResult(result) if protocol.discover => {
                        result.supported_versions
                    }
                    other => {
                        return Err(ClientInitializeError::ExpectedInitResult(Some(other)).into());
                    }
                };
                preflight.protocol.version = select_protocol_version(
                    &supported_versions(!protocol.discover),
                    &server_supported,
                )
                .ok_or_else(|| {
                    ClientInitializeError::NoCompatibleProtocolVersion {
                        client_supported: supported_versions(!protocol.discover),
                        server_supported,
                    }
                })?;
                return Ok(preflight);
            }
            Some(ServerJsonRpcMessage::Error(error)) => {
                if let Some(id) = error.id
                    && id != RequestId::Number(0)
                {
                    return Err(ClientInitializeError::UncorrelatedErrorResponse {
                        expected: RequestId::Number(0),
                        received: id,
                    }
                    .into());
                }
                let error = error.error;
                let server_supported = error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("supported"))
                    .and_then(|value| {
                        serde_json::from_value::<Vec<ProtocolVersion>>(value.clone()).ok()
                    });
                if error.code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION
                    || (!protocol.discover
                        && error.code == ErrorCode::INVALID_PARAMS
                        && server_supported.is_some())
                {
                    let server_supported = server_supported.unwrap_or_default();
                    let candidates = supported_versions(!protocol.discover)
                        .into_iter()
                        .filter(|candidate| !attempted.contains(candidate))
                        .collect::<Vec<_>>();
                    protocol.version = select_protocol_version(&candidates, &server_supported)
                        .ok_or_else(|| ClientInitializeError::NoCompatibleProtocolVersion {
                            client_supported: supported_versions(!protocol.discover),
                            server_supported,
                        })?;
                    continue;
                }
                if !protocol.discover
                    || matches!(
                        error.code,
                        ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY | ErrorCode::HEADER_MISMATCH
                    )
                {
                    return Err(ClientInitializeError::JsonRpcError(error).into());
                }
            }
            None if !protocol.discover && !preflight.status.is_success() => return Ok(preflight),
            None if !preflight.status.is_client_error() => {
                return Err(rmcp::RmcpError::transport_creation::<ReqwestHttpTransport>(
                    "MCP preflight did not return a JSON-RPC response",
                ));
            }
            None => {}
            Some(other) => {
                return Err(ClientInitializeError::ExpectedInitResponse(Some(other)).into());
            }
        }
        // A legacy signal, not a recognized modern version error, switches lifecycle.
        protocol = NegotiatedProtocol {
            version: ProtocolVersion::LATEST_WITH_INITIALIZE,
            discover: false,
        };
        attempted.clear();
    }
}

#[allow(clippy::result_large_err)]
async fn send_preflight_request(
    url: &str,
    client: &reqwest::Client,
    info: &ClientConfig,
    bearer_token: Option<&str>,
    protocol: NegotiatedProtocol,
) -> Result<(PreflightResponse, Option<ServerJsonRpcMessage>), rmcp::RmcpError> {
    use rmcp::transport::common::http_header::{
        EVENT_STREAM_MIME_TYPE, HEADER_MCP_METHOD, HEADER_MCP_PROTOCOL_VERSION, JSON_MIME_TYPE,
    };
    let info = info.clone().with_protocol_version(protocol.version.clone());
    let request = if protocol.discover {
        let mut request = DiscoverRequest::new(DiscoverRequestParams {});
        request
            .extensions
            .insert(RequestMetaObject::with_client_context(
                protocol.version.clone(),
                info.client_info,
                info.capabilities,
            ));
        ClientRequest::DiscoverRequest(request)
    } else {
        ClientRequest::InitializeRequest(InitializeRequest::new(info))
    };
    let message = ClientJsonRpcMessage::request(request, RequestId::Number(0));

    let mut request = client
        .post(url)
        .timeout(Duration::from_secs(10))
        .header(HEADER_MCP_PROTOCOL_VERSION, protocol.version.as_str())
        .header(
            http::header::ACCEPT,
            [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "),
        )
        .json(&message);
    if protocol.discover {
        request = request.header(HEADER_MCP_METHOD, "server/discover");
    }

    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(rmcp::RmcpError::transport_creation::<ReqwestHttpTransport>)?;

    let www_authenticate = response
        .headers()
        .get_all(http::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(ToString::to_string)
        .collect();

    let preflight = PreflightResponse {
        status: response.status(),
        protocol,
        www_authenticate,
    };
    if preflight.status == reqwest::StatusCode::UNAUTHORIZED
        || preflight.status == reqwest::StatusCode::FORBIDDEN
        || preflight.status.is_server_error()
    {
        return Ok((preflight, None));
    }
    let message = read_preflight_response(response)
        .await
        .map_err(rmcp::RmcpError::transport_creation::<ReqwestHttpTransport>)?;
    Ok((preflight, message))
}

async fn read_preflight_response(
    response: reqwest::Response,
) -> std::io::Result<Option<ServerJsonRpcMessage>> {
    let is_sse = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next() == Some("text/event-stream"));
    let mut remaining: usize = 1024 * 1024;
    let stream = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        remaining = remaining
            .checked_sub(chunk.len())
            .ok_or_else(|| std::io::Error::other("MCP preflight response exceeds 1 MiB"))?;
        Ok::<_, std::io::Error>(chunk)
    });
    if is_sse {
        let mut events = Box::pin(sse_stream::SseStream::from_bytes_stream(stream));
        while let Some(event) = events.try_next().await.map_err(std::io::Error::other)? {
            if let Some(data) = event.data
                && !data.is_empty()
            {
                let message = serde_json::from_str(&data).map_err(std::io::Error::other)?;
                if matches!(
                    message,
                    ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
                ) {
                    return Ok(Some(message));
                }
            }
        }
        Ok(None)
    } else {
        let mut stream = Box::pin(stream);
        let mut body = Vec::new();
        while let Some(chunk) = stream.try_next().await? {
            body.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&body).ok())
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
