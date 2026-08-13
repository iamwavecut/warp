use super::*;

#[test]
fn decorated_mcp_titles_use_existing_local_product_icons() {
    let icon = ExternalProductIcon::from_string("GitHub (OAuth)")
        .expect("decorated GitHub title should resolve to its bundled icon");

    assert_eq!(icon.get_path(), "bundled/svg/github.svg");
}

#[test]
fn removed_product_icons_are_not_restored_by_prefix_matching() {
    assert!(ExternalProductIcon::from_string("Sentry (OAuth)").is_none());
    assert!(ExternalProductIcon::from_string("Slack workspace").is_none());
}
