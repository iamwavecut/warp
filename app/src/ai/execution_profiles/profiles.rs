use core::fmt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use crate::ai::llms::LLMId;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerManagerEvent;

use crate::ai::mcp::TemplatableMCPServerManager;
use crate::server::ids::ClientId;
use crate::server::ids::SyncId;
use crate::settings::AgentModeCommandExecutionPredicate;
use crate::LaunchMode;

use super::{AIExecutionProfile, ActionPermission, WriteToPtyPermission};

const LOCAL_PROFILES_PREF_KEY: &str = "LocalAIExecutionProfiles";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedAIExecutionProfile {
    id: SyncId,
    profile: AIExecutionProfile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedAIExecutionProfiles {
    default_profile: PersistedAIExecutionProfile,
    profiles: Vec<PersistedAIExecutionProfile>,
}

/// ExecutionProfileId is the identifier that users of the AIExecutionProfilesModel use
/// to refer back to a specific profile. These are unique across the lifespan of the app.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClientProfileId(usize);

impl ClientProfileId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> ClientProfileId {
        static NEXT_PROFILE_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_PROFILE_ID.fetch_add(1, Ordering::Relaxed);
        ClientProfileId(raw)
    }
}

impl fmt::Display for ClientProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

#[derive(Clone, Debug)]
pub struct AIExecutionProfileInfo {
    id: ClientProfileId,
    sync_id: Option<SyncId>,
    data: AIExecutionProfile,
}

impl AIExecutionProfileInfo {
    pub fn id(&self) -> &ClientProfileId {
        &self.id
    }

    /// Stable local ID for this profile, if it has been persisted.
    pub fn sync_id(&self) -> Option<SyncId> {
        self.sync_id
    }

    pub fn data(&self) -> &AIExecutionProfile {
        &self.data
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DefaultProfileState {
    Local {
        id: ClientProfileId,
        sync_id: Option<SyncId>,
        profile: AIExecutionProfile,
    },
    /// Currently, the behavior of the CLI default is that it
    /// cannot be updated and will never be persisted.
    #[allow(dead_code)]
    Cli {
        id: ClientProfileId,
        profile: AIExecutionProfile,
    },
}

impl std::fmt::Display for DefaultProfileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultProfileState::Local { sync_id, .. } => {
                if sync_id.is_some() {
                    write!(f, "LocalSaved")
                } else {
                    write!(f, "LocalUnsaved")
                }
            }
            DefaultProfileState::Cli { .. } => write!(f, "CLI"),
        }
    }
}

impl DefaultProfileState {
    pub fn id(&self) -> ClientProfileId {
        match self {
            DefaultProfileState::Local { id, .. } => *id,
            DefaultProfileState::Cli { id, .. } => *id,
        }
    }
}

pub struct AIExecutionProfilesModel {
    /// The default profile starts as unsaved local data and becomes backed by a
    /// local ID when edited or loaded from private preferences.
    default_profile_state: DefaultProfileState,
    profile_id_to_sync_id: HashMap<ClientProfileId, SyncId>,
    profile_id_to_profile: HashMap<ClientProfileId, AIExecutionProfile>,
    /// Only contains entries for non-default profiles.
    active_profiles_per_session: HashMap<EntityId, ClientProfileId>,
}

