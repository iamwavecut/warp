//! Runs local ACP adapters and renders their turns in locally persisted conversations.
mod attachments;
mod bridge;
mod connection;
mod launch;
mod manifest;
mod mapping;
mod policy;
mod protocol;
mod steering;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
pub(crate) use bridge::run_bridge;
use futures::{FutureExt, future, pin_mut, select, select_biased};
pub(crate) use launch::AcpLaunchSpec;
use parking_lot::Mutex;
use serde_json::Value;
use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_core::channel::ChannelState;
use warp_managed_secrets::ManagedSecretValue;
use warp_multi_agent_api::response_event::{StreamFinished, stream_finished};
use warp_multi_agent_api::{ClientAction, ResponseEvent, response_event};
use warp_util::path::{EscapeChar, ShellFamily};
use warpui::r#async::Timer;
use warpui::r#async::executor::Background;
use warpui::{ModelContext, ModelHandle, ModelSpawner, SingletonEntity};

use self::attachments::AttachmentResolver;
use self::bridge::{BridgeListener, bridge_command};
use self::connection::{AcpConnection, InboundNotification, RpcError};
use self::mapping::{AcpTurnMapper, TurnEvent};
use self::policy::{PolicyDecision, PolicyRequest, select_permission};
use self::protocol::{
    CancelParams, ClientCapabilities, ClientInfo, ContentBlock, FsCapabilities, InitializeParams,
    InitializeResponse, McpServer, NewSessionParams, NewSessionResponse, PROTOCOL_VERSION,
    PromptParams, PromptResponse, ReadTextFileParams, ReadTextFileResponse,
    RequestPermissionParams, RequestPermissionResponse, SessionUpdateParams, StopReason,
    WriteTextFileParams,
};
use super::super::terminal::{CommandHandle, TerminalDriver};
use super::super::{AgentDriver, AgentDriverError};
use super::claude_code::prepare_claude_environment_config;
use super::codex::prepare_codex_environment_config;
use super::gemini::prepare_gemini_environment_config;
use super::{
    HarnessCleanupDisposition, HarnessKind, HarnessRunner, JSONMCPServer,
    LocalHarnessResumePayload, SavePoint, ThirdPartyHarness, harness_kind, validate_cli_installed,
};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::ambient_agents::task::HarnessModelConfig;
use crate::ai::blocklist::{BlocklistAIController, ResponseStreamId};
use crate::ai::blocklist::{ExternalHarnessPrompt, ExternalHarnessTurn};
use crate::ai::mcp::JSONTransportType;
use crate::server::server_api::ServerApi;
use crate::terminal::CLIAgent;

/// Bound on the agent connecting back through the bridge plus its `initialize` and
/// `session/new` handshakes.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// A third-party harness driven over ACP. `harness` is the harness identity (which decides
/// local configuration and CLI checks); `launch` is the ACP adapter process.
pub(crate) struct AcpHarness {
    harness: Harness,
    launch: AcpLaunchSpec,
    manifest: Option<manifest::HarnessManifest>,
}

impl AcpHarness {
    pub(crate) fn new(harness: Harness, launch: AcpLaunchSpec) -> Self {
        Self {
            harness,
            launch,
            manifest: None,
        }
    }

    pub(crate) fn from_manifest(harness: Harness) -> Result<Self, AgentDriverError> {
        if !matches!(
            harness,
            Harness::Claude | Harness::Codex | Harness::Gemini | Harness::Pi
        ) {
            return Err(AgentDriverError::HarnessSetupFailed {
                harness: harness.to_string(),
                reason: "this harness has no local ACP adapter".into(),
            });
        }
        let manifest = manifest::resolve(harness)?;
        let mut harness = Self::new(harness, manifest.launch.clone());
        harness.manifest = Some(manifest);
        Ok(harness)
    }

    /// The terminal-based harness with the same identity, whose per-harness knowledge (install
    /// docs, auth checks) applies regardless of transport.
    fn pty_harness(&self) -> Option<Box<dyn ThirdPartyHarness>> {
        if self.harness == Harness::Pi {
            return None;
        }
        match harness_kind(self.harness) {
            Ok(HarnessKind::ThirdParty(harness)) => Some(harness),
            Ok(HarnessKind::Oz | HarnessKind::Unsupported(_)) | Err(_) => None,
        }
    }

