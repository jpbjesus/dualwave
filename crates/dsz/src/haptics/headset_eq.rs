//! Headphone-only tone controls; inactive EQ is an exact bypass after a short transition.
use super::dsp::Biquad;
use crate::profile::HeadsetEq;

const FADE: usize = 480;

#[derive(Clone, Copy)]
struct Bank {
    bands: [[Biquad; 3]; 2],
    gain: f32,
}
impl Bank {
    fn new(db: [f32; 3]) -> Self {
        let bands = [Biquad::shelf(48_000.0, 120.0, db[0], false),
            Biquad::peaking(48_000.0, 1000.0, 0.7, db[1]),
            Biquad::shelf(48_000.0, 6000.0, db[2], true)];
        // The sum of positive band gains bounds the cascade's maximum
        // frequency gain. Reserve that headroom before any filtering.
        let headroom: f32 = db.iter().map(|v| v.max(0.0)).sum();
        Self { bands: [bands; 2], gain: 10.0f32.powf(-headroom / 20.0) }
    }
    fn run(&mut self, frame: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        for ch in 0..2 {
            if !frame[ch].is_finite() || frame[ch].abs() > 16.0 {
                // Reject invalid or implausibly large source samples before
                // arithmetic can overflow recursive state (24 dB above full scale).
                for band in &mut self.bands[ch] { band.reset(); }
                continue;
            }
            let mut v = frame[ch] * self.gain;
            for band in &mut self.bands[ch] { v = band.run(v); }
            if v.is_finite() {
                out[ch] = v;
            } else {
                for band in &mut self.bands[ch] { band.reset(); }
            }
        }
        out
    }
}

pub(super) struct Equalizer {
    current: Bank,
    next: Bank,
    applied: [f32; 3],
    remaining: usize,
    active: bool,
}
impl Equalizer {
    pub(super) fn new() -> Self {
        Self { current: Bank::new([0.0; 3]), next: Bank::new([0.0; 3]),
            applied: [0.0; 3], remaining: 0, active: false }
    }

