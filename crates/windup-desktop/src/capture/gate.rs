//! Whether a capture is under way on a Wayland host, where the picture is
//! taken on a worker thread and the overlay lives on the Wayland thread. A
//! capture hotkey pressed while one is grabbing or open, or while a
//! recording runs, is ignored, as it is on Windows.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gate {
    #[default]
    Idle,
    /// The picture is being taken.
    Grabbing,
    /// The overlay, or the notice saying why there is none, is up.
    Open,
    /// The overlay closed on Record: the recording starts, runs and is
    /// joined. Ends with `end` once the file is saved or the start failed.
    Recording,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Gate {
    /// Starts a capture. False, and nothing changes, when one is under way.
    pub fn begin(&mut self) -> bool {
        if *self != Gate::Idle {
            return false;
        }
        *self = Gate::Grabbing;
        true
    }

    /// The picture is in and the overlay asked for. Only from `Grabbing`.
    pub fn opened(&mut self) {
        if *self == Gate::Grabbing {
            *self = Gate::Open;
        }
    }

    /// The overlay closed on Record; the gate stays shut until the
    /// recording has ended.
    pub fn recording(&mut self) {
        *self = Gate::Recording;
    }

    /// The capture is over, however it went.
    pub fn end(&mut self) {
        *self = Gate::Idle;
    }

    pub fn grabbing(self) -> bool {
        self == Gate::Grabbing
    }
}

/// The worker's way back: it sends its result with [`Reply::send`], and a
/// worker that ends without one (a panic in the grab, the decoding, anything)
/// sends `fallback` as it unwinds. The gate then always hears back, and never
/// stays `Grabbing` until a restart.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct Reply<T> {
    tx: std::sync::mpsc::Sender<T>,
    fallback: Option<T>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl<T> Reply<T> {
    pub fn new(tx: std::sync::mpsc::Sender<T>, fallback: T) -> Self {
        Self {
            tx,
            fallback: Some(fallback),
        }
    }

    pub fn send(mut self, value: T) {
        self.fallback = None;
        let _ = self.tx.send(value);
    }
}

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        if let Some(fallback) = self.fallback.take() {
            let _ = self.tx.send(fallback);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    #[test]
    fn a_second_capture_is_ignored_while_one_grabs_or_is_open() {
        let mut g = Gate::default();
        assert!(g.begin());
        assert!(g.grabbing());
        assert!(!g.begin(), "while grabbing");
        assert_eq!(g, Gate::Grabbing);
        g.opened();
        assert_eq!(g, Gate::Open);
        assert!(!g.grabbing());
        assert!(!g.begin(), "while open");
        assert_eq!(g, Gate::Open);
    }

    #[test]
    fn after_the_end_the_next_capture_starts() {
        let mut g = Gate::default();
        assert!(g.begin());
        g.end();
        assert_eq!(g, Gate::Idle);
        assert!(g.begin());
        g.opened();
        g.end();
        assert!(g.begin());
    }

    /// What the background loop does with whatever the worker sends: any
    /// failure ends the capture.
    fn drain(g: &mut Gate, rx: &Receiver<Result<(), String>>) {
        while let Ok(r) = rx.try_recv() {
            if r.is_err() {
                g.end();
            }
        }
    }

    #[test]
    fn a_panicking_job_still_reports_and_the_gate_reopens() {
        let mut g = Gate::default();
        assert!(g.begin());
        let (tx, rx) = channel::<Result<(), String>>();
        let reply = Reply::new(tx, Err("the grab thread panicked".to_owned()));
        let joined = std::thread::spawn(move || {
            let _reply = reply;
            panic!("in the grab");
        })
        .join();
        assert!(joined.is_err());
        assert_eq!(
            rx.try_recv(),
            Ok(Err("the grab thread panicked".to_owned()))
        );
        g.end();
        assert!(g.begin(), "the next capture starts");
    }

    #[test]
    fn a_dropped_job_reports_once_and_a_finished_one_only_its_result() {
        let mut g = Gate::default();
        assert!(g.begin());
        let (tx, rx) = channel();
        drop(Reply::new(tx.clone(), Err("dropped".to_owned())));
        drain(&mut g, &rx);
        assert!(g.begin());

        Reply::new(tx, Err("dropped".to_owned())).send(Ok(()));
        assert_eq!(rx.try_recv(), Ok(Ok(())));
        assert!(rx.try_recv().is_err(), "nothing more after the result");
    }

    #[test]
    fn a_capture_hotkey_is_ignored_while_recording() {
        let mut g = Gate::default();
        assert!(g.begin());
        g.opened();
        // The overlay closed on Record: the gate stays shut.
        g.recording();
        assert_eq!(g, Gate::Recording);
        assert!(!g.begin(), "while recording");
        assert_eq!(g, Gate::Recording);
        assert!(!g.grabbing());
    }

    #[test]
    fn ending_a_recording_reopens_the_gate() {
        let mut g = Gate::default();
        assert!(g.begin());
        g.opened();
        g.recording();
        g.end();
        assert_eq!(g, Gate::Idle);
        assert!(g.begin(), "the next capture starts");
    }

    #[test]
    fn opened_does_nothing_unless_grabbing() {
        let mut g = Gate::default();
        g.opened();
        assert_eq!(g, Gate::Idle);
        assert!(g.begin());
    }
}
