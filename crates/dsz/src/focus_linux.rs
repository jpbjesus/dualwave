//! Optional desktop observations. No window titles, input capture or subprocesses.
use std::io::Write;
use std::sync::{Arc, atomic::{AtomicU32, AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use x11rb::{connection::Connection, protocol::xproto::{AtomEnum, ConnectionExt}, rust_connection::RustConnection};
use crate::games::FocusState;

#[derive(Default)]
pub struct Watcher {
    backend: Option<Backend>,
    retry: Option<Instant>,
    error: Option<String>,
}
impl Watcher {
    pub fn observe(&mut self, enabled: bool) -> FocusState {
        if !enabled { self.backend = None; self.retry = None; self.error = None; return FocusState::NotRequested; }
        let result = (|| {
            if self.backend.is_none() {
                if self.retry.is_some_and(|at| Instant::now() < at) { return Ok(FocusState::Unavailable); }
                self.backend = Some(Backend::new()?);
            }
            self.backend.as_ref().unwrap().poll()
        })();
        match result {
            Ok(state) => { self.error = None; state },
            Err(error) => {
                if self.error.as_ref() != Some(&error) { log::warn!("window focus unavailable: {error}"); }
                self.error = Some(error);
                self.backend = None;
                self.retry = Some(Instant::now() + Duration::from_secs(10));
                FocusState::Unavailable
            }
        }
    }
}

enum Backend { Kwin(Kwin), X11(X11) }
impl Backend {
    fn new() -> Result<Self, String> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland");
        // XWayland cannot see native Wayland focus; never use it as a fallback.
        if wayland { Kwin::new().map(Self::Kwin) } else { X11::new().map(Self::X11) }
    }
    fn poll(&self) -> Result<FocusState, String> {
        match self { Self::Kwin(k) => k.poll(), Self::X11(x) => x.poll() }
    }
}

const UNKNOWN: u32 = u32::MAX;

fn observed_pid(value: &str) -> u32 {
    value.parse::<u32>().ok().filter(|pid| *pid != UNKNOWN).unwrap_or(0)
}
struct FocusReceiver { owner: String, pid: Arc<AtomicU32> }
#[zbus::interface(name = "org.dualwave.WindowFocus")]
impl FocusReceiver {
    fn update(&self, pid: &str, #[zbus(header)] header: zbus::message::Header<'_>) {
        if header.sender().is_some_and(|sender| sender.as_str() == self.owner) {
            self.pid.store(observed_pid(pid), Ordering::Release);
        }
    }
}

struct Kwin {
    connection: zbus::blocking::Connection,
    owner: String,
    name: String,
    pid: Arc<AtomicU32>,
    _script: tempfile::NamedTempFile,
}
impl Kwin {
    fn new() -> Result<Self, String> {
        let lookup = zbus::blocking::connection::Builder::session().map_err(|e| e.to_string())?
            .method_timeout(Duration::from_secs(2)).build().map_err(|e| e.to_string())?;
        let owner: String = zbus::blocking::Proxy::new(&lookup, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
            .map_err(|e| e.to_string())?.call("GetNameOwner", &("org.kde.KWin",)).map_err(|e| e.to_string())?;
        let pid = Arc::new(AtomicU32::new(UNKNOWN));
        // Register during build so zbus starts its object server inside its runtime.
        let connection = zbus::blocking::connection::Builder::session().map_err(|e| e.to_string())?
            .method_timeout(Duration::from_secs(2))
            .serve_at("/WindowFocus", FocusReceiver { owner: owner.clone(), pid: pid.clone() }).map_err(|e| e.to_string())?
            .build().map_err(|e| e.to_string())?;
        drop(lookup);
        let destination = serde_json::to_string(connection.unique_name().ok_or("No D-Bus identity")?.as_str()).map_err(|e| e.to_string())?;
        let mut script = tempfile::Builder::new().prefix("dualwave-window-focus-").suffix(".js").tempfile().map_err(|e| e.to_string())?;
        write!(script, "function observe() {{ var w = workspace.activeWindow; callDBus({destination}, '/WindowFocus', 'org.dualwave.WindowFocus', 'Update', String(w ? w.pid : 0)); }} workspace.windowActivated.connect(observe); observe();").map_err(|e| e.to_string())?;
        script.flush().map_err(|e| e.to_string())?;
        static NEXT_OBSERVER: AtomicU64 = AtomicU64::new(0);
        let observer = NEXT_OBSERVER.fetch_add(1, Ordering::Relaxed);
        let monitor = Self { connection, owner, name: format!("dualwave-window-focus-{}-{observer}", std::process::id()), pid, _script: script };
        let proxy = monitor.scripting()?;
        // Retire our observer left by a previous crashed instance, not user scripts.
        let _: bool = proxy.call("unloadScript", &(monitor.name.as_str(),)).map_err(|e| e.to_string())?;
        let id: i32 = proxy.call("loadScript", &(monitor._script.path().to_str().ok_or("Invalid observer path")?, monitor.name.as_str())).map_err(|e| e.to_string())?;
        if id < 0 { return Err("KWin could not load the focus observer".into()); }
        zbus::blocking::Proxy::new(&monitor.connection, monitor.owner.as_str(), format!("/Scripting/Script{id}"), "org.kde.kwin.Script")
            .map_err(|e| e.to_string())?.call::<_, _, ()>("run", &()).map_err(|e| e.to_string())?;
        drop(proxy);
        Ok(monitor)
    }
    fn scripting(&self) -> Result<zbus::blocking::Proxy<'_>, String> {
        zbus::blocking::Proxy::new(&self.connection, self.owner.as_str(), "/Scripting", "org.kde.kwin.Scripting").map_err(|e| e.to_string())
    }
    fn poll(&self) -> Result<FocusState, String> {
        let loaded: bool = self.scripting()?.call("isScriptLoaded", &(self.name.as_str(),)).map_err(|e| e.to_string())?;
        if !loaded { return Err("KWin focus observer stopped".into()); }
        Ok(match self.pid.load(Ordering::Acquire) {
            UNKNOWN => FocusState::Unavailable, 0 => FocusState::Known(None), pid => FocusState::Known(Some(pid)),
        })
    }
}
impl Drop for Kwin {
    fn drop(&mut self) {
        if let Ok(proxy) = self.scripting() { let _ = proxy.call::<_, _, bool>("unloadScript", &(self.name.as_str(),)); }
    }
}

struct X11 { connection: RustConnection, root: u32, active: u32, pid: u32, machine: u32, hostname: String }
impl X11 {
    fn new() -> Result<Self, String> {
        let (connection, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = connection.setup().roots[screen].root;
        let atom = |name: &[u8]| -> Result<u32, String> {
            Ok(connection.intern_atom(false, name).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
        };
        let active = atom(b"_NET_ACTIVE_WINDOW")?;
        let pid = atom(b"_NET_WM_PID")?;
        let machine = atom(b"WM_CLIENT_MACHINE")?;
        let mut hostname = [0u8; 256];
        if unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) } != 0 { return Err("Cannot identify the local X11 host".into()); }
        let hostname = String::from_utf8(hostname.split(|b| *b == 0).next().unwrap_or_default().to_vec()).map_err(|e| e.to_string())?;
        Ok(Self { connection, root, active, pid, machine, hostname })
    }
    fn poll(&self) -> Result<FocusState, String> {
        let active = self.connection.get_property(false, self.root, self.active, AtomEnum::WINDOW, 0, 1)
            .map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        if active.type_ == 0 { return Err("X11 window manager does not expose active-window state".into()); }
        let Some(window) = active.value32().and_then(|mut values| values.next()).filter(|w| *w != 0) else { return Ok(FocusState::Known(None)); };
        // EWMH PIDs are host-local: a remote X11 client must not match a local game.
        let machine = self.connection.get_property(false, window, self.machine, AtomEnum::STRING, 0, 64)
            .map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        if machine.bytes_after != 0 || !machine.value.split(|b| *b == 0).next().unwrap_or_default().eq_ignore_ascii_case(self.hostname.as_bytes()) {
            return Ok(FocusState::Known(None));
        }
        let pid = self.connection.get_property(false, window, self.pid, AtomEnum::CARDINAL, 0, 1)
            .map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        Ok(FocusState::Known(pid.value32().and_then(|mut values| values.next()).filter(|p| *p != 0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unidentified_window_pids_clear_the_previous_focus() {
        assert_eq!(observed_pid("123"), 123);
        for value in ["0", "-1", "undefined", "", "4294967295", "999999999999999"] { assert_eq!(observed_pid(value), 0); }
    }

    #[test]
    fn disabled_focus_never_creates_a_desktop_observer() {
        let mut watcher = Watcher::default();
        assert_eq!(watcher.observe(false), FocusState::NotRequested);
        assert!(watcher.backend.is_none());
        assert!(watcher.retry.is_none());
    }

    #[test]
    #[ignore = "requires an explicitly enabled live KDE Plasma 6 session"]
    fn kwin_observer_reports_a_pid_and_unloads_on_drop() {
        assert_eq!(std::env::var("DSZ_TEST_KWIN_FOCUS").as_deref(), Ok("1"));
        let monitor = Kwin::new().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while monitor.poll().unwrap() == FocusState::Unavailable {
            assert!(Instant::now() < deadline, "KWin did not report focus");
            std::thread::sleep(Duration::from_millis(20));
        }
        let other = Kwin::new().unwrap();
        assert_ne!(monitor.name, other.name);
        assert!(monitor.poll().is_ok());
        drop(other);
        assert!(monitor.poll().is_ok(), "independent observers must not unload each other");
        let connection = monitor.connection.clone();
        let owner = monitor.owner.clone();
        let name = monitor.name.clone();
        let dir = std::env::temp_dir().join(format!("dualwave-focus-shutdown-{}", std::process::id()));
        let engine = crate::engine::Engine::new(dir.clone());
        engine.focus_monitor.lock().backend = Some(Backend::Kwin(monitor));
        engine.shutdown();
        assert!(engine.focus_monitor.lock().backend.is_none());
        let _ = std::fs::remove_dir_all(dir);
        let proxy = zbus::blocking::Proxy::new(&connection, owner.as_str(), "/Scripting", "org.kde.kwin.Scripting").unwrap();
        assert!(!proxy.call::<_, _, bool>("isScriptLoaded", &(name.as_str(),)).unwrap());
    }
    #[test]
    #[ignore = "requires an explicitly enabled isolated X11 server"]
    fn x11_focus_uses_local_pid_properties_and_clears_missing_focus() {
        use x11rb::{wrapper::ConnectionExt as _, protocol::xproto::{CreateWindowAux, PropMode, WindowClass}};
        assert_eq!(std::env::var("DSZ_TEST_X11_FOCUS").as_deref(), Ok("1"));
        let monitor = X11::new().unwrap();
        assert!(monitor.poll().is_err(), "isolated server must not have an active window manager");
        let conn = &monitor.connection;
        let window = conn.generate_id().unwrap();
        conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, window, monitor.root, 0, 0, 10, 10, 0,
            WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new()).unwrap().check().unwrap();
        conn.change_property32(PropMode::REPLACE, monitor.root, monitor.active, AtomEnum::WINDOW, &[window]).unwrap().check().unwrap();
        conn.change_property32(PropMode::REPLACE, window, monitor.pid, AtomEnum::CARDINAL, &[123]).unwrap().check().unwrap();
        conn.change_property8(PropMode::REPLACE, window, monitor.machine, AtomEnum::STRING, monitor.hostname.as_bytes()).unwrap().check().unwrap();
        assert_eq!(monitor.poll().unwrap(), FocusState::Known(Some(123)));
        conn.change_property8(PropMode::REPLACE, window, monitor.machine, AtomEnum::STRING, b"remote.invalid").unwrap().check().unwrap();
        assert_eq!(monitor.poll().unwrap(), FocusState::Known(None));
        conn.change_property8(PropMode::REPLACE, window, monitor.machine, AtomEnum::STRING, monitor.hostname.as_bytes()).unwrap().check().unwrap();
        conn.delete_property(window, monitor.pid).unwrap().check().unwrap();
        assert_eq!(monitor.poll().unwrap(), FocusState::Known(None));
        conn.change_property32(PropMode::REPLACE, monitor.root, monitor.active, AtomEnum::WINDOW, &[0]).unwrap().check().unwrap();
        assert_eq!(monitor.poll().unwrap(), FocusState::Known(None));
        conn.delete_property(monitor.root, monitor.active).unwrap().check().unwrap();
        conn.destroy_window(window).unwrap().check().unwrap();
    }

}
