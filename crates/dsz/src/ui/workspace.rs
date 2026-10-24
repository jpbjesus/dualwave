//! Controller workspace. Preview motion never writes to a device.
use std::sync::Arc;
use std::time::Instant;
use eframe::egui::{self, RichText, Stroke};
use ds_proto::{input::Button, TriggerEffect};
use crate::{device::{Device, Link}, profile::*};
use crate::i18n::tr;
use super::{App, Page, controller_view::{self, View}, theme::*, widgets};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Part { Lighting, Triggers, Buttons, Motion }
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Side { Left, Both, Right }

fn apply_trigger(p: &mut Profile, side: Side, effect: TriggerEffect) {
    if side != Side::Right { p.triggers.left = effect.clone(); }
    if side != Side::Left { p.triggers.right = effect; }
}

const PRESET_LABELS: [&str; 6] = ["Off", "Light", "Heavy", "Pistol", "Rifle", "Bow"];
const PRESET_DESCRIPTIONS: [&str; 6] = [
    "No additional resistance.", "Continuous light resistance.", "Continuous strong resistance.",
    "Resistance concentrated around the firing point.", "Resistance with an earlier release point.",
    "Progressive resistance with a release at the end.",
];

fn preset_effect(index: usize) -> TriggerEffect {
    match index {
        1 => TriggerEffect::Feedback { position: 1, strength: 2 },
        2 => TriggerEffect::Feedback { position: 0, strength: 7 },
        3 => TriggerEffect::Weapon { start: 4, end: 6, strength: 7 },
        4 => TriggerEffect::Weapon { start: 2, end: 5, strength: 5 },
        5 => TriggerEffect::Bow { start: 1, end: 6, strength: 6, snap: 7 },
        _ => TriggerEffect::Off,
    }
}

fn intensity(effect: &TriggerEffect) -> Option<u8> {
    match effect {
        TriggerEffect::Feedback { strength, .. } | TriggerEffect::Weapon { strength, .. } | TriggerEffect::Bow { strength, .. } => Some(*strength),
        _ => None,
    }
}

fn set_intensity(effect: &mut TriggerEffect, value: u8) {
    match effect {
        TriggerEffect::Feedback { strength, .. } | TriggerEffect::Weapon { strength, .. } | TriggerEffect::Bow { strength, .. } => *strength = value.clamp(1, 7),
        _ => {},
    }
}

fn adjust_intensity(p: &mut Profile, side: Side, value: u8) {
    if side != Side::Right { set_intensity(&mut p.triggers.left, value); }
    if side != Side::Left { set_intensity(&mut p.triggers.right, value); }
}

fn matches_preset(effect: &TriggerEffect, index: usize) -> bool {
    let mut preset = preset_effect(index);
    if let Some(value) = intensity(effect) { set_intensity(&mut preset, value); }
    *effect == preset
}

