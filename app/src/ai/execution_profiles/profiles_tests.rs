use serde_json::Value;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{App, SingletonEntity};

use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::execution_profiles::ActionPermission;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::settings::PrivacySettings;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::LaunchMode;

fn install_singletons(app: &mut App) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
    app.add_singleton_model(PrivacySettings::mock);
}

#[test]
fn edits_persist_on_local_default_profile() {
    App::test((), |mut app| async move {
        install_singletons(&mut app);
        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });

        let default_profile_id = profile_model.read(&app, |model, _ctx| model.default_profile_id());

        profile_model.read(&app, |model, ctx| {
            assert!(
                model.default_profile(ctx).sync_id().is_none(),
                "fresh default profile should not have a persisted local id"
            );
            assert!(matches!(
                model.default_profile(ctx).data().apply_code_diffs,
                ActionPermission::AgentDecides
            ));
        });

        profile_model.update(&mut app, |model, ctx| {
            model.set_apply_code_diffs(default_profile_id, &ActionPermission::AlwaysAllow, ctx);
        });

        profile_model.read(&app, |model, ctx| {
            assert!(
                model.default_profile(ctx).sync_id().is_some(),
                "editing the default profile should assign a stable local id"
            );
            assert_eq!(
                model.default_profile(ctx).data().apply_code_diffs,
                ActionPermission::AlwaysAllow
            );
        });

        app.read(|ctx| {
            let persisted = ctx
                .private_user_preferences()
                .read_value("LocalAIExecutionProfiles")
                .expect("read local profile preferences")
                .expect("local profiles should be persisted");
            let value: Value =
                serde_json::from_str(&persisted).expect("local profiles should be valid JSON");
            assert_eq!(
                value["default_profile"]["profile"]["apply_code_diffs"],
                Value::String("AlwaysAllow".to_string())
            );
        });
    })
}
