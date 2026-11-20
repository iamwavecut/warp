use super::*;
use reqwest::header::USER_AGENT;

#[test]
fn mcp_http_headers_include_default_user_agent() {
    let headers = build_header_map(&HashMap::new());
    assert_eq!(headers[USER_AGENT], "WarpOss-MCP-Client/1.0");
}

#[test]
fn mcp_http_headers_preserve_configured_user_agent() {
    let headers = build_header_map(&HashMap::from([(
        "User-Agent".to_owned(),
        "custom-agent/1.0".to_owned(),
    )]));
    assert_eq!(headers[USER_AGENT], "custom-agent/1.0");
}