    /// Writes the harness's own config files (trust, auth, system prompt) the way the terminal
    /// transport does, so the ACP adapter finds the same environment its CLI would.
    fn prepare_harness_environment(
        &self,
        system_prompt: Option<&str>,
        working_dir: &Path,
        env: &HashMap<OsString, OsString>,
        secrets: &HashMap<String, ManagedSecretValue>,
        config: Option<&HarnessModelConfig>,
    ) -> Result<()> {
        match self.harness {
            Harness::Claude => prepare_claude_environment_config(working_dir, env),
            Harness::Codex => prepare_codex_environment_config(
                working_dir,
                system_prompt,
                env,
                secrets,
                &HashMap::new(),
                config,
            ),
            Harness::Gemini => prepare_gemini_environment_config(working_dir, system_prompt),
            _ => Ok(()),
        }
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl ThirdPartyHarness for AcpHarness {
    fn harness(&self) -> Harness {
        self.harness
    }

    fn cli_agent(&self) -> CLIAgent {
        match self.harness {
            Harness::Claude => CLIAgent::Claude,
            Harness::Codex => CLIAgent::Codex,
            Harness::Gemini => CLIAgent::Gemini,
            Harness::Oz | Harness::OpenCode | Harness::Pi | Harness::Unknown => CLIAgent::Unknown,
        }
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        self.pty_harness()
            .and_then(|harness| harness.install_docs_url())
    }

    fn validate(&self) -> Result<(), AgentDriverError> {
        validate_cli_installed(&self.launch.program, self.install_docs_url())
    }

    fn auth_check_command(&self) -> Option<String> {
        if let Some(manifest) = &self.manifest {
            return manifest.auth_check_command.clone();
        }
        self.pty_harness()
            .and_then(|harness| harness.auth_check_command())
    }

    /// The agent's stderr lands in its terminal block, so the CLI's failure signatures apply.
    fn runtime_error_patterns(&self) -> &'static [&'static str] {
        self.pty_harness()
            .map(|harness| harness.runtime_error_patterns())
            .unwrap_or(&[])
    }

    fn runtime_error_patterns_for_run(&self) -> Vec<String> {
        if let Some(manifest) = &self.manifest {
            return manifest.runtime_error_patterns.clone();
        }
        self.runtime_error_patterns()
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect()
    }

    fn drives_cli_agent_session(&self) -> bool {
        false
    }

    fn build_runner(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        resumption_prompt: Option<&str>,
        context: Option<&str>,
        harness_working_dir: &Path,
        local_resume: Option<LocalHarnessResumePayload>,
        task_id: Option<AmbientAgentTaskId>,
        _server_api: Arc<ServerApi>,
        terminal_driver: ModelHandle<TerminalDriver>,
        resolved_env_vars: &HashMap<OsString, OsString>,
        resolved_secrets: &HashMap<String, ManagedSecretValue>,
        resolved_mcp_servers: &HashMap<String, JSONMCPServer>,
        third_party_harness_model_config: Option<&HarnessModelConfig>,
    ) -> Result<Box<dyn HarnessRunner>, AgentDriverError> {
        if local_resume.as_ref().is_some_and(|resume| resume.is_resume) {
            return Err(AgentDriverError::HarnessSetupFailed { harness: self.harness.to_string(), reason: "PTY transcript resume cannot be used with ACP; local conversation history remains available".into() });
        }
        self.prepare_harness_environment(
            system_prompt,
            harness_working_dir,
            resolved_env_vars,
            resolved_secrets,
            third_party_harness_model_config,
        )
        .map_err(|error| AgentDriverError::HarnessConfigSetupFailed {
            harness: self.harness.to_string(),
            error,
        })?;

        // ACP has no system-prompt channel of its own, so every server-composed block that the
        // harness bootstrap did not already place is folded into the user turn ahead of the
        // prompt itself.
        let system_prompt_in_turn = match self.harness {
            Harness::Claude | Harness::Oz | Harness::OpenCode | Harness::Pi | Harness::Unknown => {
                system_prompt
            }
            Harness::Codex | Harness::Gemini => None,
        };
        let prompt_text = [
            resumption_prompt,
            context,
            system_prompt_in_turn,
            Some(prompt),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        let attachments = AttachmentResolver::default();
        Ok(Box::new(AcpHarnessRunner {
            harness: self.harness,
            launch: self.launch.clone(),
            user_prompt: prompt.to_owned(),
            prompt_text,
            harness_working_dir: harness_working_dir.to_path_buf(),
            task_id,
            terminal_driver,
            attachments: Arc::new(attachments),
            mcp_servers: resolved_mcp_servers
                .iter()
                .map(|(name, server)| mcp_server_for_acp(name, server))
                .collect(),
            agent: Mutex::new(None),
            settling: Arc::new(AtomicBool::new(false)),
        }))
    }
}

/// The protocol session with the agent running in the terminal.
struct RunningAgent {
    connection: Arc<AcpConnection>,
    session_id: Option<String>,
}

pub(crate) struct AcpHarnessRunner {
    harness: Harness,
    launch: AcpLaunchSpec,
    /// The prompt as the user wrote it, echoed into the conversation.
    user_prompt: String,
    /// The full text sent to the agent, including server-composed context.
    prompt_text: String,
    harness_working_dir: PathBuf,
    task_id: Option<AmbientAgentTaskId>,
    terminal_driver: ModelHandle<TerminalDriver>,
    attachments: Arc<AttachmentResolver>,
    mcp_servers: Vec<McpServer>,
    agent: Mutex<Option<RunningAgent>>,
    settling: Arc<AtomicBool>,
}

impl AcpHarnessRunner {
    fn setup_failed(&self, reason: String) -> AgentDriverError {
        AgentDriverError::HarnessSetupFailed {
            harness: self.harness.to_string(),
            reason,
        }
    }

    fn running_agent(&self) -> Option<(Arc<AcpConnection>, Option<String>)> {
        self.agent
            .lock()
            .as_ref()
            .map(|agent| (agent.connection.clone(), agent.session_id.clone()))
    }

    /// Interrupts whatever the terminal session is running, which is the agent's block while
    /// the harness is up.
    async fn interrupt_terminal(&self, foreground: &ModelSpawner<AgentDriver>) {
        let terminal_driver = self.terminal_driver.clone();
        let _ = foreground
            .spawn(move |_, ctx| {
                terminal_driver.update(ctx, |driver, ctx| {
                    driver.with_terminal_view(ctx, |terminal, ctx| {
                        terminal.write_user_bytes_to_pty(vec![3], ctx);
                    })
                });
            })
            .await;
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl HarnessRunner for AcpHarnessRunner {
    fn drives_native_conversation(&self) -> bool {
        true
    }
    fn is_settling_turn(&self) -> bool {
        self.settling.load(Ordering::SeqCst)
    }
    fn harness_name(&self) -> &str {
        &self.launch.program
    }

    async fn start(
        &self,
        foreground: &ModelSpawner<AgentDriver>,
    ) -> Result<CommandHandle, AgentDriverError> {
        let (background, shell_family, host_local): (Arc<Background>, ShellFamily, bool) =
            foreground
                .spawn(|me, ctx| {
                    let shell_family = me
                        .terminal_driver
                        .as_ref(ctx)
                        .active_session_shell_type(ctx)
                        .map(ShellFamily::from)
                        .unwrap_or(ShellFamily::Posix);
                    let host_local = matches!(
                        me.terminal_driver.as_ref(ctx).active_shell_launch_data(ctx),
                        Some(
                            crate::terminal::ShellLaunchData::Executable { .. }
                                | crate::terminal::ShellLaunchData::MSYS2 { .. }
                        )
                    );
                    (ctx.background_executor(), shell_family, host_local)
                })
                .await
                .map_err(|_| AgentDriverError::InvalidRuntimeState)?;

        if !host_local {
            return Err(
                self.setup_failed("ACP requires a host-local shell, not WSL, Docker or SSH".into())
            );
        }
        let listener = BridgeListener::bind(&self.launch.program, &self.launch.args)
            .map_err(|error| self.setup_failed(format!("{error:#}")))?;
        let command = bridge_command(listener.launch_file(), shell_family)
            .map_err(|error| self.setup_failed(format!("{error:#}")))?;
        let terminal_driver = self.terminal_driver.clone();
        let command_handle = foreground
            .spawn(move |_, ctx| {
                terminal_driver.update(ctx, |driver, ctx| driver.execute_command(&command, ctx))
            })
            .await??
            .await?;

        let handshake_started = instant::Instant::now();
        let (reader, writer) = match listener.accept(HANDSHAKE_TIMEOUT).await {
            Ok(halves) => halves,
            Err(error) => {
                self.interrupt_terminal(foreground).await;
                return Err(self.setup_failed(format!("{error:#}")));
            }
        };
        drop(listener);
        let request_context = Arc::new(AgentRequestContext {
            foreground: foreground.clone(),
            terminal_driver: self.terminal_driver.clone(),
            escape_char: shell_family.escape_char(),
        });
        let (connection, notifications) = AcpConnection::new(
            reader,
            writer,
            agent_request_handler(request_context),
            &background,
        );
        let connection = Arc::new(connection);
        *self.agent.lock() = Some(RunningAgent {
            connection: connection.clone(),
            session_id: None,
        });

        let handshake_connection = connection.clone();
        let attachment_capabilities = self.attachments.clone();
        let handshake = async move {
            let connection = handshake_connection;
            let initialize: InitializeResponse = connection
                .request(
                    protocol::METHOD_INITIALIZE,
                    InitializeParams {
                        protocol_version: PROTOCOL_VERSION,
                        client_capabilities: ClientCapabilities {
                            fs: FsCapabilities {
                                read_text_file: true,
                                write_text_file: true,
                            },
                            terminal: false,
                        },
                        client_info: ClientInfo {
                            name: "warposs".to_owned(),
                            title: "WarpOss".to_owned(),
                            version: ChannelState::app_version()
                                .unwrap_or(env!("CARGO_PKG_VERSION"))
                                .to_owned(),
                        },
                    },
                )
                .await
                .context("initialize failed")?;
            attachment_capabilities.set_prompt_capabilities(initialize.agent_capabilities.prompt_capabilities);
            let agent_description = initialize
                .agent_info
                .as_ref()
                .map(|info| format!("{} {}", info.name, info.version))
                .unwrap_or_else(|| "unknown".to_owned());
            // The agent answers with the highest version it supports that is no newer than
            // ours; anything else means the two sides would not agree on the wire format.
            if initialize.protocol_version != PROTOCOL_VERSION {
                anyhow::bail!(
                    "agent {agent_description} speaks ACP protocol v{} but this client requires v{PROTOCOL_VERSION}",
                    initialize.protocol_version
                );
            }
            log::info!(
                "ACP agent initialized: protocol v{} agent={agent_description} auth_methods={}",
                initialize.protocol_version,
                initialize.auth_methods.len(),
            );
            let session: NewSessionResponse = connection
                .request(
                    protocol::METHOD_SESSION_NEW,
                    NewSessionParams {
                        cwd: self.harness_working_dir.display().to_string(),
                        mcp_servers: self.mcp_servers.clone(),
                    },
                )
                .await
                .context("session/new failed")?;
            Ok::<_, anyhow::Error>(session.session_id)
        }
        .fuse();
        pin_mut!(handshake);
        let handshake_budget = HANDSHAKE_TIMEOUT.saturating_sub(handshake_started.elapsed());
        let session_id = select! {
            result = handshake => result,
            _ = Timer::after(handshake_budget).fuse() => {
                Err(anyhow!("the agent did not complete its handshake within {HANDSHAKE_TIMEOUT:?}"))
            }
        };
        let session_id = match session_id {
            Ok(session_id) => session_id,
            Err(error) => {
                connection.close().await;
                self.interrupt_terminal(foreground).await;
                return Err(self.setup_failed(format!("{error:#}")));
            }
        };
        log::info!("ACP session {session_id} created");
        if let Some(agent) = self.agent.lock().as_mut() {
            agent.session_id = Some(session_id.clone());
        }

        let run_id = self.task_id.map(|id| id.to_string());
        let turn = match begin_turn(foreground, &self.terminal_driver, run_id.clone()).await {
            Ok(turn) => turn,
            Err(error) => {
                connection.close().await;
                self.interrupt_terminal(foreground).await;
                return Err(error);
            }
        };
        let (follow_up_tx, follow_up_rx) = async_channel::unbounded::<ExternalHarnessPrompt>();
        let (cancel_tx, cancel_rx) = async_channel::unbounded();
        with_ai_controller(foreground, &self.terminal_driver, move |controller, _| {
            controller.set_external_harness_prompt_sink(follow_up_tx);
            controller.set_external_harness_cancel_sink(cancel_tx);
        })
        .await?;

        let mut mapper = AcpTurnMapper::new(Uuid::new_v4().to_string(), turn.request_id.clone());
        apply_actions(
            foreground,
            &self.terminal_driver,
            &turn.stream_id,
            mapper.initial_actions(&self.user_prompt),
        )
        .await?;

        let turn_driver = TurnDriver {
            connection,
            notifications,
            follow_ups: follow_up_rx,
            cancels: cancel_rx,
            queued: steering::QueuedPrompts::default(),
            settling: self.settling.clone(),
            foreground: foreground.clone(),
            terminal_driver: self.terminal_driver.clone(),
            run_id,
            turn: Mutex::new(turn),
            session_id,
            prompt_text: self.prompt_text.clone(),
            attachments: self.attachments.clone(),
        };
        background
            .spawn(async move { turn_driver.run(&mut mapper).await })
            .detach();

        // The agent's block is the command the driver waits on. The driver decides when the run
        // is over (from the native conversation's status and its idle windows) and ends it
        // through `exit`; the turn driver just keeps the session serviceable until then.
        Ok(command_handle)
    }

    async fn save_conversation(
        &self,
        _point: SavePoint,
        foreground: &ModelSpawner<AgentDriver>,
    ) -> Result<()> {
        // Each native conversation update is already queued to local SQLite.
        with_ai_controller(foreground, &self.terminal_driver, |controller, ctx| {
            controller
                .native_prompt_conversation_id()
                .is_some_and(|id| {
                    crate::ai::blocklist::BlocklistAIHistoryModel::as_ref(ctx)
                        .conversation(&id)
                        .is_some()
                })
        })
        .await?;
        Ok(())
    }

    async fn exit(&self, _foreground: &ModelSpawner<AgentDriver>) -> Result<()> {
        let Some((connection, session_id)) = self.running_agent() else {
            return Ok(());
        };
        if let Some(session_id) = session_id {
            connection
                .notify(protocol::METHOD_SESSION_CANCEL, CancelParams { session_id })
                .await?;
        }
        connection.close().await;
        Ok(())
    }

    /// Interrupts the agent's block; the driver's exit ladder force-kills the process group if
    /// that is not enough.
    async fn exit_followup(&self, foreground: &ModelSpawner<AgentDriver>) -> Result<()> {
        if self.running_agent().is_some() {
            self.interrupt_terminal(foreground).await;
        }
        Ok(())
    }

    async fn cleanup(
        &self,
        _cleanup_disposition: HarnessCleanupDisposition,
        _foreground: &ModelSpawner<AgentDriver>,
    ) -> Result<()> {
        self.agent.lock().take();
        Ok(())
    }
}

/// Runs the initial prompt turn, then any follow-up prompts that arrive while the session is
/// open. The session ends when the driver closes the connection (its idle window elapsed or it
/// is shutting down), when the controller unbinds the conversation, or when the agent leaves.
///
/// A prompt turn spans one or more MAA request streams: every time the agent reports results
/// for tool calls announced in the current stream, that stream is finished and a new one is
/// opened, as the MAA server does for its own tool round trips.
struct TurnDriver {
    connection: Arc<AcpConnection>,
    notifications: async_channel::Receiver<InboundNotification>,
    /// Follow-up prompts injected into the conversation (e.g. by shared-session viewers).
    follow_ups: async_channel::Receiver<ExternalHarnessPrompt>,
    cancels: async_channel::Receiver<()>,
    queued: steering::QueuedPrompts,
    settling: Arc<AtomicBool>,
    foreground: ModelSpawner<AgentDriver>,
    terminal_driver: ModelHandle<TerminalDriver>,
    run_id: Option<String>,
    /// The currently open request stream.
    turn: Mutex<ExternalHarnessTurn>,
    session_id: String,
    prompt_text: String,
    attachments: Arc<AttachmentResolver>,
}

impl TurnDriver {
    /// A failed turn is already reflected in the conversation status (the stream finishes with
    /// an internal error), which is what the task status follows; the block's exit code remains
    /// the agent's own.
    async fn run(mut self, mapper: &mut AcpTurnMapper) {
        let initial = vec![ContentBlock::Text {
            text: self.prompt_text.clone(),
        }];
        let mut turn_result = self.run_turn(mapper, initial).await;

        log::info!("ACP turn finished; accepting follow-ups until the driver ends the run");
        loop {
            if turn_result.is_err() {
                break;
            }
            self.queued.collect_ready(&self.follow_ups);
            if let Some(prompt) = self
                .queued
                .take(matches!(turn_result, Ok(StopReason::Cancelled)))
            {
                turn_result = self.run_follow_up(mapper, prompt).await;
                continue;
            }
            select! {
                cancel = self.cancels.recv().fuse() => match cancel {
                    Ok(()) => {
                        self.mark_cancelled().await;
                    }
                    Err(_) => break,
                },
                prompt = self.follow_ups.recv().fuse() => match prompt {
                    Ok(prompt) => {
                        log::info!("Running ACP follow-up turn");
                        self.queued.push(prompt);
                        self.queued.collect_ready(&self.follow_ups);
                        let prompt = self.queued.take(false).expect("queued follow-up");
                        turn_result = self.run_follow_up(mapper, prompt).await;
                    }
                    // The controller unbound the conversation; nothing more can be routed here.
                    Err(_) => break,
                },
                notification = self.notifications.recv().fuse() => match notification {
                    Ok(notification) => log::debug!(
                        "Ignoring ACP notification {} received between turns",
                        notification.method
                    ),
                    // The agent closed its side, typically because the driver closed ours.
                    Err(_) => break,
                },
            }
        }
        self.connection.close().await;
        match turn_result {
            Ok(stop_reason) => log::info!("ACP turns finished (last stop reason {stop_reason:?})"),
            Err(error) => log::warn!("ACP turns finished with an error: {error:#}"),
        }
    }

    /// Opens a new request stream for a follow-up prompt, echoes it, and runs the turn. The
    /// echo shows the text only; attachments reach the agent but are not yet rendered on the
    /// user-query entry.
    async fn run_follow_up(
        &mut self,
        mapper: &mut AcpTurnMapper,
        prompt: ExternalHarnessPrompt,
    ) -> Result<StopReason> {
        let content = self
            .attachments
            .prompt_content(prompt.text.clone(), prompt.attachments)
            .await?;
        let next = begin_turn(&self.foreground, &self.terminal_driver, self.run_id.clone())
            .await
            .map_err(|error| {
                self.settling.store(false, Ordering::SeqCst);
                anyhow!("{error}")
            })?;
        mapper.start_segment(next.request_id.clone());
        *self.turn.lock() = next;
        self.settling.store(false, Ordering::SeqCst);
        let echo = vec![mapper.user_query_action(&prompt.text)];
        self.apply_events(mapper, vec![TurnEvent::Actions(echo)])
            .await?;
        self.run_turn(mapper, content).await
    }

    /// Runs one prompt turn to completion and finishes its last request stream.
    async fn run_turn(
        &mut self,
        mapper: &mut AcpTurnMapper,
        prompt: Vec<ContentBlock>,
    ) -> Result<StopReason> {
        let turn_result = self.prompt(mapper, prompt).await;
        self.queued.collect_ready(&self.follow_ups);
        if turn_result.is_err() {
            self.queued.clear();
        }
        let cancelled = matches!(&turn_result, Ok(StopReason::Cancelled));
        self.settling
            .store(!self.queued.is_empty(), Ordering::SeqCst);
        let finished = match &turn_result {
            Ok(stop_reason) => stream_finished_for(*stop_reason),
            Err(error) => {
                log::error!("ACP turn failed: {error:#}");
                StreamFinished {
                    reason: Some(stream_finished::Reason::InternalError(
                        stream_finished::InternalError {
                            message: format!("{error:#}"),
                        },
                    )),
                    ..Default::default()
                }
            }
        };
        let finish_events = mapper.finish_events();
        if let Err(error) = self.apply_events(mapper, finish_events).await {
            log::warn!("Failed to settle ACP tool calls: {error}");
        }
        let stream_id = self.turn.lock().stream_id.clone();
        if cancelled {
            let reason = if self.queued.is_empty() {
                crate::ai::agent::CancellationReason::ManuallyCancelled
            } else {
                crate::ai::agent::CancellationReason::FollowUpSubmitted {
                    is_for_same_conversation: true,
                }
            };
            let _ = with_ai_controller(
                &self.foreground,
                &self.terminal_driver,
                move |controller, ctx| {
                    controller.cancel_external_harness_turn(&stream_id, reason, ctx);
                },
            )
            .await;
        } else if finish_turn(&self.foreground, &self.terminal_driver, stream_id, finished)
            .await
            .is_err()
        {
            log::warn!("Agent driver dropped before the ACP turn could be finished");
        }
        turn_result
    }

    async fn mark_cancelled(&self) {
        let _ = with_ai_controller(
            &self.foreground,
            &self.terminal_driver,
            |controller, ctx| controller.mark_external_harness_cancelled(ctx),
        )
        .await;
    }

    /// Sends the prompt and applies every update the agent streams before responding.
    async fn prompt(
        &mut self,
        mapper: &mut AcpTurnMapper,
        prompt: Vec<ContentBlock>,
    ) -> Result<StopReason> {
        let connection = self.connection.clone();
        let prompt_fut = connection
            .send_request::<_, PromptResponse>(
                protocol::METHOD_SESSION_PROMPT,
                PromptParams {
                    session_id: self.session_id.clone(),
                    prompt,
                },
            )
            .await?
            .fuse();
        pin_mut!(prompt_fut);
        let mut cancellation = steering::Cancellation::default();
        let mut deadline = future::pending::<()>().boxed().fuse();
        let mut retry = future::pending::<()>().boxed().fuse();
        loop {
            select_biased! {
                response = prompt_fut => {
                    let response = match response {
                        Ok(response) => response,
                        Err(error) if cancellation.is_requested() && error.downcast_ref::<RpcError>().is_some() => PromptResponse { stop_reason: StopReason::Cancelled },
                        Err(error) => return Err(error),
                    };
                    while let Ok(notification) = self.notifications.try_recv() {
                        self.handle_notification(mapper, notification).await?;
                    }
                    return Ok(response.stop_reason);
                },
                cancel = self.cancels.recv().fuse() => {
                    cancel.map_err(|_| anyhow!("local ACP cancellation channel closed"))?;
                    self.queued.clear();
                    if cancellation.interrupt() {
                        self.settling.store(true, Ordering::SeqCst);
                        steering::cancel_turn(&connection, &self.session_id).await?;
                        deadline = Timer::after(steering::CANCEL_TIMEOUT).map(|_| ()).boxed().fuse();
                        retry = future::pending::<()>().boxed().fuse();
                    }
                },
                _ = deadline => {
                    if cancellation.expire(&mut self.queued) {
                        self.interrupt_after_cancel_timeout().await;
                        return Err(anyhow!("ACP agent did not acknowledge session/cancel within 15 seconds"));
                    }
                    // A queued correction must not kill the active local CLI or discard user input.
                    deadline = future::pending::<()>().boxed().fuse();
                    retry = future::pending::<()>().boxed().fuse();
                },
                _ = retry => {
                    steering::cancel_turn(&connection, &self.session_id).await?;
                    retry = match cancellation.next_retry_delay() {
                        Some(after) => Timer::after(after).map(|_| ()).boxed().fuse(),
                        None => future::pending::<()>().boxed().fuse(),
                    };
                },
                incoming = async {
                    select! {
                        prompt = self.follow_ups.recv().fuse() => future::Either::Left(prompt),
                        notification = self.notifications.recv().fuse() => future::Either::Right(notification),
                    }
                }.fuse() => match incoming {
                    future::Either::Left(follow_up) => {
                        self.queued.push(follow_up.map_err(|_| anyhow!("local ACP prompt channel closed"))?);
                        if cancellation.follow_up() {
                            self.settling.store(true, Ordering::SeqCst);
                            steering::cancel_turn(&connection, &self.session_id).await?;
                            deadline = Timer::after(steering::CANCEL_TIMEOUT).map(|_| ()).boxed().fuse();
                            if let Some(after) = cancellation.next_retry_delay() {
                                retry = Timer::after(after).map(|_| ()).boxed().fuse();
                            }
                        }
                    },
                    future::Either::Right(Ok(notification)) => self.handle_notification(mapper, notification).await?,
                    // The pending prompt resolves with the read-loop error.
                    future::Either::Right(Err(_)) => break,
                },
            }
        }
        prompt_fut.await.map(|response| response.stop_reason)
    }

    async fn interrupt_after_cancel_timeout(&self) {
        let terminal_driver = self.terminal_driver.clone();
        let _ = self
            .foreground
            .spawn(move |_, ctx| {
                terminal_driver.update(ctx, |driver, ctx| {
                    driver.with_terminal_view(ctx, |terminal, ctx| {
                        terminal.write_user_bytes_to_pty(vec![3], ctx);
                    })
                });
            })
            .await;
    }

    async fn handle_notification(
        &self,
        mapper: &mut AcpTurnMapper,
        notification: InboundNotification,
    ) -> Result<()> {
        if notification.method != protocol::METHOD_SESSION_UPDATE {
            log::debug!("Ignoring ACP notification {}", notification.method);
            return Ok(());
        }
        let params: SessionUpdateParams = match serde_json::from_value(notification.params) {
            Ok(params) => params,
            Err(error) => {
                log::warn!("Ignoring unparseable ACP session/update: {error}");
                return Ok(());
            }
        };
        if params
            .session_id
            .as_deref()
            .is_some_and(|id| id != self.session_id)
        {
            log::warn!("Ignoring ACP update for another local session");
            return Ok(());
        }
        let events = mapper.map_update(params.update);
        self.apply_events(mapper, events).await
    }

    async fn apply_events(&self, mapper: &mut AcpTurnMapper, events: Vec<TurnEvent>) -> Result<()> {
        for event in events {
            match event {
                TurnEvent::Actions(actions) => {
                    let stream_id = self.turn.lock().stream_id.clone();
                    apply_actions(&self.foreground, &self.terminal_driver, &stream_id, actions)
                        .await
                        .map_err(|error| anyhow!("{error}"))?;
                }
                TurnEvent::SegmentBoundary => {
                    let stream_id = self.turn.lock().stream_id.clone();
                    finish_turn(
                        &self.foreground,
                        &self.terminal_driver,
                        stream_id,
                        stream_finished_for(StopReason::EndTurn),
                    )
                    .await
                    .map_err(|error| anyhow!("{error}"))?;
                    let next =
                        begin_turn(&self.foreground, &self.terminal_driver, self.run_id.clone())
                            .await
                            .map_err(|error| anyhow!("{error}"))?;
                    mapper.start_segment(next.request_id.clone());
                    *self.turn.lock() = next;
                }
            }
        }
        Ok(())
    }
}

/// Runs `f` against the driver terminal's AI controller on the foreground thread, which owns
/// every entity involved. The terminal view handle is cloned only for the synchronous callback
/// (the borrow of `ctx` through the driver must end before the view can be updated) and is
/// released when it returns.
async fn with_ai_controller<T: Send + 'static>(
    foreground: &ModelSpawner<AgentDriver>,
    terminal_driver: &ModelHandle<TerminalDriver>,
    f: impl FnOnce(&mut BlocklistAIController, &mut ModelContext<BlocklistAIController>) -> T
    + Send
    + 'static,
) -> Result<T, AgentDriverError> {
    let terminal_driver = terminal_driver.clone();
    foreground
        .spawn(move |_, ctx| {
            let terminal = terminal_driver.as_ref(ctx).terminal_view().clone();
            terminal.update(ctx, |terminal, ctx| terminal.ai_controller().update(ctx, f))
        })
        .await
        .map_err(|_| AgentDriverError::InvalidRuntimeState)
}

/// Opens a request stream on the driver's bound native conversation.
async fn begin_turn(
    foreground: &ModelSpawner<AgentDriver>,
    terminal_driver: &ModelHandle<TerminalDriver>,
    run_id: Option<String>,
) -> Result<ExternalHarnessTurn, AgentDriverError> {
    with_ai_controller(foreground, terminal_driver, move |controller, ctx| {
        let conversation_id = controller
            .native_prompt_conversation_id()
            .unwrap_or_else(|| controller.bind_native_prompt_conversation(None, ctx));
        controller.begin_external_harness_turn(conversation_id, run_id, ctx)
    })
    .await?
    .map_err(|error| AgentDriverError::HarnessSetupFailed {
        harness: "acp".to_owned(),
        reason: format!("could not open conversation turn: {error}"),
    })
}

async fn finish_turn(
    foreground: &ModelSpawner<AgentDriver>,
    terminal_driver: &ModelHandle<TerminalDriver>,
    stream_id: ResponseStreamId,
    finished: StreamFinished,
) -> Result<(), AgentDriverError> {
    apply_event(
        foreground,
        terminal_driver,
        stream_id,
        response_event::Type::Finished(finished),
    )
    .await
}

async fn apply_actions(
    foreground: &ModelSpawner<AgentDriver>,
    terminal_driver: &ModelHandle<TerminalDriver>,
    stream_id: &ResponseStreamId,
    actions: Vec<ClientAction>,
) -> Result<(), AgentDriverError> {
    if actions.is_empty() {
        return Ok(());
    }
    apply_event(
        foreground,
        terminal_driver,
        stream_id.clone(),
        response_event::Type::ClientActions(response_event::ClientActions { actions }),
    )
    .await
}

async fn apply_event(
    foreground: &ModelSpawner<AgentDriver>,
    terminal_driver: &ModelHandle<TerminalDriver>,
    stream_id: ResponseStreamId,
    event: response_event::Type,
) -> Result<(), AgentDriverError> {
    with_ai_controller(foreground, terminal_driver, move |controller, ctx| {
        controller.apply_external_harness_event(
            &stream_id,
            ResponseEvent {
                r#type: Some(event),
            },
            ctx,
        );
    })
    .await
}

fn stream_finished_for(stop_reason: StopReason) -> StreamFinished {
    let reason = match stop_reason {
        StopReason::EndTurn | StopReason::Cancelled => {
            stream_finished::Reason::Done(stream_finished::Done {})
        }
        StopReason::MaxTokens => {
            stream_finished::Reason::MaxTokenLimit(stream_finished::ReachedMaxTokenLimit {})
        }
        StopReason::MaxTurnRequests | StopReason::Refusal | StopReason::Unknown => {
            stream_finished::Reason::InternalError(stream_finished::InternalError {
                message: format!("ACP agent stopped: {stop_reason:?}"),
            })
        }
    };
    StreamFinished {
        reason: Some(reason),
        ..Default::default()
    }
}

/// What answering the agent's requests needs from the driver: a way onto the foreground thread
/// to consult the permission model, and the shell whose quoting rules the policy parses
/// commands with.
struct AgentRequestContext {
    foreground: ModelSpawner<AgentDriver>,
    terminal_driver: ModelHandle<TerminalDriver>,
    escape_char: EscapeChar,
}

impl AgentRequestContext {
    async fn evaluate(&self, request: PolicyRequest) -> Result<PolicyDecision, RpcError> {
        let terminal_driver = self.terminal_driver.clone();
        let escape_char = self.escape_char;
        self.foreground
            .spawn(move |_, ctx| {
                let terminal = terminal_driver.as_ref(ctx).terminal_view().clone();
                let controller = terminal.as_ref(ctx).ai_controller().clone();
                request.evaluate(controller.as_ref(ctx), terminal.id(), escape_char, ctx)
            })
            .await
            .map_err(|_| RpcError::internal("the agent driver is no longer running"))
    }
}

/// Answers the requests an ACP agent may make of its client. Runs on the connection's read
/// loop (a background runtime thread), so file system work is moved to blocking threads to keep
/// protocol reads flowing.
fn agent_request_handler(context: Arc<AgentRequestContext>) -> connection::AgentRequestHandler {
    Arc::new(move |method, params| {
        let context = context.clone();
        match method {
            protocol::METHOD_REQUEST_PERMISSION => {
                async move { answer_permission_request(&context, params).await }.boxed()
            }
            protocol::METHOD_FS_READ_TEXT_FILE => blocking(move || read_text_file(params)).boxed(),
            protocol::METHOD_FS_WRITE_TEXT_FILE => {
                async move { write_text_file(&context, params).await }.boxed()
            }
            other => future::ready(Err(RpcError::method_not_found(other))).boxed(),
        }
    })
}

async fn blocking(
    work: impl FnOnce() -> Result<Value, RpcError> + Send + 'static,
) -> Result<Value, RpcError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("acp-local-file".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(RpcError::internal)?;
    rx.await.map_err(RpcError::internal)?
}

/// Grants only a single use explicitly allowed by local permission policy.
async fn answer_permission_request(
    context: &AgentRequestContext,
    params: Value,
) -> Result<Value, RpcError> {
    let params: RequestPermissionParams =
        serde_json::from_value(params).map_err(RpcError::internal)?;
    let decision = match PolicyRequest::for_tool_call(&params.tool_call) {
        Some(request) => context.evaluate(request).await?,
        None => PolicyDecision::Deny {
            reason: "the request cannot be checked against local permission policy".into(),
        },
    };
    serde_json::to_value(RequestPermissionResponse {
        outcome: select_permission(&decision, &params.options),
    })
    .map_err(RpcError::internal)
}

fn read_text_file(params: Value) -> Result<Value, RpcError> {
    let params: ReadTextFileParams = serde_json::from_value(params).map_err(RpcError::internal)?;
    if !Path::new(&params.path).is_absolute() {
        return Err(RpcError::internal(
            "ACP file paths must be absolute host-local paths",
        ));
    }
    let content = std::fs::read_to_string(&params.path)
        .with_context(|| format!("reading {}", params.path))
        .map_err(RpcError::internal)?;
    let content = match (params.line, params.limit) {
        (None, None) => content,
        (line, limit) => {
            let start = line.unwrap_or(1).saturating_sub(1) as usize;
            let lines = content.lines().skip(start);
            match limit {
                Some(limit) => lines.take(limit as usize).collect::<Vec<_>>().join("\n"),
                None => lines.collect::<Vec<_>>().join("\n"),
            }
        }
    };
    serde_json::to_value(ReadTextFileResponse { content }).map_err(RpcError::internal)
}

async fn write_text_file(context: &AgentRequestContext, params: Value) -> Result<Value, RpcError> {
    let params: WriteTextFileParams = serde_json::from_value(params).map_err(RpcError::internal)?;
    if !Path::new(&params.path).is_absolute() {
        return Err(RpcError::internal(
            "ACP file paths must be absolute host-local paths",
        ));
    }
    if let PolicyDecision::Deny { reason } =
        context.evaluate(PolicyRequest::write(&params.path)).await?
    {
        log::warn!("Refusing ACP fs/write_text_file: {reason}");
        return Err(RpcError::internal(format!(
            "writing {} is not permitted: {reason}",
            params.path
        )));
    }
    blocking(move || {
        if let Some(parent) = Path::new(&params.path).parent() {
            std::fs::create_dir_all(parent).map_err(RpcError::internal)?;
        }
        std::fs::write(&params.path, params.content)
            .with_context(|| format!("writing {}", params.path))
            .map_err(RpcError::internal)?;
        Ok(Value::Null)
    })
    .await
}

fn mcp_server_for_acp(name: &str, server: &JSONMCPServer) -> McpServer {
    match &server.transport_type {
        JSONTransportType::CLIServer {
            command, args, env, ..
        } => McpServer::Stdio {
            name: name.to_owned(),
            command: command.clone(),
            args: args.clone(),
            env: env
                .iter()
                .map(|(name, value)| protocol::EnvVariable {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        },
        JSONTransportType::SSEServer { url, headers } => McpServer::Remote {
            kind: "http",
            name: name.to_owned(),
            url: url.clone(),
            headers: headers
                .iter()
                .map(|(name, value)| protocol::HttpHeader {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        },
    }
}
