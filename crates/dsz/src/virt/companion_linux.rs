//! The Explorer/Game Bar helper has no Linux desktop contract. Virtual
//! controllers still carry Guide through their normal USB reports.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ds_proto::{Buttons, virtual_pad as vp};
use crate::engine::Engine;

#[derive(Default)]
pub struct Companion { warned: AtomicBool }

impl Companion {
    pub fn can_hide(&self) -> bool { false }
    pub fn set(&self, _device: u64, buttons: u32) {
        if buttons != 0 && !self.warned.swap(true, Ordering::Relaxed) {
            log::warn!("Xbox desktop helper mappings are unavailable on Linux; use an active virtual Xbox controller for game Guide input");
        }
    }
    pub fn stop(&self) {}
    pub fn make_way(&self) {}
}

pub(crate) fn pad(buttons: u32) -> vp::PadState {
    let b = Buttons(buttons);
    vp::PadState {
        buttons: b,
        l2: if b.has(ds_proto::input::Button::L2) { 255 } else { 0 },
        r2: if b.has(ds_proto::input::Button::R2) { 255 } else { 0 },
        ..Default::default()
    }
}

pub(crate) fn usb_id(serial: &str) -> String {
    std::fs::read_dir("/sys/bus/usb/devices").ok().and_then(|entries| {
        entries.filter_map(Result::ok).find_map(|entry| {
            let path = entry.path();
            (std::fs::read_to_string(path.join("serial")).is_ok_and(|s| s.trim() == serial))
                .then(|| std::fs::canonicalize(path).ok()).flatten()
        })
    }).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

// Only the Windows hidden helper uses an Explorer/Game Bar allow list.
pub(crate) fn readers() -> Vec<PathBuf> { Vec::new() }

pub fn run(_engine: Arc<Engine>) {
    log::debug!("Explorer/Game Bar Guide-button helper is not applicable on Linux");
}
