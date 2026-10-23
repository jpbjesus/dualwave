//! Native Linux hidraw access. Enumeration only reads sysfs; opening checks
//! the node identity and descriptor again before permitting controller I/O.
//!
//! hidraw sends the supplied bytes verbatim. In particular, Bluetooth reports
//! must never receive Windows' padding to the largest advertised output report.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const BUS_USB: u32 = 3;
const BUS_BLUETOOTH: u32 = 5;
const BUS_VIRTUAL: u32 = 6;
const MAX_DESCRIPTOR: usize = 4096;
const MAX_REPORT: usize = 8192;
const MEDIA_NAME: &str = "DualWave Microphone Transport";

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub input_len: usize,
    pub output_len: usize,
    pub feature_len: usize,
    pub usage_page: u16,
    pub usage: u16,
    pub serial: String,
    pub product: String,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn hidraw_name(name: &str) -> bool {
    name.strip_prefix("hidraw")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

fn property<'a>(uevent: &'a str, key: &str) -> Option<&'a str> {
    uevent.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

fn identity(uevent: &str) -> Option<(u32, u16, u16)> {
    let mut fields = property(uevent, "HID_ID")?.split(':');
    let bus = u32::from_str_radix(fields.next()?, 16).ok()?;
    let vid = u16::try_from(u32::from_str_radix(fields.next()?, 16).ok()?).ok()?;
    let pid = u16::try_from(u32::from_str_radix(fields.next()?, 16).ok()?).ok()?;
    if fields.next().is_some() { return None; }
    Some((bus, vid, pid))
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    size: usize,
    count: usize,
    id: usize,
}

#[derive(Clone, Debug)]
struct Reports {
    input: [usize; 256],
    output: [usize; 256],
    feature: [usize; 256],
    gamepad: bool,
}

impl Reports {
    fn max_input(&self) -> usize { self.input.iter().copied().max().unwrap_or(0) }
    fn max_output(&self) -> usize { self.output.iter().copied().max().unwrap_or(0) }
    fn max_feature(&self) -> usize { self.feature.iter().copied().max().unwrap_or(0) }
}

/// Read enough of the public HID item format to calculate byte lengths and
/// validate a top-level Generic Desktop/Game Pad application collection.
/// Counts accumulate in bits: separate 4-bit items share the same byte.
fn reports(descriptor: &[u8]) -> io::Result<Reports> {
    if descriptor.is_empty() || descriptor.len() > MAX_DESCRIPTOR {
        return Err(invalid("Invalid HID report descriptor length"));
    }
    let mut result = Reports { input: [0; 256], output: [0; 256], feature: [0; 256], gamepad: false };
    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut usage = None;
    let mut depth = 0usize;
    let mut offset = 0;
    while offset < descriptor.len() {
        let prefix = descriptor[offset];
        offset += 1;
        if prefix == 0xfe {
            let header = descriptor.get(offset..offset + 2).ok_or_else(|| invalid("Truncated HID long item"))?;
            offset += 2 + header[0] as usize;
            if offset > descriptor.len() { return Err(invalid("Truncated HID long item")); }
            continue;
        }
        let size = match prefix & 3 { 3 => 4, n => n as usize };
        let bytes = descriptor.get(offset..offset + size).ok_or_else(|| invalid("Truncated HID item"))?;
        offset += size;
        let value = bytes.iter().enumerate().fold(0u32, |n, (i, b)| n | ((*b as u32) << (i * 8)));
        let tag = prefix >> 4;
        match (prefix >> 2 & 3, tag) {
            (1, 0) => globals.page = value,
            (1, 7) => globals.size = value as usize,
            (1, 8) => {
                if value == 0 || value > 255 { return Err(invalid("Invalid HID report ID")); }
                globals.id = value as usize;
            }
            (1, 9) => globals.count = value as usize,
            (1, 10) => {
                if stack.len() >= 32 { return Err(invalid("HID global stack overflow")); }
                stack.push(globals);
            }
            (1, 11) => globals = stack.pop().ok_or_else(|| invalid("Unbalanced HID global stack"))?,
            (2, 0) => usage = Some(if size == 4 { value } else { (globals.page << 16) | value }),
            (0, 10) => {
                if depth == 0 && value == 1 && usage == Some(0x0001_0005) { result.gamepad = true; }
                depth += 1;
                usage = None;
            }
            (0, 12) => {
                depth = depth.checked_sub(1).ok_or_else(|| invalid("Unbalanced HID collection"))?;
                usage = None;
            }
            (0, 8 | 9 | 11) => {
                let bits = globals.size.checked_mul(globals.count).ok_or_else(|| invalid("HID report size overflow"))?;
                let target = match tag { 8 => &mut result.input, 9 => &mut result.output, _ => &mut result.feature };
                target[globals.id] = target[globals.id].checked_add(bits).filter(|n| *n <= MAX_REPORT * 8)
                    .ok_or_else(|| invalid("HID report is too large"))?;
                usage = None;
            }
            (0, _) => usage = None,
            _ => {}
        }
    }
    if depth != 0 || !stack.is_empty() { return Err(invalid("Unbalanced HID descriptor")); }
    for table in [&mut result.input, &mut result.output, &mut result.feature] {
        for (id, bits) in table.iter_mut().enumerate() {
            if *bits > 0 { *bits = bits.div_ceil(8) + usize::from(id != 0); }
            if *bits > MAX_REPORT { return Err(invalid("HID report is too large")); }
        }
    }
    Ok(result)
}

