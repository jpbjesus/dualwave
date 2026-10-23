//! Native Linux desktop, scheduling and process helpers.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)] // Retained cross-platform API; Linux UI uses egui viewport commands.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn raise_thread_priority(critical: bool) {
    // Nice applies to the calling Linux thread. A missing CAP_SYS_NICE is
    // normal for an unprivileged desktop app; never enable SCHED_FIFO.
    let priority = if critical { -10 } else { -5 };
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, priority) } != 0 {
        log::debug!("could not raise thread priority: {}", io::Error::last_os_error());
    }
    timer_resolution_1ms();
}

pub fn timer_resolution_1ms() {
    // Timer slack is per-thread and inherited by new threads. Linux already
    // has high-resolution timers; don't change a global kernel clock.
    if unsafe { libc::prctl(libc::PR_SET_TIMERSLACK, 1_000 as libc::c_ulong, 0, 0, 0) } != 0 {
        log::debug!("could not reduce timer slack: {}", io::Error::last_os_error());
    }
}

pub fn opt_out_of_power_throttling() {
    // Linux does not throttle the process's timers when its window is hidden.
    timer_resolution_1ms();
}

pub struct PreciseTimer;
impl PreciseTimer {
    pub fn new() -> PreciseTimer {
        timer_resolution_1ms();
        PreciseTimer
    }

    pub fn sleep_until(&self, deadline: Instant) {
        let Some(delay) = deadline.checked_duration_since(Instant::now()) else { return; };
        let mut target = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut target) } != 0 {
            std::thread::sleep(delay);
            return;
        }
        let nanos = target.tv_nsec as u64 + delay.subsec_nanos() as u64;
        target.tv_sec += delay.as_secs() as libc::time_t + (nanos / 1_000_000_000) as libc::time_t;
        target.tv_nsec = (nanos % 1_000_000_000) as libc::c_long;
        loop {
            let result = unsafe {
                libc::clock_nanosleep(libc::CLOCK_MONOTONIC, libc::TIMER_ABSTIME, &target, std::ptr::null_mut())
            };
            if result == 0 { break; }
            if result != libc::EINTR {
                if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                    std::thread::sleep(remaining);
                }
                break;
            }
        }
    }
}

pub fn bluetooth_address(mac: &str) -> Option<u64> {
    if mac.chars().any(|c| !c.is_ascii_hexdigit() && c != ':' && c != '-') { return None; }
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (hex.len() == 12).then(|| u64::from_str_radix(&hex, 16).ok()).flatten()
}

pub fn bt_disconnect(mac: &str) -> bool {
    let Some(address) = bluetooth_address(mac) else { return false; };
    let address = format!("{address:012X}");
    let device = (0..6).map(|i| &address[i * 2..i * 2 + 2]).collect::<Vec<_>>().join("_");
    let Ok(adapters) = std::fs::read_dir("/sys/class/bluetooth") else { return false; };
    for adapter in adapters.flatten() {
        let name = adapter.file_name();
        let name = name.to_string_lossy();
        if !name.strip_prefix("hci").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())) {
            continue;
        }
        let path = format!("/org/bluez/{name}/dev_{device}");
        match std::process::Command::new("busctl")
            .args(["--system", "--timeout=5", "call", "org.bluez", &path, "org.bluez.Device1", "Disconnect"])
            .output()
        {
            Ok(output) if output.status.success() => {
                log::info!("disconnected Bluetooth controller {mac}");
                return true;
            }
            Ok(output) => log::debug!("BlueZ disconnect: {}", String::from_utf8_lossy(&output.stderr).trim()),
            Err(error) => {
                log::warn!("Bluetooth disconnect requires BlueZ and busctl: {error}");
                return false;
            }
        }
    }
    log::warn!("could not disconnect Bluetooth controller {mac}");
    false
}

fn xdg_home(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(fallback))
}

pub fn config_dir() -> PathBuf {
    xdg_home("XDG_CONFIG_HOME", ".config").join("dualwave")
}

