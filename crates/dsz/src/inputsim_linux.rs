//! Native Linux keyboard and mouse output through uinput.
//!
//! Saved profiles retain Windows virtual-key values. Translate at this one
//! boundary, and reference count the resulting Linux key so aliases (Ctrl
//! and Left Ctrl, for example) cannot release each other prematurely.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use crate::profile::MouseButton;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 11;
const REL_HWHEEL_HI_RES: u16 = 12;
const BTN_LEFT: u16 = 0x110;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
const UI_SET_EVBIT: libc::c_ulong = 0x40045564;
const UI_SET_KEYBIT: libc::c_ulong = 0x40045565;
const UI_SET_RELBIT: libc::c_ulong = 0x40045566;

#[repr(C)]
struct UinputSetup {
    id: libc::input_id,
    name: [libc::c_char; 80],
    ff_effects_max: u32,
}
const UI_DEV_SETUP: libc::c_ulong = 0x40000000 | ((std::mem::size_of::<UinputSetup>() as libc::c_ulong) << 16) | 0x5503;

/// Linux input-event-codes.h values, mapped from the stable Windows profile ABI.
fn linux_key(vk: u16) -> Option<u16> {
    Some(match vk {
        0x08 => 14, 0x09 => 15, 0x0C => 76, 0x0D => 28,
        0x10 | 0xA0 => 42, 0x11 | 0xA2 => 29, 0x12 | 0xA4 => 56,
        0x13 => 119, 0x14 => 58, 0x1B => 1, 0x20 => 57,
        0x21 => 104, 0x22 => 109, 0x23 => 107, 0x24 => 102,
        0x25 => 105, 0x26 => 103, 0x27 => 106, 0x28 => 108,
        0x2C => 99, 0x2D => 110, 0x2E => 111,
        0x30 => 11, 0x31..=0x39 => vk - 0x31 + 2,
        0x41..=0x5A => [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44][(vk - 0x41) as usize],
        0x5B => 125, 0x5C => 126, 0x5D => 127, 0x5F => 142,
        0x60..=0x69 => [82, 79, 80, 81, 75, 76, 77, 71, 72, 73][(vk - 0x60) as usize],
        0x6A => 55, 0x6B => 78, 0x6C => 121, 0x6D => 74, 0x6E => 83, 0x6F => 98,
        0x70..=0x79 => vk - 0x70 + 59,
        0x7A => 87, 0x7B => 88, 0x7C..=0x87 => vk - 0x7C + 183,
        0x90 => 69, 0x91 => 70, 0xA1 => 54, 0xA3 => 97, 0xA5 => 100,
        0xA6 => 158, 0xA7 => 159, 0xA8 => 173, 0xA9 => 128,
        0xAA => 217, 0xAB => 156, 0xAC => 172,
        0xAD => 113, 0xAE => 114, 0xAF => 115,
        0xB0 => 163, 0xB1 => 165, 0xB2 => 166, 0xB3 => 164,
        0xB4 => 155, 0xB5 => 226, 0xB6 => 148, 0xB7 => 149,
        0xBA => 39, 0xBB => 13, 0xBC => 51, 0xBD => 12, 0xBE => 52,
        0xBF => 53, 0xC0 => 41, 0xDB => 26, 0xDC => 43, 0xDD => 27, 0xDE => 40,
        0xE2 => 86,
        _ => return None,
    })
}

fn mouse_code(button: MouseButton) -> u16 {
    BTN_LEFT + match button {
        MouseButton::Left => 0, MouseButton::Right => 1, MouseButton::Middle => 2,
        MouseButton::Back => 3, MouseButton::Forward => 4,
    }
}

#[derive(Default)]
struct HeldInput(HashMap<u16, u32>);
impl HeldInput {
    fn transition(&mut self, code: u16, down: bool) -> bool {
        if down {
            let count = self.0.entry(code).or_insert(0);
            *count = count.saturating_add(1);
            *count == 1
        } else if let Some(count) = self.0.get_mut(&code) {
            *count -= 1;
            if *count == 0 { self.0.remove(&code); true } else { false }
        } else { false }
    }

    fn release_codes(&mut self) -> Vec<u16> {
        self.0.drain().map(|(code, _)| code).collect()
    }
}