fn own_serial(serial: &str) -> bool {
    let serial = serial.trim().to_ascii_uppercase();
    serial.strip_prefix("DSZV").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        || (serial.len() == 12 && serial.starts_with("0244535A") && serial.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn usb_parent(physical: &Path) -> Option<&Path> {
    physical.ancestors().find(|p| p.join("idVendor").is_file() && p.join("idProduct").is_file())
}

fn virtual_identity(physical: &Path, serial: &str) -> bool {
    let vhci = physical.components().any(|part| part.as_os_str().to_str().is_some_and(|s| s.starts_with("vhci_hcd.")));
    vhci && own_serial(serial)
}

struct Metadata {
    info: DeviceInfo,
    bus: u32,
    physical: PathBuf,
    reports: Reports,
    descriptor: Vec<u8>,
    virtual_device: bool,
}

fn metadata(class: &Path, devices: &Path, name: &str) -> io::Result<Metadata> {
    if !hidraw_name(name) { return Err(invalid("Invalid hidraw node name")); }
    let physical = fs::canonicalize(class.join(name).join("device"))?;
    let uevent = fs::read_to_string(physical.join("uevent"))?;
    let (bus, vid, pid) = identity(&uevent).ok_or_else(|| invalid("Invalid HID identity"))?;
    let kind = ds_proto::ds4::identify(vid, pid);
    if !matches!(bus, BUS_USB | BUS_BLUETOOTH) || kind.is_none() {
        return Err(invalid("Unsupported controller identity or transport"));
    }
    let usb = usb_parent(&physical);
    // A userspace USB-shaped UHID device is not a physical USB controller.
    if bus == BUS_USB && usb.is_none() { return Err(invalid("USB controller has no USB parent")); }
    let descriptor = fs::read(physical.join("report_descriptor"))?;
    let reports = reports(&descriptor)?;
    let input_len = if bus == BUS_BLUETOOTH { 78 } else { 64 };
    let state_id = if bus == BUS_USB { 1 } else if kind.unwrap().model.is_dualshock4() { 0x11 } else { 0x31 };
    if !reports.gamepad || reports.input[state_id] < input_len {
        return Err(invalid("Controller does not expose a compatible gamepad collection"));
    }
    let serial = usb.and_then(|p| fs::read_to_string(p.join("serial")).ok())
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| property(&uevent, "HID_UNIQ").unwrap_or_default().to_string());
    let virtual_device = virtual_identity(&physical, &serial);
    let info = DeviceInfo {
        path: devices.join(name).to_string_lossy().into_owned(), vid, pid, input_len,
        output_len: reports.max_output(), feature_len: reports.max_feature(),
        usage_page: 1, usage: 5, serial,
        product: property(&uevent, "HID_NAME").unwrap_or_default().to_string(),
    };
    Ok(Metadata { info, bus, physical, reports, descriptor, virtual_device })
}

pub fn enumerate() -> Vec<DeviceInfo> { enumerate_with(false) }

/// Enumerate gamepad collections without opening hidraw or issuing reports.
pub fn enumerate_with(virtual_only: bool) -> Vec<DeviceInfo> {
    enumerate_at(Path::new("/sys/class/hidraw"), Path::new("/dev"), virtual_only)
}

fn enumerate_at(class: &Path, devices: &Path, virtual_only: bool) -> Vec<DeviceInfo> {
    let Ok(entries) = fs::read_dir(class) else { return Vec::new(); };
    let mut found: Vec<_> = entries.flatten().filter_map(|entry| {
        let name = entry.file_name();
        let node = metadata(class, devices, name.to_str()?).ok()?;
        (node.virtual_device == virtual_only).then_some(node.info)
    }).collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    for node in &found {
        log::trace!("HID {} {} {:04x}:{:04x} usage {:x}/{:x}, input {} output {} feature {}",
            node.path, node.product, node.vid, node.pid, node.usage_page, node.usage,
            node.input_len, node.output_len, node.feature_len);
    }
    found
}

// Linux asm-generic ioctl encoding, used by the x86_64/aarch64 app bundles.
fn ioctl_request(direction: u32, number: u32, len: usize) -> libc::c_ulong {
    ((direction << 30) | ((len as u32) << 16) | (u32::from(b'H') << 8) | number) as libc::c_ulong
}

fn raw_identity(file: &File) -> io::Result<(u32, u16, u16)> {
    let mut bytes = [0u8; 8];
    if unsafe { libc::ioctl(file.as_raw_fd(), ioctl_request(2, 3, bytes.len()), bytes.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u32::from_ne_bytes(bytes[..4].try_into().unwrap()),
        u16::from_ne_bytes(bytes[4..6].try_into().unwrap()), u16::from_ne_bytes(bytes[6..8].try_into().unwrap())))
}

#[repr(C)]
struct RawDescriptor { size: u32, value: [u8; MAX_DESCRIPTOR] }

fn raw_descriptor(file: &File) -> io::Result<Vec<u8>> {
    let mut size: libc::c_int = 0;
    if unsafe { libc::ioctl(file.as_raw_fd(), ioctl_request(2, 1, 4), &mut size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if size <= 0 || size as usize > MAX_DESCRIPTOR { return Err(invalid("Invalid hidraw descriptor length")); }
    let mut descriptor = RawDescriptor { size: size as u32, value: [0; MAX_DESCRIPTOR] };
    if unsafe { libc::ioctl(file.as_raw_fd(), ioctl_request(2, 2, std::mem::size_of::<RawDescriptor>()), &mut descriptor) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if descriptor.size as usize != size as usize { return Err(invalid("HID descriptor changed while opening")); }
    Ok(descriptor.value[..size as usize].to_vec())
}

fn open_node(path: &Path, write: bool) -> io::Result<File> {
    OpenOptions::new().read(true).write(write)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW).open(path)
}

fn validate_node(file: &File, class: &Path, name: &str) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_char_device() { return Err(invalid("HID node is not a character device")); }
    let device = fs::read_to_string(class.join(name).join("dev"))?;
    let (major, minor) = device.trim().split_once(':').ok_or_else(|| invalid("Invalid hidraw device number"))?;
    let major = major.parse::<u32>().map_err(|_| invalid("Invalid hidraw major number"))?;
    let minor = minor.parse::<u32>().map_err(|_| invalid("Invalid hidraw minor number"))?;
    if metadata.rdev() != libc::makedev(major, minor) { return Err(invalid("HID node changed while opening")); }
    Ok(())
}

pub struct HidDevice {
    io: File,
    physical: PathBuf,
    bus: u32,
    reports: Reports,
    writer_active: AtomicBool,
    pub info: DeviceInfo,
}

impl HidDevice {
    pub fn open(info: &DeviceInfo) -> io::Result<Self> {
        let path = Path::new(&info.path);
        let name = path.file_name().and_then(|n| n.to_str()).filter(|s| hidraw_name(s))
            .ok_or_else(|| invalid("Select a /dev/hidrawN controller"))?;
        if path.parent() != Some(Path::new("/dev")) { return Err(invalid("Select a /dev/hidrawN controller")); }
        let class = Path::new("/sys/class/hidraw");
        let before = metadata(class, Path::new("/dev"), name)?;
        if before.info.vid != info.vid || before.info.pid != info.pid || before.info.input_len != info.input_len
            || (!info.serial.is_empty() && before.info.serial != info.serial) {
            return Err(invalid("Controller identity changed since enumeration"));
        }
        let io = open_node(path, true)?;
        validate_node(&io, class, name)?;
        if raw_identity(&io)? != (before.bus, info.vid, info.pid) || raw_descriptor(&io)? != before.descriptor {
            return Err(invalid("Opened HID identity or descriptor does not match the controller"));
        }
        let after = metadata(class, Path::new("/dev"), name)?;
        if before.physical != after.physical || before.info.serial != after.info.serial {
            return Err(invalid("Controller changed while opening"));
        }
        Ok(Self { io, physical: before.physical, bus: before.bus, reports: before.reports,
            writer_active: AtomicBool::new(false), info: before.info })
    }

    pub fn reader(&self) -> io::Result<Reader<'_>> { Reader::new(self) }

    /// The driver owns the physical Mute-button latch across userspace restarts.
    pub fn microphone_muted(&self) -> io::Result<Option<bool>> {
        read_kernel_mute(&self.physical.join("dualwave_mic_muted"))
    }

    pub fn writer(&self) -> io::Result<Writer<'_>> {
        if self.writer_active.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "Controller already has an active writer"));
        }
        // Advisory locking coordinates DualWave and the standalone audio CLI.
        // Read-only virtual-controller probes must not take this lock. Other
        // applications must cooperate; hidraw has no exclusive-writer ioctl.
        if unsafe { libc::flock(self.io.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            self.writer_active.store(false, Ordering::Release);
            return Err(io::Error::last_os_error());
        }
        Ok(Writer { dev: self })
    }

    /// Request exactly the caller's wire length, including the report ID.
    pub fn get_feature(&self, id: u8, len: usize) -> io::Result<Vec<u8>> {
        if id == 0 || !(2..=MAX_REPORT).contains(&len) { return Err(invalid("Invalid HID feature report request")); }
        let mut bytes = vec![0; len];
        bytes[0] = id;
        let n = unsafe { libc::ioctl(self.io.as_raw_fd(), ioctl_request(3, 7, len), bytes.as_mut_ptr()) };
        if n < 0 { return Err(io::Error::last_os_error()); }
        if n == 0 || n as usize > len || bytes[0] != id { return Err(invalid("Invalid HID feature report response")); }
        bytes.truncate(n as usize);
        Ok(bytes)
    }
}

