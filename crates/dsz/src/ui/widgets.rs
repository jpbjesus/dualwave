//! Reusable widgets: cards, toggles, segmented controls, setting rows.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};

use super::theme::*;

/// A rounded card with an optional title and subtitle.
pub fn card<R>(ui: &mut Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_glow(ui, title, subtitle, 0.0, add).0
}

/// [`card`] lit up in the accent color by `glow` (0 to 1), to point at it.
/// Also returns the card's response, to scroll to it.
pub fn card_glow<R>(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    glow: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, Response) {
    let w = (ui.available_width() - 2.0).max(100.0);
    let r = egui::Frame::new()
        .fill(mix(surface_color(), accent_color(), glow * 0.12))
        .stroke(Stroke::new(glow, accent_color()))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(egui::Margin::symmetric(0, 12))
        .show(ui, |ui| {
            ui.set_width(w);
            if !title.is_empty() {
                ui.label(RichText::new(title).font(heading(16.0)).color(text_color()));
                if !subtitle.is_empty() {
                    ui.label(RichText::new(subtitle).size(12.5).color(muted_color()));
                }
                ui.add_space(8.0);
            }
            add(ui)
        });
    (r.inner, r.response)
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).font(heading(24.0)).color(text_color()));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).size(13.5).color(muted_color()));
    }
    ui.add_space(8.0);
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = egui::vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let bg = mix(Color32::from_rgb(0x2C, 0x35, 0x48), accent_color(), t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), bg);
    let r = rect.height() / 2.0 - 3.0;
    let x = egui::lerp((rect.left() + r + 3.0)..=(rect.right() - r - 3.0), t);
    p.circle_filled(egui::pos2(x, rect.center().y), r, Color32::WHITE);
    if resp.hovered() {
        p.rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(1.0_f32, with_alpha(Color32::WHITE, 40)),
            StrokeKind::Outside,
        );
    }
    resp
}

/// A setting row: label (+ hint) on the left, control on the right.
pub fn row<R>(ui: &mut Ui, label: &str, hint: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let origin = ui.cursor().min;
    let width = ui.available_width();
    // Lay out controls first so a switch reserves only its actual width.
    // Help text can then use the rest, rather than a fixed 42% column.
    let control_rect = egui::Rect::from_min_size(origin + egui::vec2(width * 0.42, 0.0), egui::vec2(width * 0.58, 34.0));
    let mut controls = ui.new_child(egui::UiBuilder::new().max_rect(control_rect)
        .layout(Layout::right_to_left(Align::Center)));
    let out = control(&mut controls);
    let control_bounds = controls.min_rect();
    let label_right = (control_bounds.left() - 16.0).max(origin.x + 1.0);
    let mut labels = ui.new_child(egui::UiBuilder::new()
        .max_rect(egui::Rect::from_min_max(origin + egui::vec2(0.0, 3.0), egui::pos2(label_right, origin.y + 34.0)))
        .layout(Layout::top_down(Align::Min)));
    labels.spacing_mut().item_spacing.y = 4.0;
    labels.add(egui::Label::new(RichText::new(label).color(text_color())).wrap());
    if !hint.is_empty() {
        labels.add(egui::Label::new(RichText::new(hint).size(12.0).color(muted_color())).wrap());
    }
    let bottom = labels.min_rect().bottom().max(control_bounds.bottom()).max(origin.y + 34.0);
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(origin, egui::pos2(origin.x + width, bottom)));
    out
}

pub fn toggle_row(ui: &mut Ui, label: &str, hint: &str, on: &mut bool) -> bool {
    row(ui, label, hint, |ui| toggle(ui, on).changed())
}

pub fn slider_row<N: egui::emath::Numeric>(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    v: &mut N,
    range: std::ops::RangeInclusive<N>,
    suffix: &str,
) -> bool {
    row(ui, label, hint, |ui| {
        ui.add(
            egui::Slider::new(v, range)
                .suffix(suffix)
                .trailing_fill(true),
        )
        .changed()
    })
}

/// Segmented control. Returns true when the value changed. Lays itself
/// out as one fixed-size block, so it reads left to right in any parent.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = egui::FontId::proportional(13.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| {
            ui.fonts(|f| f.layout_no_wrap(l.to_string(), font.clone(), text_color()).size().x) + 24.0
        })
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * (options.len().saturating_sub(1)) as f32 + 6.0;
    let width = total.min(ui.available_width()).max(1.0);
    let mut positions = Vec::new();
    let mut x = 3.0;
    let mut y = 3.0;
    for natural_width in &widths {
        let w = natural_width.min((width - 6.0).max(1.0));
        if x > 3.0 && x + w > width - 3.0 {
            x = 3.0;
            y += 32.0;
        }
        positions.push((x, y, w));
        x += w + 2.0;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, y + 31.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, CornerRadius::same(R_CTRL + 2), field_color());
    p.rect_stroke(
        rect,
        CornerRadius::same(R_CTRL + 2),
        Stroke::new(1.0_f32, border_color()),
        StrokeKind::Inside,
    );
    let mut changed = false;
    for (i, ((v, label), (x, y, w))) in options.iter().zip(positions).enumerate() {
        let r = egui::Rect::from_min_size(rect.min + egui::vec2(x, y), egui::vec2(w, 28.0));
        let resp = ui.interact(
            r,
            ui.id()
                .with(("seg", i, rect.min.x as i32, rect.min.y as i32)),
            Sense::click(),
        );
        let sel = *value == *v;
        if sel {
            p.rect_filled(r, CornerRadius::same(R_CTRL), accent_color());
        } else if resp.hovered() {
            p.rect_filled(r, CornerRadius::same(R_CTRL), card_hi_color());
        }
        let fg = if sel {
            accent_text_color()
        } else if resp.hovered() {
            text_color()
        } else {
            muted_color()
        };
        p.with_clip_rect(ui.clip_rect().intersect(r)).text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            fg,
        );
        if resp.clicked() && !sel {
            *value = v.clone();
            changed = true;
        }
    }
    changed
}

pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = egui::FontId::proportional(12.0);
    let w = ui.fonts(|f| {
        f.layout_no_wrap(text.to_string(), font.clone(), color)
            .size()
            .x
    });
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w + 18.0, 22.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(11), with_alpha(color, 34));
    p.rect_stroke(
        rect,
        CornerRadius::same(11),
        Stroke::new(1.0_f32, with_alpha(color, 90)),
        StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        color,
    );
    resp
}

/// Horizontal level meter 0..=1.
pub fn meter(ui: &mut Ui, v: f32, color: Color32, width: f32) -> Response {
    // Never wider than the room left, so a narrow column doesn't push the
    // page wider.
    let width = width.min(ui.available_width()).max(24.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), field_color());
    let mut fill = rect;
    fill.set_width(rect.width() * v.clamp(0.0, 1.0));
    if fill.width() > 0.5 {
        p.rect_filled(fill, CornerRadius::same(4), color);
    }
    resp
}

pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(accent_text_color()))
            .fill(accent_color())
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(R_CTRL))
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(text)
            .corner_radius(CornerRadius::same(R_CTRL))
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(bad_color()))
            .fill(with_alpha(bad_color(), 24))
            .stroke(Stroke::new(1.0_f32, with_alpha(bad_color(), 80)))
            .corner_radius(CornerRadius::same(R_CTRL))
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

pub fn muted(ui: &mut Ui, text: &str) {
    // Wraps in the room left even inside a row, where labels would
    // otherwise run past the column.
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(muted_color())).wrap());
}

pub fn note(ui: &mut Ui, text: &str, color: Color32) {
    let w = (ui.available_width() - 26.0).max(100.0);
    egui::Frame::new()
        .fill(with_alpha(color, 22))
        .stroke(Stroke::new(1.0_f32, with_alpha(color, 70)))
        .corner_radius(CornerRadius::same(R_CTRL + 2))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(w);
            ui.label(RichText::new(text).size(13.0).color(mix(color, text_color(), 0.55)));
        });
}

/// Swatches for quick color picks. Returns the picked color.
pub fn swatches(ui: &mut Ui, current: [u8; 3], colors: &[[u8; 3]]) -> Option<[u8; 3]> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for c in colors {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
            let p = ui.painter();
            p.circle_filled(rect.center(), 11.0, rgb(*c));
            if *c == current {
                p.circle_stroke(rect.center(), 12.5, Stroke::new(2.0_f32, Color32::WHITE));
            } else if resp.hovered() {
                p.circle_stroke(
                    rect.center(),
                    12.5,
                    Stroke::new(1.5_f32, with_alpha(Color32::WHITE, 120)),
                );
            }
            if resp.clicked() {
                picked = Some(*c);
            }
        }
    });
    picked
}

/// Two-column grid of cards with equal widths.
/// `f` is called with column 0, then column 1. Stacks on narrow windows.
/// In a wrapped row, start a new line unless `w` points still fit. Combo
/// boxes and sliders lay out in their own inner row, which a wrapped row
/// can't break, so without this they push the row (and the page) wider.
pub fn fit(ui: &mut Ui, w: f32) {
    // A wrapped row's available width is the whole row; what's left is
    // from the cursor to the row's right edge.
    let left = ui.max_rect().right() - ui.cursor().min.x;
    if left < w + ui.spacing().item_spacing.x {
        ui.end_row();
    }
}

pub fn two_columns(ui: &mut Ui, mut f: impl FnMut(&mut Ui, usize)) {
    let w = ui.available_width();
    // Below this a column can't hold the widest rows (the six lightbar
    // modes, a trigger's byte string beside its button).
    if w < 900.0 {
        f(ui, 0);
        ui.add_space(12.0);
        f(ui, 1);
        return;
    }
    let spacing = ui.spacing().item_spacing.x;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 28.0;
        ui.columns(2, |cols| {
            cols[0].spacing_mut().item_spacing.x = spacing;
            cols[1].spacing_mut().item_spacing.x = spacing;
            // Columns default to a justified layout, which spreads wrapped text.
            let (a, b) = cols.split_at_mut(1);
            a[0].with_layout(Layout::top_down(Align::Min), |ui| f(ui, 0));
            b[0].with_layout(Layout::top_down(Align::Min), |ui| f(ui, 1));
        });
    });
}
