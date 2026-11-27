use std::{error::Error, io::Write, os::fd::AsFd};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap,
    Point, PremultipliedColorU8, Rect, SpreadMode, Stroke, Transform};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1 as blur_manager, ext_background_effect_surface_v1 as blur_surface,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as shell, zwlr_layer_surface_v1 as layer,
};

use super::Row;
use crate::i18n::tr;

const WIDTH: i32 = 328;
const SCALE: i32 = 2;
const ROW_HEIGHT: i32 = 48;
const RADIUS: i32 = 26;

fn text(pixmap: &mut Pixmap, font: &FontArc, size: f32, x: i32, y: i32, limit: i32, value: &str, color: [u8; 3]) {
    let font = font.as_scaled(PxScale::from(size * SCALE as f32));
    let mut x = (x * SCALE) as f32;
    let y = (y * SCALE) as f32 + font.ascent();
    let edge = (limit * SCALE) as f32;
    let stride = pixmap.width() as usize;
    let height = pixmap.height() as i32;
    let pixels = pixmap.pixels_mut();
    let mut label: String = value.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    let width = |text: &str| {
        let mut prev = None;
        text.chars().map(|ch| {
            let id = font.glyph_id(ch);
            let advance = font.h_advance(id) + prev.map_or(0.0, |p| font.kern(p, id));
            prev = Some(id);
            advance
        }).sum::<f32>()
    };
    if width(&label) > edge - x {
        let allowance = edge - x - font.h_advance(font.glyph_id('…'));
        let mut used = 0.0;
        let mut prev = None;
        let cut = label.char_indices().find_map(|(offset, ch)| {
            let id = font.glyph_id(ch);
            used += font.h_advance(id) + prev.map_or(0.0, |p| font.kern(p, id));
            prev = Some(id);
            (used > allowance).then_some(offset)
        }).unwrap_or(label.len());
        label.truncate(cut);
        label.push('…');
    }
    let mut prev = None;
    for ch in label.chars() {
        let id = font.glyph_id(ch);
        if let Some(prev) = prev { x += font.kern(prev, id); }
        let advance = font.h_advance(id);
        if x + advance > edge { break; }
        let mut glyph = font.scaled_glyph(ch);
        glyph.position = point(x, y);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|cx, cy, coverage| {
                let px = bounds.min.x as i32 + cx as i32;
                let py = bounds.min.y as i32 + cy as i32;
                if px < 0 || px >= stride as i32 || py < 0 || py >= height { return; }
                let dst = &mut pixels[py as usize * stride + px as usize];
                let mix = |s: u8, d: u8| (f32::from(s) * coverage + f32::from(d) * (1.0 - coverage)).round() as u8;
                let a = mix(255, dst.alpha());
                *dst = PremultipliedColorU8::from_rgba(
                    mix(color[0], dst.red()), mix(color[1], dst.green()), mix(color[2], dst.blue()), a,
                ).unwrap_or(*dst);
            });
        }
        x += advance;
        prev = Some(id);
    }
}

