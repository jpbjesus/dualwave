//! Colors, fonts, and egui style.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};

static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn palette(light: u32, dark: u32) -> Color32 {
    let c = if DARK.load(std::sync::atomic::Ordering::Relaxed) { dark } else { light };
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
pub fn bg_color() -> Color32 { palette(0xf8f9fc, 0x11151c) }
pub fn surface_color() -> Color32 { palette(0xf8f9fc, 0x11151c) }
pub fn card_hi_color() -> Color32 { palette(0xe7ecf3, 0x252d39) }
pub fn field_color() -> Color32 { palette(0xeef1f7, 0x171c24) }
pub fn border_color() -> Color32 { palette(0xc0cad7, 0x343e4e) }
pub fn text_color() -> Color32 { palette(0x192333, 0xedf1f7) }
pub fn muted_color() -> Color32 { palette(0x526277, 0xa4b0c1) }
pub fn faint_color() -> Color32 { palette(0x526277, 0xa4b0c1) }
pub fn accent_color() -> Color32 { palette(0x2855df, 0x8ab4ff) }
pub fn accent_text_color() -> Color32 { palette(0xffffff, 0x11151c) }
pub fn accent_soft_color() -> Color32 { palette(0xe7ecfa, 0x23365a) }
pub fn violet_color() -> Color32 { palette(0x6d3eb6, 0xb9a2ed) }
pub fn good_color() -> Color32 { palette(0x27664e, 0xa4d5c2) }
pub fn warn_color() -> Color32 { palette(0x82531e, 0xe6c497) }
pub fn bad_color() -> Color32 { palette(0xb12e38, 0xff9ba4) }

pub const R_CARD: u8 = 0;
pub const R_CTRL: u8 = 5;

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn heading(size: f32) -> FontId {
    FontId::new(size, semibold())
}

fn load_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

pub fn install(ctx: &egui::Context, scale: f32) {
    let mut fonts = FontDefinitions::default();
    let fallback_prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    if let Some(bytes) = load_font(&[r"C:\Windows\Fonts\segoeui.ttf", "/usr/share/fonts/noto/NotoSans-Regular.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf"]) {
        fonts
            .font_data
            .insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "segoe".into());
    }
    let mut sb = Vec::new();
    if let Some(bytes) = load_font(&[
        r"C:\Windows\Fonts\seguisb.ttf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        r"C:\Windows\Fonts\segoeuib.ttf",
    ]) {
        fonts
            .font_data
            .insert("segoe_sb".into(), Arc::new(FontData::from_owned(bytes)));
        sb.push("segoe_sb".to_string());
    }
    // Symbols (arrows, shapes) for icons and glyphs.
    for (name, paths) in [("segoe_sym", &[r"C:\Windows\Fonts\seguisym.ttf", "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf"][..]), ("noto_sym", &["/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf"][..])] {
        if let Some(bytes) = load_font(paths) {
            fonts
                .font_data
                .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .push(name.into());
            sb.push(name.into());
        }
    }
    if let Some(bytes) = load_font(&[r"C:\Windows\Fonts\consola.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"]) {
        fonts
            .font_data
            .insert("consolas".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .get_mut(&FontFamily::Monospace)
            .unwrap()
            .insert(0, "consolas".into());
    }
    for (name, bytes) in [
        ("manrope", include_bytes!("../../assets/Manrope-Regular.ttf").as_slice()),
        ("manrope_sb", include_bytes!("../../assets/Manrope-SemiBold.ttf").as_slice()),
        ("manrope_heavy", include_bytes!("../../assets/Manrope-ExtraBold.ttf").as_slice()),
    ] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "manrope".into());
    sb.insert(0, "manrope_sb".into());
    fonts.families.insert(FontFamily::Name("signature".into()), vec!["manrope_heavy".into()]);
    sb.extend(fallback_prop);
    fonts.families.insert(semibold(), sb);
    ctx.set_fonts(fonts);

    // A zoom on top of Windows' own display scale, so it follows the window
    // to a monitor with a different scale.
    ctx.set_zoom_factor(scale.clamp(0.75, 2.0));

    ctx.set_theme(egui::ThemePreference::System);
    for dark_mode in [false, true] {
        DARK.store(dark_mode, std::sync::atomic::Ordering::Relaxed);
        ctx.style_mut_of(if dark_mode { egui::Theme::Dark } else { egui::Theme::Light }, |s| {
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (TextStyle::Heading, FontId::new(22.0, semibold())),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size = egui::vec2(36.0, 28.0);
        s.spacing.slider_width = 160.0;
        s.spacing.combo_width = 180.0;
        s.spacing.icon_width = 16.0;
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.interaction.selectable_labels = false;

        let v = &mut s.visuals;
        *v = if dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.override_text_color = Some(text_color());
        v.window_fill = surface_color();
        v.panel_fill = bg_color();
        v.extreme_bg_color = field_color();
        v.faint_bg_color = card_hi_color();
        v.code_bg_color = field_color();
        v.window_stroke = Stroke::new(1.0_f32, border_color());
        v.window_corner_radius = CornerRadius::same(R_CARD);
        v.menu_corner_radius = CornerRadius::same(R_CTRL + 2);
        v.selection.bg_fill = accent_soft_color();
        v.selection.stroke = Stroke::new(1.0_f32, text_color());
        v.hyperlink_color = accent_color();
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        v.window_shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.popup_shadow = v.window_shadow;

        let r = CornerRadius::same(R_CTRL);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = surface_color();
        w.noninteractive.weak_bg_fill = surface_color();
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, border_color());
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, muted_color());
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = card_hi_color();
        w.inactive.weak_bg_fill = card_hi_color();
        w.inactive.bg_stroke = Stroke::new(1.0_f32, border_color());
        w.inactive.fg_stroke = Stroke::new(1.0_f32, text_color());
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = card_hi_color();
        w.hovered.weak_bg_fill = card_hi_color();
        w.hovered.bg_stroke = Stroke::new(1.0_f32, accent_color());
        w.hovered.fg_stroke = Stroke::new(1.5_f32, text_color());
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = accent_soft_color();
        w.active.weak_bg_fill = accent_soft_color();
        w.active.bg_stroke = Stroke::new(1.0_f32, accent_color());
        // egui also uses active foreground for all strong text, including labels.
        w.active.fg_stroke = Stroke::new(1.5_f32, text_color());
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = card_hi_color();
        w.open.weak_bg_fill = card_hi_color();
        w.open.bg_stroke = Stroke::new(1.0_f32, accent_color());
        w.open.corner_radius = r;
    });
    }
    sync(ctx);
}

