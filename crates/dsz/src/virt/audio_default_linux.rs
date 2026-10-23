//! Windows has six default endpoint roles; Linux session managers choose
//! render/capture targets using their own policy and saved preferences.
//! Do not apply the Windows arrival heuristic to WirePlumber: it can undo
//! an intentional user selection. Shared settings retain their Windows ABI.

pub struct Watcher {
    pub prefs: [bool; 6],
}

impl Watcher {
    pub fn new(prefs: [bool; 6]) -> Self {
        log::debug!("Windows default-audio role guard is unavailable on Linux; audio routing follows the session manager");
        Self { prefs }
    }
    pub fn busy(&self) -> bool { false }
    /// No Windows role preference can change on this platform.
    pub fn tick(&mut self) -> bool { false }
}

pub fn describe() {
    println!("Default audio routing is managed by PipeWire/WirePlumber; Windows endpoint roles are unavailable on Linux.");
}
