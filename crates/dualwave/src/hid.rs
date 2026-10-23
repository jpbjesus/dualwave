//! Linux hidraw access. Device selection is checked again after opening.
use std::{fs::{self, File, OpenOptions}, io::{self, Read, Write}, os::fd::AsRawFd,
    os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}, time::{Duration, Instant}};
use ds_proto::{crc, Connection, InputState, Model};

pub struct Hid {
    file: File,
    pub path: PathBuf,
}

impl Hid {
    pub fn open(path: &Path, write: bool) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        let valid_name = path.file_name().and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("hidraw"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
        if path.parent() != Some(Path::new("/dev")) || !valid_name {
            return Err(io::Error::other("Select a /dev/hidrawN device"));
        }
        let file = OpenOptions::new().read(true).write(write)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path)?;
        // struct hidraw_devinfo: bus (u32), vendor (s16), product (s16).
        let mut info = [0u8; 8];
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), 0x80084803 as libc::c_ulong, info.as_mut_ptr()) };
        if rc < 0 { return Err(io::Error::last_os_error()); }
        let bus = u32::from_ne_bytes(info[..4].try_into().unwrap());
        let vid = u16::from_ne_bytes(info[4..6].try_into().unwrap());
        let pid = u16::from_ne_bytes(info[6..8].try_into().unwrap());
        if bus != 5 || vid != 0x054c || pid != 0x0ce6 {
            return Err(io::Error::other("Select a standard DualSense connected over Bluetooth"));
        }
        // Prevent two DualWave sessions from sharing a controller.
        if write && unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(io::Error::other("Controller is already in use by another DualWave session"));
        }
        Ok(Self { file, path: path.to_owned() })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.file.read(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Controller disconnected")),
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn write(&mut self, report: &[u8]) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_millis(50);
        loop {
            match self.file.write(report) {
                Ok(n) if n == report.len() => return Ok(()),
                Ok(_) => return Err(io::Error::new(io::ErrorKind::WriteZero, "Incomplete HID report")),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    if Instant::now() >= deadline { return Err(io::Error::new(io::ErrorKind::TimedOut, "HID write timed out")); }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn feature(&self, id: u8, len: usize) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; len];
        bytes[0] = id;
        let request = 0xc0004807u64 | ((len as u64) << 16);
        let n = unsafe { libc::ioctl(self.file.as_raw_fd(), request as libc::c_ulong, bytes.as_mut_ptr()) };
        if n < 0 { return Err(io::Error::last_os_error()); }
        bytes.truncate(n as usize);
        if bytes.len() != len || bytes[0] != id || !crc::verify(0xa3, &bytes) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid feature report or CRC"));
        }
        Ok(bytes)
    }
}

pub fn state(report: &[u8]) -> Option<InputState> {
    // Short Bluetooth packets have no CRC or jack status: do not use them here.
    if report.len() != ds_proto::BT_INPUT_LEN || report[0] != 0x31 { return None; }
    ds_proto::input::parse(report, Connection::Bluetooth, Model::DualSense, true).ok()
}

pub fn microphone(report: &[u8]) -> Option<(u8, &[u8])> {
    if report.len() != ds_proto::BT_INPUT_LEN || report[0] != 0x31
        || report[1] & 0x0f != 2 || !crc::verify(crc::PREFIX_INPUT, report) {
        return None;
    }
    Some((report[2], &report[3..74]))
}

pub fn steam_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else { return true; };
    entries.flatten().any(|entry| {
        fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|name| matches!(name.trim(), "steam" | "steamwebhelper"))
    })
}

