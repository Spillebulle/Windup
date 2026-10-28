//! Whether a capture is under way on a Wayland host, where the picture is
//! taken on a worker thread and the overlay lives on the Wayland thread. A
//! capture hotkey pressed while one is grabbing or open is ignored, as it is
//! on Windows.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gate {
    #[default]
    Idle,
    /// The picture is being taken.
    Grabbing,
    /// The overlay, or the notice saying why there is none, is up.
    Open,
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

    /// The capture is over, however it went.
    pub fn end(&mut self) {
        *self = Gate::Idle;
    }

    pub fn grabbing(self) -> bool {
        self == Gate::Grabbing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn opened_does_nothing_unless_grabbing() {
        let mut g = Gate::default();
        g.opened();
        assert_eq!(g, Gate::Idle);
        assert!(g.begin());
    }
}
