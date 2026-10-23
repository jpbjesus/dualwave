//! Per-controller exclusive input: EVIOCGRAB owns all physical evdev streams,
//! and the opt-in DualWave kernel child redirects validated raw state to DSZ.
//! Device nodes remain enumerated. Linux has no executable allowlist here.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const UNAVAILABLE: &str = "Exclusive physical input requires the DualWave kernel driver and access to this controller's input devices. Device nodes remain visible.";
const CONTROL_ID: u8 = 0xf0;
const CONTROL_VERSION: u8 = 1;
const CONTROL_LEN: usize = 8;
const MEDIA_NAME: &str = "DualWave Microphone Transport";
const EVIOCGRAB: libc::c_ulong = 0x40044590;
const fn feature_ioctl(set: bool) -> libc::c_ulong {
    (3 << 30) | ((CONTROL_LEN as libc::c_ulong) << 16) | (b'H' as libc::c_ulong) << 8 | if set { 6 } else { 7 }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub active: bool,
    pub inverse: bool,
    pub apps: Vec<String>,
    pub devices: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Changes {
    pub devices_added: Vec<String>,
    pub app_added: Option<String>,
    pub apps_added: Vec<String>,
    pub activated: bool,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.devices_added.is_empty() && self.app_added.is_none()
            && self.apps_added.is_empty() && !self.activated
    }
}

fn unsupported(message: &str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, message) }
fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}
fn identity(text: &str) -> Option<(u16, u16, u16)> {
    let mut parts = property(text, "HID_ID")?.split(':');
    let id = (u16::from_str_radix(parts.next()?, 16).ok()?,
        u16::from_str_radix(parts.next()?, 16).ok()?,
        u16::from_str_radix(parts.next()?, 16).ok()?);
    parts.next().is_none().then_some(id)
}
fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

#[derive(Debug)]
struct Nodes {
    child: PathBuf,
    events: Vec<(PathBuf, PathBuf)>,
}

/// Match one exact physical HID, never its USB/Bluetooth ancestor. Fixture
/// roots keep discovery tests independent of the user's devices.
fn discover(physical: &Path, input_class: &Path, dev_root: &Path) -> io::Result<Nodes> {
    let parent_id = identity(&fs::read_to_string(physical.join("uevent"))?);
    if !matches!(parent_id, Some((3 | 5, 0x054c, 0x0ce6 | 0x0df2))) {
        return Err(unsupported("Exclusive input currently requires a physical Sony DualSense and the DualWave kernel driver"));
    }
    let mut child = None;
    for entry in fs::read_dir(physical)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() { continue; }
        let text = match fs::read_to_string(entry.path().join("uevent")) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if identity(&text) != Some((6, 0, 0)) || property(&text, "HID_NAME") != Some(MEDIA_NAME) { continue; }
        for raw in fs::read_dir(entry.path().join("hidraw"))? {
            let raw = raw?;
            if !numbered(&raw.file_name().to_string_lossy(), "hidraw") { continue; }
            if child.replace(raw.path()).is_some() { return Err(invalid("Ambiguous DualWave control transport")); }
        }
    }
    let child = child.ok_or_else(|| unsupported(UNAVAILABLE))?;
    let mut events = Vec::new();
    for entry in fs::read_dir(input_class)? {
        let entry = entry?;
        if !numbered(&entry.file_name().to_string_lossy(), "event") { continue; }
        let device = match fs::canonicalize(entry.path().join("device")) {
            Ok(device) => device,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        // Only the real controller input subtree; never the vendor child or siblings.
        if device.starts_with(physical.join("input")) {
            events.push((dev_root.join("input").join(entry.file_name()), device));
        }
    }
    if events.is_empty() { return Err(invalid("Controller has no physical evdev devices to acquire")); }
    events.sort();
    Ok(Nodes { child, events })
}

fn open_verified(path: &Path, expected_device: &Path, write: bool) -> io::Result<File> {
    let file = OpenOptions::new().read(true).write(write)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_char_device() { return Err(invalid("Input path is not a character device")); }
    let sys = PathBuf::from(format!("/sys/dev/char/{}:{}/device", libc::major(meta.rdev()), libc::minor(meta.rdev())));
    if fs::canonicalize(sys)? != expected_device { return Err(invalid("Controller device changed while opening")); }
    Ok(file)
}

