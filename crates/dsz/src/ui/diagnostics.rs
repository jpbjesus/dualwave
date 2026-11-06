//! Live observations only: rendering never opens HID or PipeWire connections.
use std::sync::{Arc, atomic::Ordering};
use eframe::egui::{self, RichText};
use crate::{device::{Device, Link}, i18n::tr, profile::VirtualKind};
use super::{App, Page, theme::*, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State { Ready, Active, Waiting, Disabled, Unavailable, NotChecked, NotApplicable }
impl State {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready", Self::Active => "Active", Self::Waiting => "Waiting",
            Self::Disabled => "Disabled", Self::Unavailable => "Unavailable",
            Self::NotChecked => "Not checked", Self::NotApplicable => "Not applicable",
        }
    }
}

fn microphone_state(demand: bool, admitted: bool) -> State {
    if demand && admitted { State::Active } else { State::Ready }
}

struct PointerObservation { touchpad: State, gyro: State, physical: State }
impl PointerObservation {
    fn collect(dev: Option<&Device>) -> Self {
        let mut out = Self { touchpad: State::NotChecked, gyro: State::NotChecked, physical: State::NotChecked };
        let Some(d) = dev else { return out; };
        if !matches!(*d.link.lock(), Link::Live) || d.dead.load(Ordering::Relaxed) || d.stop.load(Ordering::Relaxed) {
            return out;
        }
        let live = d.snapshot();
        out.touchpad = if live.touch_mouse_active { State::Active } else { State::Disabled };
        out.gyro = if live.gyro_active && !live.gyro_stick { State::Active } else { State::Disabled };
        let status = d.virt.status.lock();
        out.physical = if status.hidden { State::Active }
            else if status.hide_note.is_some() { State::Unavailable } else { State::Disabled };
        out
    }
}

struct Observation {
    link: State, driver: State, playback: State, microphone: State, virtual_output: State,
    attempts: u32, input_hz: u32, retry_pending: bool, pointer: PointerObservation,
}
impl Observation {
    fn collect(dev: Option<&Device>) -> Self {
        let mut out = Self { link: State::Waiting, driver: State::NotChecked, playback: State::NotChecked,
            microphone: State::NotChecked, virtual_output: State::NotChecked, attempts: 0, input_hz: 0, retry_pending: false, pointer: PointerObservation::collect(dev) };
        let Some(d) = dev else { return out; };
        out.link = match &*d.link.lock() { Link::Live => State::Ready, Link::Opening => State::Waiting, _ => State::Unavailable };
        out.input_hz = d.stats.input_hz.load(Ordering::Relaxed);
        let bluetooth_audio = d.is_bluetooth() && !d.model.is_dualshock4();
        out.driver = if !bluetooth_audio { State::NotApplicable } else if d.media_transport_ready() { State::Ready } else { State::Unavailable };
        out.attempts = d.native_audio_attempts.load(Ordering::Relaxed);
        out.retry_pending = d.native_audio_retry_pending();
        if !bluetooth_audio {
            out.playback = State::NotApplicable;
            out.microphone = State::NotApplicable;
        } else if let Some(audio) = d.native_audio().filter(|a| a.healthy()) {
            out.playback = if audio.playing() { State::Active } else { State::Ready };
            out.microphone = microphone_state(audio.recording(), d.microphone.recording());
        } else {
            out.playback = State::Unavailable;
            out.microphone = State::Unavailable;
        }
        let st = d.virt.status.lock();
        out.virtual_output = if st.error.is_some() { State::Unavailable }
            else if st.active.is_some() { State::Ready }
            else if st.want.is_some_and(|k| k != VirtualKind::Off) { State::Waiting }
            else { State::Disabled };
        out
    }

    /// Fixed vocabulary deliberately excludes identities, paths, profile names,
    /// daemon error strings and all captured audio/input samples.
    fn report(&self) -> String {
        format!("DualWave {}\nPlatform: {}\nController: {}\nInput rate: {} Hz\nIsolated media: {}\nPlayback endpoint: {}\nMicrophone endpoint: {}\nVirtual controller: {}\nAudio connection attempts: {}\nAudio retry queued: {}\nDualWave touchpad mouse: {}\nDualWave gyro mouse: {}\nPhysical input isolation: {}\nExternal pointer source: Not checked\n",
            env!("CARGO_PKG_VERSION"), std::env::consts::OS, self.link.label(), self.input_hz,
            self.driver.label(), self.playback.label(), self.microphone.label(), self.virtual_output.label(), self.attempts, self.retry_pending,
            self.pointer.touchpad.label(), self.pointer.gyro.label(), self.pointer.physical.label())
    }
}

fn diagnostic_row(ui: &mut egui::Ui, title: &str, help: &str, state: State) {
    widgets::row(ui, tr(title), tr(help), |ui| {
        let color = match state {
            State::Ready | State::Active => good_color(), State::Unavailable => warn_color(), _ => muted_color(),
        };
        widgets::pill(ui, tr(state.label()), color);
    });
}

