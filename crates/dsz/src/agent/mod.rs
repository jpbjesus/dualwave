//! Shared configuration operations. The listener holds Engine::config_edit so
//! a UI frame cannot overwrite an agent edit with an older profile snapshot.

use serde_json::{json, Value};
use crate::engine::Engine;
use crate::profile::{Action, MacroInput, Mapping, Profile, PlayerCommand};

mod transport;
mod mcp;
pub use transport::Server;

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Expected string: {key}"))
}

fn revision(args: &Value, current: u64) -> Result<(), String> {
    if args.get("expected_revision").and_then(Value::as_u64) == Some(current) { Ok(()) }
    else { Err("Revision conflict: read the current configuration and retry with its revision".into()) }
}

fn known_fields(input: &Value, output: &Value) -> Result<(), String> {
    match (input, output) {
        (Value::Object(a), Value::Object(b)) => for (k, v) in a {
            known_fields(v, b.get(k).ok_or_else(|| format!("Unknown field: {k}"))?)?;
        },
        (Value::Array(a), Value::Array(b)) => for (a, b) in a.iter().zip(b) { known_fields(a, b)?; },
        _ => {}
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned + serde::Serialize>(value: Value) -> Result<T, String> {
    let parsed: T = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    known_fields(&value, &serde_json::to_value(&parsed).map_err(|e| e.to_string())?)?;
    Ok(parsed)
}

fn merge(target: &mut Value, patch: &Value) {
    if let Some(kind) = patch.get("kind") {
        if target.get("kind") != Some(kind) { *target = patch.clone(); return; }
    }
    if let (Value::Object(target), Value::Object(patch)) = (&mut *target, patch) {
        for (key, value) in patch {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    } else { *target = patch.clone(); }
}

fn validate_mapping(m: &Mapping) -> Result<(), String> {
    let key_ok = |vk: u16| (1..=254).contains(&vk);
    match &m.action {
        Action::Player { player, .. } if !crate::profile::valid_player_target(player) => return Err("Select a valid MPRIS player destination".into()),
        Action::Run { .. } => return Err("Executable actions cannot be created through agent access".into()),
        Action::Key { vk, .. } if !key_ok(*vk) => return Err("Key code must be between 1 and 254".into()),
        Action::Macro { steps } => {
            if steps.len() > 128 { return Err("A macro supports at most 128 steps".into()); }
            for step in steps {
                if step.hold_ms > 60000 || step.wait_ms > 60000 { return Err("Macro hold and pause must be at most 60000 ms".into()); }
                if let MacroInput::Key { vk, .. } = step.input {
                    if !key_ok(vk) { return Err("Key code must be between 1 and 254".into()); }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn update_settings(e: &Engine, args: &Value, edit: impl FnOnce(&mut crate::settings::Settings) -> Result<(),String>) -> Result<(),String> {
    let mut settings = e.settings.write();
    revision(args,e.settings_revision())?;
    let mut next = settings.clone();
    edit(&mut next)?;
    *settings = next;
    drop(settings);
    e.settings_changed();
    Ok(())
}

pub fn execute(e: &Engine, operation: &str, args: Value) -> Result<Value, String> {
    let tool = tools().into_iter().find(|t| t["name"] == operation).ok_or("Unknown operation")?;
    if !args.is_object() { return Err("Arguments must be an object".into()); }
    let properties = tool["inputSchema"]["properties"].as_object().unwrap();
    if let Some(k) = args.as_object().unwrap().keys().find(|k| !properties.contains_key(*k)) { return Err(format!("Unknown argument: {k}")); }
    match operation {
        "describe" => Ok(json!({"tools":tools(),"default_profile":Profile::default(),"default_mapping":Mapping::default(),
            "buttons":ds_proto::input::Button::ALL.iter().map(|b| json!({"value":b,"label":b.short_name()})).collect::<Vec<_>>(),
            "keys":crate::inputsim::key_catalog().into_iter().map(|vk|json!({"vk":vk,"name":crate::inputsim::key_name(vk)})).collect::<Vec<_>>(),
            "game_volume_actions":[{"kind":"game_volume_up"},{"kind":"game_volume_down"}],
            "player_commands":PlayerCommand::ALL, "player_action_example":{"kind":"player","player":"org.mpris.MediaPlayer2.vlc","command":"PlayPause"},
            "writable_settings":WRITABLE_SETTINGS})),
        "list_devices" => Ok(json!({"devices":e.device_list().iter().map(|d| json!({"id":d.id,"name":d.info.product,"profile":e.profile_name_for(d)})).collect::<Vec<_>>()})),
        "list_profiles" => Ok(json!({"profiles":e.profiles.names(),"revision":e.profiles.revision()})),
        "get_settings" => Ok(json!({"settings":*e.settings.read(),"revision":e.settings_revision()})),
        "patch_settings" => {
            revision(&args, e.settings_revision())?;
            let patch = args.get("patch").filter(|v|v.is_object()).ok_or("Expected patch object")?;
            for k in patch.as_object().unwrap().keys() {
                if !WRITABLE_SETTINGS.contains(&k.as_str()) { return Err(format!("Setting is not writable through agent access: {k}")); }
            }
            update_settings(e,&args,|settings| {
                let mut value = serde_json::to_value(&*settings).unwrap();
                merge(&mut value, patch);
                let next: crate::settings::Settings = parse(value)?;
                if !(0.8..=1.6).contains(&next.ui_scale) || next.idle_off_minutes > 60 { return Err("Setting outside the supported range".into()); }
                if !e.profiles.exists(&next.default_profile) { return Err("Default profile does not exist".into()); }
                *settings = next;
                Ok(())
            })?;
            execute(e,"get_settings",json!({}))
        }
        "create_profile" => {
            let name = text(&args,"name")?;
            if name.trim().is_empty() || name.len() > 120 || crate::profile::sanitize_file_name(name) != name { return Err("Use a nonempty file-safe profile name (at most 120 bytes)".into()); }
            if e.profiles.exists(name) { return Err("Profile already exists".into()); }
            let mut profile = if let Some(source) = args.get("source") {
                (*e.profiles.find(source.as_str().ok_or("Expected source string")?).ok_or("Source profile not found")?).clone()
            } else { Profile::default() };
            profile.name = name.into();
            let name = e.profiles.add(profile);
            execute(e,"get_profile",json!({"profile":name}))
        }
        "get_profile" | "patch_profile" | "set_mapping" | "remove_mapping" | "apply_profile" => {
            let name = text(&args,"profile")?;
            let old = e.profiles.find(name).ok_or("Profile not found")?;
            if operation == "get_profile" { return Ok(json!({"profile":*old,"revision":e.profiles.revision()})); }
            if operation == "apply_profile" {
                let id = args.get("device_id").and_then(Value::as_u64).ok_or("Expected device_id")?;
                let device = e.device_list().into_iter().find(|d| d.id == id).ok_or("Device not connected")?;
                e.set_device_profile(&device,name);
                return Ok(json!({"device_id":id,"profile":name}));
            }
            revision(&args,e.profiles.revision())?;
            let mut profile = (*old).clone();
            let mut index_result = None;
            if operation == "patch_profile" {
                let patch = args.get("patch").filter(|v|v.is_object()).ok_or("Expected patch object")?;
                if patch.get("name").is_some() { return Err("Profile name cannot be patched".into()); }
                let mut value = serde_json::to_value(&profile).unwrap();
                merge(&mut value,patch);
                profile = parse(value)?;
                if profile.mappings != old.mappings {
                    for m in &profile.mappings { if !old.mappings.contains(m) { validate_mapping(m)?; } }
                }
            } else if operation == "set_mapping" {
                let mapping: Mapping = parse(args.get("mapping").cloned().ok_or("Expected mapping")?)?;
                validate_mapping(&mapping)?;
                let index = match args.get("index") {
                    None => profile.mappings.len(),
                    Some(v) => v.as_u64().and_then(|v|usize::try_from(v).ok()).ok_or("Expected nonnegative index")?,
                };
                if index > profile.mappings.len() { return Err("Mapping index out of range".into()); }
                if index == profile.mappings.len() { profile.mappings.push(mapping); }
                else { profile.mappings[index] = mapping; }
                index_result = Some(index);
            } else {
                let index = args.get("index").and_then(Value::as_u64).and_then(|v|usize::try_from(v).ok()).ok_or("Expected index")?;
                if index >= profile.mappings.len() { return Err("Mapping index out of range".into()); }
                profile.mappings.remove(index);
            }
            e.profiles.update(profile);
            Ok(json!({"profile":name,"revision":e.profiles.revision(),"index":index_result}))
        }
        _ => Err("Unknown operation".into())
    }
}

const WRITABLE_SETTINGS: &[&str] = &["language","appearance","color_theme","reduced_motion","show_tips","ui_scale","idle_off_minutes","default_profile","auto_profiles"];

pub fn tools() -> Vec<Value> {
    let string = json!({"type":"string"});
    let integer = json!({"type":"integer","minimum":0});
    let object = json!({"type":"object"});
    let definitions = [
        ("describe","Get configuration defaults, button/key catalog and supported operations",json!({}),vec![]),
        ("list_devices","List connected controllers and their active profiles",json!({}),vec![]),
        ("list_profiles","List profile names and their shared revision",json!({}),vec![]),
        ("get_profile","Read the complete profile before editing it",json!({"profile":string}),vec!["profile"]),
        ("create_profile","Create a profile, optionally copying an existing source",json!({"name":string,"source":string}),vec!["name"]),
        ("patch_profile","Merge fields into a profile. Objects merge; arrays replace. Name cannot change. Read first; preserve unrelated mappings.",json!({"profile":string,"expected_revision":integer,"patch":object}),vec!["profile","expected_revision","patch"]),
        ("set_mapping","Append a mapping or replace the mapping at a zero-based index. Use describe for defaults and button/key values.",json!({"profile":string,"expected_revision":integer,"index":integer,"mapping":object}),vec!["profile","expected_revision","mapping"]),
        ("remove_mapping","Remove the mapping at a zero-based index",json!({"profile":string,"expected_revision":integer,"index":integer}),vec!["profile","expected_revision","index"]),
        ("apply_profile","Apply an existing profile to one connected controller immediately",json!({"profile":string,"device_id":integer}),vec!["profile","device_id"]),
        ("get_settings","Read application settings and their revision",json!({}),vec![]),
        ("patch_settings","Change language, appearance, color_theme (Classic, Arctic, Tidal, Iris, Ember), reduced_motion, show_tips, ui_scale (0.8–1.6), idle_off_minutes (0–60), default_profile or auto_profiles",json!({"expected_revision":integer,"patch":object}),vec!["expected_revision","patch"]),
    ];
    definitions.into_iter().map(|(name,description,properties,required)| json!({"name":name,"description":description,
        "inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":matches!(name,"describe"|"list_devices"|"list_profiles"|"get_profile"|"get_settings"),"openWorldHint":false}})).collect()
}

#[cfg(test)]
mod tests;

/// Return before GUI/single-instance initialization for automation invocations.
pub fn command(args: &[String]) -> Option<i32> {
    let mode = args.get(1)?.as_str();
    if mode != "--agent" && mode != "--mcp" { return None; }
    crate::platform::attach_console();
    let data = crate::platform::data_dir();
    if mode == "--mcp" {
        let result = mcp::run(&mut std::io::stdin().lock(),&mut std::io::stdout().lock(),|name,args|transport::call(&data,name,args));
        if let Err(error) = result { eprintln!("{error}"); return Some(1); }
        return Some(0);
    }
    let result = (|| -> Result<Value,String> {
        let op = args.get(2).ok_or("Usage: dsz --agent <operation> [JSON | --file <path>]\nUse dsz --agent help to list operations, or dsz --mcp for stdio MCP.")?;
        if op == "help" { return Ok(json!({"tools":tools()})); }
        let value = match args.get(3).map(String::as_str) {
            None if args.len() == 3 => json!({}),
            Some("--file") if args.len() == 5 => {
                let bytes = std::fs::read(&args[4]).map_err(|e|e.to_string())?;
                if bytes.len() > transport::LIMIT { return Err("Arguments exceed 1 MiB".into()); }
                serde_json::from_slice(&bytes).map_err(|e|e.to_string())?
            }
            Some(s) if args.len() == 4 => serde_json::from_str(s).map_err(|e|e.to_string())?,
            _ => return Err("Usage: dsz --agent <operation> [JSON | --file <path>]".into()),
        };
        transport::call(&data,op,value)
    })();
    match result {
        Ok(value) => { println!("{}",json!({"ok":true,"result":value})); Some(0) }
        Err(error) => { println!("{}",json!({"ok":false,"error":error})); Some(1) }
    }
}
