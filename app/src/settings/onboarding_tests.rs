use ai::LLMId;
use onboarding::slides::{AgentAutonomy, AgentDevelopmentSettings, ProjectOnboardingSettings};
use onboarding::SelectedSettings;
use serde_json::json;
use warp_core::user_preferences::GetUserPreferences;
use warpui::App;

use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::execution_profiles::{AIExecutionProfile, ActionPermission};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::server::ids::{ClientId, SyncId};
use crate::settings::{apply_onboarding_settings, PrivacySettings};
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::LaunchMode;

#[test]
fn apply_onboarding_settings_preserves_existing_local_profile() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| TemplatableMCPServerManager::default());
        app.add_singleton_model(PrivacySettings::mock);
        app.add_singleton_model(UserWorkspaces::default_mock);

        let local_stored_model = LLMId::from("local-existing-model");
        let local_profile = AIExecutionProfile {
            name: "Default".to_string(),
            is_default_profile: true,
            base_model: Some(local_stored_model.clone()),
            apply_code_diffs: ActionPermission::AlwaysAllow,
            read_files: ActionPermission::AlwaysAllow,
            execute_commands: ActionPermission::AlwaysAllow,
            mcp_permissions: ActionPermission::AlwaysAllow,
            ..Default::default()
        };
        let local_sync_id = SyncId::ClientId(ClientId::new());

        app.update(|ctx| {
            let persisted = json!({
                "default_profile": {
                    "id": local_sync_id,
                    "profile": local_profile,
                },
                "profiles": [],
            });
            ctx.private_user_preferences()
                .write_value(
                    "LocalAIExecutionProfiles",
                    serde_json::to_string(&persisted).expect("serialize local profile"),
                )
                .expect("write local profile");
        });

        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });

        profile_model.read(&app, |model, ctx| {
            let info = model.default_profile(ctx);
            assert_eq!(info.sync_id(), Some(local_sync_id));
            assert_eq!(info.data().base_model, Some(local_stored_model.clone()));
        });

        let onboarding_settings = SelectedSettings::AgentDrivenDevelopment {
            agent_settings: AgentDevelopmentSettings {
                selected_model_id: LLMId::from("onboarding-chosen-model"),
                autonomy: Some(AgentAutonomy::None),
                cli_agent_toolbar_enabled: true,
                session_default: onboarding::SessionDefault::Agent,
                disable_oz: false,
                show_agent_notifications: true,
            },
            project_settings: ProjectOnboardingSettings::default(),
            ui_customization: None,
        };

        app.update(|ctx| {
            apply_onboarding_settings(&onboarding_settings, ctx);
        });

        profile_model.read(&app, |model, ctx| {
            let info = model.default_profile(ctx);
            assert_eq!(info.sync_id(), Some(local_sync_id));
            assert_eq!(info.data().base_model, Some(local_stored_model.clone()));
            assert_eq!(info.data().apply_code_diffs, ActionPermission::AlwaysAllow);
            assert_eq!(info.data().read_files, ActionPermission::AlwaysAllow);
            assert_eq!(info.data().execute_commands, ActionPermission::AlwaysAllow);
            assert_eq!(info.data().mcp_permissions, ActionPermission::AlwaysAllow);
        });
    })
}
