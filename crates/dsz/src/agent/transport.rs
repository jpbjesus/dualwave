//! The listener and UI serialize configuration edits, even while the window is hidden.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(super) const LIMIT: usize = 1024 * 1024;
const FILE: &str = "agent-endpoint.json";

#[derive(Serialize, Deserialize)]
struct Endpoint { port: u16, token: String }

pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
    port: u16,
}

pub(super) fn line(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    let n = reader.take((LIMIT + 1) as u64).read_until(b'\n', &mut bytes)?;
    if n == 0 { return Ok(None); }
    if n > LIMIT || bytes.last() != Some(&b'\n') { return Err(io::Error::new(io::ErrorKind::InvalidData,"Message must be a newline-terminated JSON object, at most 1 MiB")); }
    serde_json::from_slice(&bytes).map(Some).map_err(|e|io::Error::new(io::ErrorKind::InvalidData,e))
}

pub(super) fn write(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer,value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8;32];
    #[cfg(target_os = "linux")]
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        if unsafe { BCryptGenRandom(std::ptr::null_mut(),bytes.as_mut_ptr(),bytes.len() as u32,BCRYPT_USE_SYSTEM_PREFERRED_RNG) } < 0 {
            return Err(io::Error::other("Could not create local access token"));
        }
    }
    Ok(bytes.iter().map(|v|format!("{v:02x}")).collect())
}

struct RequestReader<'a> {
    stream: &'a mut TcpStream,
    stop: &'a AtomicBool,
    deadline: Instant,
}

impl Read for RequestReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.stop.load(Ordering::Acquire) { return Err(io::Error::new(io::ErrorKind::ConnectionAborted,"Agent access disabled")); }
            if Instant::now() >= self.deadline { return Err(io::Error::new(io::ErrorKind::TimedOut,"Request deadline exceeded")); }
            match self.stream.read(bytes) {
                Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
                result => return result,
            }
        }
    }
}

