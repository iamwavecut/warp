use crate::auth::auth_state::AuthStateProvider;
use crate::remote_server::auth_context::server_api_auth_context;
use instant::Instant;
use remote_server::auth::RemoteServerAuthContext;
use settings::Setting;
use std::path::PathBuf;
use std::sync::Arc;
use warp_core::SessionId;
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity, WeakModelHandle};

use crate::terminal::warpify::settings::SshExtensionInstallMode;

use crate::remote_server::manager::{RemoteServerManager, RemoteServerManagerEvent};
use crate::remote_server::ssh_transport::SshTransport;
use crate::server::server_api::ServerApiProvider;
use crate::settings::PrivacySettings;
use crate::terminal::model::session::{IsLegacySSHSession, SessionInfo};
use crate::terminal::model_events::{ModelEvent, ModelEventDispatcher};
use crate::terminal::warpify::settings::WarpifySettings;
use remote_server::setup::{PreinstallCheckResult, PreinstallStatus};
use remote_server::transport::Error;

use super::pty_controller::{EventLoopSender, PtyController};

/// Per-SSH-init state machine. Encoding the state as an enum makes invalid
/// transitions unrepresentable and ensures the `SessionInfo` stash cannot be
/// accessed after it has been consumed.
///
/// Every active state carries `setup_start` so that the total setup duration
/// can be measured when the flow reaches `SessionConnected`.
enum SshInitState {
    Idle,
    /// Stash held, `check_binary` in flight.
    AwaitingCheck {
        session_info: SessionInfo,
        transport: SshTransport,
        setup_start: Instant,
    },
    /// Stash held, choice block showing.
    AwaitingUserChoice {
        session_info: SessionInfo,
        transport: SshTransport,
        setup_start: Instant,
    },
    /// Stash held, `install_binary` in flight.
    AwaitingInstall {
        session_id: SessionId,
        session_info: SessionInfo,
        transport: SshTransport,
        setup_start: Instant,
    },
    /// Stash held, `connect_session` in flight. Bootstrap is flushed only
    /// once `SessionConnected` arrives (or on connection failure).
    AwaitingConnect {
        session_id: SessionId,
        session_info: SessionInfo,
        setup_start: Instant,
    },
}

/// Per-pane orchestrator that defers the bootstrap script write for SSH sessions,
/// checks for the remote-server binary, and presents a two-option choice block when the binary is missing.
///
/// Uses a [`WeakModelHandle`] back to [`PtyController`] to avoid preventing
/// `PtyController` from being deallocated.
pub struct RemoteServerController<T: EventLoopSender> {
    pty_controller: WeakModelHandle<PtyController<T>>,
    model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
    state: SshInitState,
    /// Whether the binary was installed during this setup flow.
    did_install: bool,
}

impl<T: EventLoopSender> Entity for RemoteServerController<T> {
    type Event = ();
}

impl<T: EventLoopSender> RemoteServerController<T> {
    pub fn new(
        pty_controller: WeakModelHandle<PtyController<T>>,
        model_event_dispatcher: ModelHandle<ModelEventDispatcher>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&model_event_dispatcher, |me, event, ctx| {
            if let ModelEvent::SshInitShell {
                pending_session_info,
            } = event
            {
                me.on_ssh_init_shell_requested(pending_session_info.as_ref().clone(), ctx);
            }
        });

        let mgr = RemoteServerManager::handle(ctx);
        ctx.subscribe_to_model(&mgr, |me, event, ctx| match event {
            RemoteServerManagerEvent::BinaryCheckComplete {
                session_id,
                result,
                remote_platform,
                preinstall_check,
                has_old_binary,
            } => {
                let _ = remote_platform;
                me.on_binary_check_complete(
                    *session_id,
                    result.clone(),
                    preinstall_check.clone(),
                    *has_old_binary,
                    ctx,
                );
            }
            RemoteServerManagerEvent::BinaryInstallComplete {
                session_id,
                result,
                install_source: _,
            } => {
                me.on_binary_install_complete(*session_id, result.clone(), ctx);
            }
            RemoteServerManagerEvent::SessionConnected { session_id, .. } => {
                me.on_session_connected(*session_id, ctx);
            }
            RemoteServerManagerEvent::SessionConnectionFailed { session_id, .. } => {
                me.on_session_connection_failed(*session_id, ctx);
            }
            RemoteServerManagerEvent::SessionConnecting { .. }
            | RemoteServerManagerEvent::SessionDisconnected { .. }
            | RemoteServerManagerEvent::SessionReconnected { .. }
            | RemoteServerManagerEvent::SessionDeregistered { .. }
            | RemoteServerManagerEvent::HostConnected { .. }
            | RemoteServerManagerEvent::HostDisconnected { .. }
            | RemoteServerManagerEvent::NavigatedToDirectory { .. }
            | RemoteServerManagerEvent::RepoMetadataSnapshot { .. }
            | RemoteServerManagerEvent::RepoMetadataUpdated { .. }
            | RemoteServerManagerEvent::RepoMetadataDirectoryLoaded { .. }
            | RemoteServerManagerEvent::CodebaseIndexStatusesSnapshot { .. }
            | RemoteServerManagerEvent::CodebaseIndexStatusUpdated { .. }
            | RemoteServerManagerEvent::SetupStateChanged { .. }
            | RemoteServerManagerEvent::ClientRequestFailed { .. }
            | RemoteServerManagerEvent::ServerMessageDecodingError { .. }
            | RemoteServerManagerEvent::BufferUpdated { .. }
            | RemoteServerManagerEvent::BufferConflictDetected { .. }
            | RemoteServerManagerEvent::DiffStateSnapshotReceived { .. }
            | RemoteServerManagerEvent::DiffStateMetadataUpdateReceived { .. }
            | RemoteServerManagerEvent::DiffStateFileDeltaReceived { .. } => {}
        });

