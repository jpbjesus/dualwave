use eframe::egui::{self, CornerRadius, RichText, Stroke};

use ds_proto::input::Button;

use super::button_choices;
use crate::inputsim::{key_catalog, key_name, vk_from_egui};
use crate::profile::*;
use crate::ui::theme::*;
use crate::ui::widgets::*;
use crate::ui::App;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AKind {
    None,
    Key,
    Mouse,
    Macro,
    ScrollUp,
    ScrollDown,
    Media,
    Next,
    Prev,
    Gyro,
    Touch,
    Run,
    Profile,
    HoldProfile,
    Gamepad,
    Xbox,
    TurnOff,
}

fn akind(a: &Action) -> AKind {
    match a {
        Action::None => AKind::None,
        Action::Key { .. } => AKind::Key,
        Action::Mouse { .. } => AKind::Mouse,
        Action::Macro { .. } => AKind::Macro,
        Action::ScrollUp => AKind::ScrollUp,
        Action::ScrollDown => AKind::ScrollDown,
        Action::Media { .. } => AKind::Media,
        Action::NextProfile => AKind::Next,
        Action::PreviousProfile => AKind::Prev,
        Action::ToggleGyroMouse => AKind::Gyro,
        Action::ToggleTouchpadMouse => AKind::Touch,
        Action::Run { .. } => AKind::Run,
        Action::Profile { .. } => AKind::Profile,
        Action::HoldProfile { .. } => AKind::HoldProfile,
        Action::Gamepad { .. } => AKind::Gamepad,
        Action::Xbox { .. } => AKind::Xbox,
        Action::TurnOffController => AKind::TurnOff,
    }
}

const KINDS: [(AKind, &str); 17] = [
    (AKind::None, "Nothing"),
    (AKind::Gamepad, "PlayStation button"),
    (AKind::Xbox, "Xbox button"),
    (AKind::Key, "Keyboard key"),
    (AKind::Mouse, "Mouse button"),
    (AKind::Macro, "Macro"),
    (AKind::ScrollUp, "Scroll up"),
    (AKind::ScrollDown, "Scroll down"),
    (AKind::Media, "Media key"),
    (AKind::Next, "Next profile"),
    (AKind::Prev, "Previous profile"),
    (AKind::Gyro, "Toggle gyro mouse"),
    (AKind::Touch, "Toggle touchpad mouse"),
    (AKind::Run, "Open a program / file"),
    (AKind::Profile, "Switch to profile"),
    (AKind::HoldProfile, "Use profile while held"),
    (AKind::TurnOff, "Turn off controller"),
];

fn mouse_label(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => crate::i18n::tr("Left"),
        MouseButton::Right => crate::i18n::tr("Right"),
        MouseButton::Middle => crate::i18n::tr("Middle"),
        MouseButton::Back => crate::i18n::tr("Back"),
        MouseButton::Forward => crate::i18n::tr("Forward"),
    }
}

fn key_label(vk: u16) -> String {
    crate::i18n::tr(&key_name(vk)).to_string()
}


