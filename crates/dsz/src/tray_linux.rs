//! Freedesktop StatusNotifier tray with an egui window activation bridge.
//!
//! A desktop without a working notifier keeps the ordinary window visible.
//! Losing the notifier while hidden restores the window immediately.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use eframe::egui::{Context, ViewportCommand};
use ksni::blocking::TrayMethods;
use parking_lot::Mutex;

use crate::engine::Engine;

#[allow(dead_code)] // Retained native-handle API; Linux uses the egui context.
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
static ON_SHOW: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static TRAY: OnceLock<ksni::blocking::Handle<DualWaveTray>> = OnceLock::new();

#[derive(Default)]
struct Visibility {
    context: Option<Context>,
    available: bool,
    hidden: bool,
    pending_show: bool,
}
impl Visibility {
    fn show(&mut self) {
        self.hidden = false;
        self.pending_show = self.context.is_none();
        if let Some(context) = &self.context {
            context.send_viewport_cmd(ViewportCommand::Visible(true));
            context.send_viewport_cmd(ViewportCommand::Minimized(false));
            context.send_viewport_cmd(ViewportCommand::Focus);
            context.request_repaint();
        }
    }

    fn hide(&mut self) {
        if !self.available { self.show(); return; }
        self.hidden = true;
        if let Some(context) = &self.context {
            context.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }

    fn notifier_offline(&mut self) {
        self.available = false;
        if self.hidden { self.show(); }
    }
}

fn visibility() -> &'static Mutex<Visibility> {
    static VISIBILITY: OnceLock<Mutex<Visibility>> = OnceLock::new();
    VISIBILITY.get_or_init(|| Mutex::new(Visibility::default()))
}

#[allow(dead_code)] // Retained native-handle API for shared callers.
pub fn set_main_window(handle: isize) { MAIN_WINDOW.store(handle, Ordering::Release); }
#[allow(dead_code)] // Retained native-handle API for shared callers.
pub fn main_window() -> *mut std::ffi::c_void { MAIN_WINDOW.load(Ordering::Acquire) as *mut std::ffi::c_void }

pub fn set_context(context: &Context) {
    let mut visibility = visibility().lock();
    visibility.context = Some(context.clone());
    if visibility.pending_show || !visibility.available { visibility.show(); }
    else if visibility.hidden { context.send_viewport_cmd(ViewportCommand::Visible(false)); }
}

/// Call after start() and before constructing the native viewport.
pub fn set_start_hidden(hidden: bool) {
    let mut visibility = visibility().lock();
    visibility.hidden = hidden && visibility.available && !visibility.pending_show;
}

pub fn available() -> bool { visibility().lock().available }
pub fn is_hidden() -> bool { visibility().lock().hidden }

pub fn show_main_window() {
    visibility().lock().show();
    if let Some(on_show) = ON_SHOW.get() { on_show(); }
}

pub fn hide_main_window() { visibility().lock().hide(); }

pub use crate::icon_art::icon_rgba;

fn argb_icon(size: usize) -> Vec<u8> {
    icon_rgba(size).chunks_exact(4).flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]]).collect()
}

pub fn notify(title: &str, text: &str) {
    let title = title.to_owned();
    let text = text.to_owned();
    let action = format!("--action=default={}", crate::i18n::tr("Open DSZ").replace("DSZ", "DualWave"));
    let _ = std::thread::Builder::new().name("notification".into()).spawn(move || {
        match std::process::Command::new("notify-send")
            .args(["--app-name=DualWave", "--icon=dualwave", "--expire-time=10000", &action, "--", &title, &text]).output()
        {
            Ok(output) if output.status.success() => {
                if String::from_utf8_lossy(&output.stdout).trim() == "default" {
                    crate::update::request_settings();
                    show_main_window();
                }
            },
            Ok(output) => log::debug!("desktop notification failed: {}", String::from_utf8_lossy(&output.stderr).trim()),
            Err(error) => log::debug!("desktop notification unavailable: {error}"),
        }
    });
}

