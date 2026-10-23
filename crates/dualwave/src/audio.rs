//! Experimental Bluetooth bridge. Uses upstream packet builders and Opus settings unchanged.
use std::{io, path::Path, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
use ds_proto::{crc, input::Button, output::{AudioRoute, Microphone, MicrophoneSource, OutputState},
    stream::{self, StreamCounters, BLOCKS, HAPTIC_FRAME, MAX_REPORT, OPUS_FRAME, OPUS_SAMPLES}, Connection};
use crate::{hid::{self, Hid, InputGuard}, pipewire::Bridge};

// Share the upstream codec and measured 16:15 resampler instead of changing wire behavior.
#[path = "../../dsz/src/haptics/opus.rs"]
mod opus;
#[allow(dead_code)]
#[path = "../../dsz/src/haptics/dsp.rs"]
mod dsp;

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) { STOP.store(true, Ordering::Relaxed); }

pub fn install_signals() {
    STOP.store(false, Ordering::Relaxed);
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
}

pub fn stopped() -> bool { STOP.load(Ordering::Relaxed) }

/// Only audio flags are valid: never take ownership of triggers, rumble or lighting.
fn control(headset: bool, volume: u8, source: Option<MicrophoneSource>, muted: bool) -> Vec<u8> {
    let state = OutputState {
        rumble_valid: false,
        audio: if headset { AudioRoute::Headset { volume } } else { AudioRoute::Untouched },
        microphone: source.map(|source| Microphone { source, muted }),
        mute_led: u8::from(muted),
        ..Default::default()
    };
    let mut buf = [0u8; MAX_REPORT];
    let report = state.build(Connection::Bluetooth, &mut buf);
    let mut out = report.to_vec();
    out[2] &= 0xf0;
    out[3] &= if source.is_some() { 0x83 } else { 0x80 };
    out[40] = 0;
    crc::seal(crc::PREFIX_OUTPUT, &mut out);
    out
}

