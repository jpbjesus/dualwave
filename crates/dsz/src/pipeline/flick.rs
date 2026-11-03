//! Horizontal camera turns, advanced only by validated controller input.

#[derive(Default)]
pub(super) struct Flick {
    angle: Option<f32>,
    pending: f32,
    remaining: f32,
}

impl Flick {
    pub(super) fn process(&mut self, x: u8, y: u8, dt: f32, pixels_per_turn: f32) -> f32 {
        if !pixels_per_turn.is_finite() || pixels_per_turn <= 0.0 {
            *self = Self::default();
            return 0.0;
        }
        let x = (x as f32 - 128.0) / 127.0;
        let y = (y as f32 - 128.0) / 127.0;
        let radius = x.hypot(y);
        let mut turn = 0.0;
        // Hysteresis keeps a finger near the activation radius from
        // repeatedly starting the same flick.
        if radius >= if self.angle.is_some() { 0.8 } else { 0.9 } {
            let angle = x.atan2(-y).to_degrees();
            if let Some(last) = self.angle {
                turn = (angle - last + 180.0).rem_euclid(360.0) - 180.0;
            } else {
                self.pending += angle;
                self.remaining = 0.1;
            }
            self.angle = Some(angle);
        } else {
            self.angle = None;
        }
        if self.remaining > 0.0 {
            let dt = if dt.is_finite() { dt.clamp(0.0, 0.03) } else { 0.0 };
            let amount = self.pending * (dt / self.remaining).min(1.0);
            self.pending -= amount;
            self.remaining = (self.remaining - dt).max(0.0);
            turn += amount;
        }
        turn * pixels_per_turn.min(100_000.0) / 360.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(f: &mut Flick, x: u8, y: u8) -> f32 {
        (0..30).map(|_| f.process(x, y, 0.004, 360.0)).sum()
    }

    #[test]
    fn cardinal_flicks_and_no_drift_while_held() {
        for (x, y, expected) in [(128, 0, 0.0), (255, 128, 90.0), (128, 255, 180.0), (0, 128, -90.0)] {
            let mut f = Flick::default();
            assert!((settle(&mut f, x, y) - expected).abs() < 0.01);
            assert!(settle(&mut f, x, y).abs() < 0.001);
        }
    }

    #[test]
    fn rotation_crosses_back_without_a_full_turn() {
        let mut f = Flick::default();
        settle(&mut f, 140, 255);
        let dx = f.process(116, 255, 0.004, 360.0);
        assert!(dx > 5.0 && dx < 15.0, "{dx}");
    }

    #[test]
    fn lifting_finishes_turn_and_rearms() {
        let mut f = Flick::default();
        let start = f.process(255, 128, 0.004, 360.0);
        assert!(start > 0.0 && start < 90.0);
        assert!((start + settle(&mut f, 128, 128) - 90.0).abs() < 0.01);
        assert!((settle(&mut f, 0, 128) + 90.0).abs() < 0.01);
    }

    #[test]
    fn neutral_and_invalid_sensitivity_never_move() {
        let mut f = Flick::default();
        assert_eq!(settle(&mut f, 128, 128), 0.0);
        for scale in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert_eq!(f.process(255, 128, 0.004, scale), 0.0);
        }
    }
}
