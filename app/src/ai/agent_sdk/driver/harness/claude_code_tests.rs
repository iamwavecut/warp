use std::collections::HashMap;
use std::fs;

use tempfile::TempDir;
use uuid::Uuid;

use super::*;

#[test]
fn claude_command_uses_session_id_when_not_resuming() {
    let uuid = Uuid::new_v4();
    let cmd = claude_command("claude", &uuid, "/tmp/prompt.txt", None, None);
    assert!(
        cmd.contains(&format!("--session-id {uuid}")),
        "expected --session-id flag in non-resume command, got: {cmd}"
    );
    assert!(
        !cmd.contains("--resume"),
        "non-resume command should not contain --resume, got: {cmd}"
    );
}

#[test]
fn claude_command_pipes_prompt_path() {
    let uuid = Uuid::new_v4();
    let cmd = claude_command("claude", &uuid, "/tmp/prompt with spaces.txt", None, None);
    assert!(
        cmd.contains("< '/tmp/prompt with spaces.txt'"),
        "expected single-quoted stdin redirect of the prompt path, got: {cmd}"
    );
    assert!(
        cmd.contains("--dangerously-skip-permissions"),
        "expected --dangerously-skip-permissions, got: {cmd}"
    );
}

#[test]
fn serialize_claude_mcp_config_cli_server() {
    let servers = HashMap::from([(
        "test-server".to_string(),
        JSONMCPServer {
            transport_type: JSONTransportType::CLIServer {
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                env: HashMap::from([("API_KEY".to_string(), "secret".to_string())]),
                working_directory: None,
            },
        },
    )]);
    let json = serialize_claude_mcp_config(&servers).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let server = &parsed["mcpServers"]["test-server"];
    assert_eq!(server["type"], "stdio");
    assert_eq!(server["command"], "node");
    assert_eq!(server["args"][0], "server.js");
    assert_eq!(server["env"]["API_KEY"], "secret");
}

#[test]
fn serialize_claude_mcp_config_cli_server_with_cwd() {
    let servers = HashMap::from([(
        "test-server".to_string(),
        JSONMCPServer {
            transport_type: JSONTransportType::CLIServer {
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                env: HashMap::new(),
                working_directory: Some("/opt/mcp".to_string()),
            },
        },
    )]);
    let json = serialize_claude_mcp_config(&servers).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let server = &parsed["mcpServers"]["test-server"];
    assert_eq!(server["cwd"], "/opt/mcp");
}

#[test]
fn serialize_claude_mcp_config_cli_server_omits_cwd_when_none() {
    let servers = HashMap::from([(
        "test-server".to_string(),
        JSONMCPServer {
            transport_type: JSONTransportType::CLIServer {
                command: "node".to_string(),
                args: vec![],
                env: HashMap::new(),
                working_directory: None,
            },
        },
    )]);
    let json = serialize_claude_mcp_config(&servers).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let server = &parsed["mcpServers"]["test-server"];
    assert!(server.get("cwd").is_none());
}

#[test]
fn serialize_claude_mcp_config_sse_server() {
    let servers = HashMap::from([(
        "remote".to_string(),
        JSONMCPServer {
            transport_type: JSONTransportType::SSEServer {
                url: "https://mcp.example.com".to_string(),
                headers: HashMap::from([("Authorization".to_string(), "Bearer tok".to_string())]),
            },
        },
    )]);
    let json = serialize_claude_mcp_config(&servers).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let server = &parsed["mcpServers"]["remote"];
    assert_eq!(server["type"], "http");
    assert_eq!(server["url"], "https://mcp.example.com");
    assert_eq!(server["headers"]["Authorization"], "Bearer tok");
}

