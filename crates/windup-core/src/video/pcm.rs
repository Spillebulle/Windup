//! Sound as ffmpeg reads it from Windup's pipes: interleaved f32le at the
//! device's own rate and channel count (ffmpeg resamples, we never do), and
//! the bookkeeping that keeps it in step with the picture.

use std::collections::VecDeque;

/// Why the audio toggles are disabled on a Linux desktop without PipeWire.
pub const SOUND_NEEDS_PIPEWIRE: &str = "Sound needs PipeWire.";

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

/// Keeps a source at real time. ffmpeg times raw sound by counting samples,
/// so a device that sends nothing while nothing plays (WASAPI loopback) would
/// slide everything after it earlier than the picture. `Pad` counts the
/// frames sent since the clock started and says how many silent frames make
/// up a gap longer than `SILENCE_AFTER`.
#[derive(Debug, Default)]
pub struct Pad {
    rate: u32,
    t0: Option<f64>,
    sent: u64,
}

impl Pad {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            t0: None,
            sent: 0,
        }
    }

    /// Starts the clock again at the next `fill`: a new segment.
    pub fn reset(&mut self) {
        self.t0 = None;
        self.sent = 0;
    }

    /// `got` frames arrived by `now` (seconds on any steady clock). Returns
    /// how many frames of silence to send after them.
    pub fn fill(&mut self, now: f64, got: u64) -> u64 {
        let t0 = *self.t0.get_or_insert(now);
        self.sent += got;
        let due = ((now - t0).max(0.0) * f64::from(self.rate)) as u64;
        let behind = due.saturating_sub(self.sent);
        if behind as f64 > SILENCE_AFTER * f64::from(self.rate) {
            self.sent += behind;
            behind
        } else {
            0
        }
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

    #[test]
    fn steady_sound_needs_no_padding() {
        let mut pad = Pad::new(1000);
        assert_eq!(pad.fill(10.0, 0), 0, "the clock starts here");
        // A period every 10 ms, and a late one now and then.
        assert_eq!(pad.fill(10.01, 10), 0);
        assert_eq!(pad.fill(10.15, 10), 0);
        assert_eq!(pad.fill(10.16, 150), 0);
    }

    #[test]
    fn a_silent_device_is_padded_to_real_time() {
        let mut pad = Pad::new(1000);
        pad.fill(5.0, 0);
        assert_eq!(pad.fill(5.1, 0), 0, "still within a period");
        assert_eq!(pad.fill(5.5, 0), 500);
        assert_eq!(pad.fill(5.6, 0), 0);
        // Sound again, and it is not padded on top of.
        assert_eq!(pad.fill(5.61, 10), 0);
        assert_eq!(pad.fill(6.0, 0), 490);
    }

    #[test]
    fn a_device_ahead_of_the_clock_is_not_padded() {
        let mut pad = Pad::new(1000);
        pad.fill(0.0, 2000);
        assert_eq!(pad.fill(1.0, 0), 0);
        assert_eq!(pad.fill(2.5, 0), 500);
    }

    #[test]
    fn reset_starts_the_clock_again() {
        let mut pad = Pad::new(1000);
        pad.fill(0.0, 0);
        pad.fill(1.0, 0);
        pad.reset();
        // Paused for a minute: nothing owed for it.
        assert_eq!(pad.fill(61.0, 0), 0);
        assert_eq!(pad.fill(61.5, 0), 500);
    }
}
