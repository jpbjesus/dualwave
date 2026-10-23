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
