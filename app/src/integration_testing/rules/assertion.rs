use std::path::PathBuf;

use ai::project_context::local_rule_repository::LocalRuleRepository;
use warpui::{
    async_assert, async_assert_eq,
    integration::{AssertionCallback, AssertionWithDataCallback},
};

use crate::{
    ai::facts::view::AIFactPage,
    integration_testing::{rules::step::registered_rule_count, view_getters::workspace_view},
};

/// Assert that a specific local rule exists at its captured exact path.
pub fn assert_rule_exists(
    expected_path_key: impl Into<String>,
    expected_content: impl Into<String>,
) -> AssertionWithDataCallback {
    let expected_path_key = expected_path_key.into();
    let expected_content = expected_content.into();
    Box::new(move |_, _window_id, data| {
        let path: &PathBuf = data
            .get(&expected_path_key)
            .expect("No saved local rule path");
        let root = path.parent().expect("local rule must have a parent");
        let repository = LocalRuleRepository::new_for_test(Vec::new(), [root.to_path_buf()]);
        match repository.read(path) {
            Ok(rule) => {
                async_assert_eq!(rule.path, *path, "Local rule path should match");
                async_assert_eq!(
                    rule.content,
                    expected_content,
                    "Local rule content should match"
                )
            }
            Err(error) => async_assert!(false, "Local rule should exist: {error}"),
        }
    })
}

/// Assert the number of local fixtures created by this integration process.
pub fn assert_rule_count(expected_count: usize) -> AssertionCallback {
    Box::new(move |_, _| {
        async_assert_eq!(
            registered_rule_count(),
            expected_count,
            "Local rule count should match"
        )
    })
}

pub fn assert_rule_pane_open(key: impl Into<String>) -> AssertionWithDataCallback {
    let key = key.into();
    Box::new(move |app, window_id, data| {
        let path: &PathBuf = data.get(&key).expect("No saved local rule path");
        let root = path.parent().expect("local rule must have a parent");
        let repository = LocalRuleRepository::new_for_test(Vec::new(), [root.to_path_buf()]);
        let path_is_readable = repository.read(path).is_ok();
        workspace_view(app, window_id).read(app, |workspace, _ctx| {
            workspace.ai_fact_view().read(app, |ai_fact_view, _ctx| {
                let current_page = ai_fact_view.current_page();
                async_assert!(
                    path_is_readable && current_page == AIFactPage::Rules,
                    "Local rule pane should be open for the exact path"
                )
            })
        })
    })
}
