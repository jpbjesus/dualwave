//! Visual input choices shared by single actions and macro steps.
use super::*;

pub(super) fn shortcuts(ui: &mut egui::Ui) -> Option<MacroInput> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for (label, keys, vk, ctrl, alt, win) in [
            ("Switch window", "Alt + Tab", 9, false, true, false),
            ("Copy", "Ctrl + C", 67, true, false, false),
            ("Paste", "Ctrl + V", 86, true, false, false),
            ("Show desktop", "Win + D", 68, false, false, true),
        ] {
            let label = crate::i18n::tr(label);
            let font = egui::TextStyle::Button.resolve(ui.style());
            let width = ui.fonts(|f| f.layout_no_wrap(label.to_string(), font, text_color()).size().x + ui.spacing().button_padding.x * 2.0).max(104.0);
            fit(ui, width);
            ui.vertical(|ui| {
                ui.set_width(width);
                if ui.add(egui::Button::new(label).min_size(egui::vec2(104.0, 30.0))).clicked() {
                    chosen = Some(MacroInput::Key { vk, ctrl, shift: false, alt, win });
                }
                ui.label(RichText::new(keys).small().color(muted_color()));
            });
        }
    });
    chosen
}

fn keycap(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new().fill(field_color()).stroke(Stroke::new(1.0, border_color()))
        .corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| { ui.label(RichText::new(label).strong().size(17.0)); });
}

pub(super) fn caps(ui: &mut egui::Ui, vk: u16, ctrl: bool, shift: bool, alt: bool, win: bool) {
    let mut labels = Vec::new();
    if ctrl { labels.push("Ctrl".to_string()); }
    if shift { labels.push(crate::i18n::tr("Shift").to_string()); }
    if alt { labels.push("Alt".to_string()); }
    if win { labels.push("Win".to_string()); }
    labels.push(key_label(vk));
    for (i, label) in labels.iter().enumerate() {
        if i > 0 { ui.label(RichText::new("+").color(muted_color())); }
        keycap(ui, label);
    }
}

pub(super) fn keyboard(ui: &mut egui::Ui, vk: &mut u16, ctrl: &mut bool, shift: &mut bool, alt: &mut bool, win: &mut bool, recording: bool) -> bool {
    let mut record = false;
    ui.horizontal_wrapped(|ui| {
        caps(ui, *vk, *ctrl, *shift, *alt, *win);
        record = ui.add(egui::Button::new(crate::i18n::tr(if recording { "Press a key…" } else { "Record shortcut" }))
            .selected(recording).min_size(egui::vec2(0.0, 32.0))).clicked();
        if recording { muted(ui, crate::i18n::tr("Esc to cancel")); }
    });
    egui::CollapsingHeader::new(crate::i18n::tr("Edit keys")).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.toggle_value(ctrl, "Ctrl");
            ui.toggle_value(shift, crate::i18n::tr("Shift"));
            ui.toggle_value(alt, "Alt");
            ui.toggle_value(win, "Win");
        });
        let id = ui.id().with("key-search");
        let mut search = ui.ctx().data_mut(|d| d.get_temp::<String>(id).unwrap_or_default());
        ui.add(egui::TextEdit::singleline(&mut search).hint_text(crate::i18n::tr("Search keys")).desired_width(ui.available_width().min(260.0)));
        ui.ctx().data_mut(|d| d.insert_temp(id, search.clone()));
        egui::ScrollArea::vertical().id_salt("key-tiles").max_height(150.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for key in key_catalog() {
                    let label = key_label(key);
                    if !label.to_lowercase().contains(&search.to_lowercase()) { continue; }
                    if ui.add(egui::Button::new(label).selected(*vk == key).min_size(egui::vec2(32.0, 30.0))).clicked() { *vk = key; }
                }
            });
        });
    });
    record
}

pub(super) fn buttons(ui: &mut egui::Ui, selected: &mut Button, edge: bool, source: bool) {
    ui.horizontal_wrapped(|ui| {
        for button in button_choices(edge).into_iter().filter(|b| source || !b.is_touchpad_zone()) {
            if ui.add(egui::Button::new(crate::i18n::tr(button.short_name())).selected(*selected == button)
                .min_size(egui::vec2(36.0, 30.0))).clicked() { *selected = button; }
        }
    });
}

pub(super) fn controller(ui: &mut egui::Ui, selected: &mut Button, edge: bool) {
    let input = ds_proto::InputState::default();
    let view = crate::ui::controller_view::View {
        input: &input, lightbar: [60, 100, 220], player_leds: 0, mute_led: false,
        model: if edge { ds_proto::Model::DualSenseEdge } else { ds_proto::Model::DualSense }, connected: true,
    };
    let width = ui.available_width().min(340.0);
    let (_, picked) = crate::ui::controller_view::draw_stage(ui, egui::vec2(width, width * 0.58), &view, [0.0; 3], egui::Vec2::ZERO, Some(*selected));
    if let Some(button) = picked { *selected = button; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn touch_zones_are_source_choices_but_never_virtual_output_choices() {
        for source in [false, true] {
            let ctx = egui::Context::default();
            let mut selected = Button::Cross;
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(1200.0,800.0))), ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| buttons(ui,&mut selected,false,source));
            });
            for zone in [Button::TouchpadLeft,Button::TouchpadCenter,Button::TouchpadRight] {
                let visible = output.shapes.iter().any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(zone.short_name())));
                assert_eq!(visible,source,"{zone:?} is only an input source");
            }
        }
    }
}
