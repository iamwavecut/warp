//! macOS process-level setup for headless launches.

use anyhow::{Result, bail};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Tells macOS to treat this process as a background-only application, so it
/// never gets a Dock tile for a headless invocation.
pub(crate) fn mark_process_as_background_only() -> Result<()> {
    let Some(main_thread) = MainThreadMarker::new() else {
        bail!("must be called on the main thread");
    };

    let app = NSApplication::sharedApplication(main_thread);
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited) {
        bail!("NSApplication::setActivationPolicy(.prohibited) returned false");
    }

    Ok(())
}
