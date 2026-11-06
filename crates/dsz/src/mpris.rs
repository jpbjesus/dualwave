//! Explicit player commands only; no default-player fallback or service activation.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use crossbeam_channel::{bounded, Sender};
use parking_lot::{Mutex, RwLock};
use crate::profile::{valid_player_target, PlayerCommand};

#[derive(Clone, Debug)]
pub struct Player { pub name: String, pub identity: String }
impl Player {
    pub fn label(&self) -> String {
        let suffix = self.name.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(&self.name);
        if self.identity.eq_ignore_ascii_case(suffix) { self.identity.clone() } else { format!("{} · {}", self.identity, suffix) }
    }
}
#[derive(Clone, Default)]
pub struct Status { pub players: Vec<Player>, pub refreshing: bool, pub failed: bool, pub failed_player: Option<String> }
struct Request { player: String, command: PlayerCommand, expires: Instant }
#[derive(Default)]
pub struct Service {
    commands: Mutex<Option<Sender<Request>>>,
    status: Arc<RwLock<Status>>,
    stopped: Arc<AtomicBool>,
}
impl Service {
    pub fn snapshot(&self) -> Status { self.status.read().clone() }
    pub fn refresh(&self) {
        if self.stopped.load(Ordering::Acquire) { return; }
        {
            let mut status = self.status.write();
            if status.refreshing { return; }
            status.refreshing = true;
        }
        let status = self.status.clone();
        let stopped = self.stopped.clone();
        if let Err(error) = std::thread::Builder::new().name("media-player-list".into()).spawn(move || {
            let result = if stopped.load(Ordering::Acquire) { Ok(Vec::new()) } else { list_players() };
            let mut status = status.write();
            status.refreshing = false;
            status.failed = result.is_err();
            status.failed_player = None;
            match result {
                Ok(players) => status.players = players,
                Err(error) => { status.players.clear(); log::warn!("media player discovery: {error}"); },
            }
        }) {
            log::warn!("media player discovery could not start: {error}");
            let mut status = self.status.write();
            status.refreshing = false;
            status.failed = true;
            status.failed_player = None;
        }
    }
    pub fn send(&self, player: &str, command: PlayerCommand) {
        if !valid_player_target(player) || self.stopped.load(Ordering::Acquire) { return; }
        let mut sender = self.commands.lock();
        if self.stopped.load(Ordering::Acquire) { return; }
        if sender.is_none() {
            let (tx, rx) = bounded::<Request>(8);
            let status = self.status.clone();
            let stopped = self.stopped.clone();
            match std::thread::Builder::new().name("media-player-control".into()).spawn(move || {
                let mut bus = None;
                let mut warned = false;
                while let Ok(request) = rx.recv() {
                    if stopped.load(Ordering::Acquire) { break; }
                    if Instant::now() >= request.expires { continue; }
                    let result = (|| {
                        if bus.is_none() { bus = Some(connection()?); }
                        execute(bus.as_ref().unwrap(), &request.player, request.command, request.expires)
                    })();
                    {
                        let mut status = status.write();
                        status.failed = result.is_err();
                        status.failed_player = result.is_err().then(|| request.player.clone());
                    }
                    if let Err(error) = result {
                        if !warned { log::warn!("selected media player: {error}"); }
                        warned = true;
                        bus = None;
                    } else { warned = false; }
                }
            }) {
                Ok(_) => *sender = Some(tx),
                Err(error) => {
                    log::warn!("media player worker could not start: {error}");
                    let mut status = self.status.write();
                    status.failed = true;
                    status.failed_player = Some(player.into());
                    return;
                },
            }
        }
        let request = Request { player: player.into(), command, expires: Instant::now() + Duration::from_millis(750) };
        if let Err(error) = sender.as_ref().unwrap().try_send(request) {
            if matches!(error, crossbeam_channel::TrySendError::Disconnected(_)) { *sender = None; }
            let mut status = self.status.write();
            status.failed = true;
            status.failed_player = Some(player.into());
        }
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.commands.lock().take();
    }
}
impl Drop for Service { fn drop(&mut self) { self.stop(); } }

