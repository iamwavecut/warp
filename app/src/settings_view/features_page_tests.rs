use super::tab_behavior_for_bindings;
use crate::settings::TabBehavior;

#[test]
fn tab_binding_selects_completions_instead_of_user_defined() {
    assert_eq!(
        tab_behavior_for_bindings("Tab", "Right"),
        TabBehavior::Completions
    );
}

#[test]
fn tab_binding_selects_autosuggestions_instead_of_user_defined() {
    assert_eq!(
        tab_behavior_for_bindings("Ctrl-Space", "Tab"),
        TabBehavior::Autosuggestions
    );
}

#[test]
fn custom_bindings_remain_user_defined() {
    assert_eq!(
        tab_behavior_for_bindings("Ctrl-Space", "Right"),
        TabBehavior::UserDefined
    );
    assert_eq!(tab_behavior_for_bindings("", ""), TabBehavior::UserDefined);
}
