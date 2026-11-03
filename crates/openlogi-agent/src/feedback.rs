//! Local patch: haptic pulses for fired gestures and, on Hyprland, for the
//! pointer moving to another monitor. Both reuse the Actions Ring's haptic
//! player and its per-device `haptics` toggle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use openlogi_agent_core::orchestrator::Orchestrator;
use openlogi_hid::HapticWaveform;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::debug;

use crate::server::RingHapticPlayer;

/// Two pulses closer than this collapse into one.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Play a confirmation pulse for every gesture the HID++ watcher dispatched.
pub(crate) fn spawn_gesture_feedback(
    mut fired: UnboundedReceiver<String>,
    orchestrator: Arc<Mutex<Orchestrator>>,
    haptics: RingHapticPlayer,
) {
    tokio::spawn(async move {
        while let Some(key) = fired.recv().await {
            let route = orchestrator.lock().await.feedback_haptic_route(Some(&key));
            debug!(key, has_route = route.is_some(), "gesture haptic");
            haptics.play(route, HapticWaveform::DampStateChange, "gesture");
        }
    });
}

/// Pulse when Hyprland reports a monitor focus change (`focusedmon`).
#[cfg(target_os = "linux")]
pub(crate) fn spawn_monitor_feedback(
    orchestrator: Arc<Mutex<Orchestrator>>,
    haptics: RingHapticPlayer,
) {
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut last = Instant::now() - DEBOUNCE;
        loop {
            let Some(path) = hyprland_event_socket() else {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };
            match tokio::net::UnixStream::connect(&path).await {
                Ok(stream) => {
                    debug!(path = %path.display(), "listening for Hyprland monitor changes");
                    let mut lines = BufReader::new(stream).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if !line.starts_with("focusedmon>>") || last.elapsed() < DEBOUNCE {
                            continue;
                        }
                        last = Instant::now();
                        let route = orchestrator.lock().await.feedback_haptic_route(None);
                        debug!(line, has_route = route.is_some(), "monitor haptic");
                        haptics.play(route, HapticWaveform::SubtleCollision, "monitor");
                    }
                }
                Err(error) => debug!(%error, "Hyprland event socket unavailable"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// The event socket of the running Hyprland instance. After a Hyprland
/// restart the inherited signature is stale, so fall back to the newest one.
#[cfg(target_os = "linux")]
fn hyprland_event_socket() -> Option<std::path::PathBuf> {
    let base = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr");
    let named = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .map(|signature| base.join(signature).join(".socket2.sock"))
        .filter(|path| path.exists());
    named.or_else(|| {
        std::fs::read_dir(&base)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path().join(".socket2.sock"))
            .filter(|path| path.exists())
            .max_by_key(|path| path.metadata().and_then(|m| m.modified()).ok())
    })
}