struct Session {
    hid: Hid,
    counters: StreamCounters,
    source: Option<MicrophoneSource>,
    // Keep input hidden until the microphone stop command has drained.
    _input: InputGuard,
}
impl Session {
    fn open(path: &Path, source: Option<MicrophoneSource>) -> io::Result<Self> {
        if hid::steam_running() {
            return Err(io::Error::other("Close Steam before audio probes (upstream AGENTS.md hardware rule)"));
        }
        let hid = Hid::open(path, true)?;
        let input = InputGuard::acquire(&hid)?;
        Ok(Self { hid, counters: StreamCounters::new(), source, _input: input })
    }
    fn admission(&mut self, capture: bool) -> io::Result<()> {
        let mut out = [0; MAX_REPORT];
        let flags = stream::AUDIO_CONTROL_INPUT | if capture { stream::AUDIO_CONTROL_MIC } else { 0 };
        let report = stream::build_audio_control(&mut self.counters, flags, &mut out);
        self.hid.write(report)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        for _ in 0..2 { let _ = self.admission(false); }
        let _ = self.hid.write(&control(true, 0, self.source, true));
        // Avoid releasing evdev while the radio still has microphone packets queued.
        std::thread::sleep(Duration::from_millis(60));
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode { Run, Tone, Microphone }

pub fn run(path: &Path, mode: Mode, seconds: Option<u64>, source: MicrophoneSource, volume: u8) -> io::Result<()> {
    install_signals();
    let mut encoder = opus::Encoder::new().ok_or_else(|| io::Error::other("Opus encoder failed"))?;
    let mut decoder = opus::Decoder::new().ok_or_else(|| io::Error::other("Opus decoder failed"))?;
    let mut bridge = if mode == Mode::Run { Some(Bridge::new()?) } else { None };
    let selected = (mode != Mode::Tone).then_some(source);
    let mut session = Session::open(path, selected)?;
    let mut status = wait_state(&mut session.hid)?;
    if mode != Mode::Microphone && !status.headphones {
        return Err(io::Error::other("Connect headphones to the DualSense jack before playback"));
    }
    if source == MicrophoneSource::Headset && mode != Mode::Tone && !status.mic_plugged {
        return Err(io::Error::other("No headset microphone detected in the controller jack"));
    }
    eprintln!("DualWave · {} · Opus {}", path.display(), opus::version());
    eprintln!("Experimental audio mode: physical controller input is temporarily isolated. Ctrl+C stops it.");
    if bridge.is_some() { eprintln!("Select DualWave Headphones / DualWave Microphone in your sound settings."); }
    session.hid.write(&control(mode != Mode::Microphone, volume, selected, false))?;
    session.admission(false)?;

    let start = Instant::now();
    let period = Duration::from_nanos(stream::PERIOD_NS * 2);
    let mut next_media = Instant::now();
    let mut next_control = Instant::now();
    let mut next_log = Instant::now() + Duration::from_secs(2);
    let mut capture = false;
    let mut media = false;
    let mut muted = false;
    let mut mute_down = false;
    let mut previous = None::<u8>;
    let mut packets = 0u64;
    let mut bad = 0u64;
    let mut concealed = 0u64;
    let mut underruns = 0u64;
    let mut peak = 0f32;
    let mut squeeze = dsp::Squeeze::new();
    let mut pcm = [0i16; OPUS_SAMPLES * 2];
    let mut decoded = [0i16; 5760 * 2];
    let mut input = [[0f32; 2]; 1024];
    let mut fitted = Vec::with_capacity(960);
    let mut opus = [[0; OPUS_FRAME]; BLOCKS];
    let mut out = [0; MAX_REPORT];
    let mut report = [0; 1024];
    let mut phase = 0u64;
    let mut primed = false;
    while !STOP.load(Ordering::Relaxed) && seconds.is_none_or(|n| start.elapsed() < Duration::from_secs(n)) {
        let now = Instant::now();
        // If Steam starts mid-session, cleanup runs before any more media is sent.
        if now >= next_log {
            if hid::steam_running() { return Err(io::Error::other("Steam started; stopped audio to protect input")); }
            eprintln!("mic packets={packets}, rejected={bad}, concealed={concealed}, peak={peak:.3}, playback underruns={underruns}, jack={}, headset mic={}", status.headphones, status.mic_plugged);
            peak = 0.0;
            next_log = now + Duration::from_secs(2);
        }
        for _ in 0..128 {
            let Some(n) = session.hid.read(&mut report)? else { break; };
            let bytes = &report[..n];
            if let Some(state) = hid::state(bytes) {
                status = state;
                let down = state.buttons.has(Button::Mute);
                if down && !mute_down && mode != Mode::Tone {
                    muted = !muted;
                    if muted {
                        capture = false;
                        previous = None;
                        session.admission(false)?;
                    }
                    session.hid.write(&control(mode != Mode::Microphone, volume, selected, muted))?;
                    if let Some(bridge) = &mut bridge { bridge.clear(true); }
                    eprintln!("Microphone {}", if muted { "muted" } else { "unmuted" });
                }
                mute_down = down;
            } else if let Some((counter, packet)) = hid::microphone(bytes) {
                if !capture || muted { continue; }
                if let Some(prev) = previous {
                    let distance = counter.wrapping_sub(prev);
                    if distance == 0 || distance > 128 { continue; }
                    for _ in 1..distance.min(4) {
                        let n = decoder.conceal(&mut decoded[..960]);
                        deliver(&mut bridge, &decoded[..n * 2], &mut peak);
                        concealed += 1;
                    }
                }
                previous = Some(counter);
                let n = decoder.decode(packet, &mut decoded);
                if n == 0 { bad += 1; continue; }
                packets += 1;
                deliver(&mut bridge, &decoded[..n * 2], &mut peak);
            } else if bytes.first() == Some(&0x31) { bad += 1; }
        }
        let want_capture = mode != Mode::Tone && !muted && match &bridge {
            Some(b) => b.active(true)?,
            None => true,
        };
        if capture != want_capture {
            capture = want_capture;
            previous = None;
            decoder = opus::Decoder::new().ok_or_else(|| io::Error::other("Opus decoder reset failed"))?;
            if let Some(b) = &mut bridge { b.clear(true); }
            session.admission(capture)?;
            next_control = now + Duration::from_secs(1);
        }
        let want_media = mode != Mode::Microphone && status.headphones && match &bridge {
            Some(b) => b.active(false)?,
            None => true,
        };
        if !want_media && media {
            session.admission(capture)?;
            media = false;
            primed = false;
            if let Some(b) = &mut bridge { b.clear(false); }
        }
        if want_media && !media {
            media = true;
            next_media = now;
            squeeze = dsp::Squeeze::new();
            encoder = opus::Encoder::new().ok_or_else(|| io::Error::other("Opus encoder reset failed"))?;
            let silence = [0; OPUS_SAMPLES * 2];
            for frame in &mut opus {
                if !encoder.encode(&silence, frame) { return Err(io::Error::other("Opus silence encoding failed")); }
            }
            for _ in 0..3 {
                session.hid.write(stream::build_audio_with_microphone(&mut session.counters,
                    &opus, &[[0; HAPTIC_FRAME]; BLOCKS], true, Some(capture), &mut out))?;
            }
        }
        if media && now >= next_media {
            input.fill([0.0; 2]);
            if mode == Mode::Tone {
                for frame in &mut input {
                    // Quiet, distinct left/right test tones with a short fade.
                    let t = phase as f32 / 48_000.0;
                    let fade = (t * 20.0).min(1.0);
                    *frame = [440.0, 660.0].map(|hz| (t * hz * std::f32::consts::TAU).sin() * 0.12 * fade);
                    phase += 1;
                }
            } else if let Some(b) = &mut bridge {
                // Prime two windows; re-prime after an underrun and bound queue latency.
                if b.available() > 8192 { b.clear(false); primed = false; }
                if !primed { primed = b.available() >= 2048; }
                if primed && b.read(&mut input) != input.len() {
                    underruns += 1;
                    primed = false;
                }
            }
            fitted.clear();
            squeeze.run(&input, &mut fitted);
            if fitted.len() != 960 { return Err(io::Error::other("Resampler produced an invalid frame count")); }
            for (chunk, frame) in fitted.chunks_exact(OPUS_SAMPLES).zip(&mut opus) {
                for (i, sample) in chunk.iter().flatten().enumerate() {
                    pcm[i] = (dsp::soft_clip(*sample) * 32767.0).round() as i16;
                }
                if !encoder.encode(&pcm, frame) { return Err(io::Error::other("Opus encoding failed")); }
            }
            session.hid.write(stream::build_audio_with_microphone(&mut session.counters,
                &opus, &[[0; HAPTIC_FRAME]; BLOCKS], true, Some(capture), &mut out))?;
            next_media += period;
            if next_media + period < now { next_media = now + period; }
        } else if !media && capture && now >= next_control {
            session.admission(true)?;
            next_control = now + Duration::from_secs(1);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    eprintln!("Stopped: {packets} valid microphone packets, {bad} rejected, {concealed} concealed, {underruns} playback underruns.");
    Ok(())
}

fn deliver(bridge: &mut Option<Bridge>, pcm: &[i16], peak: &mut f32) {
    let frames: Vec<[f32; 2]> = pcm.chunks_exact(2).map(|f| {
        // Controller microphone is exposed as mono duplicated to both channels.
        let v = (f[0] as f32 + f[1] as f32) / 65536.0;
        *peak = peak.max(v.abs());
        [v, v]
    }).collect();
    if let Some(b) = bridge { b.write(&frames); }
}

fn wait_state(hid: &mut Hid) -> io::Result<ds_proto::InputState> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0; 1024];
    while Instant::now() < deadline {
        if let Some(n) = hid.read(&mut buf)? {
            if let Some(state) = hid::state(&buf[..n]) { return Ok(state); }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "No valid full Bluetooth state report"))
}

pub fn probe(path: &Path) -> io::Result<()> {
    let mut hid = Hid::open(path, false)?;
    let state = wait_state(&mut hid)?;
    println!("DualSense · Bluetooth · valid input CRC\nBattery: {}%\nHeadphones: {}\nHeadset microphone: {}",
        state.battery_percent, state.headphones, state.mic_plugged);
    match hid.feature(0x20, 64) {
        Ok(b) => {
            let version = u16::from_le_bytes([b[44], b[45]]);
            println!("Firmware: {:X}.{:02X}", version >> 8, version & 255);
        }
        Err(e) => eprintln!("Firmware unavailable: {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_controls_never_claim_gameplay_outputs_and_keep_crc() {
        let report = control(true, 38, Some(MicrophoneSource::Headset), false);
        assert_eq!(&report[..4], &[0x31, 0x02, 0xf0, 0x03]);
        assert_eq!(report[6], 38);
        assert_eq!(report[7], 0);
        assert_eq!(report[9] & 3, 2);
        assert_eq!(report[40], 0);
        assert!(crc::verify(crc::PREFIX_OUTPUT, &report));
        let report = control(true, 38, None, false);
        assert_eq!(&report[..4], &[0x31, 0x02, 0xb0, 0x00]);
        assert!(crc::verify(crc::PREFIX_OUTPUT, &report));
        let report = control(true, 0, Some(MicrophoneSource::Auto), true);
        assert_eq!(report[11], 0x10);
        assert_eq!(report[6], 0);
        assert!(crc::verify(crc::PREFIX_OUTPUT, &report));
    }
}
