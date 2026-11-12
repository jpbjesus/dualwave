//! Opt-in volume control for playback belonging to the focused game.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use crate::{focus_linux::Watcher, games::FocusState};

fn game_root(mut focus: u32, games: &[u32], parent: impl Fn(u32) -> Option<u32>) -> Option<u32> {
    for _ in 0..64 {
        if focus <= 1 { break; }
        if games.contains(&focus) { return Some(focus); }
        focus = parent(focus)?;
    }
    None
}

fn related(pid: u32, root: u32, parent: impl Fn(u32) -> Option<u32>) -> bool {
    root > 1 && game_root(pid, &[root], parent).is_some()
}

fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

fn step_volume(values: &mut [u32], up: bool) -> bool {
    const NORMAL: u32 = 65536;
    const STEP: u32 = 3277;
    let Some(max) = values.iter().copied().max() else { return false; };
    if max > 0x7fff_ffff { return false; }
    let next = if up {
        if max >= NORMAL { return false; }
        max.saturating_add(STEP).min(NORMAL)
    } else { max.saturating_sub(STEP) };
    if max == next { return false; }
    for value in values {
        *value = if max == 0 { next } else { ((*value as u64 * next as u64 + max as u64 / 2) / max as u64) as u32 };
    }
    true
}

unsafe extern "C" {
    fn dw_game_volume(select: extern "C" fn(u32, *const std::ffi::c_char, *mut u32, u32, *mut std::ffi::c_void) -> i32, userdata: *mut std::ffi::c_void) -> i32;
}

fn audio_pid(pid: u32, binary: &str, proc_dir: &std::path::Path) -> Option<u32> {
    let candidates = std::fs::read_dir(proc_dir).ok()?.filter_map(|entry| {
        let entry = entry.ok()?;
        let host_pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
        let path = std::fs::read_link(entry.path().join("exe")).ok()?;
        if path.file_name()?.to_str()? != binary { return None; }
        let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
        let namespace_pids = status.lines().find_map(|line| line.strip_prefix("NSpid:"))?;
        namespace_pids.split_whitespace().any(|value| value.parse::<u32>() == Ok(pid)).then_some(host_pid)
    }).collect::<Vec<_>>();
    unique_pid(&candidates)
}

fn unique_pid(candidates: &[u32]) -> Option<u32> {
    match candidates { [pid] => Some(*pid), _ => None }
}