fn icon(bitmap: &mut Pixmap, transform: Transform, x: f32, y: f32, points: &[(f32, f32)]) {
    let mut path = PathBuilder::new();
    path.move_to(x + points[0].0, y + points[0].1);
    for &(dx, dy) in &points[1..] { path.line_to(x + dx, y + dy); }
    let path = path.finish().unwrap();
    let mut paint = Paint::default();
    let mut stroke = Stroke {
        width: 3.0, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    paint.set_color_rgba8(16, 25, 43, 165);
    bitmap.stroke_path(&path, &paint, &stroke, transform, None);
    stroke.width = 1.5;
    paint.set_color_rgba8(231, 244, 255, 255);
    bitmap.stroke_path(&path, &paint, &stroke, transform, None);
}

fn render(rows: &[Row], blur: bool) -> Pixmap {
    let rows = &rows[..rows.len().min(5)];
    let height = ROW_HEIGHT * rows.len().max(1) as i32 + 12;
    let mut bitmap = Pixmap::new((WIDTH * SCALE) as u32, (height * SCALE) as u32).unwrap();
    let (w, h, r) = (WIDTH as f32, height as f32, RADIUS as f32);
    let mut outline = PathBuilder::new();
    outline.move_to(r, 1.0);
    outline.line_to(w - r, 1.0);
    outline.quad_to(w - 1.0, 1.0, w - 1.0, r);
    outline.line_to(w - 1.0, h - r);
    outline.quad_to(w - 1.0, h - 1.0, w - r, h - 1.0);
    outline.line_to(r, h - 1.0);
    outline.quad_to(1.0, h - 1.0, 1.0, h - r);
    outline.line_to(1.0, r);
    outline.quad_to(1.0, 1.0, r, 1.0);
    outline.close();
    let outline = outline.finish().unwrap();
    let transform = Transform::from_scale(SCALE as f32, SCALE as f32);
    let mut paint = Paint::default();
    let (light, dark) = if blur { (55, 106) } else { (130, 172) };
    paint.shader = LinearGradient::new(Point::from_xy(0.0, 0.0), Point::from_xy(w, h),
        vec![GradientStop::new(0.0, Color::from_rgba8(52, 68, 89, light)),
             GradientStop::new(1.0, Color::from_rgba8(16, 25, 43, dark))],
        SpreadMode::Pad, Transform::identity()).unwrap();
    bitmap.fill_path(&outline, &paint, FillRule::Winding, transform, None);
    paint.shader = LinearGradient::new(Point::from_xy(0.0, 0.0), Point::from_xy(0.0, h),
        vec![GradientStop::new(0.0, Color::from_rgba8(240, 248, 255, 144)),
             GradientStop::new(1.0, Color::from_rgba8(200, 222, 255, 40))],
        SpreadMode::Pad, Transform::identity()).unwrap();
    bitmap.stroke_path(&outline, &paint, &Stroke { width: 0.8, ..Default::default() }, transform, None);

    let normal = FontArc::try_from_slice(include_bytes!("../../assets/InstrumentSans-SemiBold.ttf")).unwrap();
    let needs_fallback = |value: &str| value.chars().any(|ch| normal.glyph_id(ch).0 == 0);
    let empty = tr("No controller connected");
    let cjk = (rows.is_empty() && needs_fallback(empty) || rows.iter().any(|row| needs_fallback(&row.profile)))
        .then(|| ["/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"].iter()
            .find_map(|p| std::fs::read(p).ok().and_then(|bytes| FontArc::try_from_vec(bytes).ok())))
        .flatten();
    let label_font = |value: &str| cjk.as_ref().filter(|_| needs_fallback(value)).unwrap_or(&normal);
    let white = [249, 252, 255];
    let ice = [206, 241, 255];
    let draw_label = |bitmap: &mut Pixmap, value: &str, size: f32, x, y, edge, color| {
        text(bitmap, label_font(value), size, x, y + 1, edge, value, [11, 19, 32]);
        text(bitmap, label_font(value), size, x, y, edge, value, color);
    };
    if rows.is_empty() {
        draw_label(&mut bitmap, empty, 22.0, 24, 18, WIDTH - 24, white);
    }
    for (index, row) in rows.iter().enumerate() {
        let y = 6 + index as i32 * ROW_HEIGHT;
        let cy = y as f32 + 24.0;
        let level = row.level();
        let transport = if row.bluetooth {
            &[(0.0, -10.0), (5.0, -5.0), (-5.0, 5.0), (0.0, 0.0), (5.0, 5.0), (0.0, 10.0), (0.0, -10.0)][..]
        } else {
            &[(0.0, -9.0), (0.0, 9.0), (0.0, 2.0), (-6.0, -3.0), (-6.0, -6.0), (-6.0, -3.0), (0.0, 2.0), (6.0, -3.0), (6.0, -6.0)][..]
        };
        icon(&mut bitmap, transform, 25.0, cy, transport);
        if row.bluetooth {
            icon(&mut bitmap, transform, 25.0, cy, &[(-5.0, -5.0), (0.0, 0.0)]);
        } else {
            icon(&mut bitmap, transform, 25.0, cy, &[(-3.0, -6.0), (0.0, -9.0), (3.0, -6.0)]);
        }
        draw_label(&mut bitmap, &row.profile, 24.0, 46, y + 12, WIDTH - 105, white);

        let battery_x = (WIDTH - 90) as f32;
        icon(&mut bitmap, transform, battery_x, cy,
            &[(-12.0, -6.0), (10.0, -6.0), (10.0, 6.0), (-12.0, 6.0), (-12.0, -6.0)]);
        icon(&mut bitmap, transform, battery_x, cy, &[(13.0, -2.0), (13.0, 2.0)]);
        if row.charge.is_some() && row.status == ds_proto::input::BatteryStatus::Charging {
            icon(&mut bitmap, transform, battery_x, cy, &[(0.0, -4.0), (-3.0, 1.0), (2.0, 1.0), (-1.0, 5.0)]);
        } else if let Some(level) = row.charge.filter(|n| *n > 0) {
            paint.set_color_rgba8(206, 241, 255, 225);
            bitmap.fill_rect(Rect::from_xywh(battery_x - 9.0, cy - 3.0,
                16.0 * f32::from(level.min(100)) / 100.0, 6.0).unwrap(), &paint, transform, None);
        }
        draw_label(&mut bitmap, &level, 23.0, WIDTH - 66, y + 13, WIDTH - 13, ice);
    }
    bitmap
}

#[derive(Default)]
struct State {
    configured: bool,
    closed: bool,
    blur: bool,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<layer::ZwlrLayerSurfaceV1, ()> for State {
    fn event(state: &mut Self, layer: &layer::ZwlrLayerSurfaceV1, event: layer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            layer::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                state.configured = true;
            }
            layer::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}
delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore shell::ZwlrLayerShellV1);
impl Dispatch<blur_manager::ExtBackgroundEffectManagerV1, ()> for State {
    fn event(state: &mut Self, _: &blur_manager::ExtBackgroundEffectManagerV1, event: blur_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let blur_manager::Event::Capabilities { flags } = event {
            state.blur = matches!(flags, wayland_client::WEnum::Value(flags) if flags.contains(blur_manager::Capability::Blur));
        }
    }
}
delegate_noop!(State: ignore blur_surface::ExtBackgroundEffectSurfaceV1);

pub(super) struct Overlay {
    _connection: Connection,
}

pub(super) fn show(rows: &[Row]) -> Result<Overlay, Box<dyn Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 3..=4, ())?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let shell: shell::ZwlrLayerShellV1 = globals.bind(&qh, 1..=1, ())?;
    let height = (ROW_HEIGHT * rows.len().clamp(1, 5) as i32 + 12) * SCALE;
    let width = WIDTH * SCALE;
    let blur_manager = globals.bind::<blur_manager::ExtBackgroundEffectManagerV1, _, _>(&qh, 1..=1, ()).ok();
    let surface = compositor.create_surface(&qh, ());
    let layer = shell.get_layer_surface(&surface, None, shell::Layer::Overlay, "dualwave-status".into(), &qh, ());
    layer.set_size(width as u32 / SCALE as u32, height as u32 / SCALE as u32);
    layer.set_anchor(layer::Anchor::Bottom);
    layer.set_margin(0, 0, 36, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(layer::KeyboardInteractivity::None);
    let input = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&input));
    input.destroy();
    surface.commit();
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    if !state.configured || state.closed { return Err("layer did not configure".into()); }
    if let Some(manager) = blur_manager.filter(|_| state.blur) {
        let blur = manager.get_background_effect(&surface, &qh, ());
        let region = compositor.create_region(&qh, ());
        let h = height / SCALE;
        for y in 1..h-1 {
            let d = (RADIUS - y.min(h - y)).max(0) as f32;
            let radius = RADIUS as f32;
            let inset = (radius - (radius * radius - d * d).sqrt()).ceil() as i32 + 1;
            region.add(inset, y, WIDTH - 2 * inset, 1);
        }
        blur.set_blur_region(Some(&region));
        region.destroy();
    }

    let mut file = tempfile::tempfile()?;
    let mut pixels = render(rows, state.blur).take();
    // wl_shm ARGB8888 stores bytes as BGRA on little-endian Linux.
    for pixel in pixels.chunks_exact_mut(4) { pixel.swap(0, 2); }
    file.write_all(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &qh, ());
    let buffer = pool.create_buffer(0, width, height, width * 4, wl_shm::Format::Argb8888, &qh, ());
    pool.destroy();
    surface.set_buffer_scale(SCALE);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, width / SCALE, height / SCALE);
    surface.commit();
    queue.roundtrip(&mut state)?;
    if state.closed { return Err("layer closed".into()); }
    log::debug!("controller status glass shown; compositor blur capability: {}", state.blur);
    Ok(Overlay { _connection: conn })
}
