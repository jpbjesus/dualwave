//! Temporary mouse turn for checking a game's sensitivity.
use std::time::{Duration, Instant};

pub struct Turn {
    profile: String,
    pub left: bool,
    pixels: f32,
    start: Instant,
    last: Instant,
    sent: i32,
}

impl Turn {
    pub fn new(profile: &str, enabled: bool, pixels: f32, now: Instant) -> Option<Self> {
        if !enabled || !pixels.is_finite() || !(1.0..=100_000.0).contains(&pixels) { return None; }
        let start = now + Duration::from_secs(5);
        Some(Self { profile: profile.into(), left: false, pixels, start, last: start, sent: 0 })
    }

    pub fn step(&mut self, profile: &str, enabled: bool, pixels: f32, now: Instant) -> Option<i32> {
        if self.profile != profile || !enabled || self.pixels != pixels || self.sent == self.pixels.round() as i32 {
            return None;
        }
        if now < self.start { return Some(0); }
        // Never replay a backlog after a stalled reader or a sleeping controller.
        if now.saturating_duration_since(self.last) > Duration::from_millis(100) { return None; }
        let elapsed = now.duration_since(self.start).as_secs_f64().min(1.0);
        let target = (self.pixels as f64 * elapsed).round() as i32;
        let dx = target - self.sent;
        self.sent = target;
        self.last = now;
        Some(dx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_then_emits_exactly_one_turn_and_stops() {
        let now = Instant::now();
        let mut turn = Turn::new("Game", true, 4320.0, now).unwrap();
        assert_eq!(turn.step("Game", true, 4320.0, now), Some(0));
        assert_eq!(turn.step("Game", true, 4320.0, now + Duration::from_secs(5)), Some(0));
        let sum: i32 = (1..=100).map(|i| turn.step("Game", true, 4320.0,
            now + Duration::from_millis(5000 + i * 10)).unwrap()).sum();
        assert_eq!(sum, 4320);
        assert_eq!(turn.step("Game", true, 4320.0, now + Duration::from_millis(6010)), None);
    }

    #[test]
    fn changes_and_stale_packets_cancel_without_catching_up() {
        let now = Instant::now();
        for (name, enabled, pixels, elapsed) in [
            ("Other", true, 4320.0, 0), ("Game", false, 4320.0, 0),
            ("Game", true, 5000.0, 0), ("Game", true, 4320.0, 7000),
        ] {
            let mut turn = Turn::new("Game", true, 4320.0, now).unwrap();
            assert_eq!(turn.step(name, enabled, pixels, now + Duration::from_millis(elapsed)), None);
        }
        let mut turn = Turn::new("Game", true, 4320.0, now).unwrap();
        turn.step("Game", true, 4320.0, now + Duration::from_secs(5));
        assert_eq!(turn.step("Game", true, 4320.0, now + Duration::from_millis(5500)), None);
    }

    #[test]
    fn rejects_invalid_or_disabled_requests() {
        for pixels in [f32::NAN, f32::INFINITY, 0.0, -1.0, 100001.0] {
            assert!(Turn::new("Game", true, pixels, Instant::now()).is_none());
        }
        assert!(Turn::new("Game", false, 4320.0, Instant::now()).is_none());
    }
}