struct UinputDevice { file: File }
impl UinputDevice {
    fn create(mouse: bool) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open("/dev/uinput")
            .or_else(|first| if first.kind() == io::ErrorKind::NotFound {
                OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open("/dev/input/uinput")
            } else { Err(first) })?;
        let device = UinputDevice { file };
        device.set_bit(UI_SET_EVBIT, EV_KEY)?;
        if mouse {
            for code in BTN_LEFT..=BTN_LEFT + 4 { device.set_bit(UI_SET_KEYBIT, code)?; }
            device.set_bit(UI_SET_EVBIT, EV_REL)?;
            for code in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES] {
                device.set_bit(UI_SET_RELBIT, code)?;
            }
        } else {
            let mut keys: Vec<_> = (0..=0xFF).filter_map(linux_key).collect();
            keys.sort_unstable();
            keys.dedup();
            for code in keys { device.set_bit(UI_SET_KEYBIT, code)?; }
        }
        let mut setup = UinputSetup {
            id: libc::input_id { bustype: 0x06, vendor: 0, product: 0, version: 1 },
            name: [0; 80], ff_effects_max: 0,
        };
        let name: &[u8] = if mouse { b"DualWave Mouse" } else { b"DualWave Keyboard" };
        for (dest, source) in setup.name.iter_mut().zip(name.iter()) { *dest = *source as libc::c_char; }
        if unsafe { libc::ioctl(device.file.as_raw_fd(), UI_DEV_SETUP, &setup) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::ioctl(device.file.as_raw_fd(), UI_DEV_CREATE) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(device)
    }

    fn set_bit(&self, request: libc::c_ulong, code: u16) -> io::Result<()> {
        if unsafe { libc::ioctl(self.file.as_raw_fd(), request, code as libc::c_int) } < 0 {
            Err(io::Error::last_os_error())
        } else { Ok(()) }
    }

    fn send(&mut self, events: &[(u16, u16, i32)]) -> io::Result<()> {
        for &(kind, code, value) in events.iter().chain(std::iter::once(&(EV_SYN, 0, 0))) {
            let mut event: libc::input_event = unsafe { std::mem::zeroed() };
            event.type_ = kind;
            event.code = code;
            event.value = value;
            let bytes = unsafe { std::slice::from_raw_parts(&event as *const _ as *const u8, std::mem::size_of_val(&event)) };
            self.file.write_all(bytes)?;
        }
        Ok(())
    }
}
impl Drop for UinputDevice {
    fn drop(&mut self) {
        // Removing a device also releases its state in every evdev consumer,
        // including when the process terminates without its shutdown handler.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY); }
    }
}

#[derive(Default)]
struct InputState {
    keyboard: Option<UinputDevice>, mouse: Option<UinputDevice>, held: HeldInput,
    wheel_remainder: [i64; 2], last_error: Option<String>, retry_after: Option<Instant>,
}
impl InputState {
    fn ensure_device(&mut self, mouse: bool) -> io::Result<()> {
        if (if mouse { &self.mouse } else { &self.keyboard }).is_some() { return Ok(()); }
        if self.retry_after.is_some_and(|t| Instant::now() < t) {
            return Err(io::Error::other(self.last_error.clone().unwrap_or_default()));
        }
        match UinputDevice::create(mouse) {
            Ok(device) => {
                if mouse { self.mouse = Some(device); } else { self.keyboard = Some(device); }
                self.last_error = None;
                self.retry_after = None;
                // Let udev/libinput discover the new device before its first
                // event. initialize() allows the application to do this before
                // starting controller readers rather than in a mapping tick.
                std::thread::sleep(Duration::from_millis(250));
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(format!("Keyboard/mouse mappings require access to /dev/uinput: {error}"));
                self.retry_after = Some(Instant::now() + Duration::from_secs(5));
                log::warn!("{}", self.last_error.as_deref().unwrap());
                Err(error)
            }
        }
    }