/// Shared action controls for the mapping list and the selected Workspace button.
pub(crate) fn action_editor(
    ui: &mut egui::Ui, m: &mut Mapping, i: usize, profile_names: &[String],
    p_name: &str, virtual_on: bool, xbox_on: bool, can_hide: bool,
    recording: bool, original_label: bool,
) -> Option<bool> {
    let mut capture = None;
    let mut k = akind(&m.action);
    let before = k;
    ui.label(RichText::new(crate::i18n::tr("What should it do?")).strong());
    ui.horizontal_wrapped(|ui| {
        for (kind, label) in [(AKind::Key, "Keyboard"), (AKind::Mouse, "Mouse"), (AKind::Gamepad, "Controller"), (AKind::Macro, "Macro")] {
            if ui.add(egui::Button::new(crate::i18n::tr(label)).selected(k == kind).min_size(egui::vec2(0.0, 32.0))).clicked() { k = kind; }
        }
        let primary = matches!(k, AKind::Key | AKind::Mouse | AKind::Gamepad | AKind::Macro);
        fit(ui, 170.0);
        egui::ComboBox::from_id_salt(("map-kind", i)).width(170.0)
            .selected_text(crate::i18n::tr(if primary { "More actions" } else if original_label && k == AKind::None { "Original" } else { KINDS.iter().find(|x| x.0 == k).map_or("More actions", |x| x.1) }))
            .show_ui(ui, |ui| {
                for (kind, label) in KINDS {
                    if matches!(kind, AKind::Key | AKind::Mouse | AKind::Gamepad | AKind::Macro) { continue; }
                    ui.selectable_value(&mut k, kind, crate::i18n::tr(if original_label && kind == AKind::None { "Original" } else { label }));
                }
            });
    });
    ui.add_space(6.0);
    if k != before {
        m.action = match k {
            AKind::None => Action::None,
            AKind::Key => Action::Key { vk: 0x20, ctrl: false, shift: false, alt: false, win: false },
            AKind::Mouse => Action::Mouse { button: MouseButton::Left },
            AKind::Macro => { m.mode = HoldStyle::Tap; Action::Macro { steps: Vec::new() } },
            AKind::ScrollUp => Action::ScrollUp,
            AKind::ScrollDown => Action::ScrollDown,
            AKind::Media => Action::Media { key: MediaKey::PlayPause },
            AKind::Next => Action::NextProfile,
            AKind::Prev => Action::PreviousProfile,
            AKind::Gyro => Action::ToggleGyroMouse,
            AKind::Touch => Action::ToggleTouchpadMouse,
            AKind::Run => Action::Run { path: String::new() },
            AKind::TurnOff => Action::TurnOffController,
            AKind::Profile => Action::Profile {
                name: profile_names.iter().find(|n| **n != p_name).cloned().unwrap_or_default(),
            },
            AKind::HoldProfile => {
                m.mute = true;
                Action::HoldProfile {
                    name: profile_names.iter().find(|n| **n != p_name).cloned().unwrap_or_default(),
                    then: String::new(),
                }
            }
            AKind::Gamepad => Action::Gamepad { button: m.button.physical() },
            AKind::Xbox => Action::Xbox {
                button: XboxPadButton::ALL
                    .into_iter()
                    .find(|x| x.pad_button() == m.button)
                    .unwrap_or(XboxPadButton::Guide),
            },
        };
    }
    if let Action::Macro { steps } = &mut m.action {
        if original_label {
            muted(ui, crate::i18n::tr("Edit the sequence in the full button editor."));
        } else {
            macro_editor(ui, steps, &mut m.mode, i, virtual_on);
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            match &mut m.action {
                Action::Key { vk, ctrl, shift, alt, win } => {
                    fit(ui, 110.0);
                    egui::ComboBox::from_id_salt(("map-key", i))
                        .width(110.0)
                        .selected_text(key_label(*vk))
                        .height(320.0)
                        .show_ui(ui, |ui| {
                            for c in key_catalog() {
                                ui.selectable_value(vk, c, key_label(c));
                            }
                        });
                    let rec = recording;
                    if ui
                        .add(egui::Button::new(if rec { crate::i18n::tr("Press a key…") } else { crate::i18n::tr("Record") }).fill(if rec { accent_color() } else { card_hi_color() }))
                        .clicked()
                    {
                        capture = Some(!rec);
                    }
                    ui.checkbox(ctrl, "Ctrl");
                    ui.checkbox(shift, crate::i18n::tr("Shift"));
                    ui.checkbox(alt, "Alt");
                    ui.checkbox(win, "Win");
                }
                Action::Mouse { button } => {
                    fit(ui, 110.0);
                    egui::ComboBox::from_id_salt(("map-mouse", i))
                        .width(110.0)
                        .selected_text(mouse_label(*button))
                        .show_ui(ui, |ui| {
                            for b in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward] {
                                ui.selectable_value(button, b, mouse_label(b));
                            }
                        });
                }
                Action::Media { key } => {
                    fit(ui, 150.0);
                    egui::ComboBox::from_id_salt(("map-media", i))
                        .width(150.0)
                        .selected_text(crate::i18n::tr(key.label()))
                        .show_ui(ui, |ui| {
                            for k in MediaKey::ALL {
                                ui.selectable_value(key, k, crate::i18n::tr(k.label()));
                            }
                        });
                }
                Action::Profile { name } => {
                    fit(ui, 170.0);
                    egui::ComboBox::from_id_salt(("map-prof", i))
                        .width(170.0)
                        .selected_text(if name.is_empty() { crate::i18n::tr("Choose…") } else { name.as_str() })
                        .show_ui(ui, |ui| {
                            for n in profile_names {
                                ui.selectable_value(name, n.clone(), n);
                            }
                        });
                }
                Action::HoldProfile { name, then } => {
                    fit(ui, 170.0);
                    egui::ComboBox::from_id_salt(("map-hold", i))
                        .width(170.0)
                        .selected_text(if name.is_empty() { crate::i18n::tr("Choose…") } else { name.as_str() })
                        .show_ui(ui, |ui| {
                            for n in profile_names {
                                ui.selectable_value(name, n.clone(), n);
                            }
                        });
                    fit(ui, 250.0);
                    // The label stays with its list when the row wraps.
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::tr("on release")).size(12.5).color(muted_color()));
                        egui::ComboBox::from_id_salt(("map-hold-then", i))
                            .width(170.0)
                            .selected_text(if then.is_empty() { crate::i18n::tr("Go back") } else { then.as_str() })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(then, String::new(), crate::i18n::tr("Go back"));
                                for n in profile_names {
                                    ui.selectable_value(then, n.clone(), n);
                                }
                            })
                            .response
                            .on_hover_text(crate::i18n::tr("What happens when the button is let go: go back to the profile from before, or stay on another profile."));
                    });
                }
                Action::Gamepad { button } => {
                    fit(ui, 130.0);
                    egui::ComboBox::from_id_salt(("map-pad", i))
                        .width(130.0)
                        .selected_text(crate::i18n::tr(button.short_name()))
                        .show_ui(ui, |ui| {
                            for b in button_choices(false) {
                                if b.is_touchpad_zone() {
                                    continue;
                                }
                                ui.selectable_value(button, b, crate::i18n::tr(b.short_name()));
                            }
                        });
                    if !virtual_on {
                        ui.label(RichText::new(crate::i18n::tr("needs a virtual controller")).size(12.0).color(warn_color()));
                    }
                }
                Action::Xbox { button } => {
                    fit(ui, 130.0);
                    egui::ComboBox::from_id_salt(("map-xbox", i))
                        .width(130.0)
                        .selected_text(crate::i18n::tr(button.label()))
                        .show_ui(ui, |ui| {
                            for b in XboxPadButton::ALL {
                                ui.selectable_value(button, b, crate::i18n::tr(b.label()));
                            }
                        });
                    #[cfg(target_os = "linux")]
                    if !xbox_on && !can_hide {
                        ui.label(RichText::new(crate::i18n::tr("needs a virtual Xbox 360 controller")).size(12.0).color(warn_color()));
                    }
                    #[cfg(windows)]
                    if !xbox_on && can_hide {
                        ui.label(RichText::new(crate::i18n::tr("hidden helper Xbox controller")).size(12.0).color(muted_color()))
                            .on_hover_text(crate::i18n::tr("This profile has no virtual Xbox controller, so DualWave keeps a helper one plugged in, hidden from games with HidHide. Only Windows (Task View, Game Bar) sees it. Needs usbip-win2."));
                    } else if !xbox_on {
                        ui.label(RichText::new(crate::i18n::tr("games see a helper controller")).size(12.0).color(warn_color()))
                            .on_hover_text(crate::i18n::tr("Without HidHide, DualWave plugs in a helper Xbox controller for the press, which games can see, and unplugs it 30 seconds after the last press. Windows needs about a second to see it, so the first press arrives late. Install HidHide on the Virtual controller page to hide it."));
                    }
                }
                Action::Run { path } => {
                    ui.add(egui::TextEdit::singleline(path).hint_text(if cfg!(target_os = "linux") { "/path/to/program" } else { "C:\\path\\to\\app.exe" }).desired_width(220.0));
                    if button(ui, crate::i18n::tr("Browse…")).clicked() {
                        if let Some(f) = rfd::FileDialog::new().pick_file() {
                            *path = f.display().to_string();
                        }
                    }
                }
                Action::TurnOffController => {
                    ui.label(RichText::new(crate::i18n::tr("powers off over Bluetooth")).size(12.0).color(muted_color()))
                        .on_hover_text(crate::i18n::tr("Drops the Bluetooth link, which turns the controller off. Plugged in over USB, it stays on."));
                }
                _ => {}
            }
        });
    }
    if recording && !matches!(m.action, Action::Key { .. }) { capture = Some(false); }
    capture
}

