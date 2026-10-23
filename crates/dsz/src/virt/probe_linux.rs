//! Native virtual-controller diagnostics. No physical controller is opened,
//! no keyboard/mouse injection, output reports, speaker or microphone tests.

use std::time::{Duration, Instant};
use super::{hidhide, usbip};

pub fn probe(what: &str) {
    println!("Linux USB/IP vhci_hcd available: {}", usbip::available());
    if let Some(why) = usbip::blocked() { println!("Host requirement: {why}"); }
    println!("Exclusive input: {}", hidhide::UNAVAILABLE);
    match hidhide::snapshot() {
        Ok(state) => println!("Exclusive active: {}, owned devices: {}, inverse: {}, allowlist: {}", state.active, state.devices.len(), state.inverse, state.apps.len()),
        Err(error) => println!("Exclusive status unavailable: {error}"),
    }
    let model = match what {
        "usbip-ds" => usbip::Model::DualSense,
        "usbip-ds4" => usbip::Model::DualShock4,
        "usbip-x360" | "hold-x360" => usbip::Model::Xbox360,
        "default-audio" | "default-watch" | "default-guard" => { super::audio_default::describe(); return; }
        "hidhide" => { println!("{}", hidhide::UNAVAILABLE); return; }
        "status" | "" => return,
        _ => { eprintln!("Probe {what:?} is unavailable on Linux. Supported: status, usbip-ds, usbip-ds4, usbip-x360, default-audio, hidhide."); return; }
    };
    let sink = usbip::DsSink {
        on_output: Box::new(|bytes| log::debug!("virtual controller host output: {} bytes", bytes.len())),
        mirror: Default::default(),
    };
    let started = Instant::now();
    match usbip::UsbPad::create(model, sink) {
        Ok(pad) => {
            println!("{} attached in {} ms, serial {}", model.name(), started.elapsed().as_millis(), pad.serial());
            // The shared device starts neutral and only answers host requests.
            std::thread::sleep(Duration::from_secs(3));
            println!("URBs {}, alive {}, HID {:?}, render {}, capture {}", pad.urbs(), pad.alive(), pad.hid_instance(), pad.streaming(), pad.recording());
            drop(pad);
            println!("Detached");
        }
        Err(error) => eprintln!("Virtual controller attachment failed: {error}"),
    }
}
