//! Tray presentation only: reads the engine's validated live input.
use ds_proto::input::BatteryStatus;
use crate::engine::Engine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub status: BatteryStatus,
}

fn battery(percent: u8, status: BatteryStatus, full_report: bool) -> Option<Battery> {
    (full_report && percent <= 100 && status != BatteryStatus::Error)
        .then_some(Battery { percent, status })
}

pub fn snapshot(engine: &Engine) -> (Option<Battery>, String) {
    let mut devices = engine.device_list();
    devices.sort_by_key(|device| device.id);
    let mut selected: Option<Battery> = None;
    let descriptions = devices.iter().map(|device| {
        let input = device.live.lock().input;
        let value = battery(input.battery_percent, input.battery_status, input.full);
        // Show the lowest known charge; equal charges keep connection order.
        if let Some(value) = value {
            if selected.is_none_or(|current| value.percent < current.percent) { selected = Some(value); }
        }
        let charge = match value {
            None => crate::i18n::tr("Battery unavailable").to_owned(),
            Some(value) => {
                let status = match value.status {
                    BatteryStatus::Charging => crate::i18n::tr("Charging"),
                    BatteryStatus::Full => crate::i18n::tr("Full"),
                    _ => "",
                };
                if status.is_empty() { format!("{}%", value.percent) }
                else { format!("{}% ({status})", value.percent) }
            }
        };
        format!("{} {charge}", device.display_name(engine))
    }).collect::<Vec<_>>();
    let description = if descriptions.is_empty() { crate::i18n::tr("No controller").to_owned() }
        else { descriptions.join("\n") };
    (selected, description)
}

