use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use ai::project_context::local_rule_repository::{LocalRuleRepository, ProjectRuleFile};
use uuid::Uuid;
use warpui::{WindowId, integration::TestStep, windowing::WindowManager};

use crate::{ai::facts::view::AIFactPage, integration_testing::view_getters::workspace_view};

static CREATED_RULE_PATHS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
static FIXTURE_ROOT: OnceLock<PathBuf> = OnceLock::new();

fn created_rule_paths() -> &'static Mutex<BTreeSet<PathBuf>> {
    CREATED_RULE_PATHS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn fixture_root() -> &'static PathBuf {
    FIXTURE_ROOT.get_or_init(|| {
        std::env::temp_dir().join(format!("warp-local-rule-tests-{}", Uuid::new_v4()))
    })
}

pub(crate) fn registered_rule_count() -> usize {
    created_rule_paths()
        .lock()
        .expect("local rule fixture lock poisoned")
        .iter()
        .filter(|path| path.is_file())
        .count()
}

/// Create a file-backed project rule and save its exact path into step data.
/// The fixture uses the same production repository and CAS path as the UI.
pub fn create_a_personal_rule(
    key: impl Into<String>,
    _name: impl Into<String>,
    content: impl Into<String>,
) -> TestStep {
    let key = key.into();
    let content = content.into();
    TestStep::new("Create a local project rule")
        .with_action(move |_, _, data| {
            let root = fixture_root().join(format!("project-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("create local rule fixture root");
            let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
            let created = repository
                .create_project(&root, ProjectRuleFile::Warp, &content)
                .expect("create local project rule");
            created_rule_paths()
                .lock()
                .expect("local rule fixture lock poisoned")
                .insert(created.path.clone());
            data.insert(key.clone(), created.path);
        })
        .add_assertion(move |_, _| {
            warpui::async_assert!(
                registered_rule_count() > 0,
                "Local rule exists at its exact managed path"
            )
        })
}

/// Open the local Rules pane in the active tab of the window saved at `window_key`.
pub fn open_rule_pane(window_key: impl Into<String>, key: impl Into<String>) -> TestStep {
    let window_key = window_key.into();
    let key = key.into();

    TestStep::new("Open local rule pane").with_action(move |app, _, data| {
        let window_id: &WindowId = data.get(&window_key).expect("No saved window ID");
        let path: &PathBuf = data.get(&key).expect("No saved local rule path");
        assert!(path.is_absolute(), "rule path must remain absolute");
        workspace_view(app, *window_id).update(app, |workspace, ctx| {
            WindowManager::as_ref(ctx).show_window_and_focus_app(*window_id);
            workspace.open_ai_fact_collection_pane(None, Some(AIFactPage::Rules), ctx);
        })
    })
}

/// Update a rule through the production local repository using its captured CAS revision.
pub fn update_rule_content(
    fact_key: impl Into<String>,
    new_content: impl Into<String>,
) -> TestStep {
    let fact_key = fact_key.into();
    let new_content = new_content.into();
    TestStep::new("Update local rule content").with_action(move |_, _, data| {
        let path: &PathBuf = data.get(&fact_key).expect("No saved local rule path");
        let root = path.parent().expect("local rule must have a parent");
        let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.to_path_buf()]);
        let current = repository
            .read(path)
            .expect("read local rule before update");
        let updated = repository
            .update(path, &current.revision, &new_content)
            .expect("update local rule");
        assert_eq!(updated.path, *path, "update must preserve exact rule path");
    })
}
