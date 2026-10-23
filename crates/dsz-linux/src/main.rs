//! Read-only discovery. Audio transport is not implemented yet.
use std::{env, fs, io, path::Path, process::ExitCode};

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

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.as_slice() == ["--help"] || args.as_slice() == ["-h"] {
        println!("DSZ Linux port (working name)\nUsage: dsz-linux [list]\n\nLists standard DualSense controllers without opening or writing to them.\nBluetooth audio and microphone support are not implemented yet.");
        return ExitCode::SUCCESS;
    }
    if !args.is_empty() && args.as_slice() != ["list"] {
        eprintln!("Usage: dsz-linux [list]");
        return ExitCode::FAILURE;
    }
    if !cfg!(target_os = "linux") {
        eprintln!("This diagnostic requires Linux.");
        return ExitCode::FAILURE;
    }
    match list(Path::new("/sys/class/hidraw")) {
        Ok(0) => {
            println!("No standard DualSense found. Pair and connect it, then retry.");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Could not enumerate HID devices: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
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
