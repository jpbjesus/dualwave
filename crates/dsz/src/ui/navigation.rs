//! Work areas and their directly accessible configuration pages.
use eframe::egui::{self, RichText, Stroke};
use super::{App, Page, theme::*, workspace::choice};

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
        let current = match self.page { Page::Haptics => 1, Page::Profiles => 2, Page::Settings => 3, _ => 0 };
        ui.horizontal_wrapped(|ui| {
            mark(ui);
            ui.label(RichText::new("DualWave").font(heading(24.0)).color(text_color()));
            ui.add_space(if ui.available_width() > 900.0 { ui.available_width() * 0.24 } else { 24.0 });
            let mut destination = current;
            if choice(ui, &mut destination, &[(0, "Controller"), (1, "Audio"), (2, "Profiles"), (3, "Preferences")]) {
                let (page, detailed) = match destination {
                    0 => (Page::Overview, false),
                    1 => (Page::Haptics, false),
                    2 => (Page::Profiles, false),
                    _ => (Page::Settings, false),
                };
                self.page = page;
                self.detailed = detailed;
                self.capture_key = None;
            }
        });
    }

    pub(super) fn section_navigation(&mut self, ui: &mut egui::Ui) {
        if matches!(self.page, Page::Profiles | Page::Settings) { return; }
        let mut selected = (self.page, self.detailed);
        let options: &[((Page, bool), &str)] = if self.page == Page::Haptics {
            &[((Page::Haptics, false), "Playback & microphone"), ((Page::Haptics, true), "Haptics & audio")]
        } else {
            &[
                ((Page::Overview, false), "Visual workspace"),
                ((Page::Lighting, true), "Lighting"), ((Page::Triggers, true), "Triggers"),
                ((Page::Input, true), "Motion"), ((Page::Mappings, true), "Buttons"),
                ((Page::Virtual, true), "Virtual controller"),
                ((Page::Overview, true), "Device status"),
            ]
        };
        if choice(ui, &mut selected, options) {
            self.page = selected.0;
            self.detailed = selected.1;
            self.capture_key = None;
        }
        ui.separator();
        ui.add_space(4.0);
    }
}