/// Errors indicating a detached/revoked device; quiet reads are not fatal.
pub fn is_fatal(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::UnexpectedEof || matches!(error.raw_os_error(),
        Some(libc::ENODEV | libc::ENXIO | libc::ENOENT | libc::EBADF | libc::EIO | libc::EACCES | libc::EPERM))
}

fn media_interface(physical: &Path) -> io::Result<Option<PathBuf>> {
    let mut found = None;
    for child in fs::read_dir(physical)? {
        let child = child?;
        if !child.file_type()?.is_dir() { continue; }
        let text = match fs::read_to_string(child.path().join("uevent")) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if identity(&text) != Some((BUS_VIRTUAL, 0, 0)) || property(&text, "HID_NAME") != Some(MEDIA_NAME) { continue; }
        for raw in fs::read_dir(child.path().join("hidraw"))? {
            let raw = raw?;
            if !hidraw_name(&raw.file_name().to_string_lossy()) { continue; }
            if found.is_some() { return Err(invalid("Multiple microphone transports for one controller")); }
            found = Some(raw.path());
        }
    }
    Ok(found)
}

fn open_media(dev: &HidDevice) -> io::Result<Option<File>> {
    if !matches!(dev.bus, BUS_BLUETOOTH | BUS_USB)
        || !matches!(dev.info.pid, ds_proto::DUALSENSE_PID | ds_proto::DUALSENSE_EDGE_PID)
        || dev.info.vid != ds_proto::SONY_VID {
        return Ok(None);
    }
    let Some(raw) = media_interface(&dev.physical)? else { return Ok(None); };
    let name = raw.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("Invalid microphone hidraw name"))?;
    let file = open_node(&Path::new("/dev").join(name), false)?;
    validate_node(&file, Path::new("/sys/class/hidraw"), name)?;
    if raw_identity(&file)? != (BUS_VIRTUAL, 0, 0) { return Err(invalid("Invalid microphone transport identity")); }
    let mut name_bytes = [0u8; 128];
    let n = unsafe { libc::ioctl(file.as_raw_fd(), ioctl_request(2, 4, name_bytes.len()), name_bytes.as_mut_ptr()) };
    if n < 0 { return Err(io::Error::last_os_error()); }
    let end = name_bytes.iter().position(|b| *b == 0).unwrap_or(name_bytes.len());
    if &name_bytes[..end] != MEDIA_NAME.as_bytes() { return Err(invalid("Invalid microphone transport name")); }
    let actual = fs::canonicalize(Path::new("/sys/class/hidraw").join(name).join("device"))?;
    if actual.parent() != Some(dev.physical.as_path()) { return Err(invalid("Microphone transport changed parent")); }
    Ok(Some(file))
}

