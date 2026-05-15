use std::sync::OnceLock;

use serde::Serialize;
use warp_core::channel::{Channel, ChannelState};

#[cfg(not(target_family = "wasm"))]
mod docker;
#[cfg(not(target_family = "wasm"))]
mod kubernetes;

/// Environment variable that identifies the local isolation platform.
/// The value should match one of the `IsolationPlatformType` variants in snake_case.
#[cfg(not(target_family = "wasm"))]
const WARP_ISOLATION_PLATFORM_ENV: &str = "WARP_ISOLATION_PLATFORM";

/// A kind of isolation platform. For our usage, isolation platforms are different ways where Warp
/// can be sandboxed, such as local containers. This may also include weaker forms
/// of sandboxing such as Git worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationPlatformType {
    /// Warp is running within a Docker container. Note that this does *not* mean this is a Warp-hosted
    /// Docker Sandboxes environment. Instead, it's likely a self-hosted agent.
    #[cfg(not(target_family = "wasm"))]
    Docker,
    /// Warp is running within a Kubernetes pod, likely as a self-hosted agent.
    #[cfg(not(target_family = "wasm"))]
    Kubernetes,
}

/// Detect the current isolation platform, if any.
///
/// Results are memoized for the lifetime of the process.
pub fn detect() -> Option<IsolationPlatformType> {
    static DETECTED_PLATFORM: OnceLock<Option<IsolationPlatformType>> = OnceLock::new();

    *DETECTED_PLATFORM.get_or_init(|| {
        // This never applies to integration tests.
        if ChannelState::channel() == Channel::Integration {
            return None;
        }

        // Use a closure so we can early-return.
        #[allow(clippy::redundant_closure_call)]
        let platform = (|| {
            // If the environment explicitly told us which local platform we're on, trust it.
            // This takes priority over all heuristic-based detection.
            #[cfg(not(target_family = "wasm"))]
            if let Some(platform) = platform_from_env() {
                return Some(platform);
            }

            #[cfg(not(target_family = "wasm"))]
            if kubernetes::is_in_kubernetes() {
                return Some(IsolationPlatformType::Kubernetes);
            }

            #[cfg(not(target_family = "wasm"))]
            if docker::is_in_docker() {
                return Some(IsolationPlatformType::Docker);
            }

            None
        })();

        match platform {
            Some(platform) => {
                log::debug!("Detected isolation platform: {:?}", platform);
            }
            None => {
                log::info!("No isolation platform detected");
            }
        }

        platform
    })
}

/// Parse the `WARP_ISOLATION_PLATFORM` environment variable into a platform type.
#[cfg(not(target_family = "wasm"))]
fn platform_from_env() -> Option<IsolationPlatformType> {
    let value = std::env::var(WARP_ISOLATION_PLATFORM_ENV).ok()?;
    match value.as_str() {
        "docker" => Some(IsolationPlatformType::Docker),
        "kubernetes" => Some(IsolationPlatformType::Kubernetes),
        other => {
            log::warn!("Unknown {WARP_ISOLATION_PLATFORM_ENV} value: {other}");
            None
        }
    }
}