#[test]
fn prepare_claude_config_creates_config_file_without_api_suffix() {
    let tmp = TempDir::new().unwrap();
    let claude_json_path = tmp.path().join(".claude.json");
    let working_dir = tmp.path().join("workspace/project");

    prepare_claude_config(&claude_json_path, &working_dir, None).unwrap();

    let claude_config: Value =
        serde_json::from_slice(&fs::read(claude_json_path).unwrap()).unwrap();
    assert_eq!(claude_config["hasCompletedOnboarding"], Value::Bool(true));
    assert_eq!(
        claude_config["lspRecommendationDisabled"],
        Value::Bool(true)
    );
    let working_dir_key = working_dir.to_string_lossy().to_string();
    assert_eq!(
        claude_config["projects"][working_dir_key]["hasTrustDialogAccepted"],
        Value::Bool(true)
    );
    assert_eq!(claude_config.get("customApiKeyResponses"), None);
}

#[test]
fn prepare_claude_config_creates_config_file_with_api_suffix() {
    let tmp = TempDir::new().unwrap();
    let claude_json_path = tmp.path().join(".claude.json");
    let working_dir = tmp.path().join("workspace/project");

    prepare_claude_config(
        &claude_json_path,
        &working_dir,
        Some("QLWn-dUnuwQ-hIhDiAAA"),
    )
    .unwrap();

    let claude_config: Value =
        serde_json::from_slice(&fs::read(claude_json_path).unwrap()).unwrap();
    assert_eq!(
        claude_config["customApiKeyResponses"]["approved"],
        serde_json::json!(["QLWn-dUnuwQ-hIhDiAAA"]),
    );
}

#[test]
fn prepare_claude_config_merges_existing_config() {
    let tmp = TempDir::new().unwrap();
    let claude_json_path = tmp.path().join(".claude.json");
    fs::write(
        &claude_json_path,
        r#"{"theme":"dark","projects":{"/existing/project":{"allowedTools":["Bash"],"nested":{"value":2}}},"customApiKeyResponses":{"approved":["existing-suffix-12345"]}}"#,
    )
    .unwrap();

    let working_dir = tmp.path().join("workspace/project");
    prepare_claude_config(
        &claude_json_path,
        &working_dir,
        Some("new-suffix-1234567890"),
    )
    .unwrap();

    let claude_config: Value =
        serde_json::from_slice(&fs::read(claude_json_path).unwrap()).unwrap();
    assert_eq!(claude_config["theme"], "dark");
    assert_eq!(
        claude_config["lspRecommendationDisabled"],
        Value::Bool(true)
    );
    assert_eq!(
        claude_config["projects"]["/existing/project"]["allowedTools"],
        serde_json::json!(["Bash"])
    );
    assert_eq!(
        claude_config["projects"]["/existing/project"]["nested"]["value"],
        2
    );
    // Both existing and new suffixes should be present.
    assert_eq!(
        claude_config["customApiKeyResponses"]["approved"],
        serde_json::json!(["existing-suffix-12345", "new-suffix-1234567890"]),
    );
    let working_dir_key = working_dir.to_string_lossy().to_string();
    assert_eq!(
        claude_config["projects"][working_dir_key]["hasTrustDialogAccepted"],
        Value::Bool(true)
    );
}

#[test]
fn prepare_claude_config_no_duplicate_suffix() {
    let tmp = TempDir::new().unwrap();
    let claude_json_path = tmp.path().join(".claude.json");
    fs::write(
        &claude_json_path,
        r#"{"customApiKeyResponses":{"approved":["QLWn-dUnuwQ-hIhDiAAA"]}}"#,
    )
    .unwrap();

    let working_dir = tmp.path().join("workspace/project");
    prepare_claude_config(
        &claude_json_path,
        &working_dir,
        Some("QLWn-dUnuwQ-hIhDiAAA"),
    )
    .unwrap();

    let claude_config: Value =
        serde_json::from_slice(&fs::read(claude_json_path).unwrap()).unwrap();
    assert_eq!(
        claude_config["customApiKeyResponses"]["approved"],
        serde_json::json!(["QLWn-dUnuwQ-hIhDiAAA"]),
    );
}

