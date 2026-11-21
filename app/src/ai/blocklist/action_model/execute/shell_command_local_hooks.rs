use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use command::r#async::Command;
use futures::{FutureExt, select, try_join};
use futures_lite::io::{AsyncReadExt, AsyncWriteExt};
use serde::Deserialize;
use warpui::r#async::Timer;

#[derive(Deserialize)]
struct Config {
    version: u32,
    hooks: Hooks,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hooks {
    #[serde(default)]
    before_shell_execution: Vec<Hook>,
    #[serde(default)]
    pre_tool_use: Vec<Hook>,
}

#[derive(Deserialize)]
struct Hook {
    command: String,
    matcher: Option<String>,
}

pub(super) async fn check_shell(
    config_file: &Path,
    cwd: &Path,
    conversation_id: &str,
    action_id: &str,
    shell_command: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<bool> {
    ensure!(
        config_file.is_absolute(),
        "Hook configuration path must be absolute"
    );
    let config: Config = serde_json::from_slice(&async_fs::read(config_file).await?)?;
    ensure!(
        config.version == 1,
        "Unsupported hooks configuration version"
    );
    for (event, hooks) in [
        ("beforeShellExecution", config.hooks.before_shell_execution),
        ("preToolUse", config.hooks.pre_tool_use),
    ] {
        for hook in hooks {
            if cancelled.load(Ordering::Acquire) {
                return Ok(false);
            }
            if let Some(matcher) = hook.matcher {
                let input = if event == "preToolUse" {
                    "Shell"
                } else {
                    shell_command
                };
                if !regex::Regex::new(&matcher)?.is_match(input) {
                    continue;
                }
            }
            let mut input = serde_json::json!({
                "hook_event_name": event,
                "conversation_id": conversation_id,
                "generation_id": action_id,
                "workspace_roots": [cwd],
                "cursor_version": env!("CARGO_PKG_VERSION")
            });
            if event == "preToolUse" {
                input["tool_name"] = "Shell".into();
                input["tool_input"] = serde_json::json!({"command": shell_command});
            } else {
                input["command"] = shell_command.into();
                input["cwd"] = serde_json::to_value(cwd)?;
            }
            if !run_hook(&hook.command, cwd, serde_json::to_vec(&input)?).await? {
                return Ok(false);
            }
        }
    }
    Ok(!cancelled.load(Ordering::Acquire))
}

fn allow_output(success: bool, output: &[u8]) -> Result<bool> {
    if !success {
        return Ok(false);
    }
    if output.iter().all(u8::is_ascii_whitespace) {
        return Ok(true);
    }
    let output: serde_json::Value = serde_json::from_slice(output)?;
    let output = output
        .as_object()
        .context("Expected hook response object")?;
    Ok(!output.contains_key("updated_input")
        && !output.contains_key("updatedInput")
        && !output.contains_key("hookSpecificOutput")
        && matches!(
            output.get("permission"),
            None | Some(serde_json::Value::String(_))
        )
        && output
            .get("permission")
            .is_none_or(|value| value == "allow")
        && output.get("decision").is_none_or(|value| value == "allow")
        && output.get("continue").is_none_or(|value| value == true))
}

async fn run_hook(script: &str, cwd: &Path, input: Vec<u8>) -> Result<bool> {
    #[cfg(windows)]
    let (program, flags) = ("cmd.exe", &["/D", "/S", "/C"][..]);
    #[cfg(not(windows))]
    let (program, flags) = ("/bin/sh", &["-c"][..]);
    let (mut child, _tree) = Command::new_in_job(program)
        .args(flags)
        .arg(script)
        .current_dir(cwd)
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            let key = key.to_string_lossy().to_ascii_uppercase();
            matches!(
                key.as_str(),
                "PATH"
                    | "HOME"
                    | "USERPROFILE"
                    | "SYSTEMROOT"
                    | "COMSPEC"
                    | "PATHEXT"
                    | "TMPDIR"
                    | "TEMP"
                    | "TMP"
                    | "LANG"
            ) || key.starts_with("LC_")
        }))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn_in_job()?;
    let mut stdin = child.stdin.take().context("Missing hook stdin")?;
    let stdout = child.stdout.take().context("Missing hook stdout")?;
    const MAX_OUTPUT: u64 = 64 * 1024;
    let work = async {
        let write = async move {
            stdin.write_all(&input).await?;
            stdin.close().await
        };
        let read = async move {
            let mut output = Vec::new();
            stdout.take(MAX_OUTPUT + 1).read_to_end(&mut output).await?;
            if output.len() as u64 > MAX_OUTPUT {
                return Err(std::io::Error::other("Hook response exceeds 64 KiB"));
            }
            Ok(output)
        };
        let (_, output, status) = try_join!(write, read, child.status())?;
        allow_output(status.success(), &output)
    };
    futures::pin_mut!(work);
    select! {
        result = work.fuse() => result,
        _ = Timer::after(Duration::from_secs(10)).fuse() => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::allow_output;

    #[test]
    fn only_successful_allow_or_no_opinion_passes() {
        assert!(allow_output(true, br#"{"permission":"allow"}"#).unwrap());
        assert!(allow_output(true, b"").unwrap());
        assert!(!allow_output(false, b"").unwrap());
        for response in [
            r#"{"permission":"ask"}"#,
            r#"{"permission":"deny"}"#,
            r#"{"decision":"block"}"#,
            r#"{"permission":"allow","updated_input":{"command":"another"}}"#,
            r#"{"continue":false}"#,
        ] {
            assert!(!allow_output(true, response.as_bytes()).unwrap());
        }
        assert!(allow_output(true, b"invalid JSON").is_err());
    }
}
