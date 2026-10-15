use super::*;

#[test]
fn pi_child_panes_remain_unsupported_regardless_of_legacy_gate() {
    for enabled in [false, true] {
        let _flag = FeatureFlag::LocalClaudeCodexChildHarnesses.override_enabled(enabled);
        assert!(!local_child_harness_is_enabled(Harness::Pi));
        assert!(
            local_child_harness_disabled_message(Harness::Pi)
                .unwrap()
                .contains("ACP")
        );
    }
}