fn desktop_exec(path: &Path) -> String {
    // Exec has its own quoting rules, not shell quoting. Percent is a field
    // expansion even inside quotes, and must be doubled in literal paths.
    let mut result = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => result.push_str("%%"),
            '\\' => result.push_str("\\\\\\\\"),
            '"' | '`' | '$' => { result.push_str("\\\\"); result.push(c); }
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            _ => result.push(c),
        }
    }
    result.push('"');
    result
}

fn write_autostart(path: &Path, executable: &Path, on: bool) -> io::Result<()> {
    if !on {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("autostart path has no parent"))?)?;
    let contents = format!("[Desktop Entry]\nType=Application\nName=DualWave\nComment=DualSense controller companion\nExec={} --minimized\nIcon=dualwave\nTerminal=false\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n", desktop_exec(executable));
    let temporary = path.with_extension(format!("desktop.{}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(temporary); }
    result
}

pub fn set_autostart(on: bool) -> bool {
    // AppImage mounts are temporary: save the original AppImage path.
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from)
        .filter(|p| p.is_absolute()).map(Ok).unwrap_or_else(std::env::current_exe);
    let result = exe.and_then(|exe| write_autostart(&xdg_home("XDG_CONFIG_HOME", ".config").join("autostart/dualwave.desktop"), &exe, on));
    if let Err(error) = &result { log::warn!("could not change autostart: {error}"); }
    result.is_ok()
}

pub fn process_list() -> Vec<(u32, String)> {
    let mut processes = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue; };
            let name = process_path(pid).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .or_else(|| std::fs::read_to_string(entry.path().join("comm")).ok().map(|s| s.trim_end_matches('\n').to_owned()));
            if let Some(name) = name { processes.push((pid, name.to_ascii_lowercase())); }
        }
    }
    processes.sort_unstable_by_key(|(pid, _)| *pid);
    processes
}

pub fn process_names() -> HashSet<String> {
    process_list().into_iter().map(|(_, name)| name).collect()
}

fn wine_program(cmdline: &[u8]) -> Option<&str> {
    cmdline.split(|b| *b == 0).filter_map(|arg| std::str::from_utf8(arg).ok())
        .map(|arg| arg.trim_matches('"'))
        .find(|arg| !arg.starts_with('-') && {
            let lower = arg.to_ascii_lowercase();
            lower.ends_with(".exe") || lower.ends_with(".com")
        })
}

fn wine_image_path(pid: u32, program: &str) -> Option<PathBuf> {
    let normalized = program.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let candidate = if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        let environment = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let variable = |name: &[u8]| environment.split(|b| *b == 0)
            .find_map(|item| item.strip_prefix(name)).map(|value| {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(value))
            });
        let prefix = variable(b"WINEPREFIX=").or_else(|| variable(b"HOME=").map(|home| home.join(".wine")))?;
        let drive = format!("{}:", (bytes[0] as char).to_ascii_lowercase());
        let link = prefix.join("dosdevices").join(&drive);
        let root = std::fs::canonicalize(&link).ok().or_else(|| (drive == "z:").then(|| PathBuf::from("/")))?;
        root.join(&normalized[3..])
    } else if Path::new(&normalized).is_absolute() {
        PathBuf::from(normalized)
    } else {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?.join(normalized)
    };
    std::fs::canonicalize(candidate).ok().filter(|path| path.is_file())
}

pub fn process_path(pid: u32) -> Option<PathBuf> {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    // Wine's /proc/exe points at the loader rather than the PE image. Resolve
    // the game's command line through its own prefix so profile matching and
    // DualSense DLL detection work for Steam/Proton too.
    let wine = executable.file_name()?.to_string_lossy().to_ascii_lowercase().starts_with("wine");
    if wine {
        if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
            if let Some(path) = wine_program(&cmdline).and_then(|program| wine_image_path(pid, program)) {
                return Some(path);
            }
        }
    }
    Some(executable)
}