fn microphone_packet(report: &[u8]) -> bool {
    report.len() == ds_proto::BT_INPUT_LEN && report[0] == 0x31 && report[1] & 0x0f == 2
        && ds_proto::crc::verify(ds_proto::crc::PREFIX_INPUT, report)
}

// The exclusive-input driver also redirects unchanged state through this child.
// Apply exact framing here; device.rs validates state again before injection.
fn child_packet(dev: &HidDevice, report: &[u8]) -> bool {
    let Some(model) = ds_proto::Model::from_pid(dev.info.pid) else { return false; };
    let conn = if dev.bus == BUS_BLUETOOTH {
        if microphone_packet(report) { return true; }
        if report.len() != 78 || report[0] != 0x31 { return false; }
        ds_proto::Connection::Bluetooth
    } else {
        if report.len() != 64 || report[0] != 1 { return false; }
        ds_proto::Connection::Usb
    };
    ds_proto::input::parse(report, conn, model, true).is_ok()
}

fn poll_wait(fds: &mut [libc::pollfd], deadline: Instant) -> io::Result<bool> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let millis = remaining.as_millis().saturating_add(u128::from(!remaining.is_zero())).min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
        if ready >= 0 { return Ok(ready != 0); }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted { return Err(error); }
        if Instant::now() >= deadline { return Ok(false); }
    }
}

fn pollfd(fd: RawFd, events: i16) -> libc::pollfd { libc::pollfd { fd, events, revents: 0 } }

