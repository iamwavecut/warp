//! Functionality relating to services that the application provides
//! to the host system.
//!
//! For example, on macOS, this module sets up integrations with
//! Finder such that the user can open a new Warp tab or window
//! in a given directory.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
pub mod windows;

use warpui::AppContext;

/// Crash-recovery watcher processes must remain alive to monitor their parent, not forward a
/// synthetic new-window request back to it.
#[cfg(any(target_os = "linux", windows, test))]
pub(crate) fn should_forward_startup_args(is_crash_recovery_process: bool) -> bool {
    !is_crash_recovery_process
}

pub fn init(_ctx: &mut AppContext) {
    log::info!("Initializing app services");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::init(_ctx);
    #[cfg(target_os = "macos")]
    mac::init();
    #[cfg(windows)]
    windows::init(_ctx);
}

pub fn teardown(_ctx: &mut AppContext) {
    log::info!("Tearing down app services...");

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    linux::teardown(_ctx);
}

#[cfg(test)]
mod tests {
    use super::should_forward_startup_args;

    #[test]
    fn crash_recovery_watcher_does_not_forward_startup_arguments() {
        assert!(!should_forward_startup_args(true));
        assert!(should_forward_startup_args(false));
    }
}