struct Selection<'a> {
    watcher: &'a mut Watcher,
    focus: u32,
    root: u32,
    up: bool,
    expires: Instant,
    stopped: &'a AtomicBool,
}
extern "C" fn select(pid: u32, binary: *const std::ffi::c_char, values: *mut u32, channels: u32, userdata: *mut std::ffi::c_void) -> i32 {
    // C owns the cvolume for this callback only; channels is checked before forming a slice.
    let selection = unsafe { &mut *(userdata as *mut Selection<'_>) };
    if channels == 0 || channels > 32 || values.is_null() || Instant::now() >= selection.expires
        || selection.stopped.load(Ordering::Acquire) || binary.is_null() { return 0; }
    let Ok(binary) = (unsafe { std::ffi::CStr::from_ptr(binary) }).to_str() else { return 0; };
    let Some(pid) = audio_pid(pid, binary, std::path::Path::new("/proc")) else { return 0; };
    if !related(pid, selection.root, parent) || selection.watcher.observe(true) != FocusState::Known(Some(selection.focus))
        || Instant::now() >= selection.expires || selection.stopped.load(Ordering::Acquire) { return 0; }
    let values = unsafe { std::slice::from_raw_parts_mut(values, channels as usize) };
    if step_volume(values, selection.up) { 1 } else { -1 }
}

struct Request { up: bool, games: Vec<u32>, expires: Instant }
#[derive(Default)]
pub struct Service {
    sender: Mutex<Option<Sender<Request>>>,
    stopped: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}
impl Service {
    pub fn failed(&self) -> bool { self.failed.load(Ordering::Acquire) }
    pub fn send(&self, up: bool, games: Vec<u32>) {
        if self.stopped.load(Ordering::Acquire) { return; }
        let mut sender = self.sender.lock();
        if sender.is_none() {
            let (tx, rx) = bounded::<Request>(1);
            let stopped = self.stopped.clone();
            let failed = self.failed.clone();
            match std::thread::Builder::new().name("game-volume".into()).spawn(move || {
                let mut watcher = Watcher::default();
                while let Ok(request) = rx.recv() {
                    if stopped.load(Ordering::Acquire) { break; }
                    if Instant::now() >= request.expires { continue; }
                    let mut focus = watcher.observe(true);
                    // The first KWin observation arrives asynchronously after script registration.
                    let ready = Instant::now() + Duration::from_millis(150);
                    while focus == FocusState::Unavailable && Instant::now() < ready {
                        std::thread::sleep(Duration::from_millis(5));
                        focus = watcher.observe(true);
                    }
                    let root = match focus {
                        FocusState::Known(Some(pid)) => game_root(pid, &request.games, parent).map(|root| (pid, root)),
                        _ => None,
                    };
                    let result = if let Some((focus, root)) = root {
                        let mut selection = Selection { watcher: &mut watcher, focus, root, up: request.up,
                            expires: request.expires, stopped: &stopped };
                        unsafe { dw_game_volume(select, &mut selection as *mut _ as *mut _) }
                    } else { -1 };
                    let was_failed = failed.swap(result < 0 || root.is_none(), Ordering::AcqRel);
                    if result < 0 && !was_failed { log::warn!("Active game volume unavailable: no focused game or audio server unavailable"); }
                }
            }) {
                Ok(_) => *sender = Some(tx),
                Err(error) => { self.failed.store(true, Ordering::Release); log::warn!("game volume worker: {error}"); return; }
            }
        }
        if let Some(sender) = sender.as_ref() {
            let _ = sender.try_send(Request { up, games, expires: Instant::now() + Duration::from_millis(500) });
        }
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.sender.lock().take();
    }
}
impl Drop for Service { fn drop(&mut self) { self.stop(); } }

#[cfg(test)]
mod tests {
    use super::*;
    fn parent(pid: u32) -> Option<u32> {
        match pid { 11 | 12 => Some(10), 10 | 20 => Some(2), _ => None }
    }
    #[test]
    fn audio_identity_requires_matching_binary_and_unambiguous_namespace_pid() {
        let dir = tempfile::tempdir().unwrap();
        let process = |host_pid, namespace_pid| {
            let process_dir = dir.path().join(format!("{host_pid}"));
            std::fs::create_dir(&process_dir).unwrap();
            std::os::unix::fs::symlink("/games/game.exe", process_dir.join("exe")).unwrap();
            std::fs::write(process_dir.join("status"), format!("NSpid:\t{host_pid}\t{namespace_pid}\n")).unwrap();
        };
        process(100, 10);
        assert_eq!(audio_pid(100, "game.exe", dir.path()), Some(100));
        assert_eq!(audio_pid(10, "game.exe", dir.path()), Some(100));
        assert_eq!(audio_pid(10, "music-player", dir.path()), None);
        assert_eq!(audio_pid(99, "game.exe", dir.path()), None);
        process(200, 10);
        assert_eq!(audio_pid(10, "game.exe", dir.path()), None);
        assert_eq!(audio_pid(100, "game.exe", dir.path()), Some(100));
    }
    #[test]
    fn ambiguous_namespace_pids_are_rejected() {
        assert_eq!(unique_pid(&[100]), Some(100));
        assert_eq!(unique_pid(&[100, 200]), None);
        assert_eq!(unique_pid(&[]), None);
    }
    #[test]
    fn unused_feature_has_no_worker() {
        let service = Service::default();
        assert!(service.sender.lock().is_none());
        assert!(!service.failed());
        service.stop();
        service.send(true, vec![10]);
        assert!(service.sender.lock().is_none());
    }
    #[test]
    fn expired_or_unfocused_requests_never_change_volume() {
        let mut watcher = Watcher::default();
        let stopped = AtomicBool::new(true);
        let mut selection = Selection { watcher: &mut watcher, focus: 10, root: 10,
            up: true, expires: Instant::now() - Duration::from_millis(1), stopped: &stopped };
        let mut values = [32768u32; 2];
        assert_eq!(select(10, std::ptr::null(), values.as_mut_ptr(), 2, &mut selection as *mut _ as *mut _), 0);
        assert_eq!(values, [32768; 2]);
        assert_eq!(select(10, std::ptr::null(), values.as_mut_ptr(), 33, &mut selection as *mut _ as *mut _), 0);
    }
    #[test]
    fn targets_game_descendants_without_matching_siblings_or_launchers() {
        assert_eq!(game_root(11, &[10], parent), Some(10));
        assert_eq!(game_root(20, &[10], parent), None);
        assert!(related(12, 10, parent));
        assert!(!related(20, 10, parent));
        assert!(!related(2, 10, parent));
        assert!(!related(0, 10, parent));
        assert_eq!(game_root(10, &[], parent), None);
        assert!(!related(30, 10, |_| Some(30)));
    }
    #[test]
    fn volume_steps_preserve_balance_and_do_not_boost() {
        let mut v = [32768, 16384];
        assert!(step_volume(&mut v, true));
        assert_eq!(v, [36045, 18023]);
        assert!(step_volume(&mut v, false));
        assert_eq!(v, [32768, 16384]);
        let mut v = [65000, 32500];
        step_volume(&mut v, true);
        assert_eq!(v, [65536, 32768]);
        let mut v = [131072, 65536];
        assert!(!step_volume(&mut v, true));
        step_volume(&mut v, false);
        assert_eq!(v, [127795, 63898]);
        let mut silent = [0, 0];
        assert!(!step_volume(&mut silent, false));
        assert!(step_volume(&mut silent, true));
        assert_eq!(silent, [3277, 3277]);
        assert!(!step_volume(&mut [], true));
        assert!(!step_volume(&mut [u32::MAX], true));
    }
}
