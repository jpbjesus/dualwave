//! DualWave: native Linux Bluetooth audio for the standard DualSense.
#[cfg(target_os = "linux")]
mod hid;
#[cfg(target_os = "linux")]
mod pipewire;
#[cfg(target_os = "linux")]
mod audio;
#[cfg(target_os = "linux")]
use std::{env, fs, io, path::Path, process::ExitCode};

#[cfg(target_os = "linux")]
fn dualsense_id(uevent: &str) -> Option<&str> {
    let id = uevent.lines().find_map(|line| line.strip_prefix("HID_ID="))?;
    let mut fields = id.split(':');
    let bus = u16::from_str_radix(fields.next()?, 16).ok()?;
    let vendor = u32::from_str_radix(fields.next()?, 16).ok()?;
    let product = u32::from_str_radix(fields.next()?, 16).ok()?;
    if fields.next().is_some() || vendor != 0x054c || product != 0x0ce6 {
        return None;
    }
    match bus {
        0x05 => Some("Bluetooth"),
        0x03 => Some("USB"),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn list(root: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut devices = entries.collect::<io::Result<Vec<_>>>()?;
    devices.sort_by_key(|entry| entry.file_name());
    let mut found = 0;
    for entry in devices {
        let uevent = match fs::read_to_string(entry.path().join("device/uevent")) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if let Some(transport) = dualsense_id(&uevent) {
            println!("/dev/{}  DualSense  {transport}", entry.file_name().to_string_lossy());
            found += 1;
        }
    }
    Ok(found)
}

#[cfg(target_os = "linux")]
fn execute() -> io::Result<()> {
    let mut args: Vec<_> = env::args().skip(1).collect();
    let required = args.iter().any(|arg| arg == "--require-isolated");
    args.retain(|arg| arg != "--require-isolated");
    let command = args.first().map(String::as_str).unwrap_or("list");
    if matches!(command, "--help" | "-h") {
        println!("DualWave — native Linux DualSense Bluetooth audio\n\nUsage: dualwave [list|probe|tone|mic-probe|run|pipewire-probe] [options]\n\n  list             List controllers (default; no device writes)\n  probe            Read CRC-checked jack, battery and firmware status\n  tone             Quiet stereo headphone test (3 seconds by default)\n  mic-probe        Decode microphone and print levels (5 seconds by default)\n  run              Expose DualWave Headphones and Microphone in PipeWire\n  pipewire-probe   Expose endpoints without opening a controller (5 seconds)\n\nOptions: --device /dev/hidrawN  --seconds N  --volume 0..100  --source auto|builtin|headset\n         --require-isolated (refuse legacy input grabbing)\n\nWith the dedicated kernel microphone transport, physical gamepad input stays\navailable. Legacy mode requires Steam closed and temporarily isolates input.\nSee README for the driver package and permissions.\nCtrl+C stops audio and disables the microphone stream.");
        return Ok(());
    }
    if !matches!(command, "list" | "probe" | "tone" | "mic-probe" | "run" | "pipewire-probe") {
        return Err(io::Error::other("Unknown command; use --help"));
    }
    let mut device = None;
    let mut seconds = None;
    let mut volume = 60u8;
    let mut source = ds_proto::output::MicrophoneSource::Auto;
    let mut flags = args.iter().skip(1);
    while let Some(flag) = flags.next() {
        let value = flags.next().ok_or_else(|| io::Error::other(format!("Missing value for {flag}")))?;
        match flag.as_str() {
            "--device" => device = Some(std::path::PathBuf::from(value)),
            "--seconds" => {
                let n = value.parse::<u64>().map_err(|_| io::Error::other("--seconds requires an integer"))?;
                if !(1..=3600).contains(&n) { return Err(io::Error::other("--seconds must be between 1 and 3600")); }
                seconds = Some(n);
            }
            "--volume" => {
                volume = value.parse::<u8>().ok().filter(|v| *v <= 100)
                    .ok_or_else(|| io::Error::other("--volume must be between 0 and 100"))?;
            }
            "--source" => source = match value.as_str() {
                "auto" => ds_proto::output::MicrophoneSource::Auto,
                "builtin" => ds_proto::output::MicrophoneSource::BuiltIn,
                "headset" => ds_proto::output::MicrophoneSource::Headset,
                _ => return Err(io::Error::other("--source must be auto, builtin or headset")),
            },
            _ => return Err(io::Error::other(format!("Unknown option {flag}"))),
        }
    }
    if command == "list" {
        if list(Path::new("/sys/class/hidraw"))? == 0 {
            println!("No standard DualSense found. Pair and connect it, then retry.");
        }
        return Ok(());
    }
    if command == "pipewire-probe" {
        audio::install_signals();
        let bridge = pipewire::Bridge::new()?;
        println!("DualWave PipeWire endpoints registered; no controller opened.");
        std::thread::sleep(std::time::Duration::from_millis(300));
        bridge.active(false)?;
        bridge.active(true)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds.unwrap_or(5));
        while !audio::stopped() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        return Ok(());
    }
    let path = match device {
        Some(path) => path,
        None => {
            let mut found = Vec::new();
            for entry in fs::read_dir("/sys/class/hidraw")? {
                let entry = entry?;
                if fs::read_to_string(entry.path().join("device/uevent"))
                    .is_ok_and(|text| dualsense_id(&text) == Some("Bluetooth")) {
                    found.push(Path::new("/dev").join(entry.file_name()));
                }
            }
            if found.len() != 1 { return Err(io::Error::other("Connect exactly one Bluetooth DualSense, or select --device /dev/hidrawN")); }
            found.remove(0)
        }
    };
    match command {
        "probe" => audio::probe(&path),
        "tone" => audio::run(&path, audio::Mode::Tone, Some(seconds.unwrap_or(3)), source, (volume as u32 * 127 / 100) as u8, required),
        "mic-probe" => audio::run(&path, audio::Mode::Microphone, Some(seconds.unwrap_or(5)), source, (volume as u32 * 127 / 100) as u8, required),
        "run" => audio::run(&path, audio::Mode::Run, seconds, source, (volume as u32 * 127 / 100) as u8, required),
        _ => unreachable!(),
    }
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("DualWave: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("DualWave requires Linux. Use the upstream dsz application on Windows.");
    std::process::ExitCode::FAILURE
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn discovery_requires_sony_standard_dualsense_and_known_transport() {
        assert_eq!(dualsense_id("HID_NAME=Wireless Controller\nHID_ID=0005:0000054C:00000CE6\n"), Some("Bluetooth"));
        assert_eq!(dualsense_id("HID_ID=0003:0000054C:00000CE6"), Some("USB"));
        for id in ["0005:0000054C:00000DF2", "0005:00000001:00000CE6", "0000:0000054C:00000CE6", "0005:0000054C", "0005:0000054C:00000CE6:0000", "invalid"] {
            assert_eq!(dualsense_id(&format!("HID_ID={id}")), None);
        }
    }
}
