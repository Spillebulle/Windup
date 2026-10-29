//! The recording's clock and segment list, with no process in sight.
//! Pausing ends a segment and resuming starts the next, so the join is a
//! plain concatenation of what was actually recorded.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecState {
    Recording,
    Paused,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Join these segment indices, in order.
    Join(Vec<u32>),
    Discard,
}

/// A segment that failed sooner than this left nothing worth keeping.
const MIN_KEPT_SECS: f64 = 0.5;

#[derive(Clone, Debug)]
pub struct Recorder {
    state: RecState,
    /// Recorded time of the segments already closed.
    banked: f64,
    /// When the open segment began.
    since: f64,
    segments: Vec<u32>,
    /// The index of the open segment, or of the one resume would open
    /// minus one.
    current: u32,
}

impl Recorder {
    /// Begins segment 0, recording.
    pub fn start(now: f64) -> Self {
        Self {
            state: RecState::Recording,
            banked: 0.0,
            since: now,
            segments: Vec::new(),
            current: 0,
        }
    }

    pub fn state(&self) -> RecState {
        self.state
    }

    /// Indices of the segments closed so far.
    pub fn segments(&self) -> &[u32] {
        &self.segments
    }

    /// The index of the segment open now, or the one resume would open.
    pub fn next_segment(&self) -> u32 {
        match self.state {
            RecState::Recording => self.current,
            _ => self.current + 1,
        }
    }

    /// Time spent recording; pauses do not count.
    pub fn elapsed(&self, now: f64) -> f64 {
        match self.state {
            RecState::Recording => self.banked + (now - self.since).max(0.0),
            _ => self.banked,
        }
    }

    fn close(&mut self, now: f64) {
        self.banked += (now - self.since).max(0.0);
        self.segments.push(self.current);
    }

    pub fn pause(&mut self, now: f64) {
        if self.state == RecState::Recording {
            self.close(now);
            self.state = RecState::Paused;
        }
    }

    pub fn resume(&mut self, now: f64) {
        if self.state == RecState::Paused {
            self.current += 1;
            self.since = now;
            self.state = RecState::Recording;
        }
    }

    pub fn stop(&mut self, now: f64) -> Finish {
        match self.state {
            RecState::Recording => self.close(now),
            RecState::Paused => {}
            RecState::Done => return Finish::Join(self.segments.clone()),
        }
        self.state = RecState::Done;
        Finish::Join(self.segments.clone())
    }

    pub fn cancel(&mut self) -> Finish {
        self.state = RecState::Done;
        Finish::Discard
    }

    /// The encoder died. Its file may hold partial output, so it is kept if
    /// it ran long enough; the runtime drops files that do not exist.
    pub fn segment_failed(&mut self, now: f64) {
        if self.state == RecState::Recording {
            if now - self.since >= MIN_KEPT_SECS {
                self.close(now);
            }
            self.state = RecState::Done;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_counts_only_recorded_time() {
        let mut r = Recorder::start(0.0);
        assert_eq!(r.next_segment(), 0);
        r.pause(2.0);
        assert_eq!(r.elapsed(4.0), 2.0);
        assert_eq!(r.next_segment(), 1);
        r.resume(5.0);
        assert_eq!(r.state(), RecState::Recording);
        assert_eq!(r.stop(8.0), Finish::Join(vec![0, 1]));
        assert_eq!(r.elapsed(8.0), 5.0);
        assert_eq!(r.state(), RecState::Done);
    }

    #[test]
    fn cancel_after_pause_discards() {
        let mut r = Recorder::start(0.0);
        r.pause(1.0);
        assert_eq!(r.cancel(), Finish::Discard);
        assert_eq!(r.state(), RecState::Done);
    }

    #[test]
    fn a_failed_segment_ends_the_recording_and_keeps_what_was_recorded() {
        let mut r = Recorder::start(0.0);
        r.pause(2.0);
        r.resume(3.0);
        r.segment_failed(6.0);
        assert_eq!(r.state(), RecState::Done);
        assert_eq!(r.segments(), &[0, 1]);
        assert_eq!(r.stop(7.0), Finish::Join(vec![0, 1]));
    }

    #[test]
    fn a_segment_that_died_at_once_is_dropped() {
        let mut r = Recorder::start(0.0);
        r.pause(2.0);
        r.resume(3.0);
        r.segment_failed(3.2);
        assert_eq!(r.state(), RecState::Done);
        assert_eq!(r.segments(), &[0]);
    }

    #[test]
    fn pause_while_paused_does_nothing() {
        let mut r = Recorder::start(0.0);
        r.pause(2.0);
        r.pause(4.0);
        assert_eq!(r.segments(), &[0]);
        assert_eq!(r.elapsed(9.0), 2.0);
        r.resume(5.0);
        r.resume(6.0);
        assert_eq!(r.elapsed(7.0), 4.0);
        assert_eq!(r.next_segment(), 1);
    }
}