fn validate_status(bytes: &[u8]) -> io::Result<u8> {
    if bytes.len() != CONTROL_LEN || bytes[0] != CONTROL_ID || bytes[1] != CONTROL_VERSION
        || bytes[2] & !3 != 0 || bytes[2] == 2 || bytes[3..].iter().any(|b| *b != 0) {
        return Err(invalid("Unsupported DualWave exclusive-input control response"));
    }
    Ok(bytes[2])
}
fn control(file: &File, command: Option<bool>) -> io::Result<u8> {
    let mut bytes = [CONTROL_ID, CONTROL_VERSION, command.unwrap_or(false) as u8, 0, 0, 0, 0, 0];
    let n = unsafe { libc::ioctl(file.as_raw_fd(), feature_ioctl(command.is_some()), bytes.as_mut_ptr()) };
    if n < 0 { return Err(io::Error::last_os_error()); }
    if n as usize != CONTROL_LEN { return Err(invalid("Short DualWave control response")); }
    if command.is_some() { Ok(0) } else { validate_status(&bytes) }
}

struct Grab {
    control: File,
    _events: Vec<File>, // Closing every event fd releases EVIOCGRAB, including on crash.
    owned: bool,
}
impl Grab {
    fn release(&mut self) -> io::Result<()> {
        if self.owned {
            match control(&self.control, Some(false)) {
                Ok(_) => {},
                Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV | libc::ENXIO | libc::EIO)) => {},
                Err(e) => return Err(e),
            }
            self.owned = false;
        }
        Ok(())
    }
}
impl Drop for Grab {
    fn drop(&mut self) { let _ = self.release(); }
}
fn acquire(physical: &Path) -> io::Result<Grab> {
    let nodes = discover(physical, Path::new("/sys/class/input"), Path::new("/dev"))?;
    let name = nodes.child.file_name().ok_or_else(|| invalid("Missing child hidraw name"))?;
    let child_device = nodes.child.parent().and_then(Path::parent).ok_or_else(|| invalid("Missing child HID"))?;
    let file = open_verified(&Path::new("/dev").join(name), child_device, true)?;
    let status = control(&file, None).map_err(|e| io::Error::new(e.kind(), format!("{UNAVAILABLE} ({e})")))?;
    if status != 0 { return Err(io::Error::new(io::ErrorKind::WouldBlock, "Controller exclusive input is already owned")); }
    let mut grab = Grab { control: file, _events: Vec::new(), owned: false };
    for (path, device) in nodes.events {
        let event = open_verified(&path, &device, false)?;
        if unsafe { libc::ioctl(event.as_raw_fd(), EVIOCGRAB, 1 as libc::c_int) } < 0 {
            return Err(io::Error::last_os_error());
        }
        grab._events.push(event);
    }
    control(&grab.control, Some(true))?;
    grab.owned = true;
    if control(&grab.control, None)? != 3 { return Err(invalid("Kernel did not grant exclusive input ownership")); }
    Ok(grab)
}

static GRABS: OnceLock<Mutex<BTreeMap<String, Grab>>> = OnceLock::new();
fn registry() -> io::Result<std::sync::MutexGuard<'static, BTreeMap<String, Grab>>> {
    GRABS.get_or_init(|| Mutex::new(BTreeMap::new())).lock().map_err(|_| io::Error::other("Exclusive input registry poisoned"))
}

/// Read-only capability detection; never acquire a controller as a probe.
pub fn available() -> bool {
    fs::read_dir("/sys/class/hidraw").is_ok_and(|entries| entries.filter_map(Result::ok).any(|entry| {
        let Ok(device) = fs::canonicalize(entry.path().join("device")) else { return false; };
        let Ok(text) = fs::read_to_string(device.join("uevent")) else { return false; };
        if identity(&text) != Some((6, 0, 0)) || property(&text, "HID_NAME") != Some(MEDIA_NAME) { return false; }
        let Ok(file) = open_verified(&Path::new("/dev").join(entry.file_name()), &device, false) else { return false; };
        control(&file, None).is_ok()
    }))
}

pub fn snapshot() -> io::Result<Snapshot> {
    let devices = blocklist()?;
    Ok(Snapshot { active: !devices.is_empty(), devices, ..Default::default() })
}

pub fn hide(instance_ids: &[String], exe: &Path) -> io::Result<Changes> {
    set_app(exe, true)?;
    let mut owned = registry()?;
    let mut pending = BTreeMap::new();
    for id in instance_ids {
        let physical = fs::canonicalize(id)?;
        // BlueZ UHID controllers legitimately live under /sys/devices/virtual.
        // acquire() validates the exact Sony HID identity and its own vendor child.
        if !physical.starts_with("/sys/devices") {
            return Err(invalid("Expected an exact physical controller HID sysfs identity"));
        }
        let id = physical.to_string_lossy().into_owned();
        if owned.contains_key(&id) || pending.contains_key(&id) { continue; }
        pending.insert(id, acquire(&physical)?);
    }
    let changes = Changes { devices_added: pending.keys().cloned().collect(),
        activated: owned.is_empty() && !pending.is_empty(), ..Default::default() };
    owned.append(&mut pending);
    Ok(changes)
}

