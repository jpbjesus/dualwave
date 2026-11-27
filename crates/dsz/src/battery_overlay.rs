//! Battery status, shown for a few seconds by a mapping and optionally when
//! a controller runs low. Windows uses an overlay and Linux uses a desktop
//! notification with the active profile.
//!
//! The window lives on its own thread with its own message loop, like the
//! tray icon, so it shows while the main window is hidden. That thread
//! starts the first time the overlay is shown, so with the overlay off
//! nothing runs. It never takes focus or the mouse, and only reads what the
//! device threads already publish: nothing is sent to the controllers.
//!
//! A normal window can't draw over a game in exclusive fullscreen; it shows
//! over borderless and windowed games and the desktop.

use std::sync::Arc;

use ds_proto::input::BatteryStatus;
use ds_proto::InputState;
#[cfg(test)]
use parking_lot::Mutex;


use crate::engine::Engine;
use crate::settings::OverlayCorner;

// ------------------------------------------------------------------ low battery
/// Low-battery warning: once when a controller on battery drops below the
/// threshold, then again at each lower 10 % step. Charging, or climbing
/// back over the threshold, starts over.
#[derive(Default)]
pub struct LowWatch {
    /// The lowest step already warned about.
    warned_at: Option<u8>,
}

impl LowWatch {
    pub fn update(&mut self, s: &InputState, threshold: u8) -> bool {
        // The short Bluetooth report carries no battery.
        if !s.full {
            return false;
        }
        let level = s.battery_percent;
        if s.battery_status != BatteryStatus::Discharging || level >= threshold {
            if s.battery_status != BatteryStatus::Error {
                self.warned_at = None;
            }
            return false;
        }
        if self.warned_at.is_some_and(|w| level >= w) {
            return false;
        }
        self.warned_at = Some(level);
        true
    }
}

// ------------------------------------------------------------------ window

#[cfg(test)]
static SHOWN: Mutex<Vec<(OverlayCorner, u32)>> = Mutex::new(Vec::new());

/// The overlays `show` was asked for since the last call (tests only).
#[cfg(test)]
pub(crate) fn take_shown() -> Vec<(OverlayCorner, u32)> {
    std::mem::take(&mut *SHOWN.lock())
}

/// How long it stays when nothing says otherwise: the low-battery warning
/// and a new mapping.
pub const DEFAULT_MS: u32 = 1500;
/// Shortest and longest the sliders allow (mapping and low-battery warning).
pub const MIN_MS: u32 = 500;
pub const MAX_MS: u32 = 15_000;

/// Bring a show time into the slider range. A hand-edited profile or
/// settings file can sit outside it.
pub fn clamp_ms(ms: u32) -> u32 {
    ms.clamp(MIN_MS, MAX_MS)
}

/// Show the overlay in `corner` for `ms` milliseconds (from now). Cheap and
/// non-blocking: callable from a device thread.
pub fn show(engine: &Arc<Engine>, corner: OverlayCorner, ms: u32) {
    // Tests record the call. A real one would open a window on the desktop.
    let ms = clamp_ms(ms);
    #[cfg(test)]
    SHOWN.lock().push((corner, ms));
    #[cfg(windows)]
    if !cfg!(test) { window::show(engine, corner, ms); }
    #[cfg(all(target_os = "linux", not(test)))]
    desktop::show(engine, ms);
    #[cfg(not(all(windows, not(test))))]
    let _ = (engine, corner, ms);
}

#[cfg(windows)]
mod window;
#[cfg(target_os = "linux")]
mod desktop;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_status_uses_live_battery_and_active_profile() {
        let dir = std::env::temp_dir().join(format!("dualwave-battery-status-{}", std::process::id()));
        let engine = Engine::new(dir.clone());
        let device = crate::device::Device::for_test(ds_proto::Connection::Bluetooth, "");
        engine.devices.write().push(Arc::clone(&device));
        device.live.lock().input = battery(60, BatteryStatus::Charging);
        let name = engine.profiles.add(crate::profile::Profile::named("Couch <test> & audio"));
        engine.set_device_profile(&device, &name);
        let body = desktop::body(&engine);
        assert!(body.contains("DualSense"));
        assert!(body.contains("65%"));
        assert!(body.contains("Bluetooth"));
        assert!(body.contains("Couch &lt;test&gt; &amp; audio"));
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn battery(percent: u8, status: BatteryStatus) -> InputState {
        InputState {
            full: true,
            battery_percent: percent,
            battery_status: status,
            ..Default::default()
        }
    }

    #[test]
    fn low_warning_once_per_lower_step_and_again_after_charging() {
        use BatteryStatus::*;
        let mut w = LowWatch::default();
        assert!(!w.update(&battery(30, Discharging), 20));
        assert!(!w.update(&battery(20, Discharging), 20));
        assert!(w.update(&battery(10, Discharging), 20));
        assert!(!w.update(&battery(10, Discharging), 20));
        assert!(w.update(&battery(0, Discharging), 20));
        assert!(!w.update(&battery(0, Discharging), 20));
        // Plugged in, then unplugged while still low: warns again.
        assert!(!w.update(&battery(0, Charging), 20));
        assert!(w.update(&battery(10, Discharging), 20));
    }

    #[test]
    fn low_warning_ignores_reports_without_a_battery() {
        let mut w = LowWatch::default();
        let mut s = battery(0, BatteryStatus::Discharging);
        s.full = false;
        assert!(!w.update(&s, 20));
        assert!(!w.update(&battery(0, BatteryStatus::Error), 20));
    }

    #[test]
    fn low_warning_is_off_by_default_and_old_settings_load_without_it() {
        let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert!(!old.battery_overlay.on_low);
        assert_eq!(old.battery_overlay, crate::settings::BatteryOverlay::default());
        let partial: crate::settings::Settings =
            serde_json::from_str(r#"{"battery_overlay":{"on_low":true}}"#).unwrap();
        assert!(partial.battery_overlay.on_low);
        assert_eq!(partial.battery_overlay.low_percent, 20);
        assert_eq!(partial.battery_overlay.show_ms, DEFAULT_MS);
    }

    #[test]
    fn the_mapping_action_saves_its_corner_and_time() {
        use crate::profile::Action;
        use crate::settings::OverlayCorner;
        let a = Action::ShowBatteryOverlay { corner: OverlayCorner::BottomLeft, ms: 7000 };
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, r#"{"kind":"show_battery_overlay","corner":"BottomLeft","ms":7000}"#);
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
        // Saved before it had a corner and time: the defaults.
        let bare: Action = serde_json::from_str(r#"{"kind":"show_battery_overlay"}"#).unwrap();
        assert_eq!(bare, Action::ShowBatteryOverlay { corner: OverlayCorner::TopRight, ms: DEFAULT_MS });
    }

    #[test]
    fn overlay_time_matches_the_ui() {
        assert_eq!(MIN_MS, 500);
        assert_eq!(MAX_MS, 15_000);
        assert_eq!(clamp_ms(0), MIN_MS);
        assert_eq!(clamp_ms(499), MIN_MS);
        assert_eq!(clamp_ms(500), 500);
        assert_eq!(clamp_ms(DEFAULT_MS), DEFAULT_MS);
        assert_eq!(clamp_ms(15_000), MAX_MS);
        assert_eq!(clamp_ms(15_001), MAX_MS);
        assert_eq!(clamp_ms(30_000), MAX_MS);
        assert_eq!(clamp_ms(u32::MAX), MAX_MS);
    }
}