fn preset_curve(ui: &egui::Ui, rect: egui::Rect, index: usize, color: egui::Color32) {
    let coordinates: Vec<(f32, f32)> = match index {
        1 => vec![(4.,23.), (12.,23.), (12.,17.), (60.,17.)],
        2 => vec![(4.,23.), (4.,5.), (60.,5.)],
        3 => vec![(4.,23.), (28.,23.), (28.,5.), (40.,5.), (40.,23.), (60.,23.)],
        4 => vec![(4.,23.), (17.,23.), (17.,10.), (34.,10.), (34.,23.), (60.,23.)],
        5 => {
            let mut curve = (0..24).map(|i| {
                let t = i as f32 / 23.0;
                let u = 1.0 - t;
                (u*u*u*4.0 + 3.0*u*u*t*17.0 + 3.0*u*t*t*23.0 + t*t*t*39.0,
                    u*u*u*23.0 + 3.0*u*u*t*23.0 + 3.0*u*t*t*5.0 + t*t*t*5.0)
            }).collect::<Vec<_>>();
            curve.extend([(44.,23.), (60.,23.)]);
            curve
        },
        _ => vec![(4.,23.), (60.,23.)],
    };
    let points = coordinates.into_iter().map(|(x, y)| rect.min + egui::vec2(x / 64.0 * rect.width(), y / 28.0 * rect.height())).collect();
    ui.painter().add(egui::Shape::line(points, Stroke::new(1.6, color)));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickAction { Original, Enter, Click, Next, Custom }

fn quick_action(mapping: Option<&Mapping>) -> QuickAction {
    match mapping.map(|m| &m.action) {
        None | Some(Action::None) => QuickAction::Original,
        Some(Action::Key { vk: 0x0d, ctrl: false, shift: false, alt: false, win: false }) => QuickAction::Enter,
        Some(Action::Mouse { button: MouseButton::Left }) => QuickAction::Click,
        Some(Action::NextProfile) => QuickAction::Next,
        _ => QuickAction::Custom,
    }
}

fn individual_mapping(p: &Profile, button: Button, gesture: Gesture) -> Option<usize> {
    p.mappings.iter().position(|m| m.enabled && m.button == button && m.gesture == gesture && !m.is_combo())
}

fn apply_quick_action(p: &mut Profile, button: Button, gesture: Gesture, action: QuickAction) {
    if action == QuickAction::Custom { return; }
    let index = individual_mapping(p, button, gesture);
    if action == QuickAction::Original && index.is_none() { return; }
    let index = index.unwrap_or_else(|| {
        p.mappings.push(Mapping { button, gesture, ..Default::default() });
        p.mappings.len() - 1
    });
    p.mappings[index].action = match action {
        QuickAction::Enter => Action::Key { vk: 0x0d, ctrl: false, shift: false, alt: false, win: false },
        QuickAction::Click => Action::Mouse { button: MouseButton::Left },
        QuickAction::Next => Action::NextProfile,
        _ => Action::None,
    };
    if action == QuickAction::Original { p.mappings[index].mute = false; }
}

fn choice<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    ui.horizontal_wrapped(|ui| {
        for (candidate, label) in options {
            let selected = *value == *candidate;
            let response = ui.add(egui::Button::new(RichText::new(tr(label)).color(if selected { accent_color() } else { muted_color() }))
                .frame(false).min_size(egui::vec2(42.0, 36.0)));
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, tr(label)));
            let t = ui.ctx().animate_bool(response.id.with("line"), selected);
            if t > 0.0 {
                ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(2.0 * t, accent_color()));
            }
            if response.has_focus() {
                ui.painter().rect_stroke(response.rect.expand(2.0), egui::CornerRadius::same(3), Stroke::new(1.5, accent_color()), egui::StrokeKind::Outside);
            }
            if response.clicked() { *value = *candidate; }
        }
    });
}

fn mark(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(40.0, 30.0), egui::Sense::hover());
    for (offset, alpha, width) in [(0.0, 255, 2.8), (-5.0, 90, 1.8)] {
        let points = (0..100).map(|i| {
            let x = i as f32 / 99.0;
            r.min + egui::vec2(x * 40.0, 14.0 + offset - (x * std::f32::consts::TAU * 2.0).cos() * 9.0)
        }).collect();
        ui.painter().add(egui::Shape::line(points, Stroke::new(width, with_alpha(accent_color(), alpha))));
    }
}

