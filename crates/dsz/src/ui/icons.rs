//! Small vector navigation marks, independent of font glyph coverage.
use eframe::egui::{self, Color32, Pos2, Rect, Stroke};

pub(super) fn navigation(ui: &egui::Ui, rect: Rect, label: &str, color: Color32) {
    let p = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let at = |x, y| rect.min + egui::vec2(x, y) * (rect.width() / 18.0);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect::<Vec<Pos2>>(), stroke));
    };
    match label {
        "Controller" => {
            line(&[(5., 5.), (13., 5.), (15., 7.), (17., 13.), (16., 15.), (14., 15.), (11., 12.), (7., 12.), (4., 15.), (2., 15.), (1., 13.), (3., 7.), (5., 5.)]);
            line(&[(4., 9.), (8., 9.)]); line(&[(6., 7.), (6., 11.)]);
            p.circle_filled(at(12., 8.), 1.0, color); p.circle_filled(at(14., 10.), 1.0, color);
        }
        "Audio" => {
            p.add(egui::Shape::line((0..=16).map(|i| {
                let a = std::f32::consts::PI * i as f32 / 16.;
                at(9. - 6. * a.cos(), 9. - 6. * a.sin())
            }).collect(), stroke));
            for x in [3., 13.] {
                p.rect_stroke(Rect::from_min_max(at(x, 9.), at(x + 2., 15.)), 1., stroke, egui::StrokeKind::Inside);
            }
        }
        "Profiles" => {
            line(&[(3., 5.), (9., 5.), (11., 7.), (16., 7.), (16., 15.), (3., 15.), (3., 5.)]);
            line(&[(5., 2.), (11., 2.), (13., 4.), (16., 4.)]);
        }
        "Preferences" => {
            for (x, y) in [(4., 6.), (9., 12.), (14., 7.)] {
                line(&[(x, 2.), (x, y - 2.)]); line(&[(x, y + 2.), (x, 16.)]);
                p.circle_stroke(at(x, y), 2., stroke);
            }
        }
        _ => {}
    }
}

// Drawn as vectors so these marks remain sharp at every UI scale.
pub(super) fn control(p: &egui::Painter, rect: Rect, label: &str, color: Color32) {
    let at = |x, y| rect.min + egui::vec2(x * rect.width(), y * rect.height());
    let line = |points: &[(f32, f32)], width| {
        p.add(egui::Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect::<Vec<Pos2>>(), Stroke::new(rect.width() * width, color)));
    };
    match label {
        "×" | "○" | "□" | "△" => {
            let button = match label { "×" => ds_proto::input::Button::Cross, "○" => ds_proto::input::Button::Circle, "□" => ds_proto::input::Button::Square, _ => ds_proto::input::Button::Triangle };
            super::controller_view::face_symbol(p, rect.center(), rect.width().min(rect.height()) * 0.29, button, color);
            return;
        }
        "Mute" => {
            let stroke = Stroke::new(1.5, color);
            p.rect_stroke(Rect::from_min_max(at(0.38, 0.10), at(0.62, 0.58)), rect.width() * 0.12, stroke, egui::StrokeKind::Middle);
            line(&[(0.24, 0.43), (0.24, 0.57), (0.30, 0.69), (0.42, 0.75), (0.58, 0.75), (0.70, 0.69), (0.76, 0.57), (0.76, 0.43)], 0.06);
            line(&[(0.50, 0.75), (0.50, 0.91)], 0.06);
            line(&[(0.34, 0.91), (0.66, 0.91)], 0.06);
            line(&[(0.14, 0.13), (0.88, 0.87)], 0.08);
            return;
        }
        "Lighting" => {
            let stroke = Stroke::new(1.5, color);
            p.circle_stroke(at(0.5, 0.5), rect.width() * 0.18, stroke);
            for i in 0..8 {
                let angle = std::f32::consts::TAU * i as f32 / 8.0;
                let (sin, cos) = angle.sin_cos();
                p.line_segment([at(0.5 + cos * 0.30, 0.5 + sin * 0.30), at(0.5 + cos * 0.43, 0.5 + sin * 0.43)], stroke);
            }
            return;
        }
        "Buttons" => {
            let stroke = Stroke::new(1.3, color);
            line(&[(0.50, 0.08), (0.37, 0.30), (0.63, 0.30), (0.50, 0.08)], 0.05);
            p.circle_stroke(at(0.80, 0.50), rect.width() * 0.11, stroke);
            p.rect_stroke(Rect::from_min_max(at(0.09, 0.39), at(0.31, 0.61)), 0.0, stroke, egui::StrokeKind::Middle);
            line(&[(0.40, 0.70), (0.60, 0.90)], 0.05);
            line(&[(0.60, 0.70), (0.40, 0.90)], 0.05);
            return;
        }
        "PS" => {}
        _ => { super::controller_view::trigger_icon(p, rect, label, color); return; }
    }
    super::controller_view::playstation_icon(p, rect, color);
}
