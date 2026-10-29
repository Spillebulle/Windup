//! Sound as ffmpeg reads it from Windup's pipes: interleaved f32le at the
//! device's own rate and channel count (ffmpeg resamples, we never do), and
//! the bookkeeping that keeps it in step with the picture.

use std::collections::VecDeque;

/// Why the audio toggles are disabled on a Linux desktop without PipeWire.
pub const SOUND_NEEDS_PIPEWIRE: &str = "Sound needs PipeWire.";

/// Why a sound toggle is disabled when its kind of device is missing.
pub const NO_SOUND_DEVICE: &str = "No sound device was found.";

/// Why the sound toggles are disabled while recording a GIF.
pub const GIF_HAS_NO_SOUND: &str = "GIF has no sound.";

/// Why the sound toggles are disabled before the first look for devices is
/// back (it can take seconds on Linux, so it never holds up the overlay).
pub const SOUND_STILL_CHECKING: &str = "Windup is still checking for sound devices.";

/// One 16-bit sample as f32, -32768 being exactly -1.
pub fn i16_to_f32(s: i16) -> f32 {
    f32::from(s) / 32768.0
}

/// 16-bit samples, for devices that deliver them, as f32le bytes.
pub fn to_f32le(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend(i16_to_f32(*s).to_le_bytes());
    }
    out
}

/// Appends `samples` to `out` as f32le bytes.
pub fn put_f32le(samples: &[f32], out: &mut Vec<u8>) {
    out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
}

/// What a device delivered and nobody has read yet. Bounded: past `cap`
/// samples the oldest whole frames go, so a source nobody reads (between
/// segments, while paused) never grows.
pub struct Ring {
    buf: VecDeque<f32>,
    cap: usize,
    channels: usize,
}

impl Ring {
    /// Holds at most `secs` of sound.
    pub fn new(rate: u32, channels: u16, secs: f32) -> Self {
        let channels = usize::from(channels.max(1));
        let frames = (rate as f32 * secs).ceil().max(1.0) as usize;
        Self {
            buf: VecDeque::new(),
            cap: frames * channels,
            channels,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        // Only the newest `cap` of a push that big could stay anyway.
        let keep = samples.len().min(self.cap);
        self.buf.extend(&samples[samples.len() - keep..]);
        let over = self.buf.len().saturating_sub(self.cap);
        // Round up to whole frames, so channels never swap.
        let over = over.div_ceil(self.channels) * self.channels;
        self.buf.drain(..over.min(self.buf.len()));
    }

    /// Moves everything held to the end of `out`.
    pub fn drain_into(&mut self, out: &mut Vec<f32>) {
        out.extend(self.buf.drain(..));
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// How far behind the clock a source may fall before the gap is taken for
/// silence. Devices deliver in periods (WASAPI ~10 ms, a PipeWire quantum
/// up to ~170 ms), and a gap shorter than that is just the next period on
/// its way.
pub const SILENCE_AFTER: f64 = 0.2;

/// Silent frames to send around what arrived: `before` it, then `after`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fill {
    pub before: u64,
    pub after: u64,
}

/// Keeps a source at real time. The sound is timed by counting samples, so
/// a device that sends nothing while nothing plays (WASAPI loopback) would
/// slide everything after it earlier than the picture. `Pad` counts the
/// frames sent since the clock started against the clock:
/// - while nothing arrives, a gap longer than `SILENCE_AFTER` is filled with
///   silence up to now, and the source counts as quiet;
/// - when sound arrives while quiet (and at the start, which is quiet too),
///   the silence still owed goes before it, so it ends at real time;
/// - while sound flows the device keeps time itself and nothing is added,
///   so a late period is never padded on top of.
#[derive(Debug, Default)]
pub struct Pad {
    rate: u32,
    t0: Option<f64>,
    sent: u64,
    quiet: bool,
}

impl Pad {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            t0: None,
            sent: 0,
            quiet: true,
        }
    }

    /// Starts the clock again at the next `fill`: a new segment.
    pub fn reset(&mut self) {
        self.t0 = None;
        self.sent = 0;
        self.quiet = true;
    }

    /// Frames due by `now` since the clock started.
    fn due(&mut self, now: f64) -> u64 {
        let t0 = *self.t0.get_or_insert(now);
        ((now - t0).max(0.0) * f64::from(self.rate)) as u64
    }

    /// `got` frames arrived by `now` (seconds on any steady clock). Says how
    /// much silence goes before and after them.
    pub fn fill(&mut self, now: f64, got: u64) -> Fill {
        let due = self.due(now);
        let mut fill = Fill::default();
        if got > 0 {
            if self.quiet {
                // The frames that arrived end now; what is still missing
                // before them was quiet.
                fill.before = due.saturating_sub(self.sent + got);
                self.quiet = false;
            }
        } else {
            let behind = due.saturating_sub(self.sent);
            if behind as f64 > SILENCE_AFTER * f64::from(self.rate) {
                fill.after = behind;
                self.quiet = true;
            }
        }
        self.sent += fill.before + got + fill.after;
        fill
    }