fn macro_editor(ui: &mut egui::Ui, steps: &mut Vec<MacroStep>, mode: &mut HoldStyle, i: usize, virtual_on: bool) {
    ui.push_id(("macro-editor", i), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Macro steps")).strong());
            if !steps.is_empty() && ui.add_enabled(steps.len() < 128, egui::Button::new(crate::i18n::tr("Add step"))).clicked() { steps.push(MacroStep::default()); }
        });
        muted(ui, crate::i18n::tr("Each step presses, releases, then pauses. Steps run from top to bottom."));
        playback_editor(ui, mode, true);
        ui.add_space(6.0);
        let mut remove = None;
        let mut swap = None;
        let count = steps.len().min(128);
        if steps.is_empty() { muted(ui, crate::i18n::tr("No steps yet. Add a key, mouse click or controller button.")); }
        for (index, step) in steps.iter_mut().take(128).enumerate() {
            ui.push_id(index, |ui| {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label((index + 1).to_string());
                        let mut kind = match step.input { MacroInput::Key { .. } => 0, MacroInput::Mouse { .. } => 1, MacroInput::Gamepad { .. } => 2 };
                        let before = kind;
                        egui::ComboBox::from_id_salt("input-kind").selected_text(crate::i18n::tr(["Keyboard key", "Mouse button", "PlayStation button"][kind]))
                            .show_ui(ui, |ui| {
                                for (k, label) in ["Keyboard key", "Mouse button", "PlayStation button"].iter().enumerate() {
                                    ui.selectable_value(&mut kind, k, crate::i18n::tr(label));
                                }
                            });
                        if kind != before {
                            step.input = match kind {
                                1 => MacroInput::Mouse { button: MouseButton::Left },
                                2 => MacroInput::Gamepad { button: Button::Cross },
                                _ => MacroStep::default().input,
                            };
                        }
                        match &mut step.input {
                            MacroInput::Key { vk, ctrl, shift, alt, win } => {
                                egui::ComboBox::from_id_salt("key").selected_text(key_label(*vk)).height(250.0).show_ui(ui, |ui| {
                                    for key in key_catalog() { ui.selectable_value(vk, key, key_label(key)); }
                                });
                                ui.checkbox(ctrl, "Ctrl");
                                ui.checkbox(shift, crate::i18n::tr("Shift"));
                                ui.checkbox(alt, "Alt");
                                ui.checkbox(win, "Win");
                            }
                            MacroInput::Mouse { button } => {
                                egui::ComboBox::from_id_salt("mouse").selected_text(mouse_label(*button)).show_ui(ui, |ui| {
                                    for b in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward] {
                                        ui.selectable_value(button, b, mouse_label(b));
                                    }
                                });
                            }
                            MacroInput::Gamepad { button } => {
                                egui::ComboBox::from_id_salt("gamepad").selected_text(crate::i18n::tr(button.short_name())).show_ui(ui, |ui| {
                                    for b in button_choices(false).into_iter().filter(|b| !b.is_touchpad_zone()) { ui.selectable_value(button, b, crate::i18n::tr(b.short_name())); }
                                });
                                if !virtual_on { muted(ui, crate::i18n::tr("needs a virtual controller")); }
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(crate::i18n::tr("Hold for"));
                        ui.add(egui::DragValue::new(&mut step.hold_ms).range(1..=60000).clamp_existing_to_range(false).suffix(" ms"));
                        ui.label(crate::i18n::tr("Pause after"));
                        ui.add(egui::DragValue::new(&mut step.wait_ms).range(0..=60000).clamp_existing_to_range(false).suffix(" ms"));
                        if ui.add_enabled(index > 0, egui::Button::new(crate::i18n::tr("Move up"))).clicked() { swap = Some((index, index - 1)); }
                        if ui.add_enabled(index + 1 < count, egui::Button::new(crate::i18n::tr("Move down"))).clicked() { swap = Some((index, index + 1)); }
                        if button(ui, crate::i18n::tr("Remove")).clicked() { remove = Some(index); }
                    });
                });
            });
        }
        if let Some(index) = remove { steps.remove(index); }
        else if let Some((a, b)) = swap { steps.swap(a, b); }
        if ui.add_enabled(steps.len() < 128, egui::Button::new(crate::i18n::tr("Add step"))).clicked() { steps.push(MacroStep::default()); }
        if steps.len() >= 128 { muted(ui, crate::i18n::tr("Only the first 128 steps play.")); }
    });
}

