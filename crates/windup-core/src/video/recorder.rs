//! The recording's clock and segment list, with no process in sight.
//! Pausing ends a segment and resuming starts the next, so the join is a
//! plain concatenation of what was actually recorded.
//!
//! A segment's source can take a while to give a picture (the ScreenCast
//! portal waits for its dialog, or for a restore token to be honoured), so
//! the clock of a segment opened with `start_waiting` or `resume_waiting`
//! only runs from `begin`: the timer stays put until something is recorded.

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
    /// When the open segment began; `None` while it waits for its source.
    since: Option<f64>,
    segments: Vec<u32>,
    /// The index of the open segment, or of the one resume would open
    /// minus one.
    current: u32,
}

impl Recorder {
    /// Begins segment 0, recording.
    pub fn start(now: f64) -> Self {
        let mut r = Self::start_waiting();
        r.begin(now);
        r
    }

    /// Begins segment 0 with the clock stopped until `begin`.
    pub fn start_waiting() -> Self {
        Self {
            state: RecState::Recording,
            banked: 0.0,
            since: None,
            segments: Vec::new(),
            current: 0,
        }
    }

    /// The open segment's source gives a picture: its clock runs from
    /// `now`. Does nothing once it runs, or when no segment is open.
    pub fn begin(&mut self, now: f64) {
        if self.state == RecState::Recording && self.since.is_none() {
            self.since = Some(now);
        }
    }

    /// The open segment failed at once and is being opened again: what it
    /// ran for was not recorded, so its clock waits for `begin` once more.
    /// Earlier segments' time stays.
    pub fn rewait(&mut self) {
        if self.state == RecState::Recording {
            self.since = None;
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
            RecState::Recording => self.banked + self.running(now),
            _ => self.banked,
        }
    }

    /// The open segment's time so far; none while it waits.
    fn running(&self, now: f64) -> f64 {
        self.since.map_or(0.0, |since| (now - since).max(0.0))
    }

    fn close(&mut self, now: f64) {
        self.banked += self.running(now);
        self.segments.push(self.current);
    }

    pub fn pause(&mut self, now: f64) {
        if self.state == RecState::Recording {
            self.close(now);
            self.state = RecState::Paused;
        }
    }

    pub fn resume(&mut self, now: f64) {
        self.resume_waiting();
        self.begin(now);
    }

    /// Opens the next segment with the clock stopped until `begin`.
    pub fn resume_waiting(&mut self) {
        if self.state == RecState::Paused {
            self.current += 1;
            self.since = None;
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
    /// it ran long enough; the runtime drops files that do not exist. A
    /// segment still waiting for its source recorded nothing.
    pub fn segment_failed(&mut self, now: f64) {
        if self.state == RecState::Recording {
            if self.since.is_some() && self.running(now) >= MIN_KEPT_SECS {
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
    fn the_clock_waits_for_the_first_segment_s_source() {
        let mut r = Recorder::start_waiting();
        assert_eq!(r.state(), RecState::Recording);
        // The portal's dialog is up for twelve seconds.
        assert_eq!(r.elapsed(12.0), 0.0);
        r.begin(12.0);
        assert_eq!(r.elapsed(15.0), 3.0);
        // A second `begin` does not move the start.
        r.begin(14.0);
        assert_eq!(r.elapsed(15.0), 3.0);
        assert_eq!(r.stop(20.0), Finish::Join(vec![0]));
        assert_eq!(r.elapsed(30.0), 8.0);
    }

    #[test]
    fn a_resume_waiting_for_its_source_does_not_count_the_wait() {
        let mut r = Recorder::start(0.0);
        r.pause(2.0);
        r.resume_waiting();
        assert_eq!(r.state(), RecState::Recording);
        assert_eq!(r.next_segment(), 1);
        assert_eq!(r.elapsed(9.0), 2.0);
        r.begin(9.0);
        assert_eq!(r.elapsed(10.0), 3.0);
        assert_eq!(r.stop(10.0), Finish::Join(vec![0, 1]));
    }

    #[test]
    fn a_source_that_never_came_ends_the_recording_without_its_segment() {
        let mut r = Recorder::start(0.0);
        r.pause(4.0);
        r.resume_waiting();
        r.segment_failed(30.0);
        assert_eq!(r.state(), RecState::Done);
        assert_eq!(r.segments(), &[0]);
        assert_eq!(r.elapsed(31.0), 4.0);
    }

    #[test]
    fn a_retried_segment_does_not_count_the_failed_attempt() {
        let mut r = Recorder::start(0.0);
        r.pause(2.0);
        r.resume(3.0);
        // The attempt dies at 3.8 and is opened again, the source taking
        // until 5.0 to give a picture.
        r.rewait();
        assert_eq!(r.state(), RecState::Recording);
        assert_eq!(r.elapsed(4.5), 2.0);
        r.begin(5.0);
        assert_eq!(r.elapsed(6.0), 3.0);
        assert_eq!(r.stop(6.0), Finish::Join(vec![0, 1]));
        // Nothing to wait for once paused or done.
        r.rewait();
        assert_eq!(r.elapsed(9.0), 3.0);
    }

    #[test]
    fn begin_does_nothing_while_paused() {
        let mut r = Recorder::start(0.0);
        r.pause(1.0);
        r.begin(2.0);
        assert_eq!(r.state(), RecState::Paused);
        assert_eq!(r.elapsed(5.0), 1.0);
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
