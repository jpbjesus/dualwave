//! Exact physical USB playback selection; never use a desktop default or a name match.
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use alsa::{pcm::{Access, Format, HwParams}, Direction, PCM, ValueOr};
use super::{Device, OpenError, Render, Shared};

#[derive(Debug, PartialEq, Eq)]
struct PhysicalPcm { name: String, usb: PathBuf, hid: PathBuf, playback: PathBuf }

fn pcm_numbers(name: &str) -> Option<(u32, u32)> {
    let (card, device) = name.strip_prefix("pcmC")?.strip_suffix('p')?.split_once('D')?;
    if card.is_empty() || device.is_empty() || !card.bytes().chain(device.bytes()).all(|b| b.is_ascii_digit()) { return None; }
    Some((card.parse().ok()?, device.parse().ok()?))
}

fn discover(hid: &Path, sound: &Path) -> Result<PhysicalPcm, String> {
    let hid = std::fs::canonicalize(hid).map_err(|e| e.to_string())?;
    let usb = hid.ancestors().find(|p| p.join("idVendor").is_file() && p.join("idProduct").is_file())
        .ok_or("Controller has no physical USB device")?;
    if usb.ancestors().any(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("vhci_hcd"))) {
        return Err("USB/IP audio is not a physical controller playback device".into());
    }
    let vendor = std::fs::read_to_string(usb.join("idVendor")).map_err(|e| e.to_string())?;
    let product = std::fs::read_to_string(usb.join("idProduct")).map_err(|e| e.to_string())?;
    if vendor.trim() != "054c" || !matches!(product.trim(), "0ce6" | "0df2") {
        return Err("USB game haptics require a physical Sony DualSense or Edge".into());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(sound).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some((card, device)) = pcm_numbers(&entry.file_name().to_string_lossy()) else { continue; };
        let playback = match std::fs::canonicalize(entry.path().join("device")) {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if playback.starts_with(usb) {
            found.push(PhysicalPcm { name: format!("hw:{card},{device}"), usb: usb.into(), hid: hid.clone(), playback });
        }
    }
    if found.len() != 1 { return Err(format!("Expected one physical controller playback device, found {}", found.len())); }
    Ok(found.remove(0))
}

fn target(device: &Device) -> Result<PhysicalPcm, String> {
    if device.is_bluetooth() || device.model.is_dualshock4()
        || device.info.serial.starts_with(crate::virt::usbip::SERIAL_PREFIX) {
        return Err("USB game haptics require a physical DualSense".into());
    }
    let name = Path::new(&device.info.path).file_name().ok_or("Missing controller HID node")?;
    let name_text = name.to_string_lossy();
    let suffix = name_text.strip_prefix("hidraw").ok_or("Invalid controller HID node")?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) { return Err("Invalid controller HID node".into()); }
    discover(&Path::new("/sys/class/hidraw").join(name).join("device"), Path::new("/sys/class/sound"))
}

