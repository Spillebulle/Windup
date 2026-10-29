//! Computer sound and the microphone, as `AudioSource`s a recording writes to
//! ffmpeg. Windows reads them through WASAPI (cpal; computer sound is an
//! input stream on the output device, which WASAPI turns into loopback),
//! Linux through PipeWire (`crate::linux::video::audio`).
//!
//! Every device feeds the same `Captured` source. The device's own thread
//! pushes into a bounded `Ring` (2 s, oldest dropped), and the segment's
//! writer drains it. While a recording is paused nobody drains it, and the
//! writer's `restart` at the next segment's start empties it, so sound from
//! a pause never lands in the next segment. `Pad` fills the gaps of a device
//! that sends nothing while nothing plays, so the sound keeps the picture's
//! pace. Rates and channel counts are the device's; ffmpeg resamples.

use super::run::AudioSource;

/// The sound devices settings can offer, by the names `open_system` and
/// `open_mic` take.
// Task 11's settings and bar are the callers.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AudioDevices {
    /// Where computer sound can be recorded from.
    pub outputs: Vec<String>,
    /// Microphones.
    pub inputs: Vec<String>,
    /// False when there is no sound system to ask (Linux without PipeWire).
    pub available: bool,
}

/// The source every device feeds. Unused where there is no sound backend
/// (Linux built without PipeWire, macOS).
#[cfg_attr(
    not(any(target_os = "windows", all(target_os = "linux", feature = "pipewire"))),
    allow(unused_imports)
)]
pub(crate) use captured::{Captured, SharedRing, ring};

#[cfg_attr(
    not(any(target_os = "windows", all(target_os = "linux", feature = "pipewire"))),
    allow(dead_code, unused_imports)
)]
mod captured {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use windup_core::video::pcm::{Pad, Ring};

    use super::AudioSource;

    /// How much sound a source holds for nobody.
    const RING_SECS: f32 = 2.0;

    pub(crate) type SharedRing = Arc<Mutex<Ring>>;

    pub(crate) fn ring(rate: u32, channels: u16) -> SharedRing {
        Arc::new(Mutex::new(Ring::new(rate, channels, RING_SECS)))
    }

    /// A device's sound as the recording reads it.
    pub(crate) struct Captured {
        rate: u32,
        channels: u16,
        ring: SharedRing,
        pad: Pad,
        t0: Instant,
        /// Set by the device's thread when the device goes away.
        lost: Arc<AtomicBool>,
        /// Whatever keeps the device running; dropping it stops the device.
        _device: Box<dyn Send>,
    }

    impl Captured {
        pub(crate) fn new(
            rate: u32,
            channels: u16,
            ring: SharedRing,
            lost: Arc<AtomicBool>,
            device: Box<dyn Send>,
        ) -> Self {
            Self {
                rate,
                channels,
                ring,
                pad: Pad::new(rate),
                t0: Instant::now(),
                lost,
                _device: device,
            }
        }

        fn now(&self) -> f64 {
            self.t0.elapsed().as_secs_f64()
        }

        fn silence(&self, buf: &mut Vec<f32>, frames: u64) {
            let n = frames as usize * usize::from(self.channels.max(1));
            buf.extend(std::iter::repeat_n(0.0, n));
        }
    }

    impl AudioSource for Captured {
        fn rate(&self) -> u32 {
            self.rate
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String> {
            let mut got = Vec::new();
            if let Ok(mut ring) = self.ring.lock() {
                ring.drain_into(&mut got);
            }
            let frames = (got.len() / usize::from(self.channels.max(1))) as u64;
            let fill = self.pad.fill(self.now(), frames);
            self.silence(buf, fill.before);
            buf.extend_from_slice(&got);
            self.silence(buf, fill.after);
            Ok(())
        }

        fn finish(&mut self, buf: &mut Vec<f32>) -> Result<(), String> {
            self.read(buf)?;
            let owed = self.pad.flush(self.now());
            self.silence(buf, owed);
            Ok(())
        }

        fn lost(&self) -> Option<Arc<AtomicBool>> {
            Some(Arc::clone(&self.lost))
        }

        fn restart(&mut self) {
            if let Ok(mut ring) = self.ring.lock() {
                ring.clear();
            }
            self.pad.reset();
        }
    }
}

/// The devices there are to record from.
#[allow(dead_code)]
pub(crate) fn audio_devices() -> AudioDevices {
    platform::audio_devices()
}

/// Computer sound: what `device` plays (an `outputs` name), else the default
/// output.
pub(crate) fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
    platform::open_system(device)
}

