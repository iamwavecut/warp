//! Local administrative commands in the Warp CLI.

use anyhow::{Context, Result};
use serde::Serialize;
use warp_cli::agent::OutputFormat;
use warpui::{platform::TerminationMode, AppContext, SingletonEntity};

use crate::auth::user::PrincipalType;
use crate::auth::AuthStateProvider;

pub fn login(ctx: &mut AppContext) -> Result<()> {
    println!("Login is disabled in this local-first build.");
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

#[derive(Serialize)]
struct WhoamiOutput {
    uid: String,
    #[serde(rename = "type")]
    principal_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    team_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    team_name: Option<String>,
}

/// Print information about the currently authenticated principal.
pub fn whoami(ctx: &mut AppContext, output_format: OutputFormat) -> Result<()> {
    let auth_state = AuthStateProvider::as_ref(ctx).get();
    let principal_type = auth_state.principal_type().unwrap_or_default();

    let uid = auth_state
        .user_id()
        .map(|id| {
            let s = id.as_string();
            s.strip_prefix("serviceAccount:")
                .map(String::from)
                .unwrap_or(s)
        })
        .ok_or_else(|| anyhow::anyhow!("Could not determine user ID. Are you logged in?"))?;

    let info = WhoamiOutput {
        uid,
        principal_type: match principal_type {
            PrincipalType::User => "user",
            PrincipalType::ServiceAccount => "service_account",
        },
        display_name: auth_state.display_name(),
        email: match principal_type {
            PrincipalType::User => auth_state.user_email().filter(|e| !e.is_empty()),
            PrincipalType::ServiceAccount => None,
        },
        team_uid: None,
        team_name: None,
    };

    match output_format {
        OutputFormat::Json => {
            match serde_json::to_string(&info).context("whoami output should serialize") {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    ctx.terminate_app(TerminationMode::ForceTerminate, Some(Err(err)));
                    return Ok(());
                }
            }
        }
        OutputFormat::Pretty => {
            match principal_type {
                PrincipalType::User => println!("User ID: {}", info.uid),
                PrincipalType::ServiceAccount => {
                    println!("Service account ID: {}", info.uid)
                }
            }
            if let Some(name) = &info.display_name {
                println!("Display Name: {name}");
            }
            if let Some(email) = &info.email {
                println!("Email: {email}");
            }
        }
        OutputFormat::Text => {
            println!("{}:{}", info.principal_type, info.uid);
        }
        OutputFormat::Ndjson => {
            ctx.terminate_app(
                TerminationMode::ForceTerminate,
                Some(Err(anyhow::anyhow!(
                    "`whoami` does not support `--output-format ndjson`"
                ))),
            );
            return Ok(());
        }
    }

    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}

pub fn logout(ctx: &mut AppContext) -> Result<()> {
    println!("Logout is disabled in this local-first build.");
    ctx.terminate_app(TerminationMode::ForceTerminate, None);
    Ok(())
}