/// This process owns redirection. Claiming an executable
/// allowlist for other applications would be inaccurate on Linux.
pub fn set_app(exe: &Path, allow: bool) -> io::Result<Option<String>> {
    if allow && fs::canonicalize(exe)? == fs::canonicalize(std::env::current_exe()?)? { return Ok(None); }
    Err(unsupported("Linux exclusive input has no per-application allowlist; the owning process must read the DualWave vendor transport"))
}

pub fn blocklist() -> io::Result<Vec<String>> { Ok(registry()?.keys().cloned().collect()) }
pub fn restore(changes: &Changes) -> io::Result<()> {
    // Serialized Linux IDs are harmless after a crash: fds were closed and
    // kernel ownership expires with the TGID. Preserve foreign Windows state.
    if changes.app_added.is_some() || !changes.apps_added.is_empty()
        || changes.devices_added.iter().any(|id| !Path::new(id).starts_with("/sys/devices")) {
        return Err(unsupported("Cannot restore a Windows HidHide configuration on Linux"));
    }
    let mut owned = registry()?;
    for id in &changes.devices_added {
        if let Some(grab) = owned.get_mut(id) { grab.release()?; }
        owned.remove(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn feature_abi_and_status_validation() {
        assert_eq!(feature_ioctl(true), 0xc0084806);
        assert_eq!(feature_ioctl(false), 0xc0084807);
        for status in 0..=255u8 {
            let bytes = [CONTROL_ID, CONTROL_VERSION, status, 0, 0, 0, 0, 0];
            assert_eq!(validate_status(&bytes).is_ok(), matches!(status, 0 | 1 | 3));
        }
        assert!(validate_status(&[CONTROL_ID, CONTROL_VERSION, 3]).is_err());
        assert!(validate_status(&[CONTROL_ID, 2, 3, 0, 0, 0, 0, 0]).is_err());
        assert!(validate_status(&[CONTROL_ID, 1, 3, 0, 0, 0, 0, 1]).is_err());
        let file = File::open("/dev/null").unwrap();
        assert_eq!(control(&file, None).unwrap_err().raw_os_error(), Some(libc::ENOTTY));
        assert!(control(&file, Some(true)).is_err());
    }

    #[test]
    fn discovery_scopes_events_and_child_to_exact_controller() {
        let root = std::env::temp_dir().join(format!("dualwave-exclusive-test-{}", std::process::id()));
        let physical = root.join("controller");
        let child = physical.join("0006:0000:0000.0001");
        let class = root.join("class");
        fs::create_dir_all(child.join("hidraw/hidraw42")).unwrap();
        fs::create_dir_all(physical.join("input/input4")).unwrap();
        fs::create_dir_all(root.join("keyboard/input/input5")).unwrap();
        fs::create_dir_all(class.join("event4")).unwrap();
        fs::create_dir_all(class.join("event5")).unwrap();
        symlink(physical.join("input/input4"), class.join("event4/device")).unwrap();
        symlink(root.join("keyboard/input/input5"), class.join("event5/device")).unwrap();
        fs::write(physical.join("uevent"), "HID_ID=0005:0000054C:00000CE6\n").unwrap();
        fs::write(child.join("uevent"), format!("HID_ID=0006:00000000:00000000\nHID_NAME={MEDIA_NAME}\n")).unwrap();
        let nodes = discover(&physical, &class, Path::new("/fixture/dev")).unwrap();
        assert_eq!(nodes.child, child.join("hidraw/hidraw42"));
        assert_eq!(nodes.events, vec![(PathBuf::from("/fixture/dev/input/event4"), physical.join("input/input4"))]);
        fs::write(child.join("uevent"), "HID_ID=0006:00000000:00000000\nHID_NAME=Other\n").unwrap();
        assert!(discover(&physical, &class, Path::new("/fixture/dev")).is_err());
        fs::write(physical.join("uevent"), "HID_ID=0005:0000054C:000009CC\n").unwrap();
        assert_eq!(discover(&physical, &class, Path::new("/fixture/dev")).unwrap_err().kind(), io::ErrorKind::Unsupported);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_is_idempotent_and_never_discards_foreign_config() {
        let stale = Changes { devices_added: vec!["/sys/devices/disconnected-controller".into()], activated: true, ..Default::default() };
        restore(&stale).unwrap();
        restore(&stale).unwrap();
        assert!(restore(&Changes { devices_added: vec!["HID\\physical".into()], ..Default::default() }).is_err());
        assert!(set_app(&std::env::current_exe().unwrap(), true).unwrap().is_none());
        assert_eq!(set_app(Path::new("/bin/true"), true).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(hide(&["/etc".into()], &std::env::current_exe().unwrap()).is_err());
    }
}
