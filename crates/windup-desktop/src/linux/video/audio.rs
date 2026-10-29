//! Computer sound and the microphone through PipeWire, behind the `pipewire`
//! feature like the ScreenCast streams (`pw_stream.rs`).
//!
//! Each source is one capture stream on a PipeWire main loop of its own
//! thread. Computer sound is the default sink's monitor
//! (`stream.capture.sink`), the microphone the default source; a named device
//! is targeted by its node name. The stream asks for interleaved F32LE stereo
//! at 48 kHz, so PipeWire's adapter converts whatever the device runs at and
//! the format never changes under a recording; what was agreed is what
//! `rate` and `channels` report. Samples go into the shared ring that
//! `crate::video::audio::Captured` reads, which also pads silence.

use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use pw::spa::param::ParamType;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::param::format::{MediaSubtype, MediaType};
use pw::spa::pod::{Object, Pod, Value};
use pw::spa::utils::SpaTypes;
use pw::stream::{StreamFlags, StreamState};
use pw::types::ObjectType;

use crate::video::audio::{AudioDevices, Captured, SharedRing, ring};
use crate::video::run::AudioSource;

/// Longest the registry may take to list the devices, and a stream to agree
/// its format.
const TIMEOUT: Duration = Duration::from_secs(3);

const SINK: &str = "Audio/Sink";
const SOURCE: &str = "Audio/Source";

/// A device as the registry lists it.
struct Node {
    class: String,
    /// What a stream targets.
    name: String,
    /// What a person reads.
    description: Option<String>,
}

impl Node {
    fn label(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }
}

pub(crate) fn audio_devices() -> AudioDevices {
    match nodes() {
        Ok(nodes) => {
            let labels = |class: &str| {
                nodes
                    .iter()
                    .filter(|n| n.class == class)
                    .map(|n| n.label().to_owned())
                    .collect()
            };
            AudioDevices {
                outputs: labels(SINK),
                inputs: labels(SOURCE),
                available: true,
            }
        }
        Err(e) => {
            log::debug!("no PipeWire for sound: {e}");
            AudioDevices::default()
        }
    }
}

pub(crate) fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
    open(true, device)
}

pub(crate) fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
    open(false, device)
}

/// The audio sinks and sources there are, from one registry roundtrip.
fn nodes() -> Result<Vec<Node>, String> {
    let err = |e: pw::Error| format!("PipeWire: {e}");
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(err)?;
    let context = pw::context::Context::new(&mainloop).map_err(err)?;
    let core = context.connect(None).map_err(err)?;
    let registry = core.get_registry().map_err(err)?;
    let found = Rc::new(RefCell::new(Vec::new()));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let found = Rc::clone(&found);
            move |global| {
                if global.type_ != ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else {
                    return;
                };
                let (Some(class), Some(name)) = (props.get("media.class"), props.get("node.name"))
                else {
                    return;
                };
                if class == SINK || class == SOURCE {
                    found.borrow_mut().push(Node {
                        class: class.to_owned(),
                        name: name.to_owned(),
                        description: props.get("node.description").map(str::to_owned),
                    });
                }
            }
        })
        .register();

    // Every global is announced before the reply to a sync sent after the
    // registry was asked for.
    let pending = core.sync(0).map_err(err)?;
    let done = Rc::new(Cell::new(false));
    let _core_listener = core
        .add_listener_local()
        .done({
            let done = Rc::clone(&done);
            let mainloop = mainloop.downgrade();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    if let Some(ml) = mainloop.upgrade() {
                        ml.quit();
                    }
                }
            }
        })
        .error({
            let mainloop = mainloop.downgrade();
            move |_, _, _, message| {
                log::debug!("PipeWire: {message}");
                if let Some(ml) = mainloop.upgrade() {
                    ml.quit();
                }
            }
        })
        .register();
    let timer = mainloop.loop_().add_timer({
        let mainloop = mainloop.downgrade();
        move |_| {
            if let Some(ml) = mainloop.upgrade() {
                ml.quit();
            }
        }
    });
    let _ = timer.update_timer(Some(TIMEOUT), None);
    mainloop.run();
    if !done.get() {
        return Err("PipeWire did not list its devices.".into());
    }
    Ok(found.take())
}

/// The format every stream asks for: F32LE stereo at 48 kHz.
fn format() -> Result<Vec<u8>, String> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(48000);
    info.set_channels(2);
    let mut position = [0; 64];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(out, _)| out.into_inner())
        .map_err(|e| format!("could not describe the sound format: {e:?}"))
}

