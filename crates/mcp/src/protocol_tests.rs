use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use rmcp::model::{DiscoverResult, ErrorCode, ServerCapabilities, Tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
async fn serve_http_router(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("fake MCP server should bind");
    let url = format!(
        "http://{}/mcp",
        listener
            .local_addr()
            .expect("fake MCP address should resolve")
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("fake MCP server should run");
    });
    (url, server)
}

#[derive(Clone, Copy)]
enum FixtureProtocol {
    Discover,
    InitializeCounteroffer,
    InitializeVersionError,
}

fn rpc_error(request: &Value, code: ErrorCode, data: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request["id"],
        "error": { "code": code, "message": "Request rejected", "data": data },
    })
}

fn make_client_info() -> rmcp::model::ClientConfig {
    rmcp::model::ClientConfig::new(
        Default::default(),
        rmcp::model::Implementation::new("local-test", "1"),
    )
}
fn client(headers: &HashMap<String, String>) -> reqwest::Client {
    reqwest::Client::builder()
        .default_headers(headers.try_into().unwrap())
        .build()
        .unwrap()
}
async fn preflight(
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<super::PreflightResponse, rmcp::RmcpError> {
    super::negotiate_protocol(url, &client(headers), make_client_info(), None).await
}
struct TestServerInfo {
    service: rmcp::service::RunningService<
        rmcp::RoleClient,
        Box<dyn rmcp::service::DynService<rmcp::RoleClient>>,
    >,
    tools: Vec<Tool>,
}
async fn start_http_server(
    url: String,
    headers: HashMap<String, String>,
) -> Result<TestServerInfo, rmcp::RmcpError> {
    use rmcp::ServiceExt as _;
    use rmcp::service::ClientServiceExt as _;
    tokio::time::timeout(Duration::from_secs(5), async {
        let preflight = preflight(&url, &headers).await?;
        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            client(&headers),
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url,
            ),
        );
        let service = make_client_info()
            .with_protocol_version(preflight.protocol.version.clone())
            .into_dyn()
            .serve_with_lifecycle(transport, preflight.protocol.lifecycle())
            .await?;
        let tools = service.list_all_tools().await?;
        Ok(TestServerInfo { service, tools })
    })
    .await
    .expect("local MCP startup should complete")
}
fn caps(tools: bool, _resources: bool) -> ServerCapabilities {
    if tools {
        ServerCapabilities::builder().enable_tools().build()
    } else {
        ServerCapabilities::default()
    }
}
fn test_tool(name: &str) -> Tool {
    serde_json::from_value(json!({"name":name,"inputSchema":{"type":"object"}})).unwrap()
}
async fn assert_http_startup(
    protocol: FixtureProtocol,
    version: &'static str,
    sse_responses: bool,
    static_credential: bool,
) -> Vec<Value> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let app = Router::new().route("/mcp", post(move |headers: HeaderMap, Json(request): Json<Value>| {
        captured.lock().expect("request capture").push(request.clone());
        async move {
            if static_credential && headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("test-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let method = request["method"].as_str().expect("request method");
            let result = match method {
                "server/discover" => {
                    let requested = &request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"];
                    if matches!(protocol, FixtureProtocol::InitializeCounteroffer) {
                        rpc_error(&request, ErrorCode::METHOD_NOT_FOUND, None)
                    } else if requested != version {
                        rpc_error(&request, ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                            Some(json!({ "requested": requested, "supported": [version] })))
                    } else if matches!(protocol, FixtureProtocol::Discover) {
                        let result = DiscoverResult::new(
                            vec![serde_json::from_value(json!(version)).expect("fixture version")],
                            caps(true, false),
                        );
                        json!({ "jsonrpc": "2.0", "id": request["id"], "result": result })
                    } else {
                        rpc_error(&request, ErrorCode::METHOD_NOT_FOUND, None)
                    }
                }
                "initialize" => {
                    if matches!(protocol, FixtureProtocol::Discover) {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    if matches!(protocol, FixtureProtocol::InitializeVersionError)
                        && request["params"]["protocolVersion"] != version {
                        rpc_error(&request, ErrorCode::INVALID_PARAMS, Some(json!({ "supported": [version] })))
                    } else {
                        json!({
                            "jsonrpc": "2.0", "id": request["id"],
                            "result": {
                                "protocolVersion": version, "capabilities": { "tools": {} },
                                "serverInfo": { "name": "legacy", "version": "1.0" },
                            },
                        })
                    }
                }
                "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
                "tools/list" => {
                    if headers.get("mcp-protocol-version").and_then(|v| v.to_str().ok()) != Some(version) {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    if matches!(protocol, FixtureProtocol::Discover)
                        && (request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] != version
                            || !request["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object()
                            || (version == "2026-07-28" && headers.get("mcp-method").and_then(|v| v.to_str().ok()) != Some("tools/list")))
                    {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    json!({ "jsonrpc": "2.0", "id": request["id"], "result": { "tools": [test_tool("query_metrics")] } })
                }
                _ => return StatusCode::BAD_REQUEST.into_response(),
            };
            if sse_responses {
                (
                    [(http::header::CONTENT_TYPE, "text/event-stream")],
                    format!(": keepalive\n\nevent: message\ndata: {result}\n\n"),
                ).into_response()
            } else {
                Json(result).into_response()
            }
        }
    }));
    let (url, server) = serve_http_router(app).await;
    let headers = if static_credential {
        HashMap::from([("X-Api-Key".to_string(), "test-key".to_string())])
    } else {
        HashMap::new()
    };
    let info = start_http_server(url, headers).await;
    server.abort();
    let info = info.expect("Warp startup should succeed");
    assert_eq!(info.tools, vec![test_tool("query_metrics")]);
    assert_eq!(
        info.service
            .peer_info()
            .expect("server info")
            .protocol_version
            .as_str(),
        version
    );
    info.service.cancel().await.expect("cancel service");
    requests.lock().expect("captured requests").clone()
}

#[tokio::test]
async fn http_startup_uses_modern_discovery_and_request_metadata() {
    let requests = assert_http_startup(FixtureProtocol::Discover, "2026-07-28", false, false).await;
    assert!(
        !requests
            .iter()
            .any(|r| r["method"] == "initialize" || r["method"] == "notifications/initialized")
    );
}

#[tokio::test]
async fn http_startup_retries_discovery_at_server_supported_revision_without_initializing() {
    let requests = assert_http_startup(FixtureProtocol::Discover, "2025-11-25", false, false).await;
    assert!(!requests.iter().any(|r| r["method"] == "initialize"));
    assert_eq!(
        requests[0]["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
    assert_eq!(
        requests[1]["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        "2025-11-25"
    );
}

#[tokio::test]
async fn http_startup_falls_back_and_propagates_sse_version_error_selection_with_static_headers() {
    let requests = assert_http_startup(
        FixtureProtocol::InitializeVersionError,
        "2025-06-18",
        true,
        true,
    )
    .await;
    let initialize = requests
        .iter()
        .filter(|r| r["method"] == "initialize")
        .collect::<Vec<_>>();
    assert_eq!(initialize.len(), 3);
    assert_eq!(initialize[0]["params"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialize[1]["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(initialize[2]["params"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn http_startup_passes_legacy_counteroffer_to_real_sdk_connection() {
    let requests = assert_http_startup(
        FixtureProtocol::InitializeCounteroffer,
        "2025-03-26",
        false,
        false,
    )
    .await;
    let initialize = requests
        .iter()
        .filter(|r| r["method"] == "initialize")
        .collect::<Vec<_>>();
    assert_eq!(
        initialize
            .iter()
            .map(|r| r["params"]["protocolVersion"]
                .as_str()
                .expect("protocol version"))
            .collect::<Vec<_>>(),
        vec!["2025-11-25", "2025-03-26"],
    );
}

#[tokio::test]
async fn preflight_preserves_modern_rejections_and_http_auth_or_server_errors() {
    for (status, code) in [
        (
            StatusCode::FORBIDDEN,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        ),
        (StatusCode::BAD_REQUEST, ErrorCode::HEADER_MISMATCH),
        (
            StatusCode::BAD_REQUEST,
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
        ),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let captured = requests.clone();
        let app = Router::new().route(
            "/mcp",
            post(move |Json(request): Json<Value>| {
                captured.fetch_add(1, Ordering::SeqCst);
                async move { (status, Json(rpc_error(&request, code, None))) }
            }),
        );
        let (url, server) = serve_http_router(app).await;
        let result = preflight(&url, &HashMap::new()).await;
        server.abort();
        if status == StatusCode::FORBIDDEN || status.is_server_error() {
            assert_eq!(result.unwrap().status, status);
        } else {
            assert!(result.is_err(), "status {status}, code {code:?}");
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "must not try initialize or another version"
        );
    }
}

#[tokio::test]
async fn preflight_rejects_unknown_or_exhausted_supported_versions_without_initializing() {
    for (supported, expected_requests) in [
        (json!(["2099-01-01"]), 1),
        (json!(["2025-11-25", "2026-07-28"]), 2),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let captured = requests.clone();
        let app = Router::new().route(
            "/mcp",
            post(move |Json(request): Json<Value>| {
                captured.fetch_add(1, Ordering::SeqCst);
                let supported = supported.clone();
                async move {
                    assert_eq!(request["method"], "server/discover");
                    Json(rpc_error(
                        &request,
                        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                        Some(json!({ "supported": supported })),
                    ))
                }
            }),
        );
        let (url, server) = serve_http_router(app).await;
        let result = tokio::time::timeout(Duration::from_secs(5), preflight(&url, &HashMap::new()))
            .await
            .expect("negotiation should terminate");
        server.abort();
        assert!(matches!(
            result,
            Err(rmcp::RmcpError::ClientInitialize(
                rmcp::service::ClientInitializeError::NoCompatibleProtocolVersion { .. }
            ))
        ));
        assert_eq!(requests.load(Ordering::SeqCst), expected_requests);
    }
}

#[tokio::test]
async fn preflight_rejects_unknown_legacy_counteroffer_before_real_startup() {
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let app = Router::new().route(
        "/mcp",
        post(move |Json(request): Json<Value>| {
            captured.fetch_add(1, Ordering::SeqCst);
            async move {
                let reply = if request["method"] == "server/discover" {
                    rpc_error(&request, ErrorCode::METHOD_NOT_FOUND, None)
                } else {
                    assert_eq!(request["method"], "initialize");
                    json!({ "jsonrpc": "2.0", "id": request["id"], "result": {
                    "protocolVersion": "2099-01-01", "capabilities": {},
                    "serverInfo": { "name": "unknown-version", "version": "1" },
                } })
                };
                Json(reply)
            }
        }),
    );
    let (url, server) = serve_http_router(app).await;
    let result = start_http_server(url, HashMap::new()).await;
    server.abort();
    assert!(matches!(
        result,
        Err(rmcp::RmcpError::ClientInitialize(
            rmcp::service::ClientInitializeError::NoCompatibleProtocolVersion { .. }
        ))
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn preflight_rejects_uncorrelated_version_errors_without_retrying() {
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let app = Router::new().route(
        "/mcp",
        post(move || {
            captured.fetch_add(1, Ordering::SeqCst);
            async {
                Json(rpc_error(
                    &json!({ "id": 99 }),
                    ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                    Some(json!({ "supported": ["2025-11-25"] })),
                ))
            }
        }),
    );
    let (url, server) = serve_http_router(app).await;
    let result = preflight(&url, &HashMap::new()).await;
    server.abort();
    assert!(matches!(
        result,
        Err(rmcp::RmcpError::ClientInitialize(
            rmcp::service::ClientInitializeError::UncorrelatedErrorResponse { .. }
        ))
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn preflight_routes_legacy_endpoint_not_found_to_deprecated_sse() {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let captured = methods.clone();
    let app = Router::new().route(
        "/mcp",
        post(move |Json(request): Json<Value>| {
            captured
                .lock()
                .expect("request capture")
                .push(request["method"].clone());
            async move {
                (
                    StatusCode::NOT_FOUND,
                    Json(rpc_error(&request, ErrorCode::METHOD_NOT_FOUND, None)),
                )
            }
        }),
    );
    let (url, server) = serve_http_router(app).await;
    let result = preflight(&url, &HashMap::new()).await;
    server.abort();
    assert_eq!(result.unwrap().status, StatusCode::NOT_FOUND);
    assert_eq!(
        *methods.lock().expect("captured requests"),
        vec![json!("server/discover"), json!("initialize")]
    );
}

#[tokio::test]
async fn preflight_bounds_sse_body_before_complete_event_arrives() {
    let app = Router::new().route(
        "/mcp",
        post(|| async {
            (
                [(http::header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {}", "x".repeat(1024 * 1024)),
            )
        }),
    );
    let (url, server) = serve_http_router(app).await;
    let result = preflight(&url, &HashMap::new()).await;
    server.abort();
    let message = result
        .err()
        .expect("oversized response must be rejected")
        .to_string();
    assert!(message.contains("exceeds 1 MiB"), "{message}");
}

#[tokio::test]
async fn configured_bearer_rejection_preserves_challenge_without_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let app = Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap| {
            captured.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(
                    headers.get("authorization").unwrap(),
                    "Bearer fixture-token"
                );
                (
                    StatusCode::UNAUTHORIZED,
                    [("www-authenticate", "Bearer error=\"invalid_token\"")],
                    "not a protocol reply",
                )
            }
        }),
    );
    let (url, server) = serve_http_router(app).await;
    let result = super::negotiate_protocol(
        &url,
        &client(&HashMap::new()),
        make_client_info(),
        Some("fixture-token"),
    )
    .await
    .unwrap();
    server.abort();
    assert_eq!(result.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        result.www_authenticate,
        vec!["Bearer error=\"invalid_token\""]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
