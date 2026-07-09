//! Compatibility re-exports for error logging.
//!
//! The implementation lives in `warp_errors`, which stays local-only in this
//! fork and does not upload diagnostics to remote incident services.

pub use warp_errors::*;
