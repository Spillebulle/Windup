//! Frame pacing for piped screen frames: frame `k` is due at `start + k / fps`,
//! so a capture that delivers late or not at all still writes a steady stream.

/// Tracks how many frames the encoder has been given against how many the
/// clock says it should have.
#[derive(Clone, Debug)]
pub struct Pacer {
    pub fps: u32,
    pub start: f64,
    pub sent: u64,
}

impl Pacer {
    pub fn new(fps: u32, start: f64) -> Self {
        Self {
            fps: fps.max(1),
            start,
            sent: 0,
        }
    }

    /// How many frames to write at `now` (seconds on the same clock as `start`).
    ///
    /// Frame `k` is due at `start + k / fps`, so the first call at `start` is
    /// owed one frame. A late call is owed the frames it missed, but at most
    /// half a second of them, so a long stall is not replayed in a burst. The
    /// rest of the backlog is written off, otherwise the pacer would keep
    /// running fast long after the stall had ended.
    pub fn due(&mut self, now: f64) -> u64 {
        if now < self.start {
            return 0;
        }
        // Nudge past float error, so `start + k / fps` counts as frame `k` and
        // not `k - 1`. A ten-millionth of a frame is far below anything
        // a capture clock can resolve.
        const EPS: f64 = 1e-7;
        let frames = (now - self.start) * f64::from(self.fps);
        let total = (frames + EPS).floor() as u64 + 1;
        if total <= self.sent {
            return 0;
        }
        let owed = total - self.sent;
        let cap = u64::from((self.fps / 2).max(1));
        // Past the cap, write one cap's worth now and count the rest of the
        // backlog as handled, so the next on-time call is back to one.
        self.sent = total;
        owed.min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_calls_at_frame_times_give_one_each() {
        let mut p = Pacer::new(30, 0.0);
        for k in 0..30 {
            let now = k as f64 / 30.0;
            assert_eq!(p.due(now), 1, "frame {k}");
        }
    }

    #[test]
    fn a_200_ms_stall_catches_up_then_settles_to_one() {
        let mut p = Pacer::new(30, 0.0);
        // Frames 0..=5 are on time.
        for k in 0..6 {
            assert_eq!(p.due(k as f64 / 30.0), 1);
        }
        // Next call is 200 ms later, exactly at frame 11: six frames owed.
        let stalled = 5.0 / 30.0 + 0.2;
        assert_eq!(p.due(stalled), 6);
        assert_eq!(p.due(stalled + 1.0 / 30.0), 1);
    }

    #[test]
    fn a_10_s_stall_is_capped_and_does_not_catch_up_for_ever() {
        let mut p = Pacer::new(30, 0.0);
        assert_eq!(p.due(0.0), 1);
        assert_eq!(p.due(10.0), 15);
        // The backlog beyond the cap was dropped, so the next on-time call is 1.
        assert_eq!(p.due(10.0 + 1.0 / 30.0), 1);
    }

    #[test]
    fn before_start_gives_zero() {
        let mut p = Pacer::new(30, 5.0);
        assert_eq!(p.due(4.99), 0);
        assert_eq!(p.due(0.0), 0);
        assert_eq!(p.sent, 0);
        assert_eq!(p.due(5.0), 1);
    }

    #[test]
    fn exact_multiples_of_frame_time_do_not_flicker_on_float_rounding() {
        for start in [0.0, 12.345, 98765.4321] {
            let mut p = Pacer::new(30, start);
            for k in 0..300 {
                let now = start + k as f64 / 30.0;
                assert_eq!(p.due(now), 1, "start {start}, frame {k}");
            }
        }
    }
}