fn hidraw_name(name: &str) -> bool {
    name.strip_prefix("hidraw")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

fn find_media_interface(physical: &Path) -> io::Result<Option<PathBuf>> {
    let mut found = None;
    for entry in fs::read_dir(physical)? {
        let child = entry?;
        if !child.file_type()?.is_dir() || !child.file_name().to_string_lossy().starts_with("0006:0000:0000.") {
            continue;
        }
        let uevent = match fs::read_to_string(child.path().join("uevent")) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if !uevent.lines().any(|l| l == "HID_ID=0006:00000000:00000000")
            || !uevent.lines().any(|l| l == "HID_NAME=DualWave Microphone Transport") {
            continue;
        }
        for raw in fs::read_dir(child.path().join("hidraw"))? {
            let raw = raw?;
            if !hidraw_name(&raw.file_name().to_string_lossy()) { continue; }
            if found.is_some() { return Err(io::Error::other("Multiple microphone transports for one controller")); }
            found = Some(Path::new("/dev").join(raw.file_name()));
        }
    }
    Ok(found)
}

/// A receive-only vendor HID child. Microphone reports never reach gamepad readers.
pub struct MediaReader {
    file: File,
    pub path: PathBuf,
    mute_state: PathBuf,
}

fn read_kernel_mute(path: &Path) -> io::Result<Option<bool>> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match value.trim() {
        "0" => Ok(Some(false)),
        "1" => Ok(Some(true)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid kernel microphone mute state")),
    }
}
impl MediaReader {
    pub fn for_controller(hid: &Hid) -> io::Result<Option<Self>> {
        let name = hid.path.file_name().ok_or_else(|| io::Error::other("Invalid HID path"))?;
        let physical = fs::canonicalize(Path::new("/sys/class/hidraw").join(name).join("device"))?;
        let Some(path) = find_media_interface(&physical)? else { return Ok(None); };
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path)
            .map_err(|e| io::Error::new(e.kind(), format!("Cannot open {}: {e}. Install packaging/70-dualwave.rules.", path.display())))?;
        let mut info = [0u8; 8];
        if unsafe { libc::ioctl(file.as_raw_fd(), 0x80084803 as libc::c_ulong, info.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if u32::from_ne_bytes(info[..4].try_into().unwrap()) != 6 || info[4..].iter().any(|b| *b != 0) {
            return Err(io::Error::other("Invalid microphone transport identity"));
        }
        Ok(Some(Self { file, path, mute_state: physical.join("dualwave_mic_muted") }))
    }

    pub fn kernel_muted(&self) -> io::Result<Option<bool>> { read_kernel_mute(&self.mute_state) }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.file.read(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Microphone transport disconnected")),
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Keep malformed microphone-as-input events away from desktop consumers.
/// This experimental bridge deliberately disables physical evdev input while active.
pub struct InputGuard(Vec<File>);

impl InputGuard {
    pub fn acquire(hid: &Hid) -> io::Result<Self> {
        let name = hid.path.file_name().ok_or_else(|| io::Error::other("Invalid hidraw path"))?;
        let root = Path::new("/sys/class/hidraw").join(name).join("device/input");
        let mut guard = Self(Vec::new());
        for input in fs::read_dir(root)? {
            for node in fs::read_dir(input?.path())? {
                let node = node?;
                if !node.file_name().to_string_lossy().starts_with("event") { continue; }
                let path = Path::new("/dev/input").join(node.file_name());
                let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&path)
                    .map_err(|e| io::Error::new(e.kind(), format!("Cannot protect {}: {e}. See README's input permissions.", path.display())))?;
                if unsafe { libc::ioctl(file.as_raw_fd(), 0x40044590 as libc::c_ulong, 1 as libc::c_int) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                guard.0.push(file);
            }
        }
        if guard.0.is_empty() { return Err(io::Error::other("No evdev devices found to protect")); }
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_mute_preserves_hardware_state_and_rejects_invalid_status() {
        let path = std::env::temp_dir().join(format!("dualwave-mute-{}", std::process::id()));
        fs::write(&path, "1\n").unwrap();
        let muted = read_kernel_mute(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(muted, Some(true));
        assert_eq!(read_kernel_mute(&path).unwrap(), None);
        fs::write(&path, "invalid").unwrap();
        let invalid = read_kernel_mute(&path);
        fs::remove_file(&path).unwrap();
        assert!(invalid.is_err());
    }
    #[test]
    fn media_discovery_requires_a_vendor_child_of_the_selected_controller() {
        let root = std::env::temp_dir().join(format!("dualwave-media-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(find_media_interface(&root).unwrap().is_none());
        let child = root.join("0006:0000:0000.0001");
        fs::create_dir_all(child.join("hidraw/hidraw42")).unwrap();
        fs::write(child.join("uevent"), "HID_ID=0006:00000000:00000000\nHID_NAME=Other device\n").unwrap();
        assert!(find_media_interface(&root).unwrap().is_none());
        fs::write(child.join("uevent"), "HID_ID=0006:00000000:00000000\nHID_NAME=DualWave Microphone Transport\n").unwrap();
        let result = find_media_interface(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(result, Some(PathBuf::from("/dev/hidraw42")));
    }
    #[test]
    fn microphone_requires_exact_length_type_and_crc_and_never_becomes_input() {
        let mut report = [0u8; 78];
        report[0] = 0x31;
        report[1] = 0x12;
        report[2] = 123;
        crc::seal(crc::PREFIX_INPUT, &mut report);
        assert_eq!(microphone(&report).map(|(n, p)| (n, p.len())), Some((123, 71)));
        assert!(state(&report).is_none());
        assert!(microphone(&report[..77]).is_none());
        let mut long = report.to_vec();
        long.push(0);
        assert!(microphone(&long).is_none());
        report[3] ^= 1;
        assert!(microphone(&report).is_none());
        report[1] = 0x11;
        crc::seal(crc::PREFIX_INPUT, &mut report);
        assert!(microphone(&report).is_none());
    }
}