    fn send(&mut self, mouse: bool, events: &[(u16, u16, i32)]) {
        let device = if mouse { self.mouse.as_mut() } else { self.keyboard.as_mut() };
        if let Some(device) = device {
            if let Err(error) = device.send(events) {
                self.last_error = Some(format!("uinput output failed: {error}"));
                log::warn!("{}", self.last_error.as_deref().unwrap());
                // Destroy both devices to guarantee release even after a
                // partial write. Never keep logical holds on a lost device.
                self.keyboard = None;
                self.mouse = None;
                self.held.release_codes();
                self.wheel_remainder = [0; 2];
                self.retry_after = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    fn button(&mut self, code: u16, down: bool) {
        let mouse = code >= BTN_LEFT;
        if !down && !self.held.0.contains_key(&code) { return; }
        if self.ensure_device(mouse).is_err() { return; }
        if self.held.transition(code, down) { self.send(mouse, &[(EV_KEY, code, i32::from(down))]); }
    }

    fn release(&mut self) {
        for code in self.held.release_codes() { self.send(code >= BTN_LEFT, &[(EV_KEY, code, 0)]); }
        self.wheel_remainder = [0; 2];
    }
}

fn state() -> &'static Mutex<InputState> {
    static STATE: OnceLock<Mutex<InputState>> = OnceLock::new();
    STATE.get_or_init(|| {
        extern "C" fn cleanup() { release_everything(); }
        unsafe { libc::atexit(cleanup); }
        Mutex::new(InputState::default())
    })
}

pub fn initialize() -> Result<(), String> {
    let mut state = state().lock();
    state.ensure_device(false).and_then(|_| state.ensure_device(true)).map_err(|error| {
        state.last_error.clone().unwrap_or_else(|| error.to_string())
    })
}

pub fn error() -> Option<String> { state().lock().last_error.clone() }

pub fn key(vk: u16, down: bool) {
    if let Some(code) = linux_key(vk) { state().lock().button(code, down); }
    else if vk != 0 && down { log::warn!("unsupported saved virtual key 0x{vk:02X}"); }
}

pub fn mouse_button(button: MouseButton, down: bool) { state().lock().button(mouse_code(button), down); }
pub fn release_everything() { state().lock().release(); }

pub fn mouse_move(dx: i32, dy: i32) {
    if dx == 0 && dy == 0 { return; }
    let mut state = state().lock();
    if state.ensure_device(true).is_ok() { state.send(true, &[(EV_REL, REL_X, dx), (EV_REL, REL_Y, dy)]); }
}

fn wheel_events(remainder: &mut i64, delta: i32, horizontal: bool) -> Vec<(u16, u16, i32)> {
    *remainder += delta as i64;
    let notches = (*remainder / 120) as i32;
    *remainder %= 120;
    let mut events = vec![(EV_REL, if horizontal { REL_HWHEEL_HI_RES } else { REL_WHEEL_HI_RES }, delta)];
    if notches != 0 { events.push((EV_REL, if horizontal { REL_HWHEEL } else { REL_WHEEL }, notches)); }
    events
}

/// Wheel delta is in 1/120 notches; positive is up / right. Both the legacy
/// and high-resolution Linux axes are emitted, preserving fractional input.
pub fn wheel(delta: i32, horizontal: bool) {
    if delta == 0 { return; }
    let mut state = state().lock();
    if state.ensure_device(true).is_err() { return; }
    let events = wheel_events(&mut state.wheel_remainder[usize::from(horizontal)], delta, horizontal);
    state.send(true, &events);
}

/// Name for a virtual-key code, for the UI.
pub fn key_name(vk: u16) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x10 => "Shift".into(),
        0x11 => "Ctrl".into(),
        0x12 => "Alt".into(),
        0x13 => "Pause".into(),
        0x14 => "Caps Lock".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2C => "Print Screen".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x5B => "Win".into(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x6A => "Num *".into(),
        0x6B => "Num +".into(),
        0x6D => "Num -".into(),
        0x6E => "Num .".into(),
        0x6F => "Num /".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0xA0 => "Left Shift".into(),
        0xA1 => "Right Shift".into(),
        0xA2 => "Left Ctrl".into(),
        0xA3 => "Right Ctrl".into(),
        0xA4 => "Left Alt".into(),
        0xA5 => "Right Alt".into(),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        _ => format!("Key 0x{vk:02X}"),
    }
}

/// Keys offered in the picker, in a sensible order.
pub fn key_catalog() -> Vec<u16> {
    let mut v: Vec<u16> = Vec::new();
    v.extend(0x41..=0x5A); // A-Z
    v.extend(0x30..=0x39); // 0-9
    v.extend([
        0x20, 0x0D, 0x1B, 0x09, 0x08, 0x2E, 0x2D, 0x24, 0x23, 0x21, 0x22,
    ]);
    v.extend([0x26, 0x28, 0x25, 0x27]);
    v.extend([0xA0, 0xA2, 0xA4, 0xA1, 0xA3, 0xA5, 0x5B, 0x14]);
    v.extend(0x70..=0x7B); // F1-F12
    v.extend(0x60..=0x69);
    v.extend([0x6A, 0x6B, 0x6D, 0x6E, 0x6F]);
    v.extend([
        0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC0, 0xDB, 0xDC, 0xDD, 0xDE, 0x2C, 0x13,
    ]);
    v
}

/// Translate an egui key to a virtual-key code.
pub fn vk_from_egui(k: eframe::egui::Key) -> Option<u16> {
    use eframe::egui::Key as K;
    let name = k.name();
    if name.len() == 1 {
        let c = name.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u16);
        }
    }
    Some(match k {
        K::Space => 0x20,
        K::Enter => 0x0D,
        K::Escape => 0x1B,
        K::Tab => 0x09,
        K::Backspace => 0x08,
        K::Delete => 0x2E,
        K::Insert => 0x2D,
        K::Home => 0x24,
        K::End => 0x23,
        K::PageUp => 0x21,
        K::PageDown => 0x22,
        K::ArrowUp => 0x26,
        K::ArrowDown => 0x28,
        K::ArrowLeft => 0x25,
        K::ArrowRight => 0x27,
        K::F1 => 0x70,
        K::F2 => 0x71,
        K::F3 => 0x72,
        K::F4 => 0x73,
        K::F5 => 0x74,
        K::F6 => 0x75,
        K::F7 => 0x76,
        K::F8 => 0x77,
        K::F9 => 0x78,
        K::F10 => 0x79,
        K::F11 => 0x7A,
        K::F12 => 0x7B,
        K::Minus => 0xBD,
        K::Equals => 0xBB,
        K::Comma => 0xBC,
        K::Period => 0xBE,
        K::Slash => 0xBF,
        K::Semicolon => 0xBA,
        K::OpenBracket => 0xDB,
        K::CloseBracket => 0xDD,
        K::Backslash => 0xDC,
        K::Backtick => 0xC0,
        K::Quote => 0xDE,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_all_picker_and_media_keys_without_changing_profile_values() {
        for vk in key_catalog().into_iter().chain(0xAD..=0xB3) {
            assert!(linux_key(vk).is_some(), "unmapped saved VK {vk:#x}");
        }
        assert_eq!(linux_key(0x41), Some(30));
        assert_eq!(linux_key(0x5A), Some(44));
        assert_eq!(linux_key(0xA3), Some(97));
        assert_eq!(linux_key(0xB3), Some(164));
        assert_eq!(linux_key(0), None);
        assert_eq!(linux_key(0xFFFF), None);
    }

    #[test]
    fn aliases_and_multiple_sources_release_only_after_last_hold() {
        let mut held = HeldInput::default();
        let generic = linux_key(0x11).unwrap();
        let left = linux_key(0xA2).unwrap();
        assert!(held.transition(generic, true));
        assert!(!held.transition(left, true));
        assert!(!held.transition(generic, false));
        assert!(held.transition(left, false));
        assert!(!held.transition(left, false));
        assert!(held.release_codes().is_empty());
    }

    #[test]
    fn emergency_release_drains_each_keyboard_and_mouse_code_once() {
        let mut held = HeldInput::default();
        held.transition(linux_key(0x41).unwrap(), true);
        held.transition(linux_key(0x41).unwrap(), true);
        held.transition(mouse_code(MouseButton::Back), true);
        let mut released = held.release_codes();
        released.sort_unstable();
        assert_eq!(released, vec![30, 0x113]);
        assert!(held.release_codes().is_empty());
        assert!(!held.transition(30, false));
    }

    #[test]
    fn fractional_wheel_preserves_legacy_notches_and_high_resolution() {
        let mut remainder = 0;
        assert_eq!(wheel_events(&mut remainder, 30, false), vec![(EV_REL, REL_WHEEL_HI_RES, 30)]);
        assert_eq!(wheel_events(&mut remainder, 90, false), vec![(EV_REL, REL_WHEEL_HI_RES, 90), (EV_REL, REL_WHEEL, 1)]);
        assert_eq!(remainder, 0);
        assert_eq!(wheel_events(&mut remainder, -240, true), vec![(EV_REL, REL_HWHEEL_HI_RES, -240), (EV_REL, REL_HWHEEL, -2)]);
        assert_eq!(remainder, 0);
    }

    #[test]
    fn shutdown_writes_one_release_for_each_held_input_without_injection() {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!("dualwave-input-release-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let keyboard_path = directory.join("keyboard-events");
        let mouse_path = directory.join("mouse-events");
        // Ordinary files are recording sinks, never uinput device nodes.
        let mut state = InputState {
            keyboard: Some(UinputDevice { file: File::create(&keyboard_path).unwrap() }),
            mouse: Some(UinputDevice { file: File::create(&mouse_path).unwrap() }),
            ..Default::default()
        };
        state.button(30, true);
        state.button(30, true);
        state.button(0x110, true);
        state.release();
        state.release();
        for (path, code) in [(&keyboard_path, 30), (&mouse_path, 0x110)] {
            let bytes = std::fs::read(path).unwrap();
            let events: Vec<_> = bytes.chunks_exact(std::mem::size_of::<libc::input_event>()).map(|chunk| {
                let event = unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const libc::input_event) };
                (event.type_, event.code, event.value)
            }).collect();
            assert_eq!(events, vec![(EV_KEY, code, 1), (EV_SYN, 0, 0), (EV_KEY, code, 0), (EV_SYN, 0, 0)]);
        }
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn kernel_uinput_setup_abi_matches_linux_headers() {
        assert_eq!(std::mem::size_of::<UinputSetup>(), 92);
        assert_eq!(UI_DEV_SETUP, 0x405c5503);
        assert_eq!(mouse_code(MouseButton::Left), 0x110);
        assert_eq!(mouse_code(MouseButton::Forward), 0x114);
    }
}
