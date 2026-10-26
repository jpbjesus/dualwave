//! Per-game profiles: watch running processes, switch controllers to the
//! game's profile while it runs, and switch back when it exits. The most
//! recently started game wins.
//!
//! Games without a rule are checked once for native DualSense support
//! (`dsdetect`); while one runs, controllers use the DualSense profile.
//! A Steam, GOG, or Xbox app game that does not have that support can
//! instead present the fallback virtual controller from settings.

use crate::settings::FallbackPad;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::Engine;

/// What the newest running game asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameTarget {
    /// Switch to this profile (a rule, or native DualSense support).
    Profile { profile: String, exe: String },
    /// Keep the current profile and present the fallback virtual controller.
    Fallback { exe: String },
}

impl GameTarget {
    fn exe(&self) -> &str {
        match self {
            GameTarget::Profile { exe, .. } | GameTarget::Fallback { exe } => exe,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionReason { #[default] Assigned, Rule, Native, Fallback, Manual, Disabled, MissingProfile }

impl SelectionReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Assigned => "Assigned profile", Self::Rule => "Game rule", Self::Native => "Native DualSense support",
            Self::Fallback => "Fallback controller", Self::Manual => "Manual choice for this session",
            Self::Disabled => "Automatic switching is off", Self::MissingProfile => "Game profile is missing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedGame { pub exe: String, pub native: bool }

#[derive(Clone, Debug, Default)]
pub struct GameStatus {
    pub exe: Option<String>,
    pub reason: SelectionReason,
    pub games: Vec<DetectedGame>,
    pub instances: Vec<(u32, String)>,
}

#[derive(Clone, Default)]
struct GameObservation {
    auto: bool,
    manual_revision: u64,
    target: Option<GameTarget>,
    running: Vec<(u32, String)>,
    games: Vec<DetectedGame>,
}

fn implicit_game_process(exe: &str) -> bool {
    !matches!(exe.to_ascii_lowercase().as_str(), "wine" | "wine64" | "wine-preloader" | "wine64-preloader" | "wineserver"
        | "steam" | "steam.exe" | "steamwebhelper" | "steamwebhelper.exe" | "services.exe" | "explorer.exe"
        | "winedevice.exe" | "plugplay.exe" | "rpcss.exe" | "conhost.exe" | "rundll32.exe")
}

fn order_by_start(stack: &mut [String], procs: &[(u32, String)], mut started: impl FnMut(u32) -> Option<std::time::SystemTime>) {
    let times: HashMap<String, _> = stack.iter().map(|exe| {
        let newest = procs.iter().filter(|(_, n)| n.eq_ignore_ascii_case(exe))
            .filter_map(|(pid, _)| started(*pid)).max();
        (exe.to_ascii_lowercase(), newest)
    }).collect();
    stack.sort_by_key(|exe| times.get(&exe.to_ascii_lowercase()).copied().flatten());
}

/// Commit a scan against current manual choices, never the state at scan start.
fn apply_observation(engine: &Engine, scan: GameObservation) {
    let _selection = engine.game_selection.lock();
    let auto = scan.auto && engine.settings.read().auto_profiles;
    let observed_exe = scan.target.as_ref().map(|t| t.exe().to_string());
    let mut target = if auto { scan.target } else { None };
    let mut reason = if auto { SelectionReason::Assigned } else { SelectionReason::Disabled };
    {
        let mut dismissed = engine.game_dismissed.lock();
        dismissed.retain(|exe, pids| scan.running.iter().any(|(pid, name)| name.eq_ignore_ascii_case(exe)
            && (pids.is_empty() || pids.contains(pid))));
        // The first scan may still be resolving files when a manual choice
        // is made, before there is any active override to dismiss.
        if scan.manual_revision != engine.manual_profile_revision() {
            if let Some(t) = &target {
                let pids = scan.running.iter().filter(|(_, name)| name.eq_ignore_ascii_case(t.exe()))
                    .map(|(pid, _)| *pid).collect();
                dismissed.insert(t.exe().to_ascii_lowercase(), pids);
            }
        }
        if let Some(t) = &target {
            if dismissed.contains_key(&t.exe().to_ascii_lowercase()) {
                reason = SelectionReason::Manual;
                target = None;
            }
        }
    }
    if let Some(GameTarget::Profile { profile, exe }) = &target {
        if !engine.profiles.exists(profile) { reason = SelectionReason::MissingProfile; target = None; }
        else {
            reason = if engine.settings.read().games.iter().any(|g| g.enabled && g.exe.trim().eq_ignore_ascii_case(exe)) {
                SelectionReason::Rule
            } else { SelectionReason::Native };
        }
    } else if matches!(target, Some(GameTarget::Fallback { .. })) { reason = SelectionReason::Fallback; }
    let (next_override, next_fallback) = match &target {
        Some(GameTarget::Profile { profile, exe }) => (Some((profile.clone(), exe.clone())), None),
        Some(GameTarget::Fallback { exe }) => (None, Some(exe.clone())),
        None => (None, None),
    };
    let prev_override = engine.game_override.read().clone();
    let prev_fallback = engine.fallback_exe.read().clone();
    let exe = observed_exe.or_else(|| {
        scan.games.last().map(|g| g.exe.clone())
    });
    *engine.game_status.write() = GameStatus { exe, reason, games: scan.games, instances: scan.running };
    if next_override == prev_override && next_fallback == prev_fallback { return; }
    match &target {
        Some(GameTarget::Profile { profile, exe }) => {
            log::info!("{exe}: switching to profile \"{profile}\" ({reason:?})");
            engine.toast(crate::localized_format!("{exe} detected — profile \"{profile}\"", "{exe} detectado — perfil \"{profile}\"", "{exe} detectado — perfil \"{profile}\""; "{exe} detetado — perfil \"{profile}\""));
        }
        Some(GameTarget::Fallback { exe }) => {
            let fallback = engine.settings.read().fallback_pad;
            engine.toast(crate::localized_format!("{exe} — virtual {}", "{exe} — virtual {}", "{exe} — mando virtual {}"; "{exe} — virtual {}", fallback.label()));
        }
        None => {
            let back = engine.device_list().first().map(|d| engine.assigned_profile(d))
                .unwrap_or_else(|| engine.settings.read().default_profile.clone());
            engine.toast(crate::localized_format!("Automatic profile ended — back to \"{back}\"", "Perfil automático encerrado — voltando para \"{back}\"", "Finalizó el perfil automático — se vuelve a \"{back}\""; "Perfil automático terminado — a regressar a \"{back}\""));
        }
    }
    *engine.game_override.write() = next_override;
    *engine.fallback_exe.write() = next_fallback;
    engine.bump_config();
    for d in engine.device_list() { d.identify_light(); }
}

/// One scan of a process that has no game rule.
struct Checked {
    exe: String,
    /// Why it counts as a DualSense game, when it does.
    dualsense: Option<String>,
    /// Installed under Steam, GOG, or the Xbox app.
    likely: bool,
    /// Lowercase directory of the exe, for ignoring a launcher beside a
    /// DualSense game (Elden Ring's `start_protected_game.exe`).
    dir: Option<String>,
}

/// A store install with no DualSense support of its own, and not sitting
/// next to one that has it.
fn is_plain_game(likely: bool, dir: Option<&str>, ds_dirs: &HashSet<String>) -> bool {
    likely && !dir.is_some_and(|d| ds_dirs.contains(d))
}

/// `top` is the most recently started game still running.
pub fn pick_target(
    auto: bool,
    top: Option<&str>,
    rules: &[(&str, &str)],
    ds_exes: &[&str],
    ds_profile: &str,
    ds_profile_ok: bool,
    plain_exes: &[&str],
) -> Option<GameTarget> {
    if !auto {
        return None;
    }
    let exe = top?;
    if let Some((_, profile)) = rules.iter().find(|(e, _)| e.eq_ignore_ascii_case(exe)) {
        return Some(GameTarget::Profile {
            profile: (*profile).to_string(),
            exe: exe.to_string(),
        });
    }
    if ds_profile_ok && ds_exes.iter().any(|e| e.eq_ignore_ascii_case(exe)) {
        return Some(GameTarget::Profile {
            profile: ds_profile.to_string(),
            exe: exe.to_string(),
        });
    }
    if plain_exes.iter().any(|e| e.eq_ignore_ascii_case(exe)) {
        return Some(GameTarget::Fallback {
            exe: exe.to_string(),
        });
    }
    None
}

/// Store apps that are not games and often run all the time (a wallpaper,
/// a frame-generation overlay, VR runtime, redistributable installers).
/// Folder names under `steamapps\common\`, lowercase.
const NOT_GAMES: &[&str] = &[
    "wallpaper_engine",
    "lossless scaling",
    "steamvr",
    "steamworks shared",
    "steam controller configs",
    "ovr toolkit",
    "ovr advanced settings",
    "fpsvr",
    "soundpad",
    "voicemod",
];

/// Steam, GOG, and Xbox app installs. Other programs keep the profile's
/// virtual controller; a rule covers those.
pub fn is_likely_game(path: &str) -> bool {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    if p.contains("\\_commonredist\\") {
        return false;
    }
    if let Some(i) = p.find("\\steamapps\\common\\") {
        let rest = &p[i + "\\steamapps\\common\\".len()..];
        let folder = rest.split('\\').next().unwrap_or("");
        return !NOT_GAMES.contains(&folder);
    }
    p.contains("\\gog games\\") || p.contains("\\xboxgames\\")
}

/// Re-association enables one rule and removes ambiguous duplicate entries.
pub fn associate_rule(rules: &mut Vec<crate::settings::GameRule>, exe: &str, profile: &str) -> bool {
    let exe = exe.trim();
    if exe.is_empty() || profile.trim().is_empty() || exe.contains(['/', '\\']) { return false; }
    let before = rules.clone();
    if let Some(rule) = rules.iter_mut().find(|g| g.exe.trim().eq_ignore_ascii_case(exe)) {
        rule.enabled = true;
        rule.profile = profile.into();
    } else {
        rules.push(crate::settings::GameRule { enabled: true, exe: exe.into(), profile: profile.into() });
    }
    let mut seen = false;
    rules.retain(|g| {
        if !g.exe.trim().eq_ignore_ascii_case(exe) { return true; }
        if seen { return false; }
        seen = true; true
    });
    *rules != before
}

pub fn run(engine: Arc<Engine>) {
    let mut stack: Vec<String> = Vec::new();
    // Process id -> scan result, checked once per process.
    let mut checked: HashMap<u32, Checked> = HashMap::new();
    while !engine.shutdown.load(Ordering::Acquire) {
        let manual_revision = engine.manual_profile_revision();
        let procs = crate::platform::process_list();
        let names: std::collections::HashSet<String> =
            procs.iter().map(|(_, n)| n.clone()).collect();

        let (auto, rules, auto_ds, ds_profile, fallback_pad) = {
            let s = engine.settings.read();
            (
                s.auto_profiles,
                s.games.clone(),
                s.auto_dualsense,
                s.dualsense_profile.clone(),
                s.fallback_pad,
            )
        };
        let fallback_on = matches!(fallback_pad, FallbackPad::Xbox360 | FallbackPad::DualShock4);
        checked.retain(|pid, c| procs.iter().any(|(p, name)| p == pid && name.eq_ignore_ascii_case(&c.exe)));
        let discovery_requested = engine.take_game_discovery_request();
        // Games without a rule. Programs already running when the app
        // starts are checked too, so a game started first still counts.
        let mut ds_games: Vec<(String, String)> = Vec::new();
        let mut plain_games: Vec<String> = Vec::new();
        if discovery_requested || (auto && (auto_ds || fallback_on)) {
            for (pid, name) in &procs {
                if rules
                    .iter()
                    .any(|g| g.enabled && g.exe.trim().eq_ignore_ascii_case(name))
                {
                    continue;
                }
                if !implicit_game_process(name) { continue; }
                checked.entry(*pid).or_insert_with(|| {
                    let path = crate::platform::process_path(*pid);
                    let dir = path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|d| d.to_string_lossy().to_ascii_lowercase().replace('/', "\\"));
                    Checked {
                        exe: name.clone(),
                        dualsense: path.as_ref().and_then(|p| engine.dsdetect.check(p)),
                        likely: path
                            .as_ref()
                            .is_some_and(|p| is_likely_game(&p.to_string_lossy())),
                        dir,
                    }
                });
            }
            let ds_dirs: HashSet<String> = checked
                .values()
                .filter(|c| c.dualsense.is_some())
                .filter_map(|c| c.dir.clone())
                .collect();
            for (pid, name) in &procs {
                if rules
                    .iter()
                    .any(|g| g.enabled && g.exe.trim().eq_ignore_ascii_case(name))
                {
                    continue;
                }
                if !implicit_game_process(name) { continue; }
                let Some(hit) = checked.get(pid) else {
                    continue;
                };
                if let Some(why) = &hit.dualsense {
                    if auto_ds {
                        ds_games.push((name.clone(), why.clone()));
                    }
                } else if fallback_on && is_plain_game(hit.likely, hit.dir.as_deref(), &ds_dirs) {
                    plain_games.push(name.clone());
                }
            }
        }
        let running: Vec<&crate::settings::GameRule> = rules
            .iter()
            .filter(|g| g.enabled && !g.exe.trim().is_empty())
            .filter(|g| names.contains(&g.exe.trim().to_ascii_lowercase()))
            .collect();
        let live = |exe: &str| {
            running
                .iter()
                .any(|g| g.exe.trim().eq_ignore_ascii_case(exe))
                || ds_games.iter().any(|(n, _)| n.eq_ignore_ascii_case(exe))
                || plain_games.iter().any(|n| n.eq_ignore_ascii_case(exe))
        };
        stack.retain(|exe| live(exe));
        for exe in running
            .iter()
            .map(|g| g.exe.trim().to_string())
            .chain(ds_games.iter().map(|(n, _)| n.clone()))
            .chain(plain_games.iter().cloned())
        {
            if !stack.iter().any(|e| e.eq_ignore_ascii_case(&exe)) {
                stack.push(exe);
            }
        }
        order_by_start(&mut stack, &procs, crate::platform::process_started);
        let mut detected: Vec<String> = procs.iter().filter(|(pid, name)| {
            running.iter().any(|g| g.exe.trim().eq_ignore_ascii_case(name))
                || checked.get(pid).is_some_and(|c| c.likely || c.dualsense.is_some())
        }).map(|(_, name)| name.clone()).collect();
        detected.sort(); detected.dedup();
        order_by_start(&mut detected, &procs, crate::platform::process_started);
        let detected = detected.into_iter().map(|exe| {
            let native = procs.iter().filter(|(_, name)| name.eq_ignore_ascii_case(&exe))
                .any(|(pid, _)| checked.get(pid).is_some_and(|c| c.dualsense.is_some()));
            DetectedGame { exe, native }
        }).collect();
        let rules_top: Vec<(&str, &str)> = rules
            .iter()
            .filter(|g| g.enabled)
            .map(|g| (g.exe.trim(), g.profile.as_str()))
            .collect();
        let ds_exes: Vec<&str> = ds_games.iter().map(|(n, _)| n.as_str()).collect();
        let plain_exes: Vec<&str> = plain_games.iter().map(String::as_str).collect();
        let target = pick_target(
            auto,
            stack.last().map(String::as_str),
            &rules_top,
            &ds_exes,
            &ds_profile,
            engine.profiles.exists(&ds_profile),
            &plain_exes,
        );
        apply_observation(&engine, GameObservation { auto, manual_revision, target, running: procs, games: detected });
        let wait = if rules.is_empty() && !auto_ds && !fallback_on {
            3000
        } else {
            1500
        };
        std::thread::sleep(Duration::from_millis(wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(engine: &Engine, exe: &str, profile: &str) -> GameObservation {
        GameObservation {
            auto: true,
            manual_revision: engine.manual_profile_revision(),
            target: Some(GameTarget::Profile { profile: profile.into(), exe: exe.into() }),
            running: vec![(42, exe.into())],
            games: vec![DetectedGame { exe: exe.into(), native: false }],
        }
    }

    #[test]
    fn game_selection_restores_assignment_and_manual_choices_survive_stale_scans() {
        let dir = std::env::temp_dir().join(format!("dualwave-game-selection-{}", std::process::id()));
        let engine = Engine::new(dir.clone());
        let device = crate::device::Device::for_test(ds_proto::Connection::Bluetooth, "00:11:22:33:44:55");
        engine.set_device_profile(&device, "Xbox");
        engine.settings.write().games.push(crate::settings::GameRule { enabled: true, exe: "game.exe".into(), profile: "DualSense".into() });
        let scan = observation(&engine, "game.exe", "DualSense");
        apply_observation(&engine, scan.clone());
        assert_eq!(engine.profile_name_for(&device), "DualSense");
        assert_eq!(engine.game_status.read().reason, SelectionReason::Rule);
        apply_observation(&engine, GameObservation::default());
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        apply_observation(&engine, scan.clone());
        engine.set_device_profile(&device, "Xbox");
        // A scan computed before the manual choice must still be suppressed.
        apply_observation(&engine, scan);
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        assert_eq!(engine.game_status.read().reason, SelectionReason::Manual);
        apply_observation(&engine, GameObservation::default());
        assert!(engine.game_dismissed.lock().is_empty());
        apply_observation(&engine, observation(&engine, "game.exe", "DualSense"));
        assert_eq!(engine.profile_name_for(&device), "DualSense");
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_choice_during_the_first_scan_wins_before_any_override_exists() {
        let dir = std::env::temp_dir().join(format!("dualwave-first-game-scan-{}", std::process::id()));
        let engine = Engine::new(dir.clone());
        let device = crate::device::Device::for_test(ds_proto::Connection::Bluetooth, "00:11:22:33:44:55");
        let in_flight = observation(&engine, "game.exe", "DualSense");
        engine.set_device_profile(&device, "Xbox");
        apply_observation(&engine, in_flight);
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        assert_eq!(engine.game_status.read().reason, SelectionReason::Manual);
        assert!(engine.game_dismissed.lock().contains_key("game.exe"));
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn associations_update_and_enable_one_case_insensitive_rule() {
        use crate::settings::GameRule;
        let mut rules = vec![GameRule { enabled: false, exe: "GAME.EXE".into(), profile: "Old".into() },
            GameRule { enabled: true, exe: "game.exe".into(), profile: "Duplicate".into() }];
        assert!(associate_rule(&mut rules, " game.exe ", "DualSense"));
        assert_eq!(rules.len(), 1);
        assert!(rules[0].enabled);
        assert_eq!(rules[0].profile, "DualSense");
        assert!(!associate_rule(&mut rules, "Game.exe", "DualSense"));
        assert!(!associate_rule(&mut rules, "/path/game.exe", "Xbox"));
        assert!(!associate_rule(&mut rules, "", "Xbox"));
    }

    #[test]
    fn manual_priority_tracks_each_game_instance_and_expires_on_relaunch() {
        let dir = std::env::temp_dir().join(format!("dualwave-manual-game-instances-{}", std::process::id()));
        let engine = Engine::new(dir.clone());
        let device = crate::device::Device::for_test(ds_proto::Connection::Bluetooth, "00:11:22:33:44:55");
        apply_observation(&engine, observation(&engine, "first.exe", "DualSense"));
        engine.set_device_profile(&device, "Xbox");
        let mut second = observation(&engine, "second.exe", "DualSense");
        second.running = vec![(42, "first.exe".into()), (99, "second.exe".into())];
        apply_observation(&engine, second);
        engine.set_device_profile(&device, "Xbox");
        assert_eq!(engine.game_dismissed.lock().len(), 2);
        apply_observation(&engine, observation(&engine, "first.exe", "DualSense"));
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        assert_eq!(engine.game_status.read().reason, SelectionReason::Manual);
        let mut relaunched = observation(&engine, "first.exe", "DualSense");
        relaunched.running[0].0 = 123;
        apply_observation(&engine, relaunched);
        assert_eq!(engine.profile_name_for(&device), "DualSense");
        assert!(engine.game_dismissed.lock().is_empty());
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn automatic_selection_never_applies_a_missing_profile_or_runs_when_disabled() {
        let dir = std::env::temp_dir().join(format!("dualwave-invalid-game-profile-{}", std::process::id()));
        let engine = Engine::new(dir.clone());
        apply_observation(&engine, observation(&engine, "game.exe", "Missing profile"));
        assert!(engine.game_override.read().is_none());
        assert_eq!(engine.game_status.read().reason, SelectionReason::MissingProfile);
        let mut disabled = observation(&engine, "game.exe", "DualSense"); disabled.auto = false;
        apply_observation(&engine, disabled);
        assert!(engine.game_override.read().is_none());
        assert_eq!(engine.game_status.read().reason, SelectionReason::Disabled);
        drop(engine);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn process_start_time_decides_priority_instead_of_rule_order() {
        let mut stack = vec!["new.exe".to_string(), "old.exe".to_string()];
        let procs = vec![(20, "old.exe".to_string()), (10, "new.exe".to_string())];
        order_by_start(&mut stack, &procs, |pid| Some(std::time::UNIX_EPOCH + Duration::from_secs(if pid == 10 { 200 } else { 100 })));
        assert_eq!(stack.last().unwrap(), "new.exe");
        assert!(!implicit_game_process("wineserver"));
        assert!(!implicit_game_process("explorer.exe"));
        assert!(implicit_game_process("eldenring.exe"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn subprocess_game_lifecycle_uses_discovery_priority_and_session_selection() {
        use std::process::{Child, Command};
        struct Game(Child);
        impl Drop for Game {
            fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
        }
        let dir = std::env::temp_dir().join(format!("dualwave-subprocess-games-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Copies give /proc/exe distinct names; shell argv aliases would not.
        for name in ["first.exe", "second.exe"] { std::fs::copy("/bin/sleep", dir.join(name)).unwrap(); }
        let launch = |name: &str| Game(Command::new(dir.join(name)).arg("120").spawn().unwrap());
        let first = launch("first.exe");
        std::thread::sleep(Duration::from_millis(1100));
        let second = launch("second.exe");
        let engine = Engine::new(dir.join("settings"));
        let device = crate::device::Device::for_test(ds_proto::Connection::Bluetooth, "00:11:22:33:44:55");
        engine.set_device_profile(&device, "Xbox");
        let scan = |pids: &[u32]| {
            let running: Vec<_> = crate::platform::process_list().into_iter().filter(|(pid, _)| pids.contains(pid)).collect();
            assert_eq!(running.len(), pids.len());
            let mut stack: Vec<_> = running.iter().map(|(_, name)| name.clone()).collect();
            order_by_start(&mut stack, &running, crate::platform::process_started);
            let target = pick_target(true, stack.last().map(String::as_str),
                &[("first.exe", "DualSense"), ("second.exe", "Xbox")], &[], "DualSense", true, &[]);
            GameObservation { auto: true, manual_revision: engine.manual_profile_revision(), target, running, games: Vec::new() }
        };
        apply_observation(&engine, scan(&[first.0.id(), second.0.id()]));
        assert_eq!(engine.game_status.read().exe.as_deref(), Some("second.exe"));
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        drop(second);
        apply_observation(&engine, scan(&[first.0.id()]));
        assert_eq!(engine.profile_name_for(&device), "DualSense");
        engine.set_device_profile(&device, "Xbox");
        apply_observation(&engine, scan(&[first.0.id()]));
        assert_eq!(engine.game_status.read().reason, SelectionReason::Manual);
        let old_pid = first.0.id();
        drop(first);
        assert!(!crate::platform::process_list().iter().any(|(pid, _)| *pid == old_pid));
        apply_observation(&engine, scan(&[]));
        assert_eq!(engine.profile_name_for(&device), "Xbox");
        let relaunched = launch("first.exe");
        apply_observation(&engine, scan(&[relaunched.0.id()]));
        assert_eq!(engine.profile_name_for(&device), "DualSense");
        drop(relaunched);
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn likely_games_are_store_installs() {
        assert!(is_likely_game(
            r"C:\Program Files (x86)\Steam\steamapps\common\Dead Cells\deadcells.exe"
        ));
        assert!(is_likely_game(r"D:\GOG Games\Hades\Hades.exe"));
        assert!(is_likely_game(r"E:\XboxGames\Forza\Content\forza.exe"));
        assert!(!is_likely_game(r"C:\Program Files (x86)\Steam\steam.exe"));
        assert!(!is_likely_game(r"C:\Windows\System32\svchost.exe"));
        assert!(!is_likely_game(r"C:\Games\portable\game.exe"));
        // Store apps that run in the background are not games.
        assert!(!is_likely_game(
            r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\wallpaper64.exe"
        ));
        assert!(!is_likely_game(
            r"D:\SteamLibrary\steamapps\common\SteamVR\bin\win64\vrserver.exe"
        ));
        assert!(!is_likely_game(
            r"D:\SteamLibrary\steamapps\common\Lossless Scaling\LosslessScaling.exe"
        ));
        assert!(!is_likely_game(
            r"D:\SteamLibrary\steamapps\common\Hades\_CommonRedist\vcredist\2019\VC_redist.x64.exe"
        ));
    }

    #[test]
    fn launcher_beside_a_dualsense_game_is_not_a_fallback() {
        let mut dirs = HashSet::new();
        dirs.insert(r"c:\steam\steamapps\common\elden ring".into());
        assert!(!is_plain_game(
            true,
            Some(r"c:\steam\steamapps\common\elden ring"),
            &dirs
        ));
        assert!(is_plain_game(
            true,
            Some(r"c:\steam\steamapps\common\dead cells"),
            &dirs
        ));
        assert!(!is_plain_game(
            false,
            Some(r"c:\steam\steamapps\common\dead cells"),
            &dirs
        ));
    }

    #[test]
    fn newest_game_picks_the_pad() {
        let rules = [("eldenring.exe", "Elden")];
        let ds = ["bg3.exe"];
        let plain = ["deadcells.exe"];
        assert_eq!(
            pick_target(
                false,
                Some("deadcells.exe"),
                &rules,
                &ds,
                "DualSense",
                true,
                &plain
            ),
            None
        );
        assert_eq!(
            pick_target(
                true,
                Some("deadcells.exe"),
                &rules,
                &ds,
                "DualSense",
                true,
                &plain
            ),
            Some(GameTarget::Fallback {
                exe: "deadcells.exe".into()
            })
        );
        assert_eq!(
            pick_target(
                true,
                Some("bg3.exe"),
                &rules,
                &ds,
                "DualSense",
                true,
                &plain
            ),
            Some(GameTarget::Profile {
                profile: "DualSense".into(),
                exe: "bg3.exe".into()
            })
        );
        assert_eq!(
            pick_target(
                true,
                Some("BG3.EXE"),
                &rules,
                &ds,
                "DualSense",
                false,
                &plain
            ),
            None,
            "no DualSense profile to switch to"
        );
        assert_eq!(
            pick_target(
                true,
                Some("eldenring.exe"),
                &rules,
                &ds,
                "DualSense",
                true,
                &plain
            ),
            Some(GameTarget::Profile {
                profile: "Elden".into(),
                exe: "eldenring.exe".into()
            })
        );
        assert_eq!(
            pick_target(
                true,
                Some("chrome.exe"),
                &rules,
                &ds,
                "DualSense",
                true,
                &plain
            ),
            None
        );
    }
}