fn playback_editor(ui: &mut egui::Ui, mode: &mut HoldStyle, is_macro: bool) {
    ui.label(crate::i18n::tr("Playback"));
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [(HoldStyle::Tap, "Play once"), (HoldStyle::Hold, "Repeat while held"), (HoldStyle::Toggle, "Repeat until pressed again")] {
            if !is_macro && value == HoldStyle::Hold { ui.selectable_value(mode, value, crate::i18n::tr("While held")); }
            else if !is_macro && value == HoldStyle::Toggle { ui.selectable_value(mode, value, crate::i18n::tr("Press again to release")); }
            else { ui.selectable_value(mode, value, crate::i18n::tr(label)); }
        }
        if !is_macro || *mode == HoldStyle::Turbo { ui.selectable_value(mode, HoldStyle::Turbo, crate::i18n::tr("Turbo")); }
    });
}

pub(crate) fn capture_action(ctx: &egui::Context, action: &mut Action) -> bool {
    let got = ctx.input(|inp| inp.events.iter().find_map(|e| match e {
        egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
        _ => None,
    }));
    if let Some((key, modifiers)) = got {
        if key != egui::Key::Escape {
            if let Some(vk) = vk_from_egui(key) {
                *action = Action::Key { vk, ctrl: modifiers.ctrl, shift: modifiers.shift, alt: modifiers.alt, win: modifiers.mac_cmd };
            }
        }
        return true;
    }
    false
}

