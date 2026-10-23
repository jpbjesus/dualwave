//! PipeWire is a desktop audio transport only; all controller packets still
//! pass through the existing validated capture and Bluetooth streamer.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::NonNull;
use std::sync::Arc;

use parking_lot::Mutex;
use super::capture::PcmSink;

extern "C" {
    fn dwg_pw_new(mode: i32, name: *const c_char, description: *const c_char, target: *const c_char) -> *mut c_void;
    fn dwg_pw_free(endpoint: *mut c_void);
    fn dwg_pw_state(endpoint: *mut c_void) -> i32;
    fn dwg_pw_demand(endpoint: *mut c_void) -> i32;
    fn dwg_pw_available(endpoint: *mut c_void) -> u32;
    fn dwg_pw_read(endpoint: *mut c_void, output: *mut f32, frames: u32) -> u32;
    fn dwg_pw_write(endpoint: *mut c_void, input: *const f32, frames: u32) -> u32;
    fn dwg_pw_enable(endpoint: *mut c_void, enabled: i32);
    fn dwg_pw_clear(endpoint: *mut c_void);
    fn dwg_pw_outputs(emit: unsafe extern "C" fn(*mut c_void, *const c_char), data: *mut c_void) -> i32;
}

pub struct Endpoint(NonNull<c_void>);
// Moving the handle does not move the PipeWire loop. Rust producers/consumers
// are serialized by ownership or a Mutex; the C audio thread owns the other end.
unsafe impl Send for Endpoint {}

impl Endpoint {
    fn new(mode: i32, name: &str, description: &str, target: &str) -> Result<Self, String> {
        let name = CString::new(name).map_err(|_| "invalid PipeWire node name")?;
        let description = CString::new(description).map_err(|_| "invalid PipeWire description")?;
        let target = CString::new(target).map_err(|_| "invalid PipeWire target")?;
        NonNull::new(unsafe { dwg_pw_new(mode, name.as_ptr(), description.as_ptr(), target.as_ptr()) })
            .map(Self).ok_or_else(|| "Cannot connect to PipeWire or create the requested audio stream".into())
    }
    pub fn monitor(target: &str) -> Result<Self, String> {
        Self::new(2, "dualwave.system-monitor", "DualWave System Audio", target)
    }
    pub fn active(&self) -> Result<bool, String> {
        match unsafe { dwg_pw_state(self.0.as_ptr()) } {
            -1 | 0 => Err("PipeWire stream disconnected or failed".into()),
            3 => Ok(true),
            _ => Ok(false),
        }
    }
    fn demand(&self) -> bool { unsafe { dwg_pw_demand(self.0.as_ptr()) != 0 } }
    fn available(&self) -> usize { unsafe { dwg_pw_available(self.0.as_ptr()) as usize } }
    pub fn read(&mut self, output: &mut [[f32; 2]]) -> usize {
        let n = unsafe { dwg_pw_read(self.0.as_ptr(), output.as_mut_ptr().cast(), output.len() as u32) as usize };
        for sample in output[..n].iter_mut().flatten() {
            *sample = finite_sample(*sample);
        }
        n
    }
    fn write(&mut self, input: &[[f32; 2]]) {
        unsafe { dwg_pw_write(self.0.as_ptr(), input.as_ptr().cast(), input.len() as u32); }
    }
    fn enable(&mut self, enabled: bool) { unsafe { dwg_pw_enable(self.0.as_ptr(), i32::from(enabled)); } }
    pub fn clear(&mut self) { unsafe { dwg_pw_clear(self.0.as_ptr()); } }
}
impl Drop for Endpoint {
    fn drop(&mut self) { unsafe { dwg_pw_free(self.0.as_ptr()); } }
}

fn finite_sample(sample: f32) -> f32 {
    if sample.is_finite() { sample.clamp(-8.0, 8.0) } else { 0.0 }
}