pub struct Reader<'a> {
    dev: &'a HidDevice,
    media: Option<File>,
    buf: Vec<u8>,
    next: usize,
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
        _ => Err(invalid("Invalid kernel microphone mute state")),
    }
}

impl<'a> Reader<'a> {
    pub fn has_isolated_media(&self) -> bool { self.media.is_some() }

    fn new(dev: &'a HidDevice) -> io::Result<Self> {
        Ok(Self { dev, media: open_media(dev)?, buf: vec![0; dev.reports.max_input().max(78) + 1], next: 0 })
    }

    /// Alternate ready sources so continuous audio cannot starve pad state.
    /// Only validated microphone or exclusive state packets leave the child.
    /// Physical reports retain their original bytes and protocol validation
    /// remains in device.rs before any input injection.
    pub fn read(&mut self, timeout_ms: u32) -> io::Result<Option<&[u8]>> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        loop {
            let mut fds = [pollfd(self.dev.io.as_raw_fd(), libc::POLLIN),
                pollfd(self.media.as_ref().map_or(-1, AsRawFd::as_raw_fd), libc::POLLIN)];
            if !poll_wait(&mut fds, deadline)? { return Ok(None); }
            let first = self.next;
            for i in 0..2 {
                let index = (first + i) % 2;
                let flags = fds[index].revents;
                if flags == 0 { continue; }
                if flags & libc::POLLIN == 0 {
                    if index == 1 { self.media = None; continue; }
                    return Err(io::Error::from_raw_os_error(libc::ENODEV));
                }
                let mut file = if index == 0 { &self.dev.io } else { self.media.as_ref().unwrap() };
                match file.read(&mut self.buf) {
                    Ok(0) if index == 1 => { self.media = None; }
                    Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Controller disconnected")),
                    Ok(n) => {
                        // Include an extra byte in read() to detect overlength
                        // reports instead of interpreting a truncated packet.
                        if n > self.dev.reports.max_input().max(78) { continue; }
                        self.next = 1 - index;
                        if index == 1 && !child_packet(self.dev, &self.buf[..n]) { continue; }
                        return Ok(Some(&self.buf[..n]));
                    }
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                    Err(_) if index == 1 => { self.media = None; }
                    Err(e) => return Err(e),
                }
            }
            if Instant::now() >= deadline { return Ok(None); }
        }
    }
}

pub struct Writer<'a> { dev: &'a HidDevice }

fn output_valid(dev: &HidDevice, report: &[u8]) -> bool {
    let Some(&id) = report.first() else { return false; };
    let Some(kind) = ds_proto::ds4::identify(dev.info.vid, dev.info.pid) else { return false; };
    if !kind.has_output { return false; }
    let expected = if kind.model.is_dualshock4() {
        if dev.bus == BUS_BLUETOOTH { (id == 0x11).then_some(78) } else { (id == 0x05).then_some(32) }
    } else if dev.bus == BUS_BLUETOOTH {
        match id { 0x31 => Some(78), 0x32 => Some(ds_proto::stream::REPORT_LEN),
            0x39 => Some(ds_proto::stream::AUDIO_REPORT_LEN), _ => None }
    } else { (id == 0x02).then_some(ds_proto::USB_OUTPUT_LEN) };
    expected == Some(report.len()) && report.len() <= MAX_REPORT
}