pub(crate) fn focus_mapping(ctx: &egui::Context, index: usize) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("mapping-focus"), index));
}

fn advanced_editor(ui: &mut egui::Ui, m: &mut Mapping, i: usize, edge: bool) {
    if !m.with.is_empty() || m.mute || m.gesture != Gesture::Press {
        ui.horizontal_wrapped(|ui| {
            if !m.with.is_empty() {
                ui.label(crate::i18n::tr("Together with"));
                ui.label(m.with.iter().map(|b| crate::i18n::tr(b.short_name())).collect::<Vec<_>>().join(" + "));
            }
            if m.mute { ui.label(crate::i18n::tr("Hide from game")); }
            let timing = match m.gesture { Gesture::LongPress => Some(m.long_ms), Gesture::DoubleTap => Some(m.double_ms), _ => None };
            if let Some(ms) = timing { ui.label(crate::localized_format!("{ms} ms", "{ms} ms", "{ms} ms"; "{ms} ms")); }
        });
    }
    ui.collapsing(crate::i18n::tr("Advanced"), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::tr("Together with"));
            let mut chosen = m.with.clone();
            let label = if chosen.is_empty() { crate::i18n::tr("nothing (single button)").to_string() }
                else { chosen.iter().map(|b| crate::i18n::tr(b.short_name())).collect::<Vec<_>>().join(" + ") };
            let mut changed = false;
            egui::ComboBox::from_id_salt(("map-with", i)).width(170.0).selected_text(label).show_ui(ui, |ui| {
                for b in button_choices(edge) {
                    if b == m.button { continue; }
                    let mut on = chosen.contains(&b);
                    if ui.checkbox(&mut on, crate::i18n::tr(b.short_name())).changed() {
                        changed = true;
                        if on { chosen.push(b); } else { chosen.retain(|x| *x != b); }
                    }
                }
            });
            if changed { chosen.sort(); m.with = chosen; }
        });
        let suppression_help = crate::i18n::tr(if m.is_combo() { "The virtual controller hides these buttons only while the combination is held." } else { "The original button is hidden only on the virtual controller." });
        ui.checkbox(&mut m.mute, crate::i18n::tr("Hide from game")).on_hover_text(suppression_help);
        if m.mute { muted(ui, suppression_help); }
        match m.gesture {
            Gesture::LongPress => { ui.add(egui::Slider::new(&mut m.long_ms, 150..=2000).clamping(egui::SliderClamping::Edits).suffix(" ms").text(crate::i18n::tr("hold for"))); }
            Gesture::DoubleTap => { ui.add(egui::Slider::new(&mut m.double_ms, 120..=800).clamping(egui::SliderClamping::Edits).suffix(" ms").text(crate::i18n::tr("Double tap window"))); }
            _ => {}
        }
    });
}

