//! Focused real-shell suite independent of legacy hosted integration fixtures.
//!
//! Run with `WARP_SHELL_PATH=/bin/zsh cargo test -p integration
//! --features native_shell_completions_suite --test native_shell_completions`.

#[path = "test/native_shell_completions.rs"]
mod native_shell_completions;

pub use native_shell_completions::*;

use crate::Builder;

fn new_builder() -> Builder {
    Builder::new()
}