struct DualWaveTray { engine: Arc<Engine> }
impl ksni::Tray for DualWaveTray {
    fn id(&self) -> String { "dualwave".into() }
    fn title(&self) -> String { crate::APP_TITLE.into() }
    fn status(&self) -> ksni::Status { ksni::Status::Active }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon { width: 32, height: 32, data: argb_icon(32) }]
    }
    fn activate(&mut self, _x: i32, _y: i32) { show_main_window(); }
    fn secondary_activate(&mut self, _x: i32, _y: i32) { show_main_window(); }
    fn tool_tip(&self) -> ksni::ToolTip {
        let devices = self.engine.device_list();
        let description = devices.iter().map(|device| {
            format!("{} {}%", device.model.name(), device.live.lock().input.battery_percent)
        }).collect::<Vec<_>>().join(", ");
        ksni::ToolTip { title: crate::APP_TITLE.into(), description, ..Default::default() }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
        let current = self.engine.device_list().first().map(|device| self.engine.profile_name_for(device))
            .unwrap_or_else(|| self.engine.settings.read().default_profile.clone());
        let profiles = self.engine.profiles.names().into_iter().take(50).map(|name| {
            CheckmarkItem {
                checked: name == current,
                label: name.clone(),
                activate: Box::new(move |tray: &mut Self| {
                    let devices = tray.engine.device_list();
                    if devices.is_empty() {
                        tray.engine.settings.write().default_profile = name.clone();
                        tray.engine.settings_changed();
                    }
                    for device in devices {
                        tray.engine.set_device_profile(&device, &name);
                        device.identify_light();
                    }
                }),
                ..Default::default()
            }.into()
        }).collect();
        vec![
            StandardItem {
                label: crate::i18n::tr("Open DSZ").replace("DSZ", "DualWave"),
                activate: Box::new(|_| show_main_window()), ..Default::default()
            }.into(),
            ksni::MenuItem::Separator,
            SubMenu { label: crate::i18n::tr("Profile").into(), submenu: profiles, ..Default::default() }.into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: crate::i18n::tr("Quit").into(),
                activate: Box::new(|_| {
                    // shutdown() waits for controller workers. Keep the D-Bus
                    // service responsive while they drain their output.
                    let _ = std::thread::Builder::new().name("quit".into()).spawn(|| quit());
                }), ..Default::default()
            }.into(),
        ]
    }
    fn watcher_online(&self) { visibility().lock().available = true; }
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        log::warn!("desktop tray unavailable: {reason:?}; keeping window accessible");
        visibility().lock().notifier_offline();
        true
    }
}

pub fn quit() -> ! {
    if let Some(engine) = ENGINE.get() { engine.shutdown(); }
    crate::inputsim::release_everything();
    std::process::exit(0);
}

pub fn start(engine: Arc<Engine>, on_show: impl Fn() + Send + Sync + 'static) {
    let _ = ENGINE.set(engine.clone());
    let _ = ON_SHOW.set(Box::new(on_show));
    if TRAY.get().is_some() { return; }
    visibility().lock().available = true;
    match (DualWaveTray { engine }).spawn() {
        Ok(handle) => {
            if handle.is_closed() { visibility().lock().notifier_offline(); }
            let monitor = handle.clone();
            let _ = TRAY.set(handle);
            if let Err(error) = std::thread::Builder::new().name("tray-refresh".into()).spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    if monitor.update(|_| {}).is_none() {
                        visibility().lock().notifier_offline();
                        break;
                    }
                }
            }) { log::warn!("could not start tray refresh: {error}"); }
        }
        Err(error) => {
            log::warn!("desktop tray unavailable: {error}; keeping window visible");
            visibility().lock().notifier_offline();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_notifier_means_hiding_keeps_a_way_back() {
        let mut state = Visibility::default();
        state.hide();
        assert!(!state.hidden);
        assert!(state.pending_show);
    }

    #[test]
    fn losing_notifier_restores_a_hidden_window() {
        let mut state = Visibility { available: true, ..Default::default() };
        state.hide();
        assert!(state.hidden);
        state.notifier_offline();
        assert!(!state.available);
        assert!(!state.hidden);
        assert!(state.pending_show);
    }

    #[test]
    fn icon_uses_status_notifier_network_argb_byte_order() {
        let rgba = icon_rgba(32);
        let argb = argb_icon(32);
        assert_eq!(argb.len(), 32 * 32 * 4);
        for (rgba, argb) in rgba.chunks_exact(4).zip(argb.chunks_exact(4)) {
            assert_eq!(argb, [rgba[3], rgba[0], rgba[1], rgba[2]]);
        }
    }
}