pub(super) struct Output {
    pub(super) name: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Output {
    pub(super) fn open(device: &Device, shared: Arc<Shared>) -> Result<Self, OpenError> {
        let endpoint = target(device)?;
        let pcm = PCM::new(&endpoint.name, Direction::Playback, true).map_err(|e| e.to_string())?;
        {
            let params = HwParams::any(&pcm).map_err(|e| e.to_string())?;
            params.set_access(Access::RWInterleaved).map_err(|e| e.to_string())?;
            params.set_format(Format::s16()).map_err(|e| e.to_string())?;
            params.set_rate_resample(false).map_err(|e| e.to_string())?;
            params.set_channels(4).map_err(|e| OpenError::Channels(e.to_string()))?;
            params.set_rate(48_000, ValueOr::Nearest).map_err(|e| OpenError::Channels(e.to_string()))?;
            params.set_period_size_near(480, ValueOr::Nearest).map_err(|e| e.to_string())?;
            params.set_buffer_size_near(960).map_err(|e| e.to_string())?;
            if params.get_rate().map_err(|e| e.to_string())? != 48_000
                || params.get_buffer_size().map_err(|e| e.to_string())? > 4800 {
                return Err("Unsupported USB playback rate or buffering".to_string().into());
            }
            pcm.hw_params(&params).map_err(|e| e.to_string())?;
        }
        // Recheck after opening: a hotplug must not redirect a recycled card number.
        if target(device)? != endpoint { return Err("Controller audio device changed while opening".to_string().into()); }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new().name(format!("ds-usb-playback-{}", device.id))
            .spawn(move || {
                if let Err(error) = playback(pcm, shared.clone(), &stopped) {
                    log::warn!("game haptics over USB: playback error: {error}");
                    shared.failed.store(true, Ordering::Release);
                }
            }).map_err(|e| e.to_string())?;
        log::info!("game haptics over USB: playing actuator channels on {} (4 ch, 48000 Hz)", endpoint.name);
        Ok(Self { name: endpoint.name, stop, thread: Some(thread) })
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() { thread.thread().unpark(); let _ = thread.join(); }
    }
}

fn playback(pcm: PCM, shared: Arc<Shared>, stop: &AtomicBool) -> Result<(), String> {
    let mut render = Render::new(shared, 48_000);
    let io = pcm.io_i16().map_err(|e| e.to_string())?;
    let mut floats = [0.0; 480 * 4];
    let mut samples = [0i16; 480 * 4];
    let mut offset = samples.len();
    while !stop.load(Ordering::Acquire) {
        if offset == samples.len() {
            render.fill(&mut floats, 4);
            for (out, value) in samples.iter_mut().zip(floats) { *out = (value * 32767.0).round() as i16; }
            offset = 0;
        }
        match io.writei(&samples[offset..]) {
            Ok(frames) => { offset += frames * 4; if frames == 0 { std::thread::park_timeout(Duration::from_millis(2)); } },
            Err(e) if e.errno() == libc::EAGAIN => std::thread::park_timeout(Duration::from_millis(2)),
            Err(e) if e.errno() == libc::EPIPE => {
                pcm.prepare().map_err(|e| e.to_string())?;
                offset = samples.len();
            },
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = pcm.drop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!("dualwave-usb-audio-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(root.join("sound")).unwrap();
            for port in ["1-1", "1-10"] {
                let usb = root.join(port);
                std::fs::create_dir_all(usb.join("hid")).unwrap();
                std::fs::write(usb.join("idVendor"), "054c\n").unwrap();
                std::fs::write(usb.join("idProduct"), "0ce6\n").unwrap();
            }
            Self(root)
        }
        fn pcm(&self, name: &str, port: &str) {
            let entry = self.0.join("sound").join(name);
            std::fs::create_dir_all(&entry).unwrap();
            symlink(self.0.join(port), entry.join("device")).unwrap();
        }
        fn find(&self) -> Result<PhysicalPcm, String> {
            discover(&self.0.join("1-1/hid"), &self.0.join("sound"))
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn matches_only_the_exact_controller_not_a_sibling_or_capture_endpoint() {
        let f = Fixture::new();
        f.pcm("pcmC2D0p", "1-1");
        f.pcm("pcmC3D0p", "1-10");
        f.pcm("pcmC2D0c", "1-1");
        let pcm = f.find().unwrap();
        assert_eq!(pcm.name, "hw:2,0");
        assert_eq!(pcm.usb, f.0.join("1-1"));
    }

    #[test]
    fn missing_ambiguous_and_non_sony_endpoints_fail_closed() {
        let f = Fixture::new();
        assert!(f.find().is_err());
        f.pcm("pcmC2D0p", "1-1");
        f.pcm("pcmC2D1p", "1-1");
        assert!(f.find().is_err());
        std::fs::remove_dir_all(f.0.join("sound/pcmC2D1p")).unwrap();
        std::fs::write(f.0.join("1-1/idVendor"), "1234").unwrap();
        assert!(f.find().is_err());
        assert!(discover(&f.0.join("virtual-hid"), &f.0.join("sound")).is_err());
    }

    #[test]
    fn usbip_virtual_devices_are_rejected_even_with_sony_identity() {
        let f = Fixture::new();
        let usb = f.0.join("vhci_hcd.0/1-2");
        std::fs::create_dir_all(usb.join("hid")).unwrap();
        std::fs::write(usb.join("idVendor"), "054c").unwrap();
        std::fs::write(usb.join("idProduct"), "0df2").unwrap();
        f.pcm("pcmC4D0p", "vhci_hcd.0/1-2");
        assert!(discover(&usb.join("hid"), &f.0.join("sound")).is_err());
    }

    #[test]
    fn playback_name_parser_rejects_plugins_capture_and_malformed_names() {
        assert_eq!(pcm_numbers("pcmC12D3p"), Some((12, 3)));
        for name in ["default", "plughw:2,0", "pcmC2D0c", "pcmC2D0pX", "pcmCD0p", "pcmC-2D0p"] {
            assert_eq!(pcm_numbers(name), None, "{name}");
        }
    }
}
