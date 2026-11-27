use ds_proto::input::BatteryStatus;
use crate::{engine::Engine, i18n::tr};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub(super) fn body(engine: &Engine) -> String {
    let mut lines = Vec::new();
    for device in engine.device_list() {
        let input = device.snapshot().input;
        let charge = if input.full && input.battery_status != BatteryStatus::Error {
            format!("{}%", input.battery_shown())
        } else {
            "—".into()
        };
        let status = match input.battery_status {
            BatteryStatus::Charging if input.full => format!(" · {}", tr("Charging")),
            BatteryStatus::Full if input.full => format!(" · {}", tr("Full")),
            _ => String::new(),
        };
        let link = if device.is_bluetooth() { "Bluetooth" } else { "USB" };
        let name = device.display_name(engine);
        let profile = engine.profile_name_for(&device);
        lines.push(escape(&format!("{name} · {charge} · {link}{status}\n{profile}")));
    }
    if lines.is_empty() { tr("No controller connected").into() } else { lines.join("\n\n") }
}

#[cfg(not(test))]
pub(super) fn show(engine: &std::sync::Arc<Engine>, ms: u32) {
    use std::sync::{OnceLock, Weak, mpsc};

    type Request = (Weak<Engine>, u32);
    static QUEUE: OnceLock<Option<mpsc::SyncSender<Request>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Request>(2);
        match std::thread::Builder::new().name("controller-status".into()).spawn(move || {
            let mut connection = None;
            let mut replaces = 0;
            let mut last = None::<std::time::Instant>;
            while let Ok(request) = rx.recv() {
                let (engine, ms) = rx.try_iter().last().unwrap_or(request);
                let Some(engine) = engine.upgrade() else { continue; };
                let now = std::time::Instant::now();
                if last.is_some_and(|time| now.duration_since(time) < std::time::Duration::from_millis(750)) { continue; }
                last = Some(now);
                match show_status(&mut connection, replaces, &body(&engine), ms) {
                    Ok(id) => replaces = id,
                    Err(error) => {
                        log::debug!("controller status notification unavailable: {error}");
                        connection = None;
                        replaces = 0;
                    }
                }
            }
        }) {
            Ok(_) => Some(tx),
            Err(error) => {
                log::warn!("could not start controller status worker: {error}");
                None
            }
        }
    });
    if let Some(queue) = queue { let _ = queue.try_send((std::sync::Arc::downgrade(engine), ms)); }
}

#[cfg(not(test))]
fn show_status(connection: &mut Option<zbus::blocking::Connection>, replaces: u32, body: &str, ms: u32) -> zbus::Result<u32> {
    use zbus::{blocking::{Proxy, connection::Builder}, zvariant::Value};

    if connection.is_none() {
        *connection = Some(Builder::session()?.method_timeout(std::time::Duration::from_secs(2)).build()?);
    }
    let connection = connection.as_ref().unwrap();
    if let Ok(osd) = Proxy::new(connection, "org.kde.plasmashell", "/org/kde/osdService", "org.kde.osdService") {
        // Plasma notifications can sit behind fullscreen windows. Its OSD
        // uses the same no-focus surface as volume and brightness indicators.
        if osd.call::<_, _, ()>("showText", &("input-gaming", body)).is_ok() {
            return Ok(0);
        }
    }
    let proxy = Proxy::new(connection, "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications", "org.freedesktop.Notifications")?;
    let hints = std::collections::HashMap::from([
        ("transient", Value::Bool(true)),
        ("suppress-sound", Value::Bool(true)),
        ("desktop-entry", Value::from("dualwave")),
    ]);
    proxy.call("Notify", &("DualWave", replaces, "dualwave", tr("Controller status"), body, Vec::<&str>::new(), hints, ms as i32))
}
