#![cfg(not(feature = "native_shell_completions_suite"))]

mod common;
#[path = "integration/shell_integration_tests.rs"]
mod shell_integration_tests;
#[path = "integration/ui_tests.rs"]
mod ui_tests;