impl App {
    pub(crate) fn page_mappings(
        &mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Profile,
    ) {
        page_header(ui, crate::i18n::tr("Button mapping"), crate::i18n::tr("Choose a controller button, then what it should do."));
        ui.horizontal_wrapped(|ui| {
            if button(ui, crate::i18n::tr("Add button action")).clicked() {
                p.mappings.push(Mapping { action: Action::Key { vk: 0x20, ctrl: false, shift: false, alt: false, win: false }, ..Default::default() });
                focus_mapping(ctx, p.mappings.len() - 1);
            }
            if primary_button(ui, crate::i18n::tr("Create macro")).clicked() {
                p.mappings.push(Mapping { mode: HoldStyle::Tap, action: Action::Macro { steps: Vec::new() }, ..Default::default() });
                focus_mapping(ctx, p.mappings.len() - 1);
            }
        });
        ui.add_space(12.0);
        #[cfg(target_os = "linux")]
        if let Some(error) = crate::inputsim::error() {
            note(ui, &crate::localized_format!("Keyboard and mouse input is unavailable: {error}", "A entrada de teclado e mouse está indisponível: {error}", "La entrada de teclado y ratón no está disponible: {error}"; "A entrada de teclado e rato está indisponível: {error}"), warn_color());
            ui.add_space(10.0);
        }
        let edge = self.engine.device_list().first().map(|d| d.model.is_edge()).unwrap_or(true);
        let virtual_on = p.virtual_out.kind != VirtualKind::Off;
        let xbox_on = p.virtual_out.kind == VirtualKind::Xbox360;
        let can_hide = self.engine.companion.can_hide();
        let profile_names = self.engine.profiles.names();
        if let Some(i) = self.capture_key {
            if let Some(map) = p.mappings.get_mut(i) {
                if capture_action(ctx, &mut map.action) { self.capture_key = None; }
            } else { self.capture_key = None; }
        }
        if p.mappings.is_empty() {
            muted(ui, crate::i18n::tr("No button actions yet. Add an action or create a macro above."));
        }
        let mut remove = None;
        let mut capture = None;
        let row_w = (ui.available_width() - 34.0).max(100.0);
        let p_name = p.name.clone();
        for (i, m) in p.mappings.iter_mut().enumerate() {
            let card = egui::Frame::new()
                .fill(surface_color())
                .stroke(Stroke::new(1.0_f32, if m.enabled { border_color() } else { with_alpha(border_color(), 120) }))
                .corner_radius(CornerRadius::same(R_CARD))
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(row_w);
                    ui.push_id(i, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(crate::i18n::tr("Controller button")).strong());
                            egui::ComboBox::from_id_salt(("map-btn", i)).width(130.0)
                                .selected_text(crate::i18n::tr(m.button.short_name())).show_ui(ui, |ui| {
                                    for b in button_choices(edge) { ui.selectable_value(&mut m.button, b, crate::i18n::tr(b.short_name())); }
                                });
                            ui.checkbox(&mut m.enabled, crate::i18n::tr("Enabled"));
                            if button(ui, crate::i18n::tr("Remove action")).clicked() { remove = Some(i); }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(crate::i18n::tr("When it activates"));
                            for (gesture, label) in [(Gesture::Press, "Press"), (Gesture::LongPress, "Long press"), (Gesture::DoubleTap, "Double tap")] {
                                ui.selectable_value(&mut m.gesture, gesture, crate::i18n::tr(label));
                            }
                        });
                        ui.add_space(8.0);
                        if let Some(record) = action_editor(ui, m, i, &profile_names, &p_name, virtual_on, xbox_on, can_hide, self.capture_key == Some(i), false) {
                            capture = Some(if record { Some(i) } else { None });
                        }
                        if !matches!(m.action, Action::Macro { .. }) {
                            ui.add_space(8.0);
                            if matches!(m.action, Action::HoldProfile { .. }) { ui.label(crate::i18n::tr("While held")); }
                            else { playback_editor(ui, &mut m.mode, false); }
                            if m.mode == HoldStyle::Turbo {
                                ui.add(egui::Slider::new(&mut m.turbo_ms, 30..=1000).clamping(egui::SliderClamping::Edits).suffix(" ms").text(crate::i18n::tr("interval")));
                            }
                        }
                        ui.add_space(8.0);
                        advanced_editor(ui, m, i, edge);
                    });
                });
            let focus = ctx.data(|data| data.get_temp::<usize>(egui::Id::new("mapping-focus")));
            if focus == Some(i) {
                card.response.scroll_to_me(Some(egui::Align::TOP));
                ctx.data_mut(|data| data.remove::<usize>(egui::Id::new("mapping-focus")));
            }
            ui.add_space(12.0);
        }
        if let Some(i) = remove { p.mappings.remove(i); self.capture_key = None; }
        if let Some(c) = capture { self.capture_key = c; }
    }
}

