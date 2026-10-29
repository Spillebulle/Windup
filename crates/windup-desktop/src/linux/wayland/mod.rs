//! Windup's own Wayland connection: one thread, one event loop, owning the
//! clipboard (data-control), the virtual keyboard, the popup surface, the
//! capture overlay and the notice card, so none of them race each other.
//!
//! - `popup.rs`: the clipboard popup;
//! - `overlay.rs`: the capture overlay, `notice.rs` the notice card,
//!   `recbar.rs` the recording bar, and `pane.rs` the surfaces they draw on;
//! - `gpu.rs`: the device they share, and putting a frame on a surface;
//! - `handlers.rs`: what the compositor says, sent on to the right one.
//!
//! The rest of the app talks to it through [`Service`], a channel of
//! [`Request`]s. It reports back through [`ServiceEvent`]s.

mod clip;
mod gpu;
mod handlers;
mod notice;
mod overlay;
mod pane;
mod popup;
mod recbar;
pub(crate) mod screencopy;
mod vkbd;

pub use clip::Payload;
pub use notice::NoticeSetup;
pub use overlay::{CaptureEnd, CaptureSetup};
pub use popup::PopupSetup;
pub use recbar::RecBarSetup;

use crate::platform::{DataControl, Host, PasteVia, Plan, Watch};
use calloop::channel::{Channel, Event as ChannelEvent, Sender as LoopSender, channel};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle};
use clip::{Clip, Manager, Selection};
use gpu::Gpu;
use handlers::EnterFrames;
use popup::Popup;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::seat::pointer::ThemedPointer;
use smithay_client_toolkit::shell::wlr_layer::{LayerShell, LayerSurface};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::subcompositor::SubcompositorState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_seat, wl_subsurface, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use windup_core::clipboard::capture::{self, Capture, Ingest};
use windup_core::clipboard::{ClipboardContent, ClipboardEntry, ClipboardHistory};

/// After the popup closes, how long to let the compositor give focus back to
/// the window underneath before pressing Ctrl+V into it.
const FOCUS_SETTLE: Duration = Duration::from_millis(80);
/// For paste-as-plain-text, fired while the shortcut's keys are still down.
const SHORTCUT_SETTLE: Duration = Duration::from_millis(150);

/// The interface names the compositor advertises, or `None` when there is no
/// Wayland session to connect to.
pub fn probe() -> Option<Vec<String>> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, _queue) = registry_queue_init::<Probe>(&conn).ok()?;
    Some(
        globals
            .contents()
            .clone_list()
            .into_iter()
            .map(|g| g.interface)
            .collect(),
    )
}

struct Probe;
impl
    Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for Probe
{
    fn event(
        _: &mut Probe,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Probe>,
    ) {
    }
}

pub enum Request {
    /// Take the clipboard, and press Ctrl+V afterwards if `paste`.
    Set {
        payload: Payload,
        paste: bool,
    },
    /// Replace the clipboard with its own plain text and paste it.
    PastePlainCurrent,
    /// Only press Ctrl+V; somebody else set the clipboard.
    Paste,
    OpenPopup(PopupSetup),
    /// Read off the clipboard by a reader thread.
    Captured(Capture),
    /// The text of a password manager's copy, to remember and never record.
    Secret(String),
    /// The current clipboard's text, read for paste-as-plain-text.
    PlainRead(String),
    /// Open the capture overlay on the picture in the session.
    OpenCapture(Box<CaptureSetup>),
    /// Show the notice card (the layer host's).
    Notice(Box<NoticeSetup>),
    /// Show the recording bar (the layer host's).
    OpenRecBar(Box<RecBarSetup>),
    /// Take the recording bar down.
    CloseRecBar,
}

pub enum ServiceEvent {
    /// The user resized the popup; the size is to be saved.
    PopupResized(f32, f32),
    /// The popup asked for a paste that the service cannot do itself: the
    /// virtual keyboard is missing, so the portal has to press the keys.
    PasteElsewhere,
    /// The capture overlay closed, or could not open.
    CaptureDone(Box<CaptureEnd>),
    /// Pause, Resume or Stop on the recording bar.
    RecBar(windup_ui::capture::recbar::RecBarAction),
}