impl App {
    pub(super) fn connection_diagnostics(&mut self, ui: &mut egui::Ui, dev: Option<&Arc<Device>>) {
        let facts = Observation::collect(dev.map(Arc::as_ref));
        widgets::page_header(ui, tr("Connection diagnostics"), tr("Live connection state and recovery actions."));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("Copy diagnostic report")).clicked() {
                ui.ctx().copy_text(facts.report());
                self.engine.toast(tr("Diagnostic report copied."));
            }
            if let Some(d) = dev.filter(|d| d.is_bluetooth() && !d.model.is_dualshock4()) {
                let can_retry = !facts.retry_pending && !d.stop.load(Ordering::Acquire);
                if ui.add_enabled(can_retry, egui::Button::new(tr(if facts.retry_pending { "Retry queued…" } else { "Reconnect audio" }))).clicked() {
                    d.request_native_audio_retry();
                }
            }
            if ui.button(tr("Audio settings")).clicked() { self.page = Page::Haptics; self.detailed = false; }
            if ui.button(tr("Virtual controller")).clicked() { self.page = Page::Virtual; self.detailed = true; }
        });
        ui.add_space(12.0);
        diagnostic_row(ui, "Controller connection", "Connect or wake the controller. Discovery continues automatically.", facts.link);
        diagnostic_row(ui, "Isolated media transport", "Required for safe Bluetooth headphone and microphone support.", facts.driver);
        diagnostic_row(ui, "Headphone endpoint", "Ready means available; Active means an application is sending playback.", facts.playback);
        diagnostic_row(ui, "Microphone endpoint", "Endpoint availability does not grant capture permission or remove mute.", facts.microphone);
        diagnostic_row(ui, "Virtual controller", "The actual output state; choosing a target alone does not make it available.", facts.virtual_output);
        #[cfg(target_os = "linux")]
        {
            ui.add_space(12.0);
            widgets::card(ui, tr("Mouse input sources"), tr("Check which controller features can move the pointer."), |ui| {
                diagnostic_row(ui, "DualWave touchpad mouse", "Current touchpad mode, including temporary button toggles. Active means touchpad motion can move the pointer.", facts.pointer.touchpad);
                diagnostic_row(ui, "DualWave gyro mouse", "Active means gyro motion can move the pointer. Gyro output to a stick is excluded.", facts.pointer.gyro);
                diagnostic_row(ui, "Physical input exclusive", "Active means DualWave acquired physical input isolation. Device nodes remain visible.", facts.pointer.physical);
                diagnostic_row(ui, "External mouse input", "Linux touchpad handling and Steam Input settings are not observed here.", State::NotChecked);
                widgets::muted(ui, tr("To check touchpad input, turn off Touchpad as mouse in Motion & touch, then touch the pad. If the pointer still moves, fully exit Steam and test again. If it continues, check desktop touchpad settings and other controller tools."));
                widgets::muted(ui, tr("Stick mouse modes, button mappings, macros and turn tests can also move the pointer. These rows show the applied controller state, not the profile being edited."));
                if ui.button(tr("Motion & touch")).clicked() { self.page = Page::Input; self.detailed = true; }
            });
        }
        if let Some(d) = dev {
            let applied = self.engine.profile_name_for(d);
            widgets::row(ui, tr("Applied profile"), tr("The profile currently assigned to this controller."), |ui| { ui.add(egui::Label::new(&applied).truncate()).on_hover_text(&applied); });
            if let Some(error) = d.native_audio_error.lock().clone() {
                ui.add_space(8.0);
                ui.label(RichText::new(error).color(warn_color()));
            }
            if let Some(error) = d.virt.status.lock().error.clone() { ui.label(RichText::new(error).color(warn_color())); }
            if facts.driver == State::Unavailable {
                widgets::muted(ui, tr("Complete system setup, reboot, then reconnect the controller. This page does not reload the driver."));
                let busy = matches!(crate::install::status(crate::install::Package::Usbip), Some(crate::install::Status::Installing));
                if ui.add_enabled(!busy, egui::Button::new(tr("Configure system support"))).clicked() {
                    let ctx = ui.ctx().clone();
                    crate::install::start(crate::install::Package::Usbip, move || ctx.request_repaint());
                }
                match crate::install::status(crate::install::Package::Usbip) {
                    Some(crate::install::Status::Failed(error)) => { ui.label(RichText::new(error).color(warn_color())); }
                    Some(crate::install::Status::Finished { ok: true }) => { widgets::muted(ui, tr("System setup finished. Reboot before testing the driver.")); }
                    Some(crate::install::Status::Finished { ok: false }) => { widgets::muted(ui, tr("System setup did not complete. Try again from the AppImage.")); }
                    Some(crate::install::Status::Installing) => { widgets::muted(ui, tr("Installing…")); }
                    _ => {}
                }
            }
        }
        ui.add_space(10.0);
        widgets::muted(ui, tr("Recovery keeps the applied profile and microphone permission. Audio applications may need to select the recreated endpoint again."));
        widgets::muted(ui, tr("The copied report excludes device addresses, paths, profile names and audio samples."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_diagnostics_use_live_state_and_do_not_guess_external_sources() {
        let offline = PointerObservation::collect(None);
        assert_eq!(offline.touchpad, State::NotChecked);
        assert_eq!(offline.gyro, State::NotChecked);
        assert_eq!(offline.physical, State::NotChecked);
        let d = Device::for_test(ds_proto::Connection::Bluetooth, "");
        d.live.lock().touch_mouse_active = true;
        d.live.lock().gyro_active = true;
        let facts = PointerObservation::collect(Some(&d));
        assert_eq!(facts.touchpad, State::Active);
        assert_eq!(facts.gyro, State::Active);
        assert_eq!(facts.physical, State::Disabled);
        d.virt.status.lock().hide_note = Some("private driver error".into());
        assert_eq!(PointerObservation::collect(Some(&d)).physical, State::Unavailable);
        d.virt.status.lock().hide_note = None;
        d.live.lock().gyro_stick = true;
        d.virt.status.lock().hidden = true;
        let facts = PointerObservation::collect(Some(&d));
        assert_eq!(facts.gyro, State::Disabled);
        assert_eq!(facts.physical, State::Active);
        *d.link.lock() = Link::Quiet;
        let facts = PointerObservation::collect(Some(&d));
        assert_eq!(facts.touchpad, State::NotChecked, "stale live flags must not claim activity");
        assert_eq!(facts.gyro, State::NotChecked);
    }

    #[test]
    fn diagnostics_actions_queue_recovery_and_keep_capture_permission() {
        let dir = std::env::temp_dir().join(format!("dualwave-diagnostics-actions-{}", std::process::id()));
        let mut app = App::new(crate::engine::Engine::new(dir.clone()));
        let d = Device::for_test(ds_proto::Connection::Bluetooth, "");
        let ctx = egui::Context::default();
        super::super::theme::install(&ctx, 1.0);
        let render = |app: &mut App, events| ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760., 640.))), events, ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { app.connection_diagnostics(ui, Some(&d)); });
        });
        let output = render(&mut app, Vec::new());
        let position = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == tr("Reconnect audio") => Some(t.pos + t.galley.rect.center().to_vec2()), _ => None,
        }).expect("recovery action visible");
        for pressed in [true, false] {
            render(&mut app, vec![egui::Event::PointerMoved(position), egui::Event::PointerButton {
                pos: position, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert!(d.native_audio_retry_pending());
        assert!(d.native_audio().is_none(), "render must not open PipeWire");
        assert_eq!(d.stats.writes.load(Ordering::Relaxed), 0, "diagnostics must not send controller reports");
        assert!(!d.microphone.enabled());
        assert_eq!(d.native_audio_attempts.load(Ordering::Relaxed), 0);
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn muted_capture_with_a_recording_client_is_not_reported_active() {
        let d = Device::for_test(ds_proto::Connection::Bluetooth, "");
        d.microphone.configure(true, true, 100);
        assert!(d.microphone.enabled());
        assert!(!d.microphone.recording());
        assert_eq!(microphone_state(true, d.microphone.recording()), State::Ready);
        assert_eq!(microphone_state(true, true), State::Active);
        assert_eq!(microphone_state(false, true), State::Ready);
    }

    #[test]
    fn offline_diagnostics_do_not_claim_system_services_are_ready() {
        let facts = Observation::collect(None);
        assert_eq!(facts.link, State::Waiting);
        assert_eq!(facts.playback, State::NotChecked);
        assert_eq!(facts.driver, State::NotChecked);
    }

    #[test]
    fn diagnostic_report_excludes_device_identity_profiles_and_raw_errors() {
        let d = Device::for_test(ds_proto::Connection::Bluetooth, "01:23:45:67:89:AB");
        *d.native_audio_error.lock() = Some("/home/private/01:23:45:67:89:AB".into());
        d.virt.status.lock().error = Some("sensitive error".into());
        let facts = Observation::collect(Some(&d));
        assert_eq!(facts.playback, State::Unavailable);
        assert_eq!(facts.virtual_output, State::Unavailable);
        let report = facts.report();
        for secret in ["01:23", "/home", "sensitive", "private"] { assert!(!report.contains(secret)); }
        assert!(report.contains("Playback endpoint: Unavailable"));
        assert!(report.contains("DualWave touchpad mouse: Disabled"));
        assert!(report.contains("External pointer source: Not checked"));
    }
}
