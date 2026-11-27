use ds_proto::input::BatteryStatus;
use crate::{engine::Engine, i18n::tr};

#[cfg(not(test))]
#[path = "glass.rs"]
mod glass;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

struct Row {
    name: String,
    profile: String,
    bluetooth: bool,
    status: BatteryStatus,
    charge: Option<u8>,
}

fn rows(engine: &Engine) -> Vec<Row> {
    let mut rows = Vec::new();
    for device in engine.device_list() {
        let input = device.snapshot().input;
        let charge = (input.full && input.battery_status != BatteryStatus::Error).then(|| input.battery_shown());
        let name = device.display_name(engine);
        let profile = engine.profile_name_for(&device);
        rows.push(Row { name, profile, bluetooth: device.is_bluetooth(), status: input.battery_status, charge });
    }
    rows
}

impl Row {
    fn level(&self) -> String {
        self.charge.map_or_else(|| "—".into(), |value| format!("{value}%"))
    }
}

#[cfg(test)]
pub(super) fn body(engine: &Engine) -> String { body_rows(&rows(engine)) }

fn body_rows(rows: &[Row]) -> String {
    let lines: Vec<_> = rows.iter().map(|row| {
        let link = if row.bluetooth { "Bluetooth" } else { "USB" };
        let status = match (row.charge, row.status) {
            (Some(_), BatteryStatus::Charging) => format!(" · {}", tr("Charging")),
            (Some(_), BatteryStatus::Full) => format!(" · {}", tr("Full")),
            _ => String::new(),
        };
        escape(&format!("{} · {} · {link}{status}\n{}", row.name, row.level(), row.profile))
    }).collect();
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
            let mut next = rx.recv();
            while let Ok(request) = next {
                let (engine, ms) = rx.try_iter().last().unwrap_or(request);
                let Some(engine) = engine.upgrade() else { next = rx.recv(); continue; };
                let rows = rows(&engine);
                drop(engine);
                match glass::show(&rows) {
                    Ok(surface) => {
                        // A fresh request replaces the surface immediately.
                        next = match rx.recv_timeout(std::time::Duration::from_millis(u64::from(ms))) {
                            Ok(request) => Ok(request),
                            Err(_) => { drop(surface); rx.recv() },
                        };
                        continue;
                    }
                    Err(error) => log::debug!("controller status glass unavailable: {error}"),
                }
                let now = std::time::Instant::now();
                if last.is_some_and(|time| now.duration_since(time) < std::time::Duration::from_millis(750)) {
                    next = rx.recv();
                    continue;
                }
                last = Some(now);
                match show_status(&mut connection, replaces, &body_rows(&rows), ms) {
                    Ok(id) => replaces = id,
                    Err(error) => {
                        log::debug!("controller status notification unavailable: {error}");
                        connection = None;
                        replaces = 0;
                    }
                }
                next = rx.recv();
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