    /// The segment ends: all the silence owed up to `now`, however little.
    pub fn flush(&mut self, now: f64) -> u64 {
        let owed = self.due(now).saturating_sub(self.sent);
        self.sent += owed;
        owed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_samples_become_f32le() {
        let bytes = to_f32le(&[-32768, 0, 32767]);
        let mut want = Vec::new();
        want.extend((-1.0f32).to_le_bytes());
        want.extend(0.0f32.to_le_bytes());
        want.extend((32767.0f32 / 32768.0).to_le_bytes());
        assert_eq!(bytes, want);
        assert_eq!(&bytes[..4], &[0x00, 0x00, 0x80, 0xBF]);
    }

    #[test]
    fn put_f32le_appends() {
        let mut out = vec![9];
        put_f32le(&[1.0], &mut out);
        assert_eq!(out, [9, 0x00, 0x00, 0x80, 0x3F]);
    }

    #[test]
    fn a_ring_keeps_the_newest_whole_frames() {
        // 4 frames of stereo at most.
        let mut ring = Ring::new(4, 2, 1.0);
        ring.push(&[1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
        ring.push(&[4.0, 4.0, 5.0, 5.0, 6.0, 6.0]);
        assert_eq!(ring.len(), 8);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, [3.0, 3.0, 4.0, 4.0, 5.0, 5.0, 6.0, 6.0]);
        assert!(ring.is_empty());
    }

    #[test]
    fn a_push_bigger_than_the_ring_keeps_its_end() {
        let mut ring = Ring::new(2, 1, 1.0);
        ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, [4.0, 5.0]);
    }

    #[test]
    fn a_cleared_ring_is_empty() {
        let mut ring = Ring::new(48000, 2, 2.0);
        ring.push(&[0.5; 960]);
        ring.clear();
        assert!(ring.is_empty());
    }

    fn fill(pad: &mut Pad, now: f64, got: u64) -> (u64, u64) {
        let f = pad.fill(now, got);
        (f.before, f.after)
    }

    #[test]
    fn steady_sound_needs_no_padding() {
        let mut pad = Pad::new(1000);
        assert_eq!(fill(&mut pad, 10.0, 0), (0, 0), "the clock starts here");
        // A period every 10 ms, and a late one now and then.
        assert_eq!(fill(&mut pad, 10.01, 10), (0, 0));
        assert_eq!(fill(&mut pad, 10.15, 10), (0, 0));
        assert_eq!(fill(&mut pad, 10.16, 150), (0, 0));
    }

    #[test]
    fn a_silent_device_is_padded_to_real_time() {
        let mut pad = Pad::new(1000);
        fill(&mut pad, 5.0, 0);
        assert_eq!(fill(&mut pad, 5.1, 0), (0, 0), "still within a period");
        assert_eq!(fill(&mut pad, 5.5, 0), (0, 500));
        assert_eq!(fill(&mut pad, 5.6, 0), (0, 0));
        // Sound again: the 100 ms of quiet not yet paid go before it, so it
        // ends at real time instead of 100 ms early.
        assert_eq!(fill(&mut pad, 5.61, 10), (100, 0));
        assert_eq!(fill(&mut pad, 5.62, 10), (0, 0));
        assert_eq!(fill(&mut pad, 6.0, 0), (0, 380));
    }

    #[test]
    fn sound_after_quiet_starts_on_time() {
        // WASAPI loopback: nothing at all until something plays at 0.14 s,
        // its first period arriving at 0.15 s.
        let mut pad = Pad::new(1000);
        fill(&mut pad, 0.0, 0);
        assert_eq!(fill(&mut pad, 0.1, 0), (0, 0));
        assert_eq!(fill(&mut pad, 0.15, 10), (140, 0));
        // From then on the device keeps time itself.
        assert_eq!(fill(&mut pad, 0.16, 10), (0, 0));
        assert_eq!(
            fill(&mut pad, 0.27, 10),
            (0, 0),
            "a late period is not quiet"
        );
        assert_eq!(fill(&mut pad, 0.28, 100), (0, 0));
    }

    #[test]
    fn a_device_ahead_of_the_clock_is_not_padded() {
        let mut pad = Pad::new(1000);
        assert_eq!(fill(&mut pad, 0.0, 2000), (0, 0));
        assert_eq!(fill(&mut pad, 1.0, 0), (0, 0));
        assert_eq!(fill(&mut pad, 2.5, 0), (0, 500));
    }

    #[test]
    fn reset_starts_the_clock_again() {
        let mut pad = Pad::new(1000);
        fill(&mut pad, 0.0, 0);
        fill(&mut pad, 1.0, 0);
        pad.reset();
        // Paused for a minute: nothing owed for it.
        assert_eq!(fill(&mut pad, 61.0, 0), (0, 0));
        assert_eq!(fill(&mut pad, 61.5, 0), (0, 500));
    }

    #[test]
    fn flush_pays_all_that_is_owed() {
        let mut pad = Pad::new(1000);
        fill(&mut pad, 0.0, 0);
        assert_eq!(fill(&mut pad, 0.1, 0), (0, 0));
        // The segment ends: the last 150 ms are owed even below the threshold.
        assert_eq!(pad.flush(0.15), 150);
        assert_eq!(pad.flush(0.15), 0);
    }
}