/// What the service shares with the rest of the app.
#[derive(Clone)]
pub struct Shared {
    pub history: Arc<Mutex<ClipboardHistory>>,
    pub max_items: Arc<AtomicUsize>,
    pub save_images: Arc<AtomicBool>,
    /// Whether clipboard history is on at all.
    pub watching: Arc<AtomicBool>,
    pub events: Sender<ServiceEvent>,
}

pub struct Service {
    tx: LoopSender<Request>,
}

impl Service {
    /// Connects and starts the thread. Fails when the compositor cannot be
    /// reached or lacks what `plan` asked for.
    pub fn spawn(plan: Plan, shared: Shared) -> Result<Service, String> {
        let (tx, rx) = channel::<Request>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let loop_tx = tx.clone();
        std::thread::Builder::new()
            .name("wayland".into())
            .spawn(move || match State::start(plan, shared, rx, loop_tx) {
                Ok((mut event_loop, mut state)) => {
                    let _ = ready_tx.send(Ok(()));
                    loop {
                        if let Err(e) = event_loop.dispatch(None, &mut state) {
                            log::error!("Wayland connection lost: {e}");
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx
            .recv()
            .map_err(|_| "the Wayland thread died while starting".to_string())??;
        Ok(Service { tx })
    }

    fn send(&self, request: Request) -> bool {
        let sent = self.tx.send(request).is_ok();
        if !sent {
            log::error!("the Wayland thread is gone");
        }
        sent
    }

    pub fn set_entry(&self, entry: &ClipboardEntry, paste: bool) {
        let payload = match &entry.content {
            ClipboardContent::Text { text, .. } => Payload::Text(text.clone()),
            ClipboardContent::Image {
                rgba,
                width,
                height,
                ..
            } => match clip::png(rgba, *width, *height) {
                Some(png) => Payload::Png {
                    png,
                    rgba: rgba.clone(),
                    width: *width,
                    height: *height,
                },
                None => return,
            },
        };
        self.send(Request::Set { payload, paste });
    }

    pub fn set_text(&self, text: String, paste: bool) {
        self.send(Request::Set {
            payload: Payload::Text(text),
            paste,
        });
    }

    pub fn paste_plain_current(&self) {
        self.send(Request::PastePlainCurrent);
    }

    pub fn press_ctrl_v(&self) {
        self.send(Request::Paste);
    }

    pub fn open_popup(&self, setup: PopupSetup) {
        self.send(Request::OpenPopup(setup));
    }

    /// False when the Wayland thread is gone and nothing will open.
    pub fn open_capture(&self, setup: CaptureSetup) -> bool {
        self.send(Request::OpenCapture(Box::new(setup)))
    }

    pub fn notice(&self, setup: NoticeSetup) -> bool {
        self.send(Request::Notice(Box::new(setup)))
    }

    pub fn open_recbar(&self, setup: RecBarSetup) {
        self.send(Request::OpenRecBar(Box::new(setup)));
    }

    pub fn close_recbar(&self) {
        self.send(Request::CloseRecBar);
    }
}

pub struct State {
    conn: Connection,
    qh: QueueHandle<State>,
    handle: LoopHandle<'static, State>,
    loop_tx: LoopSender<Request>,
    shared: Shared,
    ingest: Ingest,
    /// Which host this is, for the role the overlay's surfaces take.
    host: Host,

    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    subcompositor: Option<SubcompositorState>,
    layer_shell: Option<LayerShell>,
    /// For the overlay where there is no layer-shell.
    xdg_shell: Option<XdgShell>,
    fractional: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    vk_manager: Option<ZwpVirtualKeyboardManagerV1>,

    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer<EnterFrames>>,
    clip: Option<Clip>,
    vkbd: Option<vkbd::VirtualKeyboard>,

    instance: Option<wgpu::Instance>,
    gpu: Option<Gpu>,
    popup: Option<Popup>,
    layer: Option<LayerSurface>,
    /// The layer surface's transparent buffer, and the viewport that
    /// stretches it over the output where the compositor has viewporter.
    backdrop: Option<(SlotPool, Buffer)>,
    layer_viewport: Option<WpViewport>,
    card: Option<Card>,
    /// Only with a viewport on the card to apply it.
    popup_scale: Option<WpFractionalScaleV1>,
    redraw_queued: bool,

    overlay: Option<overlay::Overlay>,
    notice: Option<notice::Notice>,
    recbar: Option<recbar::RecBar>,
    pane_redraw_queued: bool,
    /// The modifiers held, for the overlay and the notice.
    pane_modifiers: egui::Modifiers,
}

/// The card: a subsurface of the popup's layer surface, and the only part of
/// the popup the GPU draws.
struct Card {
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    viewport: Option<WpViewport>,
}

impl State {
    fn start(
        plan: Plan,
        shared: Shared,
        rx: Channel<Request>,
        loop_tx: LoopSender<Request>,
    ) -> Result<(EventLoop<'static, State>, State), String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, queue) = registry_queue_init(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let event_loop: EventLoop<'static, State> =
            EventLoop::try_new().map_err(|e| e.to_string())?;
        let handle = event_loop.handle();
        smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource::new(
            conn.clone(),
            queue,
        )
        .insert(handle.clone())
        .map_err(|e| e.to_string())?;
        handle
            .insert_source(rx, |event, _, state: &mut State| {
                if let ChannelEvent::Msg(request) = event {
                    state.handle_request(request);
                }
            })
            .map_err(|e| e.to_string())?;

        let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let subcompositor =
            SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh).ok();
        let layer_shell = LayerShell::bind(&globals, &qh).ok();
        let xdg_shell = XdgShell::bind(&globals, &qh).ok();
        let clip = match plan.watch {
            Watch::DataControl(DataControl::Ext) => globals
                .bind(&qh, 1..=1, ())
                .ok()
                .map(|m| Clip::new(Manager::Ext(m))),
            Watch::DataControl(DataControl::Wlr) => globals
                .bind(&qh, 1..=2, ())
                .ok()
                .map(|m| Clip::new(Manager::Wlr(m))),
            Watch::Poll => None,
        };
        let vk_manager = (plan.paste == PasteVia::VirtualKeyboard)
            .then(|| globals.bind(&qh, 1..=1, ()).ok())
            .flatten();

        let mut state = State {
            registry: RegistryState::new(&globals),
            seat_state: SeatState::new(&globals, &qh),
            output_state: OutputState::new(&globals, &qh),
            fractional: bind_optional(&globals, &qh),
            viewporter: bind_optional(&globals, &qh),
            conn,
            qh,
            handle,
            loop_tx,
            shared,
            ingest: Ingest::new(),
            host: plan.host,
            compositor,
            shm,
            subcompositor,
            layer_shell,
            xdg_shell,
            vk_manager,
            seat: None,
            keyboard: None,
            pointer: None,
            clip,
            vkbd: None,
            instance: None,
            gpu: None,
            popup: None,
            layer: None,
            backdrop: None,
            layer_viewport: None,
            card: None,
            popup_scale: None,
            redraw_queued: false,
            overlay: None,
            notice: None,
            recbar: None,
            pane_redraw_queued: false,
            pane_modifiers: egui::Modifiers::default(),
        };
        // sctk calls `new_seat` only for seats announced later; the ones
        // there at startup (all of them, in practice) are bound silently.
        for seat in state.seat_state.seats() {
            state.adopt_seat(seat);
        }
        log::info!(
            "Wayland: data-control {}, virtual keyboard {}, layer-shell {}",
            state.clip.is_some(),
            state.vk_manager.is_some(),
            state.layer_shell.is_some()
        );
        Ok((event_loop, state))
    }

    /// Takes the first seat as the one Windup watches and types on.
    fn adopt_seat(&mut self, seat: wl_seat::WlSeat) {
        if self.seat.is_some() {
            return;
        }
        if let Some(clip) = self.clip.as_mut() {
            clip.attach(&seat, &self.qh);
        }
        // Made now rather than at the first paste: a seat that had no
        // keyboard before gains one, and the window being pasted into must
        // have bound it before the keys arrive or it never sees them.
        if let Some(manager) = &self.vk_manager {
            self.vkbd = vkbd::VirtualKeyboard::new(manager, &seat, &self.qh);
        }
        self.seat = Some(seat);
    }

    fn handle_request(&mut self, request: Request) {
        match request {
            Request::Set { payload, paste } => {
                if let Some(clip) = self.clip.as_mut() {
                    clip.set(payload, &self.qh);
                    let _ = self.conn.flush();
                }
                if paste {
                    self.paste_after(FOCUS_SETTLE);
                }
            }
            Request::PastePlainCurrent => self.paste_plain_current(),
            Request::Paste => self.paste_after(FOCUS_SETTLE),
            Request::PlainRead(text) => {
                if let Some(clip) = self.clip.as_mut() {
                    clip.set(Payload::Text(text), &self.qh);
                    let _ = self.conn.flush();
                }
                self.paste_after(SHORTCUT_SETTLE);
            }
            Request::Captured(capture) => self.ingest(capture),
            Request::Secret(text) => self.ingest.mark_secret(&text),
            Request::OpenPopup(setup) => {
                // A popup over an open capture would be in nobody's way but
                // the user's; the shortcut does nothing until it closes.
                if self.overlay.is_none() {
                    self.close_notice();
                    self.open_popup(setup);
                }
            }
            Request::OpenCapture(setup) => self.open_capture(setup),
            Request::Notice(setup) => self.open_notice(*setup),
            Request::OpenRecBar(setup) => self.open_recbar(*setup),
            Request::CloseRecBar => self.close_recbar(),
        }
    }

    fn ingest(&mut self, capture: Capture) {
        if !self.shared.watching.load(Ordering::Relaxed) {
            return;
        }
        let max = self.shared.max_items.load(Ordering::Relaxed);
        let images = self.shared.save_images.load(Ordering::Relaxed);
        let mut history = self.shared.history.lock().unwrap();
        self.ingest.offer(capture, &mut history, max, images);
    }

    fn on_selection(&mut self, what: Selection, conn: &Connection, _qh: &QueueHandle<State>) {
        match what {
            Selection::Cleared => {}
            Selection::Ours(payload) => self.ingest(payload.capture()),
            Selection::Theirs => {
                if !self.shared.watching.load(Ordering::Relaxed) {
                    return;
                }
                let Some(clip) = &self.clip else { return };
                let images = self.shared.save_images.load(Ordering::Relaxed);
                let Some(pick) = clip.current_mimes().and_then(|m| capture::pick(m, images)) else {
                    return;
                };
                let tx = self.loop_tx.clone();
                let (mime, html) = (pick.mime.clone(), pick.html.clone());
                clip.read_current(conn, &mime, html.as_deref(), move |main, html| {
                    let request = match main.and_then(|m| clip::decode(&pick, m, html)) {
                        Some(Capture::Text { text, .. }) if pick.secret => Request::Secret(text),
                        Some(capture) => Request::Captured(capture),
                        None => return,
                    };
                    let _ = tx.send(request);
                });
            }
        }
    }

    fn paste_plain_current(&mut self) {
        let Some(clip) = &self.clip else {
            return;
        };
        let Some(pick) = clip.current_mimes().and_then(|m| capture::pick(m, false)) else {
            // Our own text is already plain; paste it as it is. Anything else
            // has no text to paste, as on Windows.
            if clip.owns_text() {
                self.paste_after(SHORTCUT_SETTLE);
            }
            return;
        };
        let tx = self.loop_tx.clone();
        let mime = pick.mime.clone();
        clip.read_current(&self.conn, &mime, None, move |main, _| {
            if let Some(Capture::Text { text, .. }) =
                main.and_then(|m| clip::decode(&pick, m, None))
            {
                let _ = tx.send(Request::PlainRead(text));
            }
        });
    }

    fn paste_after(&mut self, delay: Duration) {
        let _ =
            self.handle
                .insert_source(Timer::from_duration(delay), |_, _, state: &mut State| {
                    state.press_ctrl_v();
                    TimeoutAction::Drop
                });
    }

    fn press_ctrl_v(&mut self) {
        log::info!("pressing Ctrl+V");
        if self.vkbd.is_none()
            && let (Some(manager), Some(seat)) = (&self.vk_manager, &self.seat)
        {
            self.vkbd = vkbd::VirtualKeyboard::new(manager, seat, &self.qh);
        }
        match &self.vkbd {
            Some(vk) => vk.ctrl_v(&self.conn),
            None => {
                let _ = self.shared.events.send(ServiceEvent::PasteElsewhere);
            }
        }
    }
}

/// `Service::set_entry`'s payload building, reused from inside the thread.
struct LocalService(LoopSender<Request>);

impl LocalService {
    fn set_entry(&self, entry: &ClipboardEntry, paste: bool) {
        Service { tx: self.0.clone() }.set_entry(entry, paste);
    }
}

fn bind_optional<I>(globals: &GlobalList, qh: &QueueHandle<State>) -> Option<I>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    globals.bind(qh, 1..=1, ()).ok()
}
