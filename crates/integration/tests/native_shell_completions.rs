#[path = "common/mod.rs"]
mod common;

integration_tests! {
    test_zsh_native_completions_without_compinit_use_filepaths,
    test_zsh_native_completions_preserve_candidates_on_error,
    test_native_shell_completions_menu,
}
