//! Bluetooth wireless haptics and controller-speaker audio.
//!
//! ```text
//! WASAPI loopback ──► AudioHub ──► per-controller FIFO
//!                                      │  every 32/3000 s (high-res timer)
//!                                      ▼
//!        high-pass + low-pass ─► 3 kHz ─► + event synth ─► int8 x 64 ─► 0x32 report
//!
//! While there is speaker sound (a game's speaker channels, or PC audio sent
//! to the speaker), every 64/3000 s instead:
//!
//!        2 haptic blocks + 1024 speaker frames ─► 16:15 squeeze ─► 2 Opus frames ─► 0x39 report
//! ```

#[cfg(windows)]
pub mod audio;
#[cfg(target_os = "linux")]
#[path = "audio_linux.rs"]
pub mod audio;
#[cfg(target_os = "linux")]
pub mod pipewire_linux;
pub mod capture;
pub mod ds4;
pub mod dsp;
pub mod game;
pub mod mic;
pub mod opus;
// Windows hardware probes use the upstream raw-HID transport. Linux diagnostics
// live in the isolated dualwave CLI to avoid exposing media to game clients.
#[cfg(windows)]
pub mod probe;
pub mod streamer;
pub mod synth;
