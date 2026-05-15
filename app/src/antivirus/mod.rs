//! Module containing utilities to query the currently running antivirus / EDR software on the
//! user's machine.

#[cfg(windows)]
mod windows;

use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that reports the currently running antivirus software.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct AntivirusInfo(Option<String>);

#[cfg(not(windows))]
#[derive(Debug, Clone)]
pub struct AntivirusInfo;

impl AntivirusInfo {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        #[cfg(windows)]
        {
            ctx.spawn(async move { Self::scan().await }, Self::on_scan_complete);
            Self(None)
        }

        #[cfg(not(windows))]
        {
            let _ = ctx;
            Self
        }
    }
}

pub enum AntivirusInfoEvent {
    #[allow(dead_code)]
    ScannedComplete,
}

impl Entity for AntivirusInfo {
    type Event = AntivirusInfoEvent;
}

impl SingletonEntity for AntivirusInfo {}
