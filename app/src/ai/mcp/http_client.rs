use std::collections::HashMap;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};

type ReqwestHttpTransport = rmcp::transport::StreamableHttpClientTransport<reqwest::Client>;

/// Builds a `HeaderMap` from a `HashMap<String, String>` of user-provided headers.
///
/// Invalid header names or values are skipped.
fn build_header_map(headers: &HashMap<String, String>) -> HeaderMap {
    let mut headers: HeaderMap = headers.try_into().unwrap_or_default();
    headers
        .entry(USER_AGENT)
        .or_insert(HeaderValue::from_static("WarpOss-MCP-Client/1.0"));
    headers
}

/// Builds a reqwest client with custom headers for MCP HTTP/SSE connections.
#[allow(clippy::result_large_err)]
pub fn build_client_with_headers(
    headers: &HashMap<String, String>,
) -> Result<reqwest::Client, rmcp::RmcpError> {
    let header_map = build_header_map(headers);

    reqwest::Client::builder()
        .default_headers(header_map)
        .build()
        .map_err(|e| {
            rmcp::RmcpError::transport_creation::<ReqwestHttpTransport>(format!(
                "Failed to build client with headers: {e}",
            ))
        })
}

#[cfg(test)]
#[path = "http_client_tests.rs"]
mod tests;
