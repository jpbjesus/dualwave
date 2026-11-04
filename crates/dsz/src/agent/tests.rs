use super::*;
use serde_json::json;

pub(super) fn engine(test: &str) -> std::sync::Arc<crate::engine::Engine> {
    let dir = std::env::temp_dir().join(format!("dualwave-agent-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::engine::Engine::new(dir)
}

#[test]
fn agents_are_opt_in_and_old_settings_keep_them_off() {
    let settings: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert!(!settings.agent_enabled);
}

#[test]
fn macro_updates_live_profile_and_survives_reload_without_losing_other_fields() {
    let e = engine("macro");
    let name = e.profiles.names()[0].clone();
    let before = e.profiles.find(&name).unwrap();
    let rev = e.profiles.revision();
    let mapping = json!({"button":"Ps", "mode":"Tap", "action":{"kind":"macro", "steps":[
        {"input":{"kind":"key","vk":9,"alt":true},"hold_ms":50,"wait_ms":50}
    ]}});
    let result = execute(&e, "set_mapping", json!({"profile":name,"expected_revision":rev,"mapping":mapping})).unwrap();
    assert_eq!(result["index"], before.mappings.len());
    let profile = e.profiles.find(&name).unwrap();
    assert_eq!(profile.mappings.len(), before.mappings.len() + 1);
    assert_eq!(profile.lighting, before.lighting);
    assert!(e.profiles.revision() > rev);
    e.flush();
    let store = crate::profile::ProfileStore::load(&e.data_dir.join("profiles"));
    assert_eq!(*store.find(&name).unwrap(), *profile);
    let _ = std::fs::remove_dir_all(&e.data_dir);
}

#[test]
fn stale_unknown_and_unsafe_changes_leave_profile_unchanged() {
    let e = engine("validation");
    let name = e.profiles.names()[0].clone();
    let before = e.profiles.find(&name).unwrap();
    let rev = e.profiles.revision();
    for patch in [json!({"typo":true}), json!({"name":"other"}), json!({"mappings":[{"button":"Ps","action":{"kind":"run","path":"bad.exe"}}]})] {
        assert!(execute(&e, "patch_profile", json!({"profile":name,"expected_revision":rev,"patch":patch})).is_err());
    }
    assert!(execute(&e, "patch_profile", json!({"profile":name,"expected_revision":rev-1,"patch":{}})).is_err());
    assert!(execute(&e, "set_mapping", json!({"profile":name,"expected_revision":rev,"mapping":{"action":{"kind":"key","vk":999}}})).is_err());
    assert_eq!(*e.profiles.find(&name).unwrap(), *before);
    let _ = std::fs::remove_dir_all(&e.data_dir);
}

#[test]
fn nested_unknown_mapping_fields_and_excessive_macros_are_rejected() {
    let e = engine("limits");
    let name = e.profiles.names()[0].clone();
    for mapping in [json!({"typo":true}), json!({"action":{"kind":"key","vk":9,"altt":true}}),
        json!({"action":{"kind":"macro","steps":vec![json!({"input":{"kind":"key","vk":9},"hold_ms":50,"wait_ms":50});129]}})] {
        assert!(execute(&e, "set_mapping", json!({"profile":name,"expected_revision":e.profiles.revision(),"mapping":mapping})).is_err());
    }
    let _ = std::fs::remove_dir_all(&e.data_dir);
}

#[test]
fn settings_patch_is_strict_and_cannot_enable_access_or_disable_crc() {
    let e = engine("settings");
    for patch in [json!({"agent_enabled":true}),json!({"check_input_crc":false}),json!({"start_with_windows":true}),json!({"typo":true})] {
        assert!(execute(&e,"patch_settings",json!({"expected_revision":e.settings_revision(),"patch":patch})).is_err());
    }
    execute(&e,"patch_settings",json!({"expected_revision":e.settings_revision(),"patch":{"show_tips":false}})).unwrap();
    assert!(!e.settings.read().show_tips);
    let _ = std::fs::remove_dir_all(&e.data_dir);
}

#[test]
fn profile_creation_patch_removal_and_conflict_preserve_unrelated_configuration() {
    let e = engine("operations");
    let source = e.profiles.names()[0].clone();
    execute(&e,"create_profile",json!({"name":"Agent test","source":source})).unwrap();
    let mut profile = e.profiles.find("Agent test").unwrap();
    assert!(execute(&e,"create_profile",json!({"name":"Agent test"})).is_err());
    let old = profile.clone();
    execute(&e,"patch_profile",json!({"profile":"Agent test","expected_revision":e.profiles.revision(),"patch":{"virtual":{"game_leds":!profile.virtual_out.game_leds}}})).unwrap();
    profile = e.profiles.find("Agent test").unwrap();
    assert_eq!(profile.triggers,old.triggers);
    assert_ne!(profile.virtual_out.game_leds,old.virtual_out.game_leds);
    execute(&e,"set_mapping",json!({"profile":"Agent test","expected_revision":e.profiles.revision(),"mapping":{"button":"Ps","action":{"kind":"key","vk":9,"alt":true}}})).unwrap();
    execute(&e,"remove_mapping",json!({"profile":"Agent test","expected_revision":e.profiles.revision(),"index":profile.mappings.len()})).unwrap();
    assert_eq!(*profile,*e.profiles.find("Agent test").unwrap());
    assert!(execute(&e,"apply_profile",json!({"profile":"Agent test","device_id":999})).is_err());
    let _ = std::fs::remove_dir_all(&e.data_dir);
}

#[test]
fn changing_action_kind_replaces_previous_variant_fields() {
    // A tagged variant changes as one value; fields of the old variant must not leak.
    let mut target = json!({"action":{"kind":"key","vk":9,"alt":true}});
    merge(&mut target,&json!({"action":{"kind":"mouse","button":"Left"}}));
    let m: Mapping = parse(target).unwrap();
    assert!(matches!(m.action,Action::Mouse {..}));
}

#[test]
fn settings_transaction_preserves_concurrent_background_bookkeeping() {
    let e = engine("transaction");
    let (start,started) = crossbeam_channel::bounded(1);
    let (done,finished) = crossbeam_channel::bounded(1);
    let worker = e.clone();
    let thread = std::thread::spawn(move || {
        started.recv().unwrap();
        worker.settings.write().last_controller = "background-controller".into();
        worker.settings_save_later();
        done.send(()).unwrap();
    });
    update_settings(&e,&json!({"expected_revision":e.settings_revision()}),|settings| {
        start.send(()).unwrap();
        assert!(finished.recv_timeout(std::time::Duration::from_millis(30)).is_err(),"Background writes must wait for the transaction");
        settings.show_tips = false;
        Ok(())
    }).unwrap();
    thread.join().unwrap();
    assert_eq!(e.settings.read().last_controller,"background-controller");
    assert!(!e.settings.read().show_tips);
    let _ = std::fs::remove_dir_all(&e.data_dir);
}
