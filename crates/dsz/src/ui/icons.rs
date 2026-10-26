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
