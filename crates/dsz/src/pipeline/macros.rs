//! Timed presses on the reader clock. A late report releases a key once;
//! it never catches up by sending a burst of missed presses.

use std::time::{Duration, Instant};
use crate::profile::{MacroInput, MacroStep};

pub(crate) const MAX_STEPS: usize = 128;

#[derive(Default, Clone)]
pub(super) struct Playback {
    index: Option<usize>,
    held: Option<MacroInput>,
    deadline: Option<Instant>,
}

impl Playback {
    pub(super) fn running(&self) -> bool { self.index.is_some() }

    pub(super) fn start(&mut self) {
        if !self.running() { self.index = Some(0); self.deadline = None; }
    }

    pub(super) fn cancel(&mut self) -> Option<(MacroInput, bool)> {
        let release = self.held.take().map(|input| (input, false));
        *self = Self::default();
        release
    }

    pub(super) fn held_button(&self) -> u32 {
        match self.held {
            Some(MacroInput::Gamepad { button }) => button.physical().bit(),
            _ => 0,
        }
    }

    pub(super) fn advance(&mut self, steps: &[MacroStep], repeat: bool, now: Instant) -> Option<(MacroInput, bool)> {
        let mut index = self.index?;
        if self.deadline.is_some_and(|at| now < at) { return None; }
        if let Some(input) = self.held.take() {
            let wait = steps.get(index).map_or(0, |s| s.wait_ms.min(60_000));
            self.index = Some(index + 1);
            self.deadline = Some(now + Duration::from_millis(wait as u64));
            return Some((input, false));
        }
        let len = steps.len().min(MAX_STEPS);
        if index >= len {
            if !repeat || len == 0 { self.cancel(); return None; }
            index = 0;
            self.index = Some(0);
        }
        let step = &steps[index];
        self.held = Some(step.input);
        self.deadline = Some(now + Duration::from_millis(step.hold_ms.clamp(1, 60_000) as u64));
        Some((step.input, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{MacroInput, MacroStep};

    fn key(vk: u16) -> MacroStep {
        MacroStep { input: MacroInput::Key { vk, ctrl: false, shift: false, alt: false, win: false }, hold_ms: 20, wait_ms: 10 }
    }

    #[test]
    fn sequence_honors_hold_and_pause_without_catching_up() {
        let steps = [key(0x41), key(0x42)];
        let mut rt = Playback::default();
        let t = Instant::now();
        rt.start();
        assert_eq!(rt.advance(&steps, false, t), Some((steps[0].input, true)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(19)), None);
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(500)), Some((steps[0].input, false)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(509)), None);
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(510)), Some((steps[1].input, true)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(530)), Some((steps[1].input, false)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(540)), None);
        assert!(!rt.running());
    }

    #[test]
    fn repeat_cancel_and_retrigger_release_only_the_held_input() {
        let steps = [key(0x41)];
        let t = Instant::now();
        let mut rt = Playback::default();
        rt.start();
        assert_eq!(rt.advance(&steps, true, t), Some((steps[0].input, true)));
        rt.start(); // A retrigger must not forget a held key.
        assert_eq!(rt.cancel(), Some((steps[0].input, false)));
        assert_eq!(rt.cancel(), None);
        assert_eq!(rt.advance(&steps, true, t), None);
        rt.start();
        assert_eq!(rt.advance(&steps, true, t), Some((steps[0].input, true)));
        assert_eq!(rt.advance(&steps, true, t + Duration::from_millis(20)), Some((steps[0].input, false)));
        assert_eq!(rt.advance(&steps, true, t + Duration::from_millis(30)), Some((steps[0].input, true)));
    }

    #[test]
    fn saved_timings_are_bounded_to_sixty_seconds() {
        let mut rt = Playback::default();
        let steps = [MacroStep { hold_ms: u32::MAX, wait_ms: u32::MAX, ..key(0x41) }];
        let t = Instant::now();
        rt.start();
        assert_eq!(rt.advance(&steps, false, t), Some((steps[0].input, true)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(59_999)), None);
        assert_eq!(rt.advance(&steps, false, t + Duration::from_secs(60)), Some((steps[0].input, false)));
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(119_999)), None);
        assert!(rt.running());
        assert_eq!(rt.advance(&steps, false, t + Duration::from_secs(120)), None);
        assert!(!rt.running());
    }

    #[test]
    fn oversized_sequences_stop_after_the_first_128_steps() {
        let mut rt = Playback::default();
        let steps = vec![MacroStep { hold_ms: 1, wait_ms: 0, ..key(0x41) }; MAX_STEPS + 1];
        let t = Instant::now();
        rt.start();
        for index in 0..MAX_STEPS {
            let at = t + Duration::from_millis(index as u64 * 2);
            assert_eq!(rt.advance(&steps, false, at), Some((steps[0].input, true)));
            assert_eq!(rt.advance(&steps, false, at + Duration::from_millis(1)), Some((steps[0].input, false)));
        }
        assert_eq!(rt.advance(&steps, false, t + Duration::from_millis(MAX_STEPS as u64 * 2)), None);
        assert!(!rt.running());
    }

    #[test]
    fn empty_and_zero_duration_macros_cannot_spin() {
        let mut rt = Playback::default();
        let t = Instant::now();
        rt.start();
        assert_eq!(rt.advance(&[], true, t), None);
        assert!(!rt.running());
        let steps = [MacroStep { hold_ms: 0, wait_ms: 0, ..key(0x41) }];
        rt.start();
        assert_eq!(rt.advance(&steps, true, t), Some((steps[0].input, true)));
        assert_eq!(rt.advance(&steps, true, t), None);
        assert_eq!(rt.advance(&steps, true, t + Duration::from_millis(1)), Some((steps[0].input, false)));
    }
}
