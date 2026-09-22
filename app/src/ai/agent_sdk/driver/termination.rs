use std::{future, io};

#[cfg(unix)]
mod unix;

/// A signal that should interrupt the standalone SDK run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Interrupt {
    Terminate,
    Interrupt,
}

/// Owns active platform signal registrations, or remains inert on unsupported platforms.
pub(super) struct InterruptWatch {
    inner: InterruptWatchInner,
}

enum InterruptWatchInner {
    #[cfg(unix)]
    Active(unix::InterruptWatch),
    Noop,
}

impl InterruptWatch {
    /// Register the local signal handlers for this one SDK run.
    pub(super) fn register() -> io::Result<Self> {
        #[cfg(unix)]
        {
            return Ok(Self {
                inner: InterruptWatchInner::Active(unix::InterruptWatch::register()?),
            });
        }

        #[cfg(not(unix))]
        {
            Ok(Self::noop())
        }
    }

    pub(super) fn noop() -> Self {
        Self {
            inner: InterruptWatchInner::Noop,
        }
    }

    /// Wait for the first signal. An unsupported/no-op watch never resolves.
    pub(super) async fn wait(&mut self) -> Interrupt {
        match &mut self.inner {
            #[cfg(unix)]
            InterruptWatchInner::Active(inner) => inner.wait().await,
            InterruptWatchInner::Noop => future::pending().await,
        }
    }

    /// Stop watching after normal completion while retaining default termination behavior.
    pub(super) fn disarm(self) {
        match self.inner {
            #[cfg(unix)]
            InterruptWatchInner::Active(inner) => inner.disarm(),
            InterruptWatchInner::Noop => {}
        }
    }

    /// Restore the signal's default action and terminate the SDK process.
    pub(super) fn terminate(self, interrupt: Interrupt) -> ! {
        match self.inner {
            #[cfg(unix)]
            InterruptWatchInner::Active(inner) => inner.terminate(interrupt),
            InterruptWatchInner::Noop => {
                let _ = interrupt;
                unreachable!("a no-op interrupt watch cannot deliver an interrupt")
            }
        }
    }
}