/// The loop's thread. Dropping it stops the loop and waits for the thread.
struct Running {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

type Ready = mpsc::Sender<Result<(u32, u16, SharedRing), String>>;

fn open(system: bool, device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
    // A device is chosen by the label settings show; the stream needs its
    // node name.
    let target = match device {
        None => None,
        Some(label) => {
            let class = if system { SINK } else { SOURCE };
            let nodes = nodes()?;
            let node = nodes
                .iter()
                .find(|n| n.class == class && (n.label() == label || n.name == label))
                .ok_or_else(|| format!("The sound device {label} is not there."))?;
            Some(node.name.clone())
        }
    };
    let (stop, stop_rx) = pw::channel::channel::<()>();
    let (ready, ready_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("windup-sound".into())
        .spawn(move || {
            if let Err(e) = run(system, target, ready.clone(), stop_rx) {
                // Before the format was agreed this reaches `open`; after,
                // the recording keeps going on silence.
                let _ = ready.send(Err(e.clone()));
                log::warn!("sound: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    // Dropped on any error below, which stops the loop.
    let running = Running {
        stop,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(TIMEOUT) {
        Ok(Ok((rate, channels, ring))) => Ok(Box::new(Captured::new(
            rate,
            channels,
            ring,
            Box::new(running),
        ))),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("The sound device did not start in time.".into()),
    }
}

/// What the stream's callbacks share, all on the loop's thread.
struct Feed {
    /// Taken once the format is agreed (or the stream fails first).
    ready: Option<Ready>,
    /// Made once the format is agreed.
    ring: Option<SharedRing>,
    channels: u16,
    samples: Vec<f32>,
    mainloop: pw::main_loop::WeakMainLoop,
}

impl Feed {
    /// The stream failed: to `open` while it still waits, else only logged,
    /// and the recording carries on with silence.
    fn fail(&mut self, why: String) {
        match self.ready.take() {
            Some(ready) => {
                let _ = ready.send(Err(why));
            }
            None => log::warn!("sound: {why}"),
        }
        if let Some(ml) = self.mainloop.upgrade() {
            ml.quit();
        }
    }

    fn format(&mut self, rate: u32, channels: u16) {
        if let Some(ready) = self.ready.take() {
            let shared = ring(rate, channels);
            self.ring = Some(std::sync::Arc::clone(&shared));
            self.channels = channels;
            let _ = ready.send(Ok((rate, channels, shared)));
        } else if channels != self.channels {
            // Asked for a fixed format, so this should not happen.
            self.fail(format!("the sound changed to {channels} channels"));
        }
    }

    fn samples(&mut self, bytes: &[u8]) {
        let Some(ring) = &self.ring else {
            return;
        };
        self.samples.clear();
        self.samples.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b)),
        );
        if let Ok(mut r) = ring.lock() {
            r.push(&self.samples);
        }
    }
}

fn run(
    system: bool,
    target: Option<String>,
    ready: Ready,
    stop: pw::channel::Receiver<()>,
) -> Result<(), String> {
    let err = |e: pw::Error| format!("PipeWire: {e}");
    // Declared in the order they are made, so they are dropped in reverse.
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(err)?;
    let context = pw::context::Context::new(&mainloop).map_err(err)?;
    let core = context.connect(None).map_err(err)?;
    let feed = Rc::new(RefCell::new(Feed {
        ready: Some(ready),
        ring: None,
        channels: 2,
        samples: Vec::new(),
        mainloop: mainloop.downgrade(),
    }));

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Production",
        *pw::keys::NODE_NAME => "windup",
    };
    if system {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }
    if let Some(target) = &target {
        // `pw::keys::TARGET_OBJECT` needs pipewire-rs's `v0_3_44` feature;
        // the key is the same string.
        props.insert("target.object", target.as_str());
    }
    let stream = pw::stream::Stream::new(&core, "windup-sound", props).map_err(err)?;
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed({
            let feed = Rc::clone(&feed);
            move |_, _, old, new| match new {
                StreamState::Error(e) => feed.borrow_mut().fail(format!("the sound stopped ({e})")),
                StreamState::Unconnected if old != StreamState::Unconnected => {
                    feed.borrow_mut().fail("the sound stopped".into());
                }
                _ => {}
            }
        })
        .param_changed({
            let feed = Rc::clone(&feed);
            move |_, _, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media, sub)) = spa::param::format_utils::parse_format(param) else {
                    return;
                };
                if media != MediaType::Audio || sub != MediaSubtype::Raw {
                    return;
                }
                let mut info = AudioInfoRaw::new();
                if info.parse(param).is_err() || info.format() != AudioFormat::F32LE {
                    feed.borrow_mut()
                        .fail("PipeWire sent sound Windup cannot read.".into());
                    return;
                }
                let channels = u16::try_from(info.channels()).unwrap_or(0);
                if info.rate() == 0 || channels == 0 {
                    feed.borrow_mut()
                        .fail("PipeWire sent sound with no rate.".into());
                    return;
                }
                feed.borrow_mut().format(info.rate(), channels);
            }
        })
        .process({
            let feed = Rc::clone(&feed);
            move |stream, _| {
                // Every buffer counts: sound is not a picture to skip.
                while let Some(mut buffer) = stream.dequeue_buffer() {
                    let Some(data) = buffer.datas_mut().first_mut() else {
                        continue;
                    };
                    let chunk = data.chunk();
                    let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
                    let Some(bytes) = data.data() else {
                        continue;
                    };
                    let end = offset.saturating_add(size).min(bytes.len());
                    let start = offset.min(end);
                    feed.borrow_mut().samples(&bytes[start..end]);
                }
            }
        })
        .register()
        .map_err(err)?;
    let format = format()?;
    let pod = Pod::from_bytes(&format).ok_or("could not describe the sound format")?;
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut [pod],
        )
        .map_err(err)?;

    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.downgrade();
        move |()| {
            if let Some(ml) = mainloop.upgrade() {
                ml.quit();
            }
        }
    });
    mainloop.run();
    Ok(())
}
