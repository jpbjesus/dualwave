//! Linux setup is an explicit user action; never fetch or run Windows installers.
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Package { Usbip, HidHide }
impl Package {
    pub fn name(self) -> &'static str {
        match self { Self::Usbip => "Linux USB/IP support", Self::HidHide => "Linux exclusive input support" }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    // Shared UI variants describe Windows downloads; Linux only installs its bundled payload.
    #[allow(dead_code)] Downloading,
    #[allow(dead_code)] Verifying, Installing, Finished { ok: bool }, Failed(String) }
fn states() -> &'static Mutex<HashMap<Package, Status>> {
    static STATES: OnceLock<Mutex<HashMap<Package, Status>>> = OnceLock::new();
    STATES.get_or_init(Default::default)
}
pub fn status(package: Package) -> Option<Status> { states().lock().get(&package).cloned() }
pub fn start(package: Package, wake: impl Fn() + Send + 'static) {
    if status(package) == Some(Status::Installing) { return; }
    states().lock().insert(package, Status::Installing);
    std::thread::spawn(move || {
        let result = (|| -> Result<bool, String> {
            let appdir = std::env::var_os("APPDIR").map(PathBuf::from)
                .ok_or("Run the DualWave AppImage to configure system support.")?;
            let setup = appdir.join("usr/lib/dualwave/setup-driver.sh");
            if !setup.is_file() { return Err("This build has no system setup payload.".into()); }
            // AppImage mounts can be inaccessible to root. The AppRun setup action
            // stages its payload before requesting administrator authorization.
            let image = std::env::var_os("APPIMAGE").ok_or("Cannot locate the running AppImage.")?;
            std::process::Command::new(image).arg("--setup").status()
                .map(|status| status.success()).map_err(|error| error.to_string())
        })();
        states().lock().insert(package, match result {
            Ok(ok) => Status::Finished { ok }, Err(error) => Status::Failed(error),
        });
        wake();
    });
}
pub(crate) fn sha256(data: &[u8]) -> Result<[u8; 32], String> { Ok(Sha256::digest(data).into()) }
pub(crate) fn download(url: &str, to: &PathBuf) -> Result<(), String> {
    if !url.starts_with("https://") { return Err("HTTPS is required.".into()); }
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(20))
        .user_agent("DualWave").build().map_err(|e| e.to_string())?;
    let response = client.get(url).send().and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    if response.content_length().is_some_and(|n| n > 2 * 1024 * 1024) {
        return Err("Update metadata is too large.".into());
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    response.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 { return Err("Update metadata is too large.".into()); }
    std::fs::write(to, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn checksum_matches_standard_vector() {
        let hash = super::sha256(b"abc").unwrap();
        let text: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(text, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
