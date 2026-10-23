//! PipeWire output-monitor capture fanned out to every
//! controller that wants it. One capture stream serves all controllers.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use super::pipewire_linux::Endpoint;
use parking_lot::Mutex;

/// Half a second at 48 kHz; older audio is dropped.
const FIFO_MAX: usize = 24_000;

pub struct Sub {
    pub fifo: Mutex<VecDeque<[f32; 2]>>,
    pub rate: AtomicU32,
    pub device: String,
    /// False for the lightbar meter: the level is updated, samples are not queued.
    queue: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureStatus {
    pub running: bool,
    pub device: String,
    pub rate: u32,
    pub channels: u16,
    pub error: Option<String>,
}

pub struct AudioHub {
    subs: Mutex<Vec<Weak<Sub>>>,
    pub status: Mutex<CaptureStatus>,
    /// Peak of the last callback, f32 bits. The haptics page meter reads this.
    pub peak: AtomicU32,
    /// Smoothed full-band level for the audio lightbar, f32 bits.
    loudness: AtomicU32,
    /// When the last capture buffer arrived, in ms since `epoch`. Loopback
    /// capture delivers nothing while nothing plays, so the level is let
    /// down from here when buffers stop.
    last_audio_ms: AtomicU64,
    epoch: Instant,
    started: AtomicBool,
}

impl AudioHub {
    pub fn new() -> Arc<AudioHub> {
        Arc::new(AudioHub {
            subs: Mutex::new(Vec::new()),
            status: Mutex::new(CaptureStatus::default()),
            peak: AtomicU32::new(0),
            loudness: AtomicU32::new(0),
            last_audio_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            started: AtomicBool::new(false),
        })
    }

    /// Subscribe to system audio from `device` (empty = desktop default).
    /// Dropping the returned `Arc` unsubscribes.
    pub fn subscribe(self: &Arc<Self>, device: &str) -> Arc<Sub> {
        self.subscribe_with(device, true)
    }

    /// Same capture as [`subscribe`](Self::subscribe), without queueing samples.
    pub fn subscribe_meter(self: &Arc<Self>, device: &str) -> Arc<Sub> {
        self.subscribe_with(device, false)
    }

