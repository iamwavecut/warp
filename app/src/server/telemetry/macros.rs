/// Sends a telemetry event to Rudderstack immediately instead of adding it to the event queue that is
/// periodically flushed. This is useful under certain conditions where we want to ensure an event
/// is immediately sent to Rudderstack even if the user quits before the queue is flushed.
#[macro_export]
macro_rules! send_telemetry_sync_from_ctx {
    ($event:expr, $ctx:expr) => {
        let _ = (&$event, &$ctx);
    };
}

/// Sends a telemetry event to Rudderstack immediately. This is the same as [`send_telemetry_sync_from_ctx`],
/// but can be used when the caller only has access to an [`App`] and not a
/// `ViewContext`.
#[macro_export]
macro_rules! send_telemetry_sync_from_app_ctx {
    ($event:expr, $app_ctx:expr) => {
        let _ = (&$event, &$app_ctx);
    };
}

/// Sends a telemetry `track` event Rudderstack asynchronously. This is the same as the
/// [`send_telemetry_from_ctx`], except can be called any time you have an Arc<Background>.
/// This should only be called when invoking one of the other macros isn't possible; for example,
/// when you are already on a background thread and thus can't access any app context.
#[macro_export]
macro_rules! send_telemetry_on_executor {
    ($auth_state: expr, $event:expr, $executor:expr) => {
        let _ = (&$auth_state, &$event, &$executor);
    };
}