fn proc_start_ticks(stat: &str) -> Option<u64> {
    // comm can itself contain spaces and parentheses; fields after the last
    // ')' begin at field 3 (state). starttime is field 22.
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

pub fn process_started(pid: u32) -> Option<SystemTime> {
    let ticks = proc_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 { return None; }
    let boot = std::fs::read_to_string("/proc/stat").ok()?.lines()
        .find_map(|line| line.strip_prefix("btime ").and_then(|s| s.parse::<u64>().ok()))?;
    UNIX_EPOCH.checked_add(Duration::from_secs(boot))?
        .checked_add(Duration::from_secs(ticks / hz as u64) + Duration::from_nanos((ticks % hz as u64) * 1_000_000_000 / hz as u64))
}

fn private_runtime_dir() -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute())
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0));
    let path = base.map(|p| p.join("dualwave")).unwrap_or_else(|| std::env::temp_dir().join(format!("dualwave-{uid}")));
    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {},
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {},
        Err(e) => return Err(e),
    }
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsafe DualWave runtime directory"));
    }
    Ok(path)
}

fn try_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(Some(file))
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(error) }
    }
}

pub fn single_instance(wait: Duration) -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();
    if LOCK.get().is_some() { return true; }
    let directory = match private_runtime_dir() {
        Ok(directory) => directory,
        Err(error) => { log::error!("single-instance directory unavailable: {error}"); return false; }
    };
    let deadline = Instant::now() + wait;
    loop {
        match try_lock(&directory.join("instance.lock")) {
            Ok(Some(file)) => {
                let _ = LOCK.set(file);
                let socket_path = directory.join("activate.sock");
                let _ = std::fs::remove_file(&socket_path);
                match UnixDatagram::bind(&socket_path) {
                    Ok(socket) => {
                        if let Err(error) = std::thread::Builder::new().name("activation".into()).spawn(move || {
                            let mut message = [0u8; 32];
                            loop {
                                match socket.recv(&mut message) {
                                    Ok(n) if &message[..n] == b"show" => crate::tray::show_main_window(),
                                    Ok(_) => {},
                                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                                    Err(_) => break,
                                }
                            }
                        }) { log::warn!("could not start activation listener: {error}"); }
                    }
                    Err(error) => log::warn!("could not listen for window activation: {error}"),
                }
                return true;
            }
            Ok(None) => {},
            Err(error) => { log::error!("single-instance lock unavailable: {error}"); return false; }
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { return false; };
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

pub fn activate_existing(_title: &str) {
    if let Ok(directory) = private_runtime_dir() {
        if let Ok(socket) = UnixDatagram::unbound() {
            if let Err(error) = socket.send_to(b"show", directory.join("activate.sock")) {
                log::warn!("could not activate existing DualWave window: {error}");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowFrame { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowCare { pub draw: bool, pub wake_ms: Option<u64> }

#[allow(dead_code)] // Retained API; Linux UI calls maintain_egui_window.
pub fn maintain_window(_handle: isize, _saved: &mut Option<WindowFrame>, _logged: &mut bool) -> WindowCare {
    // Native handles cannot be interpreted portably on Wayland. The UI uses
    // maintain_egui_window below, which consumes winit's viewport information.
    WindowCare { draw: true, wake_ms: None }
}

pub fn maintain_egui_window(ctx: &eframe::egui::Context, saved: &mut Option<WindowFrame>, logged: &mut bool) -> WindowCare {
    use eframe::egui::{vec2, ViewportCommand};
    let (minimized, inner, outer, monitor) = ctx.input(|i| {
        let v = i.viewport();
        (v.minimized == Some(true), v.inner_rect, v.outer_rect, v.monitor_size)
    });
    if minimized { return WindowCare { draw: false, wake_ms: Some(400) }; }
    if let Some(rect) = outer.filter(|r| r.width() >= 200.0 && r.height() >= 200.0) {
        *saved = Some(WindowFrame { x: rect.min.x as i32, y: rect.min.y as i32, w: rect.width() as i32, h: rect.height() as i32 });
        *logged = false;
    }
    if let Some(inner) = inner {
        if inner.width() < 200.0 || inner.height() < 200.0 {
            let mut size = saved.map(|f| vec2(f.w as f32, f.h as f32)).unwrap_or(vec2(1240.0, 800.0));
            if let Some(monitor) = monitor { size = size.min((monitor - vec2(24.0, 56.0)).max(vec2(200.0, 200.0))); }
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            if !*logged { log::info!("restoring collapsed window to {}x{}", size.x, size.y); }
            *logged = true;
            return WindowCare { draw: false, wake_ms: Some(0) };
        }
    }
    WindowCare { draw: true, wake_ms: None }
}

pub fn initial_window_size(want: [f32; 2], min: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let size = std::env::var("DSZ_SNAPSHOT_SIZE").ok().and_then(|value| {
        let (w, h) = value.split_once('x')?;
        let (w, h) = (w.parse::<f32>().ok()?, h.parse::<f32>().ok()?);
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some([w, h])
    }).unwrap_or(want);
    // Monitor size becomes available in egui once a native window exists.
    (size, [min[0].min(size[0]), min[1].min(size[1])])
}

pub fn data_dir() -> PathBuf {
    let directory = std::env::var_os("DSZ_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
        crate::portable::folder().filter(|p| p.is_dir()).unwrap_or_else(config_dir)
    });
    if let Err(error) = std::fs::create_dir_all(&directory) { log::error!("could not create settings directory {}: {error}", directory.display()); }
    directory
}

pub fn open_in_explorer(path: &Path) {
    if let Err(error) = std::process::Command::new("xdg-open").arg(path).spawn() { log::warn!("could not open {}: {error}", path.display()); }
}

pub fn open_url(url: &str) {
    if let Err(error) = std::process::Command::new("xdg-open").arg(url).spawn() { log::warn!("could not open browser: {error}"); }
}

pub fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("dualwave-platform-{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn identifies_wine_game_arguments_without_matching_launcher_flags() {
        let command = b"/usr/bin/wine64\0--log=trace.exe\0C:\\Games\\Space Game.EXE\0-option\0";
        assert_eq!(wine_program(command), Some("C:\\Games\\Space Game.EXE"));
        assert_eq!(wine_program(b"/usr/bin/wineserver\0-f\0"), None);
    }

    #[test]
    fn proc_stat_command_may_contain_parentheses_and_spaces() {
        let stat = format!("123 (name ) with spaces) S {} 987654 0 0", (0..18).map(|_| "0").collect::<Vec<_>>().join(" "));
        assert_eq!(proc_start_ticks(&stat), Some(987654));
        assert_eq!(proc_start_ticks("123 broken"), None);
    }

    #[test]
    fn reads_own_process_information() {
        let pid = std::process::id();
        assert_eq!(process_path(pid).unwrap(), std::env::current_exe().unwrap());
        assert!(process_list().iter().any(|(id, _)| *id == pid));
        let age = SystemTime::now().duration_since(process_started(pid).unwrap()).unwrap();
        assert!(age < Duration::from_secs(3600));
    }

    #[test]
    fn autostart_round_trip_uses_desktop_entry_escaping() {
        let root = scratch("autostart");
        let path = root.join("autostart/dualwave.desktop");
        write_autostart(&path, Path::new("/home/test/My $app%/DualWave.AppImage"), true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Exec=\"/home/test/My \\\\$app%%/DualWave.AppImage\" --minimized\n"));
        assert_eq!(text.lines().filter(|l| l.starts_with("Exec=")).count(), 1);
        write_autostart(&path, Path::new("unused"), false).unwrap();
        write_autostart(&path, Path::new("unused"), false).unwrap();
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flock_excludes_another_open_and_releases_on_close() {
        let root = scratch("lock");
        let path = root.join("lock");
        let first = try_lock(&path).unwrap().unwrap();
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_non_address_input_without_bluetooth_access() {
        assert_eq!(bluetooth_address("aa-bb-cc-dd-ee-ff"), Some(0xAABB_CCDD_EEFF));
        assert!(!bt_disconnect("not an address"));
        assert_eq!(bluetooth_address("junkAA:BB:CC:DD:EE:FF"), None);
    }
}
