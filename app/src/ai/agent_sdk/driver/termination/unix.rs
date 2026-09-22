use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::{SigId, flag};
use warpui::r#async::Timer;

use super::Interrupt;

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Owns the Unix signal registrations used by one standalone SDK run.
pub(super) struct InterruptWatch {
    sigterm: Arc<AtomicBool>,
    sigint: Arc<AtomicBool>,
    _handlers: RegisteredHandlers,
}

impl InterruptWatch {
    pub(super) fn register() -> std::io::Result<Self> {
        let mut handlers = RegisteredHandlers::default();
        let sigterm = Arc::new(AtomicBool::new(false));
        let sigint = Arc::new(AtomicBool::new(false));

        for (signal, observed) in [
            (SIGTERM, Arc::clone(&sigterm)),
            (SIGINT, Arc::clone(&sigint)),
        ] {
            // Keep this fallback for the remaining standalone process lifetime. Unregistering
            // signal-hook's last action does not restore the OS default disposition.
            flag::register_conditional_default(signal, Arc::clone(&handlers.terminate))?;
            handlers.push(flag::register(signal, observed)?);
            handlers.push(flag::register(signal, Arc::clone(&handlers.terminate))?);
        }

        Ok(Self {
            sigterm,
            sigint,
            _handlers: handlers,
        })
    }

    pub(super) async fn wait(&mut self) -> Interrupt {
        loop {
            if self.sigterm.load(Ordering::SeqCst) {
                log::warn!("Received SIGTERM; saving local agent state before termination");
                return Interrupt::Terminate;
            }
            if self.sigint.load(Ordering::SeqCst) {
                log::warn!("Received SIGINT; saving local agent state before termination");
                return Interrupt::Interrupt;
            }
            Timer::after(POLL_INTERVAL).await;
        }
    }

    pub(super) fn disarm(self) {
        drop(self);
    }

    pub(super) fn terminate(self, interrupt: Interrupt) -> ! {
        // Keep the registrations alive while restoring the default action. This also ensures a
        // concurrent second signal cannot be swallowed by a dropped flag handler.
        let _watch = self;
        let _ = signal_hook::low_level::emulate_default_handler(interrupt.as_raw());
        signal_hook::low_level::abort();
    }
}

impl Interrupt {
    fn as_raw(self) -> i32 {
        match self {
            Self::Terminate => SIGTERM,
            Self::Interrupt => SIGINT,
        }
    }
}

#[derive(Default)]
struct RegisteredHandlers {
    registrations: Vec<SigId>,
    terminate: Arc<AtomicBool>,
}

impl RegisteredHandlers {
    fn push(&mut self, id: SigId) {
        self.registrations.push(id);
    }
}

impl Drop for RegisteredHandlers {
    fn drop(&mut self) {
        // Also applies to partial registration failure: any installed dispatcher retains a
        // default-action fallback instead of silently swallowing a later shutdown signal.
        self.terminate.store(true, Ordering::SeqCst);
        for id in self.registrations.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

#[cfg(test)]
#[path = "unix_tests.rs"]
mod tests;