    pub(super) fn process(&mut self, setting: &HeadsetEq, headset: bool, frames: &mut [[f32; 2]]) {
        let gains = if setting.enabled { setting.gains() } else { [0.0; 3] };
        if !headset {
            if self.active { *self = Self::new(); }
            return;
        }
        if !self.active && gains == [0.0; 3] { return; }
        self.active = true;
        for frame in frames {
            // Finish each 10 ms crossfade before accepting another slider
            // value. Both banks have fixed storage and independent histories.
            if self.remaining == 0 && self.applied != gains {
                self.next = Bank::new(gains);
                self.applied = gains;
                self.remaining = FADE;
            }
            let dry = self.current.run(*frame);
            if self.remaining > 0 {
                let wet = self.next.run(*frame);
                let blend = (FADE - self.remaining + 1) as f32 / FADE as f32;
                *frame = [dry[0] + (wet[0] - dry[0]) * blend,
                    dry[1] + (wet[1] - dry[1]) * blend];
                self.remaining -= 1;
                if self.remaining == 0 {
                    if self.applied == [0.0; 3] {
                        // End the fade at identity, then leave every remaining
                        // source sample untouched, including signed zero.
                        *self = Self::new();
                        return;
                    }
                    self.current = self.next;
                }
            } else { *frame = dry; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bass() -> HeadsetEq {
        HeadsetEq { enabled: true, bass_db: 6.0, mid_db: -1.0, treble_db: 0.0 }
    }

    #[test]
    fn bypass_is_bit_exact_and_resets_on_route_changes() {
        let mut eq = Equalizer::new();
        let original = [[-0.0, 0.3], [f32::from_bits(0x3f000001), -0.7]];
        for (setting, headset) in [(HeadsetEq::default(), true), (bass(), false), (HeadsetEq { enabled: true, ..Default::default() }, true)] {
            let mut data = original;
            eq.process(&setting, headset, &mut data);
            assert_eq!(data.map(|f| f.map(f32::to_bits)), original.map(|f| f.map(f32::to_bits)));
        }
        eq.process(&bass(), true, &mut [[0.8; 2]; 1024]);
        eq.process(&bass(), false, &mut [[0.0; 2]; 32]);
        let mut silence = [[0.0; 2]; 1024];
        eq.process(&bass(), true, &mut silence);
        assert!(silence.iter().flatten().all(|v| *v == 0.0));
    }

    #[test]
    fn bypass_preserves_opus_frames_and_complete_audio_reports() {
        use ds_proto::stream::{self, BLOCKS, HAPTIC_FRAME, OPUS_FRAME, OPUS_SAMPLES};
        fn report(bypass: bool) -> Vec<u8> {
            let mut eq = Equalizer::new();
            let mut squeeze = super::super::dsp::Squeeze::new();
            let mut encoder = super::super::opus::Encoder::new().unwrap();
            let mut counters = stream::StreamCounters::default();
            let mut reports = Vec::new();
            for tick in 0..4 {
                let mut input: Vec<[f32; 2]> = (0..1024).map(|i| {
                    let x = ((tick * 1024 + i) as f32 * 0.03).sin() * 0.2;
                    [x, -x * 0.7]
                }).collect();
                if bypass { eq.process(&HeadsetEq::default(), true, &mut input); }
                let mut fitted = Vec::new();
                squeeze.run(&input, &mut fitted);
                fitted.resize(BLOCKS * OPUS_SAMPLES, [0.0; 2]);
                let mut opus = [[0u8; OPUS_FRAME]; BLOCKS];
                for (block, encoded) in fitted.chunks(OPUS_SAMPLES).zip(opus.iter_mut()) {
                    let mut pcm = [0i16; OPUS_SAMPLES * 2];
                    for (sample, frame) in pcm.chunks_mut(2).zip(block) {
                        for ch in 0..2 { sample[ch] = (frame[ch] * 32767.0).round() as i16; }
                    }
                    assert!(encoder.encode(&pcm, encoded));
                }
                let mut output = [0u8; stream::MAX_REPORT];
                reports.extend_from_slice(stream::build_audio(&mut counters, &opus,
                    &[[0; HAPTIC_FRAME]; BLOCKS], true, &mut output));
            }
            reports
        }
        assert_eq!(report(false), report(true));
    }

    #[test]
    fn active_eq_fades_to_flat_or_disabled_then_bypasses_exactly() {
        for setting in [HeadsetEq { enabled: true, ..Default::default() },
            HeadsetEq { enabled: false, ..bass() }] {
            let mut eq = Equalizer::new();
            let mut warm = [[0.1; 2]; 4800];
            eq.process(&HeadsetEq { treble_db: 6.0, ..bass() }, true, &mut warm);
            let previous = warm.last().unwrap()[0];
            let mut transition = [[0.1; 2]; FADE + 32];
            transition[FADE..].fill([-0.0, f32::from_bits(0x3f000001)]);
            eq.process(&setting, true, &mut transition);
            assert!((transition[0][0] - previous).abs() < 0.001);
            assert!(transition[..FADE].windows(2).all(|f| (f[1][0] - f[0][0]).abs() < 0.001));
            assert!(transition[FADE..].iter().all(|f| f[0].to_bits() == (-0.0f32).to_bits()
                && f[1].to_bits() == 0x3f000001));
            let mut bypass = [[-0.0, 0.7]; 32];
            eq.process(&setting, true, &mut bypass);
            assert!(bypass.iter().all(|f| f[0].to_bits() == (-0.0f32).to_bits() && f[1] == 0.7));
        }
    }

    fn energy(hz: f32) -> f32 {
        let mut eq = Equalizer::new();
        let mut data: Vec<[f32; 2]> = (0..9600).map(|n| [(n as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.1, 0.0]).collect();
        eq.process(&bass(), true, &mut data);
        assert!(data.iter().all(|f| f[1] == 0.0));
        data[4800..].iter().map(|f| f[0] * f[0]).sum()
    }

    #[test]
    fn bass_is_stronger_relative_to_mids_without_stereo_leakage() {
        assert!(energy(60.0) > energy(1000.0) * 2.0);
    }

    #[test]
    fn extreme_finite_samples_recover_and_boosts_keep_tone_headroom() {
        let mut eq = Equalizer::new();
        eq.process(&bass(), true, &mut [[0.0; 2]; 1024]);
        let mut extreme = [[f32::MAX, -f32::MAX]; 32];
        eq.process(&bass(), true, &mut extreme);
        assert!(extreme.iter().flatten().all(|v| v.is_finite()));
        // A new impulse followed by silence must not inherit poisoned state.
        let mut recovered = [[0.0; 2]; 4800];
        recovered[0] = [0.1, -0.1];
        eq.process(&bass(), true, &mut recovered);
        assert!(recovered.iter().flatten().all(|v| v.is_finite()));
        assert!(recovered[0][0].abs() > 0.001);
        assert!(recovered[4700..].iter().flatten().all(|v| v.abs() < 0.001));
        for gains in [[12.0; 3], [-12.0; 3], [12.0, -12.0, 12.0]] {
            let setting = HeadsetEq { enabled: true, bass_db: gains[0], mid_db: gains[1], treble_db: gains[2] };
            for hz in [60.0, 120.0, 1000.0, 6000.0, 12000.0] {
                let mut eq = Equalizer::new();
                let mut tone: Vec<_> = (0..9600).map(|n| [(n as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.8; 2]).collect();
                eq.process(&setting, true, &mut tone);
                assert!(tone.iter().flatten().all(|v| v.is_finite() && v.abs() <= 1.0), "{gains:?} {hz}");
            }
        }
    }

    #[test]
    fn enabled_transition_is_smooth_and_invalid_samples_are_safe() {
        let mut eq = Equalizer::new();
        let mut data = [[0.1; 2]; 2048];
        eq.process(&bass(), true, &mut data);
        assert!((data[0][0] - 0.1).abs() < 0.001);
        assert!(data.windows(2).all(|f| (f[1][0] - f[0][0]).abs() < 0.01));
        let previous = data.last().unwrap()[0];
        let mut changed = [[0.1; 2]; 1024];
        eq.process(&HeadsetEq { enabled: true, bass_db: -6.0, mid_db: 3.0, treble_db: 2.0 }, true, &mut changed);
        assert!((changed[0][0] - previous).abs() < 0.01);
        assert!(changed.windows(2).all(|f| (f[1][0] - f[0][0]).abs() < 0.01));
        let invalid = HeadsetEq { enabled: true, bass_db: f32::NAN, mid_db: f32::INFINITY, treble_db: -100.0 };
        let mut data = [[f32::NAN, f32::INFINITY]; 1024];
        eq.process(&invalid, true, &mut data);
        assert!(data.iter().flatten().all(|v| v.is_finite()));
    }
}