impl App {
    pub(super) fn workspace_navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            mark(ui);
            ui.label(RichText::new("DualWave").font(heading(24.0)).color(text_color()));
            ui.add_space(if ui.available_width() > 900.0 { ui.available_width() * 0.24 } else { 24.0 });
            let mut destination = match self.page { Page::Haptics => 1, Page::Profiles => 2, Page::Settings => 3, _ => 0 };
            let previous = destination;
            choice(ui, &mut destination, &[(0, "Controller"), (1, "Audio"), (2, "Profiles"), (3, "Preferences")]);
            if destination != previous {
                self.page = [Page::Overview, Page::Haptics, Page::Profiles, Page::Settings][destination];
                self.advanced = false;
                self.capture_key = None;
            }
        });
    }

    pub(super) fn advanced_navigation(&mut self, ui: &mut egui::Ui) {
        if matches!(self.page, Page::Profiles | Page::Settings) { return; }
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.page == Page::Overview && !self.advanced, tr("Workspace")).clicked() {
                self.page = Page::Overview;
                self.advanced = false;
            }
            ui.menu_button(tr("Advanced"), |ui| {
                for page in [Page::Overview, Page::Lighting, Page::Triggers, Page::Input, Page::Mappings, Page::Virtual, Page::Haptics] {
                    if ui.button(page.label()).clicked() { self.page = page; self.advanced = true; ui.close(); }
                }
            });
        });
        ui.add_space(10.0);
    }

    pub(super) fn page_workspace(&mut self, ui: &mut egui::Ui, p: &mut Profile, dev: Option<&Arc<Device>>) {
        let available = dev.is_some_and(|d| *d.link.lock() == Link::Live);
        ui.label(RichText::new(tr("CONTROLLER STATUS")).size(10.0).color(muted_color()));
        ui.label(RichText::new(tr(if available { "Controller available" } else { "Controller offline" })).font(heading(44.0)).color(text_color()));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Editing profile")).color(muted_color()));
            ui.label(RichText::new(&p.name).strong());
            ui.label(RichText::new(tr(if available { "Changes are saved automatically." } else { "Offline preview · no controller connected" })).size(12.0).color(muted_color()));
        });
        ui.add_space(24.0);
        let width = ui.available_width();
        if width >= 850.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| { ui.set_width(width * 0.61 - 20.0); self.controller_stage(ui, p, dev); });
                ui.add_space(18.0);
                let start = ui.cursor().min;
                ui.vertical(|ui| { ui.set_width(width * 0.39 - 22.0); self.inspector(ui, p); });
                ui.painter().line_segment([start - egui::vec2(10.0, 0.0), start + egui::vec2(-10.0, 345.0)], Stroke::new(1.0, border_color()));
            });
        } else {
            self.controller_stage(ui, p, dev);
            ui.separator();
            self.inspector(ui, p);
        }
        ui.add_space(20.0);
        ui.separator();
        ui.label(RichText::new(tr("QUICK CONTROLS · EDITED PROFILE")).size(10.0).color(muted_color()));
        ui.columns(if width >= 850.0 { 3 } else { 1 }, |cols| {
            let last = cols.len() - 1;
            cols[0].label(tr("Headphone / speaker volume"));
            cols[0].add(egui::Slider::new(&mut p.haptics.speaker_volume, 0..=100).suffix("%"));
            cols[0].checkbox(&mut p.haptics.headset_when_plugged, tr("Use headphones when connected"));
            widgets::muted(&mut cols[0], tr("Bluetooth audio level; routing follows the connected headset."));
            let mic = 1.min(last);
            cols[mic].label(tr("Microphone"));
            cols[mic].checkbox(&mut p.microphone.enabled, tr("Enable microphone"));
            cols[mic].checkbox(&mut p.microphone.muted, tr("Mute microphone"));
            widgets::muted(&mut cols[mic], tr("Permission and mute above belong to the edited profile."));
            let actual_status = if let Some(d) = dev {
                let applied = self.engine.profiles.get_or_first(&self.engine.profile_name_for(d));
                if !available { "Controller offline" }
                else if !applied.microphone.enabled { "Capture disabled in applied profile" }
                else if applied.microphone.muted || (applied.microphone.mute_button && d.toggles.lock().mute_engaged) { "Microphone muted" }
                else if d.microphone.recording() && {
                    #[cfg(windows)]
                    { d.virt.microphone_open() }
                    #[cfg(target_os = "linux")]
                    { d.virt.microphone_open() || d.native_audio().is_some_and(|audio| audio.recording()) }
                } { "Capture active" }
                else if !d.microphone.enabled() { "Capture unavailable" }
                else { "No app recording" }
            } else { "No controller" };
            cols[mic].label(tr(actual_status));
            widgets::muted(&mut cols[mic], tr("Bluetooth microphone; capture starts only while an app records."));
            if let Some(d) = dev {
                if available && d.microphone.recording() {
                    widgets::meter(&mut cols[mic], d.microphone.peak() as f32 / 255.0, accent_color(), 160.0);
                }
            }
            cols[last].label(tr("Requested virtual output"));
            choice(&mut cols[last], &mut p.virtual_out.kind, &[(VirtualKind::Off, "Native"), (VirtualKind::Xbox360, "Xbox 360"), (VirtualKind::DualSense, "DualSense"), (VirtualKind::DualShock4, "DualShock 4")]);
            let actual = dev.and_then(|d| d.virt.active());
            cols[last].horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr("Actual output")).color(muted_color()));
                ui.label(actual.map(|v| tr(v.label())).unwrap_or_else(|| tr(if dev.is_some() { "Native / no virtual output" } else { "No controller" })));
            });
            if let Some(d) = dev {
                if let Some(error) = &d.virt.status.lock().error { widgets::muted(&mut cols[last], error); }
            }
        });
    }

    fn controller_stage(&mut self, ui: &mut egui::Ui, p: &Profile, dev: Option<&Arc<Device>>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(dev.map(|d| d.model.name()).unwrap_or("DualSense")).font(heading(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(RichText::new(tr("Select a region to configure")).size(11.0).color(muted_color())); });
        });
        let width = ui.available_width();
        let h = (width * 0.59).clamp(270.0, 390.0);
        let stage = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, h));
        let reduced = self.engine.settings.read().reduced_motion || self.system_reduced_motion;
        let target = if reduced { [-8.0, 0.0, 0.0] } else { match self.part {
            Part::Lighting => [-5.0, 2.0, 0.0], Part::Buttons => [-3.0, 3.0, -3.0], Part::Motion => [-7.0, 5.0, 0.0],
            Part::Triggers => match self.trigger_side { Side::Left => [-4.0, 9.0, 7.0], Side::Right => [-11.0, 9.0, -7.0], Side::Both => [-9.0, 6.0, 0.0] },
        }};
        let pointer = if reduced { egui::Vec2::ZERO } else {
            ui.ctx().pointer_hover_pos().filter(|p| stage.contains(*p)).map(|p| {
                let v = (p - stage.center()) / (stage.size() * 0.5);
                egui::vec2(v.x.clamp(-1.0, 1.0), v.y.clamp(-1.0, 1.0))
            }).unwrap_or_default()
        };
        let duration = if reduced { 0.0 } else { 0.34 };
        let mut pose = [0.0; 3];
        for i in 0..3 { pose[i] = ui.ctx().animate_value_with_time(ui.id().with(("stage-pose", i)), target[i], duration); }
        let mut offset = egui::vec2(
            ui.ctx().animate_value_with_time(ui.id().with("pointer-x"), pointer.x * 5.0, duration),
            ui.ctx().animate_value_with_time(ui.id().with("pointer-y"), pointer.y * 3.0, duration),
        );
        pose[1] -= pointer.y * 2.0;
        pose[2] += pointer.x * if self.part == Part::Motion { 7.0 } else { 3.0 };
        ui.painter().text(stage.center() - egui::vec2(pointer.x * 3.0, 28.0), egui::Align2::CENTER_CENTER, "WAVE", egui::FontId::new(width * 0.285, egui::FontFamily::Name("signature".into())), accent_color());
        let mut input = dev.map(|d| d.snapshot().input).unwrap_or(ds_proto::InputState { lx: 128, ly: 128, rx: 128, ry: 128, ..Default::default() });
        if let Some(start) = self.simulation {
            let t = start.elapsed().as_secs_f32();
            if t < 0.42 && !reduced {
                let value = ((t / 0.42 * std::f32::consts::PI).sin() * 230.0) as u8;
                if self.trigger_side != Side::Right { input.l2 = value; }
                if self.trigger_side != Side::Left { input.r2 = value; }
                offset.y += (t / 0.42 * std::f32::consts::TAU * 3.0).sin() * 1.6;
                ui.ctx().request_repaint();
            } else { self.simulation = None; }
        }
        let lightbar = if reduced {
            if p.lighting.mode == LightMode::Off { [0; 3] } else { p.lighting.color.map(|v| (v as u16 * p.lighting.brightness as u16 / 100) as u8) }
        } else { self.preview.lightbar(p, &input, 0.0, Instant::now()) };
        let (_, clicked) = controller_view::draw_stage(ui, egui::vec2(width, h), &View { input: &input, lightbar, player_leds: 0, mute_led: p.microphone.muted, model: dev.map(|d| d.model).unwrap_or(ds_proto::Model::DualSense), connected: true }, pose, offset, (self.part == Part::Buttons).then_some(self.selected_button));
        if let Some(button) = clicked { self.selected_button = button; self.part = Part::Buttons; }
        for (part, side, label, x, y) in [(Part::Triggers, Side::Left, "L2", 0.18, 0.12), (Part::Triggers, Side::Right, "R2", 0.78, 0.12), (Part::Lighting, Side::Both, "Lighting", 0.46, 0.39), (Part::Buttons, Side::Both, "Buttons", 0.83, 0.48)] {
            let r = egui::Rect::from_center_size(stage.min + egui::vec2(width * x, h * y), egui::vec2(if label.len() > 2 { 74.0 } else { 36.0 }, 32.0));
            let selected = self.part == part && (part != Part::Triggers || self.trigger_side == side || self.trigger_side == Side::Both);
            let mut hotspot = ui.new_child(egui::UiBuilder::new().max_rect(r));
            if hotspot.add_sized(r.size(), egui::Button::new(tr(label)).wrap_mode(egui::TextWrapMode::Extend).fill(if selected { accent_soft_color() } else { bg_color() })).clicked() { self.part = part; self.trigger_side = side; }
        }
        choice(ui, &mut self.part, &[(Part::Lighting, "Lighting"), (Part::Triggers, "Triggers"), (Part::Buttons, "Buttons"), (Part::Motion, "Motion")]);
        if self.part == Part::Motion { widgets::muted(ui, tr("Preview perspective follows the pointer, not gyro data.")); }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, p: &mut Profile) {
        let (title, page) = match self.part { Part::Lighting => ("Lighting", Page::Lighting), Part::Triggers => ("Adaptive triggers", Page::Triggers), Part::Buttons => ("Buttons", Page::Mappings), Part::Motion => ("Motion", Page::Input) };
        ui.label(RichText::new(tr(title)).font(heading(23.0)));
        ui.add_space(10.0);
        match self.part {
            Part::Lighting => {
                choice(ui, &mut p.lighting.mode, &[(LightMode::Static, "Static"), (LightMode::Breathing, "Breathing"), (LightMode::Rainbow, "Rainbow"), (LightMode::Off, "Off")]);
                ui.label(tr("Color"));
                if let Some(color) = widgets::swatches(ui, p.lighting.color, &[[45,127,249], [147,94,238], [237,93,129], [239,151,54], [58,181,140], [255,255,255]]) { p.lighting.color = color; }
                ui.color_edit_button_srgb(&mut p.lighting.color);
                ui.add(egui::Slider::new(&mut p.lighting.brightness, 0..=100).text(tr("Brightness")).suffix("%"));
                widgets::muted(ui, tr("The preview reflects this profile's lightbar settings."));
            }
            Part::Triggers => {
                choice(ui, &mut self.trigger_side, &[(Side::Left, "Left · L2"), (Side::Both, "Both"), (Side::Right, "Right · R2")]);
                let selected_effect = if self.trigger_side == Side::Right { &p.triggers.right } else { &p.triggers.left };
                let selected_preset = (0..6).find(|i| matches_preset(selected_effect, *i));
                let both_match = self.trigger_side != Side::Both || p.triggers.left == p.triggers.right;
                let cell_width = ((ui.available_width() - 24.0) / 3.0).max(64.0);
                egui::Grid::new("quick-trigger-effects").num_columns(3).spacing(egui::vec2(12.0, 12.0)).show(ui, |ui| {
                    for (i, label) in PRESET_LABELS.iter().enumerate() {
                        let selected = selected_preset == Some(i) && both_match;
                        let color = if selected { accent_color() } else { muted_color() };
                        let response = ui.add(egui::Button::new("").frame(false).min_size(egui::vec2(cell_width, 70.0)));
                        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, tr(label)));
                        let curve = egui::Rect::from_center_size(response.rect.center() - egui::vec2(0.0, 10.0), egui::vec2(64.0, 28.0));
                        preset_curve(ui, curve, i, color);
                        ui.painter().text(response.rect.center_bottom() - egui::vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, tr(label), egui::FontId::proportional(12.0), color);
                        if selected || response.has_focus() { ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(2.0, accent_color())); }
                        if response.clicked() { apply_trigger(p, self.trigger_side, preset_effect(i)); }
                        if i % 3 == 2 { ui.end_row(); }
                    }
                });
                if !both_match { widgets::muted(ui, tr("Left and right use different effects.")); }
                else if let Some(i) = selected_preset { widgets::muted(ui, tr(PRESET_DESCRIPTIONS[i])); }
                else { widgets::muted(ui, tr("Custom effect · open Advanced to edit its parameters.")); }
                let effect = if self.trigger_side == Side::Right { &p.triggers.right } else { &p.triggers.left };
                if let Some(mut value) = intensity(effect) {
                    if ui.add(egui::Slider::new(&mut value, 1..=7).text(tr("Intensity")).suffix(" / 7")).changed() { adjust_intensity(p, self.trigger_side, value); }
                }
                ui.add_space(12.0);
                if ui.button(tr("Simulate visually")).clicked() && !(self.engine.settings.read().reduced_motion || self.system_reduced_motion) { self.simulation = Some(Instant::now()); }
                widgets::muted(ui, tr(if self.engine.settings.read().reduced_motion || self.system_reduced_motion { "Static preview · reduced motion enabled." } else { "Visual simulation only. No effect is sent by this button." }));
            }
            Part::Buttons => {
                choice(ui, &mut self.selected_button, &[(Button::Cross, "×"), (Button::Circle, "○"), (Button::Square, "□"), (Button::Triangle, "△"), (Button::Ps, "PS"), (Button::Mute, "Mute")]);
                ui.label(tr("Gesture"));
                choice(ui, &mut self.quick_gesture, &[(Gesture::Press, "Press"), (Gesture::LongPress, "Long press"), (Gesture::DoubleTap, "Double tap")]);
                let index = individual_mapping(p, self.selected_button, self.quick_gesture);
                let mut action = quick_action(index.map(|i| &p.mappings[i]));
                let previous = action;
                ui.label(tr("Action"));
                choice(ui, &mut action, &[(QuickAction::Original, "Original"), (QuickAction::Enter, "Enter key"), (QuickAction::Click, "Left click"), (QuickAction::Next, "Next profile")]);
                if action != previous { apply_quick_action(p, self.selected_button, self.quick_gesture, action); }
                if action == QuickAction::Custom { widgets::muted(ui, tr("Custom action · open Advanced to edit its parameters.")); }
                if let Some(i) = individual_mapping(p, self.selected_button, self.quick_gesture) {
                    ui.checkbox(&mut p.mappings[i].mute, tr("Suppress original virtual button"));
                }
                widgets::muted(ui, tr("Edits the first enabled individual mapping for this gesture. Combinations, timing and additional mappings are available in Advanced."));
            }
            Part::Motion => {
                ui.label(tr("Gyro"));
                choice(ui, &mut p.gyro.mode, &[(GyroMode::Off, "Off"), (GyroMode::Mouse, "Mouse")]);
                ui.add(egui::Slider::new(&mut p.gyro.sensitivity, 1.0..=40.0).text(tr("Sensitivity")));
                ui.label(tr("Touchpad"));
                choice(ui, &mut p.touchpad.mode, &[(TouchMode::Passthrough, "Game"), (TouchMode::Mouse, "Mouse")]);
                widgets::muted(ui, tr("Activation, calibration and advanced motion settings are available below."));
            }
        }
        ui.add_space(16.0);
        if ui.button(tr("Advanced settings →")).clicked() { self.page = page; self.advanced = true; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strength_preserves_effect_shape_and_the_opposite_side() {
        let mut p = Profile::default();
        p.triggers.left = preset_effect(3);
        p.triggers.right = preset_effect(5);
        let untouched = p.triggers.right.encode();
        adjust_intensity(&mut p, Side::Left, 3);
        assert_eq!(p.triggers.left, TriggerEffect::Weapon { start: 4, end: 6, strength: 3 });
        assert_eq!(p.triggers.right.encode(), untouched);
        let untouched = p.triggers.left.encode();
        adjust_intensity(&mut p, Side::Right, 4);
        assert_eq!(p.triggers.left.encode(), untouched);
        assert_eq!(p.triggers.right, TriggerEffect::Bow { start: 1, end: 6, strength: 4, snap: 7 });
        adjust_intensity(&mut p, Side::Both, 2);
        assert!(matches_preset(&p.triggers.left, 3));
        assert!(matches_preset(&p.triggers.right, 5));
        assert_eq!(intensity(&p.triggers.left), Some(2));
        assert_eq!(intensity(&p.triggers.right), Some(2));
        p.triggers.left = TriggerEffect::Off;
        p.triggers.right = TriggerEffect::Raw { bytes: [0x42; 11] };
        let before = p.triggers.clone();
        adjust_intensity(&mut p, Side::Both, 7);
        assert_eq!(p.triggers, before);
    }

    #[test]
    fn quick_mapping_edits_only_the_selected_individual_gesture() {
        let mut p = Profile::default();
        p.mappings.clear();
        p.mappings.push(Mapping { button: Button::Cross, with: vec![Button::L1], action: Action::NextProfile, ..Default::default() });
        p.mappings.push(Mapping { button: Button::Cross, gesture: Gesture::LongPress, action: Action::ToggleGyroMouse, ..Default::default() });
        let others = p.mappings.clone();
        // Opening an untouched gesture leaves the saved profile unchanged.
        assert_eq!(individual_mapping(&p, Button::Cross, Gesture::Press), None);
        apply_quick_action(&mut p, Button::Cross, Gesture::Press, QuickAction::Original);
        assert_eq!(p.mappings, others);
        apply_quick_action(&mut p, Button::Cross, Gesture::Press, QuickAction::Enter);
        assert_eq!(&p.mappings[..2], &others);
        assert_eq!(quick_action(p.mappings.last()), QuickAction::Enter);
        p.mappings[2].mute = true;
        apply_quick_action(&mut p, Button::Cross, Gesture::Press, QuickAction::Original);
        assert!(!p.mappings[2].mute);
        assert_eq!(p.mappings[2].action, Action::None);
        assert_eq!(&p.mappings[..2], &others);
    }

    #[test]
    fn selected_trigger_preserves_the_other_side() {
        let mut p = Profile::default();
        let original = TriggerEffect::Weapon { start: 4, end: 6, strength: 7 };
        p.triggers.right = original.clone();
        apply_trigger(&mut p, Side::Left, TriggerEffect::Feedback { position: 1, strength: 2 });
        assert_eq!(p.triggers.right, original);
        let left = p.triggers.left.clone();
        apply_trigger(&mut p, Side::Right, TriggerEffect::Off);
        assert_eq!(p.triggers.left, left);
        apply_trigger(&mut p, Side::Both, original.clone());
        assert_eq!(p.triggers.left, original);
        assert_eq!(p.triggers.left, p.triggers.right);
    }
}