impl Writer<'_> {
    /// Send exactly one protocol report, without padding or partial retries.
    /// The deadline bounds queue/poll retries. Linux drivers can block inside
    /// write() despite O_NONBLOCK; hidraw offers no cancellation for that call.
    /// The application keeps this work on its dedicated controller writer.
    pub fn write(&mut self, report: &[u8], timeout_ms: u32) -> io::Result<()> {
        if !output_valid(self.dev, report) { return Err(invalid("Invalid controller output report ID or length")); }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut file = &self.dev.io;
        loop {
            match file.write(report) {
                Ok(n) if n == report.len() => return Ok(()),
                Ok(_) => return Err(io::Error::new(io::ErrorKind::WriteZero, "Incomplete HID report")),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    if Instant::now() >= deadline { return Err(io::Error::new(io::ErrorKind::TimedOut, "HID write timed out")); }
                    let mut fds = [pollfd(self.dev.io.as_raw_fd(), libc::POLLOUT)];
                    if !poll_wait(&mut fds, deadline)? { return Err(io::Error::new(io::ErrorKind::TimedOut, "HID write timed out")); }
                    if fds[0].revents & libc::POLLOUT == 0 { return Err(io::Error::from_raw_os_error(libc::ENODEV)); }
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Writer<'_> {
    fn drop(&mut self) {
        unsafe { libc::flock(self.dev.io.as_raw_fd(), libc::LOCK_UN); }
        self.dev.writer_active.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicUsize;

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("dualwave-native-hid-{}-{}",
                std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn node(&self, name: &str, physical: &str, bus: u32, serial: &str, descriptor: &[u8]) -> PathBuf {
            let physical = self.0.join(physical);
            fs::create_dir_all(&physical).unwrap();
            fs::write(physical.join("uevent"), format!("HID_ID={bus:04X}:0000054C:00000CE6\nHID_NAME=DualSense Wireless Controller\nHID_UNIQ={serial}\n")).unwrap();
            fs::write(physical.join("report_descriptor"), descriptor).unwrap();
            let class = self.0.join("class").join(name);
            fs::create_dir_all(&class).unwrap();
            symlink(&physical, class.join("device")).unwrap();
            physical
        }
    }

    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    fn descriptor(id: u8, count: u8) -> Vec<u8> {
        vec![0x05, 1, 0x09, 5, 0xa1, 1, 0x85, id, 0x75, 8, 0x95, count,
            0x81, 2, 0x85, 0x31, 0x95, 77, 0x91, 2, 0x85, 5, 0x95, 40, 0xb1, 2, 0xc0]
    }

    fn pipe() -> (File, File) {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    fn fake_device(io: File) -> HidDevice {
        HidDevice { io, physical: PathBuf::new(), bus: BUS_BLUETOOTH,
            reports: reports(&descriptor(0x31, 77)).unwrap(), writer_active: AtomicBool::new(false),
            info: DeviceInfo { path: String::new(), vid: ds_proto::SONY_VID, pid: ds_proto::DUALSENSE_PID,
                input_len: 78, output_len: 547, feature_len: 64, usage_page: 1, usage: 5,
                serial: String::new(), product: String::new() } }
    }

    fn mic_packet() -> [u8; 78] {
        let mut packet = [0; 78];
        packet[0] = 0x31;
        packet[1] = 2;
        ds_proto::crc::seal(ds_proto::crc::PREFIX_INPUT, &mut packet);
        packet
    }

    #[test]
    fn descriptor_preserves_bit_counts_report_ids_and_global_stack() {
        let bytes = [0x05, 1, 0x09, 5, 0xa1, 1, 0x85, 1, 0x75, 4, 0x95, 1,
            0x81, 2, 0x81, 2, 0xa4, 0x85, 2, 0x75, 8, 0x95, 63, 0x81, 2,
            0xb4, 0x81, 2, 0xc0];
        let caps = reports(&bytes).unwrap();
        assert!(caps.gamepad);
        assert_eq!(caps.input[1], 3); // 12 bits + report ID.
        assert_eq!(caps.input[2], 64);
        assert_eq!(reports(&descriptor(0x31, 77)).unwrap().max_input(), 78);
    }

    #[test]
    fn descriptor_rejects_truncation_overflow_and_unbalanced_items() {
        assert!(reports(&[0x76, 1]).is_err());
        assert!(reports(&[0xb4]).is_err());
        assert!(reports(&[0xa1, 1]).is_err());
        assert!(reports(&[0x85, 0]).is_err());
        assert!(reports(&[0x77, 255, 255, 255, 255, 0x97, 255, 255, 255, 255, 0x81, 2]).is_err());
        assert!(reports(&[0xfe, 20, 0]).is_err());
    }

    #[test]
    fn kernel_mute_state_is_optional_but_rejects_malformed_values() {
        let f = Fixture::new();
        let path = f.0.join("dualwave_mic_muted");
        assert_eq!(read_kernel_mute(&path).unwrap(), None);
        fs::write(&path, "0\n").unwrap();
        assert_eq!(read_kernel_mute(&path).unwrap(), Some(false));
        fs::write(&path, "1\n").unwrap();
        assert_eq!(read_kernel_mute(&path).unwrap(), Some(true));
        fs::write(&path, "2\n").unwrap();
        assert!(read_kernel_mute(&path).is_err());
    }

    #[test]
    fn metadata_uses_transport_and_application_usage_and_excludes_media() {
        let fixture = Fixture::new();
        fixture.node("hidraw1", "uhid/0005:054C:0CE6.0001", BUS_BLUETOOTH, "01:02:03:04:05:06", &descriptor(0x31, 77));
        let mut mouse = descriptor(0x31, 77);
        mouse[3] = 2;
        fixture.node("hidraw2", "uhid/mouse", BUS_BLUETOOTH, "", &mouse);
        fixture.node("hidraw3", "uhid/media", BUS_VIRTUAL, "", &descriptor(0x31, 77));
        fixture.node("hidraw4", "uhid/short", BUS_BLUETOOTH, "", &descriptor(1, 9));
        let found = enumerate_at(&fixture.0.join("class"), Path::new("/dev"), false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/dev/hidraw1");
        assert_eq!((found[0].input_len, found[0].output_len, found[0].feature_len), (78, 78, 41));
        assert_eq!((found[0].usage_page, found[0].usage), (1, 5));
        assert_eq!(found[0].serial, "01:02:03:04:05:06");
    }

    #[test]
    fn own_virtual_device_needs_both_vhci_ancestry_and_our_serial() {
        let fixture = Fixture::new();
        let physical = fixture.node("hidraw1", "vhci_hcd.0/usb1/1-1/1-1:1.3/hid", BUS_USB, "", &descriptor(1, 63));
        let usb = physical.parent().unwrap().parent().unwrap();
        fs::write(usb.join("idVendor"), "054c\n").unwrap();
        fs::write(usb.join("idProduct"), "0ce6\n").unwrap();
        fs::write(usb.join("serial"), "DSZV10\n").unwrap();
        let found = enumerate_at(&fixture.0.join("class"), Path::new("/dev"), true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].input_len, 64);
        assert!(enumerate_at(&fixture.0.join("class"), Path::new("/dev"), false).is_empty());
        assert!(!virtual_identity(Path::new("/sys/devices/pci/usb1/1-1"), "DSZV10"));
        assert!(!virtual_identity(Path::new("/sys/devices/platform/vhci_hcd.0/usb1"), "normal"));
        assert!(own_serial("0244535A1234"));
        assert!(!own_serial("DSZVersion"));
    }

    #[test]
    fn media_discovery_requires_named_vendor_child_and_unique_node() {
        let fixture = Fixture::new();
        let physical = fixture.0.join("physical");
        fs::create_dir_all(&physical).unwrap();
        assert!(media_interface(&physical).unwrap().is_none());
        let child = physical.join("0006:0000:0000.0001");
        fs::create_dir_all(child.join("hidraw/hidraw42")).unwrap();
        fs::write(child.join("uevent"), "HID_ID=0006:00000000:00000000\nHID_NAME=Other\n").unwrap();
        assert!(media_interface(&physical).unwrap().is_none());
        fs::write(child.join("uevent"), format!("HID_ID=0006:00000000:00000000\nHID_NAME={MEDIA_NAME}\n")).unwrap();
        assert_eq!(media_interface(&physical).unwrap(), Some(child.join("hidraw/hidraw42")));
        fs::create_dir_all(child.join("hidraw/hidraw43")).unwrap();
        assert!(media_interface(&physical).is_err());
    }

    #[test]
    fn feature_ioctl_matches_linux_uapi_and_uses_requested_wire_length() {
        assert_eq!(ioctl_request(2, 3, 8), 0x8008_4803);
        assert_eq!(ioctl_request(2, 2, 4100), 0x9004_4802);
        assert_eq!(ioctl_request(3, 7, 41), 0xc029_4807);
        assert_eq!(ioctl_request(3, 7, 64), 0xc040_4807);
        let (input, _output) = pipe();
        let dev = fake_device(input);
        assert!(dev.get_feature(0, 41).is_err());
        assert!(dev.get_feature(5, 0).is_err());
        assert!(dev.get_feature(5, MAX_REPORT + 1).is_err());
    }

    #[test]
    fn writer_lock_excludes_other_sessions_but_leaves_readers_unlocked() {
        let fixture = Fixture::new();
        let path = fixture.0.join("lock");
        fs::write(&path, []).unwrap();
        let first = fake_device(OpenOptions::new().read(true).write(true).open(&path).unwrap());
        let mut second = fake_device(OpenOptions::new().read(true).write(true).open(&path).unwrap());
        second.bus = BUS_USB;
        second.physical = fixture.0.clone(); // A valid parent with no optional child.
        let writer = first.writer().unwrap();
        assert!(first.writer().is_err());
        assert!(second.writer().is_err());
        assert!(second.reader().is_ok());
        drop(writer);
        assert!(second.writer().is_ok());
    }

    #[test]
    fn media_packets_require_exact_type_length_crc_and_never_become_state() {
        let mut packet = mic_packet();
        assert!(microphone_packet(&packet));
        assert!(matches!(ds_proto::input::parse(&packet, ds_proto::Connection::Bluetooth,
            ds_proto::Model::DualSense, true), Err(ds_proto::input::ParseError::NotState(2))));
        assert!(!microphone_packet(&packet[..77]));
        packet[5] ^= 1;
        assert!(!microphone_packet(&packet));
        packet[1] = 1;
        ds_proto::crc::seal(ds_proto::crc::PREFIX_INPUT, &mut packet);
        assert!(!microphone_packet(&packet));
    }

    #[test]
    fn reader_alternates_ready_sources_and_preserves_physical_reports() {
        let (physical, mut state_tx) = pipe();
        let (media, mut media_tx) = pipe();
        let dev = fake_device(physical);
        let mut reader = Reader { dev: &dev, media: Some(media), buf: vec![0; 79], next: 0 };
        let physical_packet = [0x31, 1, 99, 3];
        state_tx.write_all(&physical_packet).unwrap();
        media_tx.write_all(&mic_packet()).unwrap();
        assert_eq!(reader.read(0).unwrap().unwrap(), &physical_packet);
        state_tx.write_all(&physical_packet).unwrap();
        assert_eq!(reader.read(0).unwrap().unwrap(), &mic_packet());
        assert_eq!(reader.read(0).unwrap().unwrap(), &physical_packet);
        assert!(reader.read(0).unwrap().is_none());
        // Truncated child state must never reach the state parser.
        media_tx.write_all(&physical_packet).unwrap();
        assert!(reader.read(0).unwrap().is_none());
    }

    #[test]
    fn exclusive_child_accepts_only_exact_validated_state() {
        let (physical, _) = pipe();
        let mut dev = fake_device(physical);
        let mut packet = mic_packet();
        packet[1] = 1;
        ds_proto::crc::seal(ds_proto::crc::PREFIX_INPUT, &mut packet);
        assert!(child_packet(&dev, &packet));
        packet[4] ^= 1;
        assert!(!child_packet(&dev, &packet));
        assert!(!child_packet(&dev, &[1; 64]));
        dev.bus = BUS_USB;
        let mut usb = [0; 64];
        usb[0] = 1;
        assert!(child_packet(&dev, &usb));
        assert!(!child_packet(&dev, &usb[..63]));
        dev.info.pid = ds_proto::DUALSENSE_EDGE_PID;
        assert!(child_packet(&dev, &usb));
    }

    #[test]
    fn invalid_media_does_not_starve_ready_physical_state_and_child_detach_is_optional() {
        let (physical, mut state_tx) = pipe();
        let (media, mut media_tx) = pipe();
        let dev = fake_device(physical);
        let mut reader = Reader { dev: &dev, media: Some(media), buf: vec![0; 79], next: 1 };
        media_tx.write_all(&[0xff; 78]).unwrap();
        state_tx.write_all(&[1, 2, 3]).unwrap();
        assert_eq!(reader.read(10).unwrap().unwrap(), &[1, 2, 3]);
        drop(media_tx);
        assert!(reader.read(0).unwrap().is_none());
        assert!(reader.media.is_none());
        drop(state_tx);
        assert!(is_fatal(&reader.read(0).unwrap_err()));
    }

    #[test]
    fn writer_never_pads_reports_to_the_windows_maximum() {
        let (mut received, output) = pipe();
        let dev = fake_device(output);
        let mut writer = Writer { dev: &dev };
        let report = [0x31; 78];
        writer.write(&report, 0).unwrap();
        let mut buffer = [0; 547];
        assert_eq!(received.read(&mut buffer).unwrap(), 78);
        assert_eq!(&buffer[..78], &report);
        assert!(writer.write(&[0x31; 547], 0).is_err());
        assert!(writer.write(&[], 0).is_err());
        assert!(!output_valid(&dev, &[0x01; 78]));
        assert!(!output_valid(&dev, &[0x31; 77]));
    }

    #[test]
    fn output_lengths_follow_model_and_transport_and_reject_no_output_clones() {
        let (input, _output) = pipe();
        let mut dev = fake_device(input);
        assert!(output_valid(&dev, &[0x32; ds_proto::stream::REPORT_LEN]));
        assert!(output_valid(&dev, &[0x39; ds_proto::stream::AUDIO_REPORT_LEN]));
        dev.info.pid = ds_proto::DUALSENSE_EDGE_PID;
        assert!(output_valid(&dev, &[0x31; 78]));
        dev.bus = BUS_USB;
        assert!(output_valid(&dev, &[0x02; 48]));
        assert!(!output_valid(&dev, &[0x31; 78]));
        dev.info.pid = 0x05c4;
        assert!(output_valid(&dev, &[0x05; 32]));
        dev.bus = BUS_BLUETOOTH;
        assert!(output_valid(&dev, &[0x11; 78]));
        assert!(!output_valid(&dev, &[0x05; 32]));
        dev.info.vid = 0x0f0d;
        dev.info.pid = 0x00ee;
        assert!(!output_valid(&dev, &[0x11; 78]));
    }

    #[test]
    fn writer_timeout_is_bounded_when_nonblocking_queue_is_full() {
        let (_received, mut output) = pipe();
        let buffer = [0; 4096];
        while output.write(&buffer).is_ok() {}
        let dev = fake_device(output);
        let mut writer = Writer { dev: &dev };
        let start = Instant::now();
        assert_eq!(writer.write(&[0x31; 78], 5).unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!is_fatal(&io::Error::new(io::ErrorKind::TimedOut, "quiet")));
        assert!(is_fatal(&io::Error::from_raw_os_error(libc::ENODEV)));
        assert!(!is_fatal(&io::Error::from_raw_os_error(libc::EAGAIN)));
    }
}