        Self {
            pty_controller,
            model_event_dispatcher,
            state: SshInitState::Idle,
            did_install: false,
        }
    }

    /// Extracts the `SessionInfo` from the stash and writes the bootstrap
    /// script to the PTY via `PtyController::initialize_shell`.
    fn flush_stashed_bootstrap(&mut self, session_info: SessionInfo, ctx: &mut ModelContext<Self>) {
        if let Some(pty) = self.pty_controller.upgrade(ctx) {
            pty.update(ctx, |pty, ctx| {
                pty.initialize_shell(&session_info, ctx);
            });
        } else {
            log::warn!("Remote server PtyController dropped before bootstrap could be flushed");
        }
    }

    /// Idle -> AwaitingCheck
    fn on_ssh_init_shell_requested(&mut self, info: SessionInfo, ctx: &mut ModelContext<Self>) {
        let IsLegacySSHSession::Yes { socket_path } = &info.is_legacy_ssh_session else {
            return;
        };
        let session_id = info.session_id;
        let socket_path = socket_path.clone();
        debug_assert!(matches!(self.state, SshInitState::Idle));
        match std::mem::replace(&mut self.state, SshInitState::Idle) {
            SshInitState::Idle => {}
            SshInitState::AwaitingCheck {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingUserChoice {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingInstall {
                session_info: old_info,
                ..
            }
            | SshInitState::AwaitingConnect {
                session_info: old_info,
                ..
            } => {
                self.flush_stashed_bootstrap(old_info, ctx);
            }
        }
        let transport = SshTransport::new(socket_path, self.build_auth_context(ctx));
        self.did_install = false;
        self.state = SshInitState::AwaitingCheck {
            session_info: info,
            transport: transport.clone(),
            setup_start: Instant::now(),
        };
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.check_binary(session_id, transport, ctx);
        });
    }

    fn on_binary_check_complete(
        &mut self,
        session_id: SessionId,
        result: Result<bool, Arc<Error>>,
        preinstall_check: Option<PreinstallCheckResult>,
        has_old_binary: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingCheck {
            ref session_info, ..
        } = self.state
        else {
            return;
        };
        if session_info.session_id != session_id {
            return;
        }

        let SshInitState::AwaitingCheck {
            session_info,
            transport,
            setup_start,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingCheck above");
        };

        // Preinstall gate. Runs **before** any user-visible install
        // affordance: if the script positively classified the host as
        // unsupported, skip the install/prompt entirely and fall back to
        // the legacy ControlMaster-backed SSH flow.
        let unsupported = preinstall_check
            .as_ref()
            .and_then(|check| match &check.status {
                PreinstallStatus::Unsupported { reason } => Some((check, reason.clone())),
                PreinstallStatus::Supported | PreinstallStatus::Unknown => None,
            });
        if let Some((check, reason)) = unsupported {
            log::info!(
                "Remote server preinstall check classified as unsupported, falling back to legacy SSH: session={session_id:?} status={:?}",
                check.status
            );
            RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
                mgr.mark_setup_unsupported(session_id, reason, ctx);
            });
            self.flush_stashed_bootstrap(session_info, ctx);
            return;
        }

        match result {
            Ok(true) => {
                let socket_path = transport.socket_path().clone();
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                    setup_start,
                };
                self.connect_session_for_current_identity(session_id, socket_path, ctx);
            }
            Ok(false) if has_old_binary => {
                self.did_install = true;
                self.state = SshInitState::AwaitingInstall {
                    session_id,
                    session_info,
                    transport: transport.clone(),
                    setup_start,
                };
                RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
                    mgr.install_binary(session_id, transport, true, ctx);
                });
            }
            Ok(false) => {
                let install_mode = *WarpifySettings::as_ref(ctx)
                    .ssh_extension_install_mode
                    .value();
                match install_mode {
                    SshExtensionInstallMode::AlwaysAsk => {
                        self.state = SshInitState::AwaitingUserChoice {
                            session_info,
                            transport,
                            setup_start,
                        };
                        self.model_event_dispatcher.update(ctx, |d, ctx| {
                            d.request_remote_server_block(session_id, ctx);
                        });
                    }
                    SshExtensionInstallMode::AlwaysInstall => {
                        self.did_install = true;
                        self.state = SshInitState::AwaitingInstall {
                            session_id,
                            session_info,
                            transport: transport.clone(),
                            setup_start,
                        };
                        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
                            mgr.install_binary(session_id, transport, false, ctx);
                        });
                    }
                    SshExtensionInstallMode::NeverInstall => {
                        self.flush_stashed_bootstrap(session_info, ctx);
                    }
                }
            }
            Err(err) => {
                log::warn!("Remote server binary check failed: session={session_id:?} error={err}");
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    pub fn handle_ssh_remote_server_install(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { .. } = self.state else {
            log::warn!(
                "Remote server install requested in unexpected state: session={session_id:?}"
            );
            return;
        };

        let SshInitState::AwaitingUserChoice {
            session_info,
            transport,
            setup_start,
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingUserChoice above");
        };

        self.did_install = true;
        self.state = SshInitState::AwaitingInstall {
            session_id,
            session_info,
            transport: transport.clone(),
            setup_start,
        };
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.install_binary(session_id, transport, false, ctx);
        });
    }

    /// Called when the remote server session is connected. Flushes the
    /// stashed bootstrap (so the session initializes with a live client)
    /// and emits the `RemoteServerSetupDuration` diagnostics event.
    fn on_session_connected(&mut self, session_id: SessionId, ctx: &mut ModelContext<Self>) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect {
            session_info,
            setup_start,
            ..
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };

        // Flush the stashed bootstrap now that the server is connected.
        // `client_for_session` will return `Some` when the session
        // subsequently initializes, so it picks `RemoteServerCommandExecutor`.
        self.flush_stashed_bootstrap(session_info, ctx);

        let _ = setup_start;
    }

    /// Called when the remote server connection failed. Flushes the stashed
    /// bootstrap so the SSH session is not permanently blocked.
    fn on_session_connection_failed(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingConnect {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingConnect { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingConnect above");
        };
        log::warn!("Remote server connection failed: session={session_id:?}");
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    pub fn handle_ssh_remote_server_skip(
        &mut self,
        session_id: SessionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingUserChoice { session_info, .. } =
            std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            log::warn!("Remote server skip requested in unexpected state: session={session_id:?}");
            return;
        };
        self.flush_stashed_bootstrap(session_info, ctx);
    }

    fn on_binary_install_complete(
        &mut self,
        session_id: SessionId,
        result: Result<(), Arc<Error>>,
        ctx: &mut ModelContext<Self>,
    ) {
        let SshInitState::AwaitingInstall {
            session_id: expected,
            ..
        } = &self.state
        else {
            return;
        };
        if *expected != session_id {
            return;
        }

        let SshInitState::AwaitingInstall {
            session_info,
            transport,
            setup_start,
            ..
        } = std::mem::replace(&mut self.state, SshInitState::Idle)
        else {
            unreachable!("just matched AwaitingInstall above");
        };
        match result {
            Ok(()) => {
                let socket_path = transport.socket_path().clone();
                self.state = SshInitState::AwaitingConnect {
                    session_id,
                    session_info,
                    setup_start,
                };
                self.connect_session_for_current_identity(session_id, socket_path, ctx);
            }
            Err(err) => {
                log::warn!(
                    "Remote server binary install failed: session={session_id:?} error={err}"
                );
                self.flush_stashed_bootstrap(session_info, ctx);
            }
        }
    }

    /// Builds a fresh [`RemoteServerAuthContext`] for the current identity.
    ///
    /// This fork does not forward crash-reporting preferences to remote daemons.
    fn build_auth_context(&self, ctx: &ModelContext<Self>) -> Arc<RemoteServerAuthContext> {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
        let auth_client = ServerApiProvider::as_ref(ctx).get_auth_client();
        Arc::new(server_api_auth_context(auth_state, auth_client))
    }

    fn connect_session_for_current_identity(
        &mut self,
        session_id: SessionId,
        socket_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        let auth_context = self.build_auth_context(ctx);
        let transport = SshTransport::new(socket_path, auth_context.clone());
        RemoteServerManager::handle(ctx).update(ctx, |mgr, ctx| {
            mgr.connect_session(session_id, transport, auth_context, ctx);
        });
    }
}