/// A microphone (an `inputs` name), else the default input.
pub(crate) fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
    platform::open_mic(device)
}

#[cfg(target_os = "linux")]
use crate::linux::video::audio as platform;

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{Device, SampleFormat};
    use windup_core::video::pcm::i16_to_f32;

    use super::{AudioDevices, AudioSource, Captured, SharedRing, ring};

    /// Longest a device may take to start.
    const START_TIMEOUT: Duration = Duration::from_secs(5);

    fn names(devices: Result<impl Iterator<Item = Device>, cpal::DevicesError>) -> Vec<String> {
        devices
            .map(|d| d.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn audio_devices() -> AudioDevices {
        let host = cpal::default_host();
        AudioDevices {
            outputs: names(host.output_devices()),
            inputs: names(host.input_devices()),
            available: true,
        }
    }

    pub(crate) fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        open(true, device)
    }

    pub(crate) fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        open(false, device)
    }

    /// The device's thread. cpal's stream may not leave the thread that
    /// built it, so it lives there until this is dropped.
    struct Running {
        stop: Option<mpsc::Sender<()>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            drop(self.stop.take());
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    type Ready = Result<(u32, u16, SharedRing), String>;

    fn open(output: bool, name: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
        let (stop, stop_rx) = mpsc::channel::<()>();
        let name = name.map(str::to_owned);
        let lost = Arc::new(AtomicBool::new(false));
        let device_lost = Arc::clone(&lost);
        let thread = std::thread::Builder::new()
            .name("windup-sound".into())
            .spawn(move || {
                let stream = match build(output, name.as_deref(), device_lost) {
                    Ok((stream, ready)) => {
                        let _ = ready_tx.send(Ok(ready));
                        stream
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Until the source is dropped.
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|e| format!("Could not start the sound: {e}"))?;
        // Dropped on any error below, which stops the thread.
        let running = Running {
            stop: Some(stop),
            thread: Some(thread),
        };
        let (rate, channels, ring) = match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(r) => r?,
            Err(_) => return Err("The sound device did not start in time.".into()),
        };
        Ok(Box::new(Captured::new(
            rate,
            channels,
            ring,
            lost,
            Box::new(running),
        )))
    }

    fn find(output: bool, name: Option<&str>) -> Result<Device, String> {
        let host = cpal::default_host();
        let found = match (name, output) {
            (None, true) => host.default_output_device(),
            (None, false) => host.default_input_device(),
            (Some(n), true) => host
                .output_devices()
                .ok()
                .and_then(|mut d| d.find(|d| d.name().is_ok_and(|x| x == n))),
            (Some(n), false) => host
                .input_devices()
                .ok()
                .and_then(|mut d| d.find(|d| d.name().is_ok_and(|x| x == n))),
        };
        found.ok_or_else(|| match name {
            Some(n) => format!("The sound device {n} is not there."),
            None if output => "There is no sound output.".into(),
            None => "There is no microphone.".into(),
        })
    }

    fn build(
        output: bool,
        name: Option<&str>,
        lost: Arc<AtomicBool>,
    ) -> Result<(cpal::Stream, (u32, u16, SharedRing)), String> {
        let device = find(output, name)?;
        // Loopback records in the output's own format.
        let supported = if output {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(|e| format!("The sound device has no format: {e}"))?;
        let format = supported.sample_format();
        if !matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32
        ) {
            return Err(format!(
                "The sound device sends {format}, which Windup cannot read."
            ));
        }
        let config = supported.config();
        let (rate, channels) = (config.sample_rate.0, config.channels);
        let shared = ring(rate, channels);
        let push = Arc::clone(&shared);
        let mut samples = Vec::new();
        let stream = device
            .build_input_stream_raw(
                &config,
                format,
                move |data: &cpal::Data, _: &cpal::InputCallbackInfo| {
                    samples.clear();
                    if let Some(s) = data.as_slice::<f32>() {
                        samples.extend_from_slice(s);
                    } else if let Some(s) = data.as_slice::<i16>() {
                        samples.extend(s.iter().map(|v| i16_to_f32(*v)));
                    } else if let Some(s) = data.as_slice::<i32>() {
                        samples.extend(s.iter().map(|v| *v as f32 / 2_147_483_648.0));
                    }
                    if let Ok(mut r) = push.lock() {
                        r.push(&samples);
                    }
                },
                // A device that goes away leaves silence, which `Pad` fills:
                // the picture keeps recording, and `lost` lets the bar say so.
                move |e| {
                    log::warn!("sound device: {e}");
                    if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                        lost.store(true, Ordering::Relaxed);
                    }
                },
                None,
            )
            .map_err(|e| format!("Could not open the sound device: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("Could not start the sound device: {e}"))?;
        Ok((stream, (rate, channels, shared)))
    }
}

/// No sound system Windup knows on this OS.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod platform {
    use super::{AudioDevices, AudioSource};

    pub(crate) fn audio_devices() -> AudioDevices {
        AudioDevices::default()
    }

    pub(crate) fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        let _ = device;
        Err("Windup cannot record sound here.".into())
    }

    pub(crate) fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        let _ = device;
        Err("Windup cannot record sound here.".into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn source(r: &SharedRing) -> Captured {
        Captured::new(1000, 2, Arc::clone(r), Arc::default(), Box::new(()))
    }

    #[test]
    fn captured_sound_is_read_in_order() {
        let r = ring(1000, 2);
        let mut src = source(&r);
        src.restart();
        r.lock().unwrap().push(&[0.1, 0.2, 0.3, 0.4]);
        let mut buf = Vec::new();
        src.read(&mut buf).unwrap();
        assert_eq!(buf, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn restart_drops_what_arrived_before_it() {
        let r = ring(1000, 2);
        let mut src = source(&r);
        // Sound from while the recording was paused.
        r.lock().unwrap().push(&[0.5; 400]);
        src.restart();
        r.lock().unwrap().push(&[0.25, 0.25]);
        let mut buf = Vec::new();
        src.read(&mut buf).unwrap();
        assert_eq!(buf, [0.25, 0.25]);
    }

    #[test]
    fn a_silent_device_still_gives_sound() {
        let r = ring(1000, 2);
        let mut src = source(&r);
        let mut buf = Vec::new();
        src.read(&mut buf).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        src.read(&mut buf).unwrap();
        // About 400 ms of silent stereo frames at 1 kHz.
        assert!((600..=1000).contains(&buf.len()), "{}", buf.len());
        assert!(buf.len().is_multiple_of(2));
        assert!(buf.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn sound_after_quiet_lands_at_its_real_time() {
        // A loopback device that sends nothing until something plays half
        // a second in, then 10 ms periods.
        let r = ring(1000, 2);
        let mut src = source(&r);
        src.restart();
        let mut out = Vec::new();
        src.read(&mut out).unwrap();
        let t0 = std::time::Instant::now();
        let mut pushed = 500u64;
        while t0.elapsed().as_secs_f64() < 0.8 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let now = (t0.elapsed().as_secs_f64() * 1000.0) as u64;
            if now > pushed {
                let n = (now - pushed) as usize;
                r.lock().unwrap().push(&vec![1.0; n * 2]);
                pushed = now;
            }
            src.read(&mut out).unwrap();
        }
        let first = out.iter().position(|s| *s != 0.0).expect("sound") / 2;
        // Its frame position is its time: 500 ms, within a period or two.
        assert!(
            (480..=530).contains(&first),
            "sound starts at frame {first}"
        );
    }

    #[test]
    fn finish_pays_the_silence_owed() {
        let r = ring(1000, 2);
        let mut src = source(&r);
        src.restart();
        let mut out = Vec::new();
        src.read(&mut out).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        src.read(&mut out).unwrap();
        assert!(out.is_empty(), "below the threshold, read waits");
        src.finish(&mut out).unwrap();
        assert!((240..=300).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn captured_sources_share_their_lost_flag() {
        let r = ring(1000, 2);
        let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let src = Captured::new(1000, 2, r, Arc::clone(&lost), Box::new(()));
        lost.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            src.lost()
                .unwrap()
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}