#[test]
fn prepare_claude_config_none_suffix_preserves_existing_responses() {
    let tmp = TempDir::new().unwrap();
    let claude_json_path = tmp.path().join(".claude.json");
    fs::write(
        &claude_json_path,
        r#"{"customApiKeyResponses":{"approved":["existing-suffix-12345"],"rejected":["bad-key"]}}"#,
    )
    .unwrap();

    let working_dir = tmp.path().join("workspace/project");
    prepare_claude_config(&claude_json_path, &working_dir, None).unwrap();

    let claude_config: Value =
        serde_json::from_slice(&fs::read(claude_json_path).unwrap()).unwrap();
    assert_eq!(
        claude_config["customApiKeyResponses"]["approved"],
        serde_json::json!(["existing-suffix-12345"]),
    );
    assert_eq!(
        claude_config["customApiKeyResponses"]["rejected"],
        serde_json::json!(["bad-key"]),
    );
}

#[test]
#[serial_test::serial]
fn resolve_suffix_from_resolved_env_vars() {
    std::env::remove_var(ANTHROPIC_API_KEY_ENV);
    let key = "sk-ant-api03-abcdefghij1234567890ABCDEFGHIJ1234567890abcdefghij1234567890QLWn-dUnuwQ-hIhDiAAA";
    let resolved = HashMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from(key))]);
    let suffix = resolve_anthropic_api_key_suffix(&resolved);
    assert_eq!(suffix.as_deref(), Some("QLWn-dUnuwQ-hIhDiAAA"));
}

#[test]
#[serial_test::serial]
fn resolve_suffix_returns_none_for_short_key() {
    std::env::remove_var(ANTHROPIC_API_KEY_ENV);
    let resolved = HashMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from("short"))]);
    assert_eq!(resolve_anthropic_api_key_suffix(&resolved), None);
}

#[test]
#[serial_test::serial]
fn resolve_suffix_returns_none_when_empty() {
    std::env::remove_var(ANTHROPIC_API_KEY_ENV);
    assert_eq!(resolve_anthropic_api_key_suffix(&HashMap::new()), None);
}

#[test]
#[serial_test::serial]
fn suffix_uses_worker_injected_env_when_present() {
    let worker_key = "sk-ant-api03-WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW-worker-suffix!";
    std::env::set_var(ANTHROPIC_API_KEY_ENV, worker_key);
    // Even when the resolved map has a different value, the worker env wins.
    let resolved = HashMap::from([(
        OsString::from("ANTHROPIC_API_KEY"),
        OsString::from(
            "sk-ant-api03-RRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRRR-resolved-val!",
        ),
    )]);
    let suffix = resolve_anthropic_api_key_suffix(&resolved);
    let expected = &worker_key[worker_key.len() - 20..];
    assert_eq!(suffix.as_deref(), Some(expected));
    std::env::remove_var(ANTHROPIC_API_KEY_ENV);
}

#[test]
fn prepare_claude_settings_creates_settings_file() {
    let tmp = TempDir::new().unwrap();
    let claude_settings_path = tmp.path().join(".claude/settings.json");

    prepare_claude_settings(&claude_settings_path).unwrap();

    let claude_settings: Value =
        serde_json::from_slice(&fs::read(claude_settings_path).unwrap()).unwrap();
    assert_eq!(
        claude_settings["skipDangerousModePermissionPrompt"],
        Value::Bool(true)
    );
}

#[test]
fn prepare_claude_settings_merges_existing_settings() {
    let tmp = TempDir::new().unwrap();
    let claude_settings_path = tmp.path().join("settings.json");
    fs::write(
        &claude_settings_path,
        r#"{"editor":"vim","nested":{"value":1}}"#,
    )
    .unwrap();

    prepare_claude_settings(&claude_settings_path).unwrap();

    let claude_settings: Value =
        serde_json::from_slice(&fs::read(claude_settings_path).unwrap()).unwrap();
    assert_eq!(claude_settings["editor"], "vim");
    assert_eq!(claude_settings["nested"]["value"], 1);
    assert_eq!(
        claude_settings["skipDangerousModePermissionPrompt"],
        Value::Bool(true)
    );
}