pub fn sync(ctx: &egui::Context) {
    DARK.store(ctx.style().visuals.dark_mode, std::sync::atomic::Ordering::Relaxed);
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Read the desktop preference once at startup; no polling or background worker.
pub fn system_reduced_motion() -> bool {
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")));
        if let Some(config) = config {
            if std::fs::read_to_string(config.join("kdeglobals")).is_ok_and(|text| kde_reduced_motion(&text)) { return true; }
        }
        if std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase().contains("gnome") {
            return std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "enable-animations"])
                .output().is_ok_and(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "false");
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
        let mut enabled = 1i32;
        let success = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut enabled as *mut i32).cast(), 0) };
        return success != 0 && enabled == 0;
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "linux")]
fn kde_reduced_motion(text: &str) -> bool {
    let mut kde = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') { kde = line == "[KDE]"; }
        if kde {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "AnimationDurationFactor" { return value.trim().parse::<f32>().is_ok_and(|value| value == 0.0); }
            }
        }
    }
    false
}

#[cfg(all(test, target_os = "linux"))]
mod motion_tests {
    #[test]
    fn kde_motion_preference_is_scoped_and_explicit() {
        assert!(super::kde_reduced_motion("[KDE]\nAnimationDurationFactor=0\n"));
        assert!(!super::kde_reduced_motion("[Other]\nAnimationDurationFactor=0\n"));
        assert!(!super::kde_reduced_motion("[KDE]\nAnimationDurationFactor=0.5\n"));
        assert!(!super::kde_reduced_motion("[KDE]\nAnimationDurationFactor=broken\n"));
    }
}