#[cfg(test)]
mod macro_tests {
    use super::*;

    fn frame(ctx: &egui::Context, steps: &mut Vec<MacroStep>, mode: &mut HoldStyle, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 800.0))),
            events, ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| macro_editor(ui, steps, mode, 0, true));
        })
    }

    fn click(ctx: &egui::Context, steps: &mut Vec<MacroStep>, mode: &mut HoldStyle, label: &str) {
        let mut output = frame(ctx, steps, mode, Vec::new());
        for _ in 0..3 { output = frame(ctx, steps, mode, Vec::new()); }
        let target = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(label) => Some(t.pos + t.galley.rect.center().to_vec2()),
            _ => None,
        }).unwrap_or_else(|| panic!("macro control {label} is visible"));
        frame(ctx, steps, mode, vec![egui::Event::PointerMoved(target)]);
        for pressed in [true, false] {
            frame(ctx, steps, mode, vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
    }

    #[test]
    fn inline_macro_editor_adds_reorders_removes_and_changes_playback() {
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx, 1.0);
        let mut steps = vec![MacroStep { hold_ms: 20, ..Default::default() }, MacroStep { hold_ms: 30, ..Default::default() }];
        let mut mode = HoldStyle::Tap;
        click(&ctx, &mut steps, &mut mode, "Add step");
        assert_eq!(steps.len(), 3);
        click(&ctx, &mut steps, &mut mode, "Move down");
        assert_eq!(steps.iter().map(|s| s.hold_ms).collect::<Vec<_>>(), [30, 20, 50]);
        click(&ctx, &mut steps, &mut mode, "Remove");
        assert_eq!(steps.iter().map(|s| s.hold_ms).collect::<Vec<_>>(), [20, 50]);
        click(&ctx, &mut steps, &mut mode, "Repeat until pressed again");
        assert_eq!(mode, HoldStyle::Toggle);
    }
    fn page_frame(app: &mut App, profile: &mut Profile, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 1000.0))),
            events, ..Default::default()
        }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| app.page_mappings(ui, ctx, profile));
            });
        })
    }

    #[test]
    fn create_macro_is_visible_and_opens_steps_directly() {
        let dir = std::env::temp_dir().join(format!("dualwave-create-macro-{}", std::process::id()));
        let mut app = App::new(crate::engine::Engine::new(dir.clone()));
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx, 1.0);
        let mut profile = Profile::default();
        profile.mappings.clear();
        let mut output = page_frame(&mut app, &mut profile, &ctx, Vec::new());
        for _ in 0..3 { output = page_frame(&mut app, &mut profile, &ctx, Vec::new()); }
        let target = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr("Create macro") => Some(t.pos + t.galley.rect.center().to_vec2()),
            _ => None,
        }).expect("macro creation is directly visible without opening a menu");
        page_frame(&mut app, &mut profile, &ctx, vec![egui::Event::PointerMoved(target)]);
        for pressed in [true, false] {
            page_frame(&mut app, &mut profile, &ctx, vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert_eq!(profile.mappings.len(), 1);
        assert!(matches!(profile.mappings[0].action, Action::Macro { .. }));
        assert_eq!(profile.mappings[0].mode, HoldStyle::Tap);
        let output = page_frame(&mut app, &mut profile, &ctx, Vec::new());
        assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr("Add step"))), "macro steps are editable immediately");
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_macro_preserves_turbo_and_saved_step_timings() {
        let ctx = egui::Context::default();
        let mut steps = vec![MacroStep { hold_ms: 0, wait_ms: u32::MAX, ..Default::default() }];
        let before = steps.clone();
        let mut mode = HoldStyle::Turbo;
        for _ in 0..3 { frame(&ctx, &mut steps, &mut mode, Vec::new()); }
        assert_eq!(steps, before, "opening the editor must not rewrite saved timing values");
        assert_eq!(mode, HoldStyle::Turbo);
    }

    #[test]
    fn opening_button_actions_preserves_advanced_settings() {
        let dir = std::env::temp_dir().join(format!("dualwave-advanced-mapping-{}", std::process::id()));
        let mut app = App::new(crate::engine::Engine::new(dir.clone()));
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx, 1.0);
        let mut profile = Profile::default();
        profile.mappings = vec![Mapping {
            with: vec![Button::R1, Button::L1], mute: true, enabled: false,
            gesture: Gesture::LongPress, long_ms: 4000, mode: HoldStyle::Turbo, turbo_ms: 20,
            action: Action::Key { vk: 65, ctrl: true, shift: false, alt: false, win: false },
            ..Default::default()
        }];
        let before = profile.clone();
        for _ in 0..3 { page_frame(&mut app, &mut profile, &ctx, Vec::new()); }
        assert_eq!(profile, before);
        let output = page_frame(&mut app, &mut profile, &ctx, Vec::new());
        let target = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr("Advanced") => Some(t.pos + t.galley.rect.center().to_vec2()), _ => None,
        }).expect("advanced settings are reachable");
        page_frame(&mut app, &mut profile, &ctx, vec![egui::Event::PointerMoved(target)]);
        for pressed in [true, false] {
            page_frame(&mut app, &mut profile, &ctx, vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]);
        }
        for _ in 0..3 { page_frame(&mut app, &mut profile, &ctx, Vec::new()); }
        assert_eq!(profile, before, "expanding Advanced preserves existing timing and combo settings");
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn inline_macros_fit_narrow_and_desktop_windows() {
        let dir = std::env::temp_dir().join(format!("dualwave-macro-layout-{}", std::process::id()));
        let mut app = App::new(crate::engine::Engine::new(dir.clone()));
        for width in [500.0, 760.0, 1240.0] {
            let ctx = egui::Context::default();
            crate::ui::theme::install(&ctx, 1.0);
            let mut profile = Profile::default();
            profile.mappings = vec![Mapping { action: Action::Macro { steps: vec![
                MacroStep { input: MacroInput::Key { vk: 65, ctrl: true, shift: true, alt: true, win: true }, ..Default::default() },
                MacroStep { input: MacroInput::Mouse { button: MouseButton::Left }, ..Default::default() },
                MacroStep { input: MacroInput::Gamepad { button: Button::Cross }, ..Default::default() },
            ] }, ..Default::default() }];
            for pass in 0..3 {
                let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let right = ui.available_rect_before_wrap().right();
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            app.page_mappings(ui, ctx, &mut profile);
                            if pass == 2 { assert!(ui.min_rect().right() <= right + 1.0, "macro editor exceeds width {width}: {} > {right}", ui.min_rect().right()); }
                        });
                    });
                });
            }
        }
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }

}