impl AIExecutionProfilesModel {
    #[allow(unused_variables)]
    pub fn new(launch_mode: &LaunchMode, ctx: &mut ModelContext<Self>) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(feature = "agent_mode_evals")] {
                let default_profile_state = DefaultProfileState::Local {
                    id: ClientProfileId::new(),
                    sync_id: None,
                    profile: AIExecutionProfile::create_agent_mode_eval_profile(),
                };
                let profile_id_to_sync_id: HashMap<ClientProfileId, SyncId> = HashMap::new();
                let profile_id_to_profile: HashMap<ClientProfileId, AIExecutionProfile> = HashMap::new();
                let active_profiles_per_session: HashMap<EntityId, ClientProfileId> = HashMap::new();
            } else {
                let active_profiles_per_session: HashMap<EntityId, ClientProfileId> = HashMap::new();

                let default_profile_state = match launch_mode {
                    LaunchMode::App { .. } | LaunchMode::Test { .. } | LaunchMode::RemoteServerProxy | LaunchMode::RemoteServerDaemon { .. } => {
                        Self::load_local_profiles(ctx).unwrap_or_else(|| {
                            (
                                DefaultProfileState::Local {
                                    id: ClientProfileId::new(),
                                    sync_id: None,
                                    profile: AIExecutionProfile::create_default_from_legacy_settings(ctx),
                                },
                                HashMap::new(),
                                HashMap::new(),
                            )
                        })
                    }
                    // When running as a CLI, we ignore the GUI default and use a more permissive default.
                    LaunchMode::CommandLine { is_sandboxed, computer_use_override, .. } => {
                        (
                            DefaultProfileState::Cli {
                                profile: AIExecutionProfile::create_default_cli_profile(*is_sandboxed, *computer_use_override),
                                id: ClientProfileId::new()
                            },
                            HashMap::new(),
                            HashMap::new(),
                        )
                    }
                };
                let (default_profile_state, profile_id_to_sync_id, profile_id_to_profile) = default_profile_state;
            }
        }

        ctx.subscribe_to_model(
            &TemplatableMCPServerManager::handle(ctx),
            |me, event, ctx| {
                me.handle_templatable_mcp_server_manager_event(event, ctx);
            },
        );

        log::info!("Initialized execution profile model with state: {default_profile_state}",);

        let mut model = Self {
            default_profile_state,
            profile_id_to_sync_id,
            profile_id_to_profile,
            active_profiles_per_session,
        };

        model.maybe_inherit_from_legacy_settings(ctx);
        model
    }

    fn load_local_profiles(
        ctx: &AppContext,
    ) -> Option<(
        DefaultProfileState,
        HashMap<ClientProfileId, SyncId>,
        HashMap<ClientProfileId, AIExecutionProfile>,
    )> {
        let serialized = ctx
            .private_user_preferences()
            .read_value(LOCAL_PROFILES_PREF_KEY)
            .ok()
            .flatten()?;

        let persisted: PersistedAIExecutionProfiles = serde_json::from_str(&serialized)
            .map_err(|err| {
                log::error!("Failed to read local AI execution profiles: {err}");
                err
            })
            .ok()?;

        let default_profile_id = ClientProfileId::new();
        let mut default_profile = persisted.default_profile.profile;
        default_profile.is_default_profile = true;
        let default_profile_state = DefaultProfileState::Local {
            id: default_profile_id,
            sync_id: Some(persisted.default_profile.id),
            profile: default_profile,
        };

        let mut profile_id_to_sync_id = HashMap::new();
        let mut profile_id_to_profile = HashMap::new();

        for mut persisted_profile in persisted.profiles {
            let profile_id = ClientProfileId::new();
            persisted_profile.profile.is_default_profile = false;
            profile_id_to_sync_id.insert(profile_id, persisted_profile.id);
            profile_id_to_profile.insert(profile_id, persisted_profile.profile);
        }

        Some((
            default_profile_state,
            profile_id_to_sync_id,
            profile_id_to_profile,
        ))
    }

    fn persist_local_profiles(&self, ctx: &AppContext) {
        let DefaultProfileState::Local {
            sync_id: Some(default_profile_id),
            profile: default_profile,
            ..
        } = &self.default_profile_state
        else {
            return;
        };

        let mut profiles = Vec::new();
        for (profile_id, sync_id) in &self.profile_id_to_sync_id {
            let Some(profile) = self.profile_id_to_profile.get(profile_id) else {
                continue;
            };
            profiles.push(PersistedAIExecutionProfile {
                id: *sync_id,
                profile: profile.clone(),
            });
        }

        let persisted = PersistedAIExecutionProfiles {
            default_profile: PersistedAIExecutionProfile {
                id: *default_profile_id,
                profile: default_profile.clone(),
            },
            profiles,
        };

        match serde_json::to_string(&persisted) {
            Ok(serialized) => {
                if let Err(err) = ctx
                    .private_user_preferences()
                    .write_value(LOCAL_PROFILES_PREF_KEY, serialized)
                {
                    log::error!("Failed to persist local AI execution profiles: {err}");
                }
            }
            Err(err) => {
                log::error!("Failed to serialize local AI execution profiles: {err}");
            }
        }
    }

    fn ensure_default_profile_local_id(&mut self) -> Option<SyncId> {
        let DefaultProfileState::Local { sync_id, .. } = &mut self.default_profile_state else {
            return None;
        };
        Some(*sync_id.get_or_insert_with(|| SyncId::ClientId(ClientId::new())))
    }

    /// This function performs one-time migrations from legacy settings into the default profile.
    /// The issue this solves is that, whenever we migrate an existing setting into the profile object,
    /// users will initialize the new field to its default value. We need to manually check to see if
    /// the legacy setting hasn't been migrated and, if it hasn't, do a one-time overwrite on the new profile
    /// field.
    fn maybe_inherit_from_legacy_settings(&mut self, ctx: &mut ModelContext<Self>) {
        let default_profile_id = self.default_profile_state.id();

        if let Some(base_llm_id) = ctx
            .private_user_preferences()
            .read_value("PreferredAgentModeLLMId")
            .ok()
            .flatten()
            .map(|s| serde_json::from_str::<Option<LLMId>>(&s))
            .and_then(|res| res.ok())
            .flatten()
        {
            if let Err(e) = ctx
                .private_user_preferences()
                .remove_value("PreferredAgentModeLLMId")
            {
                log::error!("Failed to remove old PreferredAgentModeLLMId user pref: {e}");
            }
            self.set_base_model(default_profile_id, Some(base_llm_id.clone()), ctx);
            log::info!("Overwrote default profile with legacy setting for base llm: {base_llm_id}");
        }
    }

    pub fn create_profile(&mut self, ctx: &mut ModelContext<Self>) -> Option<ClientProfileId> {
        let profile_id = ClientProfileId::new();

        let mut new_profile = self.default_profile(ctx).data().clone();
        new_profile.name = "".to_string();
        new_profile.is_default_profile = false;
        new_profile.autosync_plans_to_warp_drive = false;

        let sync_id = SyncId::ClientId(ClientId::new());

        self.profile_id_to_sync_id.insert(profile_id, sync_id);
        self.profile_id_to_profile.insert(profile_id, new_profile);
        self.persist_local_profiles(ctx);

        ctx.emit(AIExecutionProfilesModelEvent::ProfileCreated);

        Some(profile_id)
    }

    pub fn delete_profile(&mut self, profile_id: ClientProfileId, ctx: &mut ModelContext<Self>) {
        let id = self.default_profile_state.id();
        if id == profile_id {
            log::warn!("Attempted to delete default profile (id: {profile_id})");
            return;
        }

        if !self.profile_id_to_sync_id.contains_key(&profile_id) {
            return;
        }

        self.active_profiles_per_session
            .retain(|_, active_profile_id| *active_profile_id != profile_id);

        self.profile_id_to_sync_id.remove(&profile_id);
        self.profile_id_to_profile.remove(&profile_id);
        self.persist_local_profiles(ctx);

        ctx.emit(AIExecutionProfilesModelEvent::ProfileDeleted);
    }

    // Auth logout is unreachable in the local-first fork, but tests still use this to reset state.
    pub fn reset(&mut self) {
        self.default_profile_state = DefaultProfileState::Local {
            id: ClientProfileId::new(),
            sync_id: None,
            profile: AIExecutionProfile {
                is_default_profile: true,
                ..Default::default()
            },
        };
        self.profile_id_to_sync_id.clear();
        self.profile_id_to_profile.clear();
        self.active_profiles_per_session.clear();
    }

    /// Returns the active permissions profile for a specific terminal view.
    /// If no terminal_view is provided, returns the default profile.
    ///
    /// If you need to account for enterprise overrides, call `BlocklistAIPermissions::active_permissions_profile` instead.
    pub fn active_profile(
        &self,
        terminal_view_id: Option<EntityId>,
        ctx: &AppContext,
    ) -> AIExecutionProfileInfo {
        terminal_view_id
            .and_then(|id| self.active_profiles_per_session.get(&id))
            .and_then(|profile_id| self.get_profile_by_id(*profile_id, ctx))
            .unwrap_or_else(|| self.default_profile(ctx))
    }

    pub fn default_profile_id(&self) -> ClientProfileId {
        self.default_profile_state.id()
    }

    pub fn default_profile(&self, _ctx: &AppContext) -> AIExecutionProfileInfo {
        match &self.default_profile_state {
            DefaultProfileState::Local {
                id,
                sync_id,
                profile,
            } => AIExecutionProfileInfo {
                id: *id,
                sync_id: *sync_id,
                data: profile.clone(),
            },
            DefaultProfileState::Cli { id, profile } => AIExecutionProfileInfo {
                id: *id,
                sync_id: None,
                data: profile.clone(),
            },
        }
    }

    /// Sets the active profile for a specific terminal view.
    pub fn set_active_profile(
        &mut self,
        terminal_view_id: EntityId,
        profile_id: ClientProfileId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.active_profiles_per_session
            .insert(terminal_view_id, profile_id);
        ctx.emit(AIExecutionProfilesModelEvent::UpdatedActiveProfile { terminal_view_id });
    }

    /// Returns a profile by its client ID.
    /// Returns None if the profile is not found.
    pub fn get_profile_by_id(
        &self,
        profile_id: ClientProfileId,
        _ctx: &AppContext,
    ) -> Option<AIExecutionProfileInfo> {
        match &self.default_profile_state {
            DefaultProfileState::Local {
                id,
                sync_id,
                profile,
            } => {
                if profile_id == *id {
                    return Some(AIExecutionProfileInfo {
                        id: *id,
                        sync_id: *sync_id,
                        data: profile.clone(),
                    });
                }
            }
            DefaultProfileState::Cli { id, profile } => {
                if profile_id == *id {
                    return Some(AIExecutionProfileInfo {
                        id: *id,
                        sync_id: None,
                        data: profile.clone(),
                    });
                }
            }
        }

        let sync_id = self.profile_id_to_sync_id.get(&profile_id)?;
        let data = self.profile_id_to_profile.get(&profile_id)?.clone();

        Some(AIExecutionProfileInfo {
            id: profile_id,
            sync_id: Some(*sync_id),
            data,
        })
    }

    pub fn get_all_profile_ids(&self) -> Vec<ClientProfileId> {
        let default_profile_id = self.default_profile_state.id();

        // Default profile is always first in the list
        std::iter::once(default_profile_id)
            .chain(
                self.profile_id_to_sync_id
                    .keys()
                    .filter(|&&id| id != default_profile_id)
                    .cloned(),
            )
            .collect()
    }

    /// Look up a runtime profile ID from its stable local profile ID.
    pub fn get_profile_id_by_sync_id(&self, sync_id: &SyncId) -> Option<ClientProfileId> {
        if let DefaultProfileState::Local {
            id,
            sync_id: Some(default_sync_id),
            ..
        } = &self.default_profile_state
        {
            if *default_sync_id == *sync_id {
                return Some(*id);
            }
        }

        self.profile_id_to_sync_id
            .iter()
            .find_map(|(client_id, id)| {
                if id == sync_id {
                    Some(*client_id)
                } else {
                    None
                }
            })
    }

    pub fn has_multiple_profiles(&self) -> bool {
        !self.profile_id_to_profile.is_empty()
    }

    pub fn set_base_model(
        &mut self,
        profile_id: ClientProfileId,
        llm_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.base_model != llm_id {
                    profile.base_model = llm_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_coding_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.coding_model != model_id {
                    profile.coding_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_cli_agent_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.cli_agent_model != model_id {
                    profile.cli_agent_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_computer_use_model(
        &mut self,
        profile_id: ClientProfileId,
        model_id: Option<LLMId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.computer_use_model != model_id {
                    profile.computer_use_model = model_id.clone();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_context_window_limit(
        &mut self,
        profile_id: ClientProfileId,
        limit: Option<u32>,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.context_window_limit != limit {
                    profile.context_window_limit = limit;
                    return true;
                }
                false
            },
            ctx,
        );

        if changed {}
    }

    pub fn set_apply_code_diffs(
        &mut self,
        profile_id: ClientProfileId,
        apply_code_diffs: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.apply_code_diffs != *apply_code_diffs {
                    profile.apply_code_diffs = *apply_code_diffs;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_read_files(
        &mut self,
        profile_id: ClientProfileId,
        read_files: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.read_files != *read_files {
                    profile.read_files = *read_files;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_execute_commands(
        &mut self,
        profile_id: ClientProfileId,
        execute_commands: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.execute_commands != *execute_commands {
                    profile.execute_commands = *execute_commands;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_write_to_pty(
        &mut self,
        profile_id: ClientProfileId,
        write_to_pty: &WriteToPtyPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.write_to_pty != *write_to_pty {
                    profile.write_to_pty = *write_to_pty;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_mcp_permissions(
        &mut self,
        profile_id: ClientProfileId,
        mcp_permissions: &ActionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.mcp_permissions == *mcp_permissions {
                    return false;
                }

                if mcp_permissions == &ActionPermission::AlwaysAllow {
                    profile.mcp_allowlist.clear();
                } else if mcp_permissions == &ActionPermission::AlwaysAsk {
                    profile.mcp_denylist.clear();
                }
                profile.mcp_permissions = *mcp_permissions;
                true
            },
            ctx,
        );
    }

    pub fn set_computer_use(
        &mut self,
        profile_id: ClientProfileId,
        permission: &super::ComputerUsePermission,
        ctx: &mut ModelContext<Self>,
    ) {
        let current_value = self
            .get_profile_by_id(profile_id, ctx)
            .map(|p| p.data().computer_use);

        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.computer_use != *permission {
                    profile.computer_use = *permission;
                    return true;
                }
                false
            },
            ctx,
        );

        if current_value != Some(*permission) {}
    }

    pub fn set_ask_user_question(
        &mut self,
        profile_id: ClientProfileId,
        permission: super::AskUserQuestionPermission,
        ctx: &mut ModelContext<Self>,
    ) {
        let current_value = self
            .get_profile_by_id(profile_id, ctx)
            .map(|p| p.data().ask_user_question);

        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.ask_user_question != permission {
                    profile.ask_user_question = permission;
                    return true;
                }
                false
            },
            ctx,
        );

        if current_value != Some(permission) {}
    }

    pub fn set_web_search_enabled(
        &mut self,
        profile_id: ClientProfileId,
        enabled: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.web_search_enabled != enabled {
                    profile.web_search_enabled = enabled;
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn set_profile_name(
        &mut self,
        profile_id: ClientProfileId,
        name: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if profile.name != name {
                    profile.name = name.to_string();
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn add_to_command_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.command_allowlist.contains(predicate) {
                    profile.command_allowlist.push(predicate.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_command_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.command_allowlist.len();
                profile.command_allowlist.retain(|p| p != predicate);
                profile.command_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_directory_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        path: &PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.directory_allowlist.contains(path) {
                    profile.directory_allowlist.push(path.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_directory_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        path: &PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.directory_allowlist.len();
                profile.directory_allowlist.retain(|p| p != path);
                profile.directory_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_command_denylist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.command_denylist.contains(predicate) {
                    profile.command_denylist.push(predicate.clone());
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_command_denylist(
        &mut self,
        profile_id: ClientProfileId,
        predicate: &AgentModeCommandExecutionPredicate,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.command_denylist.len();
                profile.command_denylist.retain(|p| p != predicate);
                profile.command_denylist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_mcp_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.mcp_allowlist.contains(id) {
                    profile.mcp_allowlist.push(*id);
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_mcp_allowlist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.mcp_allowlist.len();
                profile.mcp_allowlist.retain(|p| p != id);
                profile.mcp_allowlist.len() != original_len
            },
            ctx,
        );
    }

    pub fn add_to_mcp_denylist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                if !profile.mcp_denylist.contains(id) {
                    profile.mcp_denylist.push(*id);
                    return true;
                }
                false
            },
            ctx,
        );
    }

    pub fn remove_from_mcp_denylist(
        &mut self,
        profile_id: ClientProfileId,
        id: &Uuid,
        ctx: &mut ModelContext<Self>,
    ) {
        self.edit_profile_internal(
            profile_id,
            |profile| {
                let original_len = profile.mcp_denylist.len();
                profile.mcp_denylist.retain(|p| p != id);
                profile.mcp_denylist.len() != original_len
            },
            ctx,
        );
    }

    /// `edit_profile_internal` edits an AIExecutionProfile and persists it locally.
    /// Parameters:
    /// * `profile_id`: The id of the profile to edit
    /// * `edit_fn`: a closure that safely modifies the AIExecutionProfile. It should return `true` if the profile was changed, `false` otherwise.
    /// * `ctx`: The model context
    ///
    /// Returns `true` if the profile was actually changed. Callers can use this to gate side effects such as diagnostics on real changes.
    fn edit_profile_internal(
        &mut self,
        profile_id: ClientProfileId,
        edit_fn: impl FnOnce(&mut AIExecutionProfile) -> bool,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        // We don't yet support editing the default profile for the CLI.
        if let DefaultProfileState::Cli { id, .. } = &self.default_profile_state {
            if *id == profile_id {
                log::warn!("Attempted to edit CLI default profile, which is not yet supported.");
                return false;
            }
        }

        if let DefaultProfileState::Local { id, profile, .. } = &self.default_profile_state {
            if *id == profile_id {
                let mut new_profile = profile.clone();
                let value_changed = edit_fn(&mut new_profile);
                if !value_changed {
                    return false;
                }

                self.ensure_default_profile_local_id();
                if let DefaultProfileState::Local { profile, .. } = &mut self.default_profile_state
                {
                    *profile = new_profile;
                }
                self.persist_local_profiles(ctx);
                log::info!("Edited local default execution profile: {profile_id:?}");
                ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(profile_id));
                return true;
            }
        }

        let Some(profile) = self.profile_id_to_profile.get_mut(&profile_id) else {
            return false;
        };
        let value_changed = edit_fn(profile);
        if !value_changed {
            return false;
        }
        self.persist_local_profiles(ctx);
        log::info!("Edited local execution profile with id: {profile_id:?}");
        ctx.emit(AIExecutionProfilesModelEvent::ProfileUpdated(profile_id));
        true
    }

    fn handle_templatable_mcp_server_manager_event(
        &mut self,
        event: &TemplatableMCPServerManagerEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            TemplatableMCPServerManagerEvent::TemplatableMCPServersUpdated => {
                self.remove_deleted_mcp_servers(ctx);
            }
            TemplatableMCPServerManagerEvent::StateChanged { uuid: _, state: _ }
            | TemplatableMCPServerManagerEvent::ServerInstallationAdded(_)
            | TemplatableMCPServerManagerEvent::ServerInstallationDeleted(_) => {}
        }
    }

    /// Handle deleted MCP servers by deleting its uuid from all profiles.
    fn remove_deleted_mcp_servers(&mut self, ctx: &mut ModelContext<Self>) {
        let all_valid_uuids = TemplatableMCPServerManager::get_all_local_template_mcp_servers(ctx);
        for profile_id in self.get_all_profile_ids() {
            self.edit_profile_internal(
                profile_id,
                |profile| {
                    let original_allowlist_len = profile.mcp_allowlist.len();
                    let original_denylist_len = profile.mcp_denylist.len();
                    profile
                        .mcp_allowlist
                        .retain(|uuid| all_valid_uuids.contains_key(uuid));
                    profile
                        .mcp_denylist
                        .retain(|uuid| all_valid_uuids.contains_key(uuid));
                    profile.mcp_allowlist.len() != original_allowlist_len
                        || profile.mcp_denylist.len() != original_denylist_len
                },
                ctx,
            );
        }
    }

    /// Replaces the given profile's data with CLI defaults for the given sandboxed state.
    /// Use in tests to simulate the profile configuration used by the sandboxed CLI agent.
    #[cfg(test)]
    pub fn apply_cli_profile_defaults_for_test(
        &mut self,
        profile_id: ClientProfileId,
        is_sandboxed: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let cli_profile = AIExecutionProfile::create_default_cli_profile(is_sandboxed, None);
        self.edit_profile_internal(
            profile_id,
            move |profile| {
                *profile = cli_profile;
                true
            },
            ctx,
        );
    }
}

#[allow(clippy::enum_variant_names)]
pub enum AIExecutionProfilesModelEvent {
    ProfileUpdated(ClientProfileId),
    ProfileCreated,
    ProfileDeleted,
    UpdatedActiveProfile { terminal_view_id: EntityId },
}

impl Entity for AIExecutionProfilesModel {
    type Event = AIExecutionProfilesModelEvent;
}

impl SingletonEntity for AIExecutionProfilesModel {}

#[cfg(test)]
#[path = "profiles_tests.rs"]
mod tests;