impl Server {
    pub fn start(engine: Arc<crate::engine::Engine>, ctx: eframe::egui::Context) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST,0))?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let path = engine.data_dir.join(FILE);
        let stop = Arc::new(AtomicBool::new(false));
        // A new file avoids retaining wider permissions from an old endpoint.
        let _ = std::fs::remove_file(&path);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&path)?;
        serde_json::to_writer(&mut file,&Endpoint {port,token:token.clone()})?;
        let stopped = stop.clone();
        let thread = match std::thread::Builder::new().name("agent-api".into()).spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) { break; }
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let result = (|| -> Result<Value,String> {
                    let reader = RequestReader { stream:&mut stream, stop:&stopped, deadline:Instant::now() + Duration::from_secs(1) };
                    let request = line(&mut BufReader::new(reader)).map_err(|e|e.to_string())?.ok_or("Empty request")?;
                    if request.get("token").and_then(Value::as_str) != Some(&token) { return Err("Unauthorized".into()); }
                    let operation = request.get("operation").and_then(Value::as_str).ok_or("Expected operation")?.to_string();
                    let arguments = request.get("arguments").cloned().unwrap_or(json!({}));
                    let deadline = Instant::now() + Duration::from_secs(4);
                    let _edit = loop {
                        if stopped.load(Ordering::Acquire) { return Err("Agent access disabled".into()); }
                        if Instant::now() >= deadline { return Err("Configuration is busy; no changes applied".into()); }
                        if let Some(guard) = engine.config_edit.try_lock_for(Duration::from_millis(100)) { break guard; }
                    };
                    let enabled = engine.settings.read().agent_enabled;
                    if stopped.load(Ordering::Acquire) || !enabled { return Err("Agent access disabled".into()); }
                    let result = super::execute(&engine,&operation,arguments);
                    ctx.request_repaint();
                    result
                })();
                let response = match result { Ok(result)=>json!({"ok":true,"result":result}),Err(error)=>json!({"ok":false,"error":error}) };
                let _ = write(&mut stream,&response);
            }
        }) {
            Ok(thread) => thread,
            Err(error) => { let _ = std::fs::remove_file(&path); return Err(error); }
        };
        Ok(Self {stop,thread:Some(thread),path,port})
    }

}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::Release);
        let _ = TcpStream::connect_timeout(&SocketAddrV4::new(Ipv4Addr::LOCALHOST,self.port).into(),Duration::from_millis(100));
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(super) fn call(data: &Path, operation: &str, arguments: Value) -> Result<Value,String> {
    let endpoint: Endpoint = serde_json::from_slice(&std::fs::read(data.join(FILE)).map_err(|_|"Open DualWave and enable Agent access in Preferences → Integrations")?).map_err(|e|format!("Invalid agent endpoint: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&SocketAddrV4::new(Ipv4Addr::LOCALHOST,endpoint.port).into(),Duration::from_secs(2)).map_err(|e|format!("DualWave agent access unavailable: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(6))).map_err(|e|e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e|e.to_string())?;
    write(&mut stream,&json!({"token":endpoint.token,"operation":operation,"arguments":arguments})).map_err(|e|e.to_string())?;
    let response = line(&mut BufReader::new(stream)).map_err(|e|e.to_string())?.ok_or("Empty response")?;
    if response["ok"] == true { Ok(response["result"].clone()) }
    else { Err(response["error"].as_str().unwrap_or("Invalid response").to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_rejects_truncation_and_limits_allocation() {
        assert!(line(&mut &b"{}"[..]).is_err());
        assert!(line(&mut vec![b' ';LIMIT+2].as_slice()).is_err());
        assert_eq!(line(&mut &b"{}\n"[..]).unwrap(),Some(json!({})));
    }
    #[test]
    fn slow_unauthenticated_client_cannot_block_disabling_access() {
        let e = crate::agent::tests::engine("slow-client");
        let server = Server::start(e.clone(),eframe::egui::Context::default()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST,server.port)).unwrap();
        stream.write_all(b"{").unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..15 {
                if stream.write_all(b" ").is_err() { return; }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        std::thread::sleep(Duration::from_millis(40));
        let started = Instant::now();
        drop(server);
        let elapsed = started.elapsed();
        writer.join().unwrap();
        assert!(elapsed < Duration::from_millis(600), "Disabling agent access blocked for {elapsed:?}");
        let _ = std::fs::remove_dir_all(&e.data_dir);
    }

    #[test]
    fn configuration_works_without_rendering_a_frame() {
        let e = crate::agent::tests::engine("hidden-window");
        e.settings.write().agent_enabled = true;
        let server = Server::start(e.clone(),eframe::egui::Context::default()).unwrap();
        let result = call(&e.data_dir,"patch_settings",json!({"expected_revision":e.settings_revision(),"patch":{"show_tips":false}}));
        drop(server);
        assert!(result.is_ok(),"A tray or hidden window must still accept configuration: {result:?}");
        assert!(!e.settings.read().show_tips);
        let _ = std::fs::remove_dir_all(&e.data_dir);
    }

    #[test]
    fn agent_edit_waits_for_ui_snapshot_and_rejects_its_stale_revision() {
        let e = crate::agent::tests::engine("ui-conflict");
        e.settings.write().agent_enabled = true;
        let server = Server::start(e.clone(),eframe::egui::Context::default()).unwrap();
        let ui_edit = e.config_edit.lock();
        let name = e.profiles.names()[0].clone();
        let revision = e.profiles.revision();
        let data = e.data_dir.clone();
        let target = name.clone();
        let client = std::thread::spawn(move ||call(&data,"set_mapping",json!({"profile":target,"expected_revision":revision,"mapping":{"button":"Ps"}})));
        let mut edited = (*e.profiles.find(&name).unwrap()).clone();
        edited.mappings.clear();
        edited.mappings.push(crate::profile::Mapping { button:ds_proto::input::Button::L1, ..Default::default() });
        e.profiles.update(edited.clone());
        drop(ui_edit);
        assert!(client.join().unwrap().unwrap_err().contains("Revision conflict"));
        assert_eq!(*e.profiles.find(&name).unwrap(),edited);
        e.settings.write().agent_enabled = false;
        assert!(call(&e.data_dir,"list_profiles",json!({})).unwrap_err().contains("disabled"));
        drop(server);
        let _ = std::fs::remove_dir_all(&e.data_dir);
    }

    #[test]
    fn authenticated_roundtrip_and_shutdown_removes_discovery() {
        let e = crate::agent::tests::engine("transport");
        e.settings.write().agent_enabled = true;
        let server = Server::start(e.clone(),eframe::egui::Context::default()).unwrap();
        let dir = e.data_dir.clone();
        let client = std::thread::spawn(move ||call(&dir,"list_profiles",json!({})));
        assert!(!client.join().unwrap().unwrap()["profiles"].as_array().unwrap().is_empty());
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST,server.port)).unwrap();
        write(&mut stream,&json!({"token":"wrong","operation":"list_profiles"})).unwrap();
        let response = line(&mut BufReader::new(stream)).unwrap().unwrap();
        assert_eq!(response["error"],"Unauthorized");
        drop(server);
        assert!(!e.data_dir.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&e.data_dir);
    }
}