    fn subscribe_with(self: &Arc<Self>, device: &str, queue: bool) -> Arc<Sub> {
        let sub = Arc::new(Sub {
            fifo: Mutex::new(VecDeque::with_capacity(if queue { FIFO_MAX } else { 0 })),
            rate: AtomicU32::new(0),
            device: device.to_string(),
            queue,
        });
        self.subs.lock().push(Arc::downgrade(&sub));
        if !self.started.swap(true, Ordering::AcqRel) {
            let hub = self.clone();
            std::thread::Builder::new()
                .name("audio-capture".into())
                .spawn(move || hub.run())
                .unwrap();
        }
        sub
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    /// Smoothed system-audio level, 0 at silence. Falls toward 0 once
    /// capture buffers stop arriving (nothing playing).
    pub fn loudness(&self) -> f32 {
        let v = f32::from_bits(self.loudness.load(Ordering::Relaxed));
        let now_ms = self.epoch.elapsed().as_millis() as u64;
        let quiet_ms = now_ms.saturating_sub(self.last_audio_ms.load(Ordering::Relaxed));
        crate::audio_led::after_gap(v, quiet_ms as f32 / 1000.0)
    }

    /// The device to capture. A haptics subscriber's choice wins over a
    /// lightbar meter's, since only haptics plays what is captured.
    fn wanted(&self) -> Option<String> {
        let mut subs = self.subs.lock();
        subs.retain(|w| w.strong_count() > 0);
        let live: Vec<Arc<Sub>> = subs.iter().filter_map(|w| w.upgrade()).collect();
        live.iter()
            .find(|s| s.queue)
            .or(live.first())
            .map(|s| s.device.clone())
    }

    fn push(&self, frames: &[[f32; 2]], rate: u32) {
        let subs: Vec<Arc<Sub>> = self
            .subs
            .lock()
            .iter()
            .filter_map(|w| w.upgrade())
            .collect();
        for s in subs {
            s.rate.store(rate, Ordering::Relaxed);
            if !s.queue {
                continue;
            }
            let mut f = s.fifo.lock();
            f.extend(frames.iter().copied());
            let over = f.len().saturating_sub(FIFO_MAX);
            if over > 0 {
                f.drain(..over);
            }
        }
    }

    pub fn output_devices() -> Vec<String> {
        match super::pipewire_linux::output_devices() {
            Ok(devices) => devices,
            Err(e) => { log::warn!("{e}"); Vec::new() }
        }
    }

    fn run(self: Arc<Self>) {
        // Do not retain the engine forever merely because capture was used once.
        let weak = Arc::downgrade(&self);
        drop(self);
        let mut current: Option<(String, Endpoint)> = None;
        let mut retry = Instant::now();
        let mut frames = [[0.0; 2]; 2048];
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let Some(hub) = weak.upgrade() else { break; };
            let Some(want) = hub.wanted() else {
                if current.take().is_some() {
                    log::info!("system audio capture stopped");
                }
                *hub.status.lock() = CaptureStatus::default();
                hub.peak.store(0, Ordering::Relaxed);
                hub.loudness.store(0, Ordering::Relaxed);
                continue;
            };
            if current.as_ref().is_some_and(|(name, endpoint)| name != &want || endpoint.active().is_err()) {
                current = None;
                retry = Instant::now();
            }
            if current.is_none() && Instant::now() >= retry {
                match Endpoint::monitor(&want) {
                    Ok(endpoint) => {
                        *hub.status.lock() = CaptureStatus {
                            running: false,
                            device: if want.is_empty() { "PipeWire default output".into() } else { want.clone() },
                            rate: 48_000,
                            channels: 2,
                            error: None,
                        };
                        current = Some((want.clone(), endpoint));
                    }
                    Err(e) => {
                        let mut status = hub.status.lock();
                        if status.error.as_ref() != Some(&e) { log::warn!("system audio capture: {e}"); }
                        status.running = false;
                        status.error = Some(e);
                        hub.loudness.store(0, Ordering::Relaxed);
                        retry = Instant::now() + Duration::from_secs(2);
                    }
                }
            }
            if let Some((_, endpoint)) = current.as_mut() {
                hub.status.lock().running = endpoint.active().unwrap_or(false);
                loop {
                    let n = endpoint.read(&mut frames);
                    if n == 0 { break; }
                    hub.received(&frames[..n]);
                    if n < frames.len() { break; }
                }
            }
        }
    }

    fn received(&self, frames: &[[f32; 2]]) {
        let peak = frames.iter().flatten().fold(0.0f32, |a, b| a.max(b.abs()));
        let old = f32::from_bits(self.peak.load(Ordering::Relaxed));
        self.peak.store(peak.max(old * 0.85).to_bits(), Ordering::Relaxed);
        let prev = f32::from_bits(self.loudness.load(Ordering::Relaxed));
        self.loudness.store(crate::audio_led::next_level(prev, frames, 48_000).to_bits(), Ordering::Relaxed);
        self.last_audio_ms.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.push(frames, 48_000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_fanout_keeps_latest_half_second_and_meter_does_not_queue() {
        let hub = AudioHub::new();
        // Set started to avoid opening desktop capture in a unit test.
        hub.started.store(true, Ordering::Relaxed);
        let audio = hub.subscribe("sink-a");
        let meter = hub.subscribe_meter("sink-b");
        let frames: Vec<_> = (0..FIFO_MAX + 100).map(|i| [i as f32, -(i as f32)]).collect();
        hub.push(&frames, 48_000);
        assert_eq!(audio.fifo.lock().len(), FIFO_MAX);
        assert_eq!(audio.fifo.lock().front(), Some(&[100.0, -100.0]));
        assert!(meter.fifo.lock().is_empty());
        assert_eq!(meter.rate.load(Ordering::Relaxed), 48_000);
        assert_eq!(hub.wanted().as_deref(), Some("sink-a"));
        drop(audio);
        assert_eq!(hub.wanted().as_deref(), Some("sink-b"));
        drop(meter);
        assert_eq!(hub.wanted(), None);
    }
}