/// A high-contrast vertical battery overlapping the right grip. The fill itself
/// conveys charge without depending on color; the bolt/check conveys power.
pub fn icon_rgba(size: usize, battery: Option<Battery>) -> Vec<u8> {
    let mut rgba = crate::icon_art::icon_rgba(size);
    let Some(battery) = battery else { return rgba; };
    if size < 12 { return rgba; }
    let scale = size as f32 / 32.0;
    let x0 = (22.0 * scale).round() as usize;
    let x1 = (30.0 * scale).round() as usize;
    let y0 = (11.0 * scale).round() as usize;
    let y1 = (30.0 * scale).round() as usize;
    let border = scale.round().max(1.0) as usize;
    let fill_height = ((y1 - y0 - 2 * border) * battery.percent as usize + 50) / 100;
    let fill = if battery.percent <= 20 { [244, 100, 85, 255] } else { [68, 206, 128, 255] };
    let terminal_inset = ((x1 - x0) / 4).max(1);
    for y in y0 - border..y1 {
        for x in x0..x1 {
            let terminal = y < y0;
            if terminal && (x < x0 + terminal_inset || x >= x1 - terminal_inset) { continue; }
            let color = if terminal || x < x0 + border || x >= x1 - border || y < y0 + border || y >= y1 - border {
                [240, 244, 250, 255]
            } else if y >= y1 - border - fill_height { fill } else { [24, 29, 40, 255] };
            rgba[(y * size + x) * 4..(y * size + x) * 4 + 4].copy_from_slice(&color);
        }
    }
    // Five-pixel glyphs scale to native tray sizes, including 16 px.
    let glyph = match battery.status {
        BatteryStatus::Charging => [0b00110, 0b01100, 0b11111, 0b00110, 0b01100],
        BatteryStatus::Full => [0b00001, 0b00011, 0b10110, 0b11100, 0b01000],
        _ => [0; 5],
    };
    let glyph_width = (5.0 * scale).round().max(2.0) as usize;
    let glyph_width = glyph_width.min(x1 - x0 - 2 * border);
    let glyph_height = (5.0 * scale).round().max(3.0) as usize;
    let gx0 = x0 + (x1 - x0 - glyph_width) / 2;
    let gy0 = y0 + (y1 - y0 - glyph_height) / 2;
    let mut marks = Vec::new();
    for y in gy0..gy0 + glyph_height {
        for x in gx0..gx0 + glyph_width {
            let gx = (x - gx0) * 5 / glyph_width;
            let gy = (y - gy0) * 5 / glyph_height;
            if glyph[gy] & (1 << (4 - gx)) != 0 {
                marks.push((x, y));
            }
        }
    }
    for &(x, y) in &marks {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (px, py) = ((x as isize + dx) as usize, (y as isize + dy) as usize);
            if px > x0 && px < x1 - border && py > y0 && py < y1 - border {
                rgba[(py * size + px) * 4..(py * size + px) * 4 + 4].copy_from_slice(&[10, 16, 24, 255]);
            }
        }
    }
    for (x, y) in marks {
        rgba[(y * size + x) * 4..(y * size + x) * 4 + 4].copy_from_slice(&[250, 252, 255, 255]);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_charge_is_not_a_percentage() {
        assert_eq!(battery(0, BatteryStatus::Discharging, false), None);
        assert_eq!(battery(101, BatteryStatus::Discharging, true), None);
        assert_eq!(battery(50, BatteryStatus::Error, true), None);
        assert_eq!(battery(0, BatteryStatus::Discharging, true).unwrap().percent, 0);
    }

    #[test]
    fn battery_is_vertical_on_right_and_fills_from_bottom() {
        let rgba = icon_rgba(32, battery(50, BatteryStatus::Discharging, true));
        let pixel = |x: usize, y: usize| &rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(pixel(25, 10), [240, 244, 250, 255]);
        assert_eq!(pixel(25, 13), [24, 29, 40, 255]);
        assert_eq!(pixel(25, 27), [68, 206, 128, 255]);
        let base = crate::icon_art::icon_rgba(32);
        for y in 0..32 {
            for x in 0..22 {
                let offset = (y * 32 + x) * 4;
                assert_eq!(&rgba[offset..offset + 4], &base[offset..offset + 4]);
            }
        }
    }

    #[test]
    fn disconnected_icon_is_unchanged_and_fill_is_proportional() {
        assert_eq!(icon_rgba(32, None), crate::icon_art::icon_rgba(32));
        let filled = |percent| icon_rgba(32, battery(percent, BatteryStatus::Discharging, true))
            .chunks_exact(4).filter(|p| p[..3] == [68, 206, 128]).count();
        assert!(filled(25) < filled(50));
        assert!(filled(50) < filled(100));
        for size in [16, 20, 24, 32, 48] {
            let ordinary = icon_rgba(size, battery(100, BatteryStatus::Discharging, true));
            assert_ne!(ordinary, icon_rgba(size, battery(100, BatteryStatus::Charging, true)));
            assert_ne!(ordinary, icon_rgba(size, battery(100, BatteryStatus::Full, true)));
            assert_eq!(ordinary.len(), size * size * 4);
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::device::Device;
    use ds_proto::Connection;

    #[test]
    fn multiple_controllers_use_lowest_known_charge_and_keep_names_in_tooltip() {
        let dir = std::env::temp_dir().join(format!("dualwave-tray-snapshot-{}",std::process::id()));
        let engine = Engine::new(dir.clone());
        let mut high = Device::for_test(Connection::Bluetooth,"A0-00-00-00-00-81");
        let mut low = Device::for_test(Connection::Bluetooth,"A0-00-00-00-00-82");
        let unknown = Device::for_test(Connection::Bluetooth,"A0-00-00-00-00-83");
        std::sync::Arc::get_mut(&mut high).unwrap().id = 3;
        std::sync::Arc::get_mut(&mut low).unwrap().id = 2;
        for (device,percent,status) in [(&high,90,BatteryStatus::Discharging),(&low,20,BatteryStatus::Charging)] {
            let mut live = device.live.lock();
            live.input.full = true;
            live.input.battery_percent = percent;
            live.input.battery_status = status;
        }
        engine.settings.write().device_names.insert(low.mac.lock().clone(),"PC".into());
        engine.settings.write().device_names.insert(high.mac.lock().clone(),"TV".into());
        engine.devices.write().extend([high.clone(),unknown.clone(),low.clone()]);
        let (selected,description) = snapshot(&engine);
        assert_eq!(selected,Some(Battery {percent:20,status:BatteryStatus::Charging}));
        assert!(description.contains("PC 20%") && description.contains("TV 90%"));
        assert!(!description.contains("DualSense 0%"),"a short report must not invent an empty battery");
        high.live.lock().input.battery_percent = 20;
        assert_eq!(snapshot(&engine).0.unwrap().status,BatteryStatus::Charging,"connection id breaks ties");
        engine.devices.write().clear();
        assert_eq!(snapshot(&engine).0,None);
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }
}