fn connection() -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::connection::Builder::session().map_err(|e| e.to_string())?
        .method_timeout(Duration::from_millis(250)).build().map_err(|e| e.to_string())
}
fn owner(connection: &zbus::blocking::Connection, name: &str) -> Result<String, String> {
    if !valid_player_target(name) { return Err("Invalid media player destination".into()); }
    // Resolve an existing owner, then address that unique connection. This never
    // activates a closed player or redirects a command after a name is reused.
    zbus::blocking::Proxy::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
        .map_err(|e| e.to_string())?.call("GetNameOwner", &(name,)).map_err(|e| e.to_string())
}
fn list_players() -> Result<Vec<Player>, String> {
    let bus = connection()?;
    let names: Vec<String> = zbus::blocking::Proxy::new(&bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
        .map_err(|e| e.to_string())?.call("ListNames", &()).map_err(|e| e.to_string())?;
    let mut names: Vec<_> = names.into_iter().filter(|n| valid_player_target(n)).collect();
    names.sort();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut players = Vec::new();
    for name in names.into_iter().take(32) {
        let Ok(destination) = owner(&bus, &name) else { continue; };
        let identity = if Instant::now() < deadline {
            zbus::blocking::Proxy::new(&bus, destination.as_str(), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2")
                .ok().and_then(|proxy| proxy.get_property::<String>("Identity").ok())
        } else { None };
        let identity = identity.map(|s| s.chars().filter(|c| !c.is_control()).take(80).collect::<String>()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| name.strip_prefix("org.mpris.MediaPlayer2.").unwrap_or(&name).into());
        players.push(Player { name, identity });
    }
    Ok(players)
}

fn volume_after(volume: f64, command: PlayerCommand) -> Option<f64> {
    if !volume.is_finite() || volume < 0.0 { return None; }
    match command {
        PlayerCommand::VolumeUp => Some(if volume >= 1.0 { volume } else { (volume + 0.05).min(1.0) }),
        PlayerCommand::VolumeDown => Some((volume - 0.05).max(0.0)),
        _ => None,
    }
}
fn execute(bus: &zbus::blocking::Connection, name: &str, command: PlayerCommand, expires: Instant) -> Result<(), String> {
    if Instant::now() >= expires { return Err("Expired media command".into()); }
    let destination = owner(bus, name)?;
    let proxy = zbus::blocking::Proxy::new(bus, destination.as_str(), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").map_err(|e| e.to_string())?;
    if !proxy.get_property::<bool>("CanControl").map_err(|e| e.to_string())? { return Err("Player control is unavailable".into()); }
    let (method, capability) = match command {
        PlayerCommand::PlayPause => {
            let playing = proxy.get_property::<String>("PlaybackStatus").map_err(|e| e.to_string())? == "Playing";
            ("PlayPause", Some(if playing { "CanPause" } else { "CanPlay" }))
        }
        PlayerCommand::Next => ("Next", Some("CanGoNext")),
        PlayerCommand::Previous => ("Previous", Some("CanGoPrevious")),
        PlayerCommand::Stop => ("Stop", None),
        PlayerCommand::VolumeUp | PlayerCommand::VolumeDown => {
            let volume = proxy.get_property::<f64>("Volume").map_err(|e| e.to_string())?;
            let next = volume_after(volume, command).ok_or("Invalid player volume")?;
            if Instant::now() >= expires { return Err("Expired media command".into()); }
            if next != volume { proxy.set_property("Volume", next).map_err(|e| e.to_string())?; }
            return Ok(());
        }
    };
    if let Some(capability) = capability {
        if !proxy.get_property::<bool>(capability).map_err(|e| e.to_string())? { return Err("Player does not support this command".into()); }
    }
    if Instant::now() >= expires { return Err("Expired media command".into()); }
    proxy.call::<_, _, ()>(method, &()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_steps_do_not_boost_or_jump_existing_boosted_volume() {
        assert_eq!(volume_after(0.5, PlayerCommand::VolumeUp), Some(0.55));
        assert_eq!(volume_after(0.98, PlayerCommand::VolumeUp), Some(1.0));
        assert_eq!(volume_after(0.02, PlayerCommand::VolumeDown), Some(0.0));
        assert_eq!(volume_after(2.0, PlayerCommand::VolumeDown), Some(1.95));
        assert_eq!(volume_after(2.0, PlayerCommand::VolumeUp), Some(2.0));
        for value in [f64::NAN, f64::INFINITY, -1.0] { assert!(volume_after(value, PlayerCommand::VolumeUp).is_none()); }
    }
    #[test]
    fn unused_and_stopped_service_never_start_a_worker() {
        let service = Service::default();
        assert!(service.commands.lock().is_none());
        service.send("", PlayerCommand::PlayPause);
        assert!(service.commands.lock().is_none());
        service.stop();
        service.send("org.mpris.MediaPlayer2.vlc", PlayerCommand::PlayPause);
        service.refresh();
        assert!(service.commands.lock().is_none());
        assert!(!service.snapshot().refreshing);
    }
    struct MockRoot;
    #[zbus::interface(name = "org.mpris.MediaPlayer2")]
    impl MockRoot {
        #[zbus(property)]
        fn identity(&self) -> &str { "Test player" }
    }
    struct MockState { calls: Vec<&'static str>, volume: f64, can_control: bool }
    struct MockPlayer(Arc<Mutex<MockState>>);
    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl MockPlayer {
        fn play_pause(&self) { self.0.lock().calls.push("PlayPause"); }
        fn next(&self) { self.0.lock().calls.push("Next"); }
        fn previous(&self) { self.0.lock().calls.push("Previous"); }
        fn stop(&self) { self.0.lock().calls.push("Stop"); }
        #[zbus(property)]
        fn volume(&self) -> f64 { self.0.lock().volume }
        #[zbus(property)]
        fn set_volume(&self, value: f64) { self.0.lock().volume = value; }
        #[zbus(property)]
        fn can_control(&self) -> bool { self.0.lock().can_control }
        #[zbus(property)]
        fn can_go_next(&self) -> bool { true }
        #[zbus(property)]
        fn can_go_previous(&self) -> bool { true }
        #[zbus(property)]
        fn can_play(&self) -> bool { true }
        #[zbus(property)]
        fn can_pause(&self) -> bool { true }
        #[zbus(property)]
        fn playback_status(&self) -> &str { "Playing" }
    }
    fn mock(name: &str) -> (zbus::blocking::Connection, Arc<Mutex<MockState>>) {
        let state = Arc::new(Mutex::new(MockState { calls: Vec::new(), volume: 0.5, can_control: true }));
        let bus = zbus::blocking::connection::Builder::session().unwrap().name(name).unwrap()
            .serve_at("/org/mpris/MediaPlayer2", MockRoot).unwrap()
            .serve_at("/org/mpris/MediaPlayer2", MockPlayer(state.clone())).unwrap().build().unwrap();
        (bus, state)
    }

    #[test]
    #[ignore = "requires an explicitly enabled isolated D-Bus session"]
    fn selected_player_commands_never_fall_back_to_another_player() {
        assert_eq!(std::env::var("DSZ_TEST_MPRIS").as_deref(), Ok("1"));
        let name_a = format!("org.mpris.MediaPlayer2.dualwave_test_a_{}", std::process::id());
        let name_b = format!("org.mpris.MediaPlayer2.dualwave_test_b_{}", std::process::id());
        let (first, a) = mock(&name_a);
        let (_second, b) = mock(&name_b);
        let client = connection().unwrap();
        let discovered = list_players().unwrap();
        assert!(discovered.iter().any(|p| p.name == name_a && p.identity == "Test player"));
        assert!(discovered.iter().any(|p| p.name == name_b));
        let run = |name: &str, command| execute(&client, name, command, Instant::now() + Duration::from_secs(2));
        for command in [PlayerCommand::PlayPause, PlayerCommand::Next, PlayerCommand::Previous, PlayerCommand::Stop] { run(&name_a, command).unwrap(); }
        assert_eq!(a.lock().calls, ["PlayPause", "Next", "Previous", "Stop"]);
        assert!(b.lock().calls.is_empty());
        run(&name_a, PlayerCommand::VolumeUp).unwrap();
        assert_eq!(a.lock().volume, 0.55);
        assert_eq!(b.lock().volume, 0.5);
        run(&name_a, PlayerCommand::VolumeDown).unwrap();
        assert_eq!(a.lock().volume, 0.5);
        a.lock().can_control = false;
        assert!(run(&name_a, PlayerCommand::Next).is_err());
        assert_eq!(a.lock().calls.len(), 4);
        assert!(execute(&client, &name_b, PlayerCommand::Next, Instant::now() - Duration::from_secs(1)).is_err());
        first.close().unwrap();
        assert!(run(&name_a, PlayerCommand::Next).is_err());
        assert!(b.lock().calls.is_empty(), "closing the selected player must not choose another one");
        let service = Service::default();
        service.send(&name_b, PlayerCommand::Next);
        let deadline = Instant::now() + Duration::from_secs(2);
        while b.lock().calls.is_empty() { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
        service.refresh();
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.snapshot().refreshing { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
        assert!(service.snapshot().players.iter().any(|p| p.name == name_b));
        assert!(!service.snapshot().players.iter().any(|p| p.name == name_a));
        service.stop();
        service.send(&name_b, PlayerCommand::PlayPause);
        assert_eq!(b.lock().calls, ["Next"]);
        assert!(service.commands.lock().is_none());
    }

}
