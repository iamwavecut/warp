mod builder;
mod step;

#[cfg(not(feature = "native_shell_completions_suite"))]
pub mod test;
#[cfg(feature = "native_shell_completions_suite")]
#[path = "native_shell_suite.rs"]
pub mod test;
pub mod user_defaults;
pub mod util;

pub use builder::Builder;
pub use warp::integration_testing::view_getters;
pub use warpui::integration::{AssertionOutcome, TestStep};