pub fn output_devices() -> Result<Vec<String>, String> {
    unsafe extern "C" fn emit(data: *mut c_void, name: *const c_char) {
        let names = unsafe { &mut *data.cast::<Vec<String>>() };
        names.push(unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned());
    }
    let mut names: Vec<String> = Vec::new();
    if unsafe { dwg_pw_outputs(emit, (&mut names as *mut Vec<String>).cast()) } < 0 {
        return Err("Cannot enumerate PipeWire audio outputs".into());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub struct ControllerAudio {
    headphones: Mutex<Endpoint>,
    microphone: Mutex<Endpoint>,
}
impl ControllerAudio {
    pub fn new(identity: &str) -> Result<Arc<Self>, String> {
        let tag: String = identity.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if tag.is_empty() { return Err("Controller has no stable identity for PipeWire endpoints".into()); }
        let short = &tag[tag.len().saturating_sub(6)..];
        let headphones = Endpoint::new(0, &format!("dualwave.headphones.{tag}"),
            &format!("DualWave Headphones ({short})"), "")?;
        let microphone = Endpoint::new(1, &format!("dualwave.microphone.{tag}"),
            &format!("DualWave Microphone ({short})"), "")?;
        Ok(Arc::new(Self { headphones: Mutex::new(headphones), microphone: Mutex::new(microphone) }))
    }
    pub fn healthy(&self) -> bool {
        self.headphones.lock().active().is_ok() && self.microphone.lock().active().is_ok()
    }
    pub fn playing(&self) -> bool { self.headphones.lock().demand() }
    pub fn recording(&self) -> bool { self.microphone.lock().demand() }
    pub fn set_recording(&self, enabled: bool) { self.microphone.lock().enable(enabled); }
    pub fn clear_playback(&self) { self.headphones.lock().clear(); }

    /// Add direct desktop playback before the original Squeeze/Opus stage.
    /// Discard excess latency after a stalled radio without replaying old audio.
    pub fn mix_playback(&self, output: &mut [[f32; 2]]) {
        let mut endpoint = self.headphones.lock();
        let mut chunk = [[0.0; 2]; 512];
        let excess = endpoint.available().saturating_sub(output.len() * 4);
        let mut left = excess;
        while left > 0 {
            let want = left.min(chunk.len());
            let got = endpoint.read(&mut chunk[..want]);
            if got == 0 { break; }
            left -= got;
        }
        for out in output.chunks_mut(chunk.len()) {
            let got = endpoint.read(&mut chunk[..out.len()]);
            mix_frames(out, &chunk[..got]);
        }
    }
}

fn mix_frames(output: &mut [[f32; 2]], input: &[[f32; 2]]) {
    for (out, input) in output.iter_mut().zip(input) {
        out[0] += finite_sample(input[0]);
        out[1] += finite_sample(input[1]);
    }
}
impl PcmSink for ControllerAudio {
    fn push(&self, pcm: &[i16]) {
        let mut endpoint = self.microphone.lock();
        let mut chunk = [[0.0; 2]; 480];
        for samples in pcm.chunks(960) {
            let frames = samples.len() / 2;
            for (out, input) in chunk[..frames].iter_mut().zip(samples.chunks_exact(2)) {
                *out = [input[0] as f32 / 32768.0, input[1] as f32 / 32768.0];
            }
            endpoint.write(&chunk[..frames]);
        }
    }
    fn clear(&self) { self.microphone.lock().clear(); }
}

/// Capture owns this single sink throughout virtual-controller attach/detach.
/// Replacing a USB/IP target cannot disconnect the native microphone.
#[derive(Default)]
pub struct MicrophoneSinks {
    native: Mutex<Option<Arc<ControllerAudio>>>,
    virtual_sink: Mutex<Option<Arc<dyn PcmSink>>>,
}
impl MicrophoneSinks {
    pub fn native(&self) -> Option<Arc<ControllerAudio>> { self.native.lock().clone() }
    pub fn set_native(&self, audio: Option<Arc<ControllerAudio>>) {
        let mut native = self.native.lock();
        if let Some(old) = native.take() { old.set_recording(false); old.clear(); }
        *native = audio;
    }
    pub fn set_virtual(&self, sink: Option<Arc<dyn PcmSink>>) {
        let mut current = self.virtual_sink.lock();
        if let Some(old) = current.take() { old.clear(); }
        *current = sink;
    }
}
impl PcmSink for MicrophoneSinks {
    fn push(&self, pcm: &[i16]) {
        if let Some(native) = self.native.lock().as_ref() { native.push(pcm); }
        if let Some(sink) = self.virtual_sink.lock().as_ref() { sink.push(pcm); }
    }
    fn clear(&self) {
        if let Some(native) = self.native.lock().as_ref() { native.clear(); }
        if let Some(sink) = self.virtual_sink.lock().as_ref() { sink.clear(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern "C" { fn dwg_pw_buffer_test() -> i32; }
    #[test]
    fn native_ring_is_bounded_wraps_and_flushes_microphone_on_disable() {
        assert_eq!(unsafe { dwg_pw_buffer_test() }, 1);
    }
    #[test]
    fn desktop_audio_mixes_with_existing_stereo_and_rejects_nonfinite_samples() {
        let mut output = [[0.25, -0.25]; 3];
        mix_frames(&mut output, &[[0.5, 0.125], [f32::NAN, f32::INFINITY]]);
        assert_eq!(output, [[0.75, -0.125], [0.25, -0.25], [0.25, -0.25]]);
    }
    #[derive(Default)]
    struct Sink(Mutex<Vec<i16>>);
    impl PcmSink for Sink {
        fn push(&self, pcm: &[i16]) { self.0.lock().extend_from_slice(pcm); }
        fn clear(&self) { self.0.lock().clear(); }
    }
    #[test]
    fn replacing_a_virtual_sink_flushes_audio_without_losing_capture_delivery() {
        let sinks = MicrophoneSinks::default();
        let first = Arc::new(Sink::default());
        let second = Arc::new(Sink::default());
        sinks.set_virtual(Some(first.clone()));
        sinks.push(&[123, -123]);
        assert_eq!(*first.0.lock(), [123, -123]);
        sinks.set_virtual(Some(second.clone()));
        assert!(first.0.lock().is_empty());
        sinks.push(&[456, -456]);
        assert_eq!(*second.0.lock(), [456, -456]);
        sinks.clear();
        assert!(second.0.lock().is_empty());
        sinks.set_virtual(None);
        sinks.push(&[999, 999]);
        assert!(second.0.lock().is_empty());
    }
}
