//! Windup's own Wayland connection: one thread, one event loop, owning the
//! clipboard (data-control), the virtual keyboard and the popup surface, so
//! the three never race each other.
//!
//! The rest of the app talks to it through [`Service`], a channel of
//! [`Request`]s. It reports back through [`ServiceEvent`]s.

mod clip;
mod popup;
mod vkbd;

pub use clip::Payload;
pub use popup::PopupSetup;

use crate::platform::{DataControl, PasteVia, Plan, Watch};
use calloop::channel::{Channel, Event as ChannelEvent, Sender as LoopSender, channel};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle};
use clip::{Clip, Manager, Selection};
use popup::{ENTER_GRACE, Gpu, Popup, egui_button, egui_key};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerData, PointerDataExt, PointerEvent, PointerEventKind, PointerHandler,
    ThemeSpec, ThemedPointer,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::subcompositor::SubcompositorState;
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_subcompositor, registry_handlers,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use windup_core::clipboard::capture::{self, Capture, Ingest};
use windup_core::clipboard::{ClipboardContent, ClipboardEntry, ClipboardHistory};
use windup_ui::PopupAction;

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
}

pub enum ServiceEvent {
    /// The user resized the popup; the size is to be saved.
    PopupResized(f32, f32),
    /// The popup asked for a paste that the service cannot do itself: the
    /// virtual keyboard is missing, so the portal has to press the keys.
    PasteElsewhere,
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

    fn send(&self, request: Request) {
        if self.tx.send(request).is_err() {
            log::error!("the Wayland thread is gone");
        }
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
}

pub struct State {
    conn: Connection,
    qh: QueueHandle<State>,
    handle: LoopHandle<'static, State>,
    loop_tx: LoopSender<Request>,
    shared: Shared,
    ingest: Ingest,

    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    subcompositor: Option<SubcompositorState>,
    layer_shell: Option<LayerShell>,
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
            compositor,
            shm,
            subcompositor,
            layer_shell,
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
            Request::OpenPopup(setup) => self.open_popup(setup),
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

    // --- popup ----------------------------------------------------------

    fn open_popup(&mut self, setup: PopupSetup) {
        if self.popup.is_some() {
            self.close_popup();
            return;
        }
        let Some(layer_shell) = &self.layer_shell else {
            log::warn!("no layer-shell; the popup cannot open here");
            return;
        };
        log::info!("popup: opening");
        let surface = self.compositor.create_surface(&self.qh);
        let layer = layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("windup-clipboard"),
            None,
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);

        if let Some(viewporter) = &self.viewporter {
            self.layer_viewport = Some(viewporter.get_viewport(layer.wl_surface(), &self.qh, ()));
            // The preferred scale is the layer's; it is applied to the card.
            if let Some(fractional) = &self.fractional {
                self.popup_scale =
                    Some(fractional.get_fractional_scale(layer.wl_surface(), &self.qh, ()));
            }
        }
        layer.commit();

        let Some(subcompositor) = &self.subcompositor else {
            log::warn!("no wl_subcompositor; the popup cannot open here");
            self.layer = Some(layer);
            self.close_popup();
            return;
        };
        let (subsurface, surface) =
            subcompositor.create_subsurface(layer.wl_surface().clone(), &self.qh);
        // Shown when wgpu presents, not at the layer's next commit.
        subsurface.set_desync();
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &self.qh, ()));
        let wgpu_surface = self.wgpu_surface(&surface);
        self.card = Some(Card {
            subsurface,
            surface,
            viewport,
        });
        self.layer = Some(layer);

        let mut popup = Popup::new(setup);
        match wgpu_surface {
            Ok(surface) => popup.surface = Some(surface),
            Err(e) => {
                log::error!("{e}");
                self.close_popup();
                return;
            }
        }
        // A fresh list each time, as on Windows: top entry selected, no search.
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.view.reset();
        }
        self.popup = Some(popup);
        let _ = self.handle.insert_source(
            Timer::from_duration(ENTER_GRACE),
            |_, _, state: &mut State| {
                if let Some(popup) = state.popup.as_mut() {
                    popup.place_fallback();
                }
                state.queue_redraw();
                TimeoutAction::Drop
            },
        );
    }

    fn wgpu_surface(
        &mut self,
        surface: &wl_surface::WlSurface,
    ) -> Result<wgpu::Surface<'static>, String> {
        let display = NonNull::new(self.conn.backend().display_ptr() as *mut _)
            .ok_or("no Wayland display pointer")?;
        let window =
            NonNull::new(surface.id().as_ptr() as *mut _).ok_or("no Wayland surface pointer")?;
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
        };
        let instance = self.instance.get_or_insert_with(|| {
            wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default())
        });
        // SAFETY: the display outlives the surface (it lives as long as this
        // thread), and the card's wl_surface is destroyed only after the wgpu
        // surface is dropped, in `close_popup`.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    fn close_popup(&mut self) {
        if let Some(mut popup) = self.popup.take() {
            log::info!("popup: closed");
            // The wgpu surface before the wl_surface it draws to.
            popup.surface = None;
        }
        if let Some(card) = self.card.take() {
            if let Some(v) = card.viewport {
                v.destroy();
            }
            card.subsurface.destroy();
            card.surface.destroy();
        }
        if let Some(f) = self.popup_scale.take() {
            f.destroy();
        }
        if let Some(v) = self.layer_viewport.take() {
            v.destroy();
        }
        self.layer = None;
        self.backdrop = None;
        let _ = self.conn.flush();
    }

    fn queue_redraw(&mut self) {
        if self.redraw_queued || self.popup.is_none() {
            return;
        }
        self.redraw_queued = true;
        self.handle.insert_idle(|state| {
            state.redraw_queued = false;
            state.redraw();
        });
    }

    fn redraw(&mut self) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        // Nothing to draw until the card has a place.
        if !popup.configured || popup.surface.is_none() || popup.card.is_none() {
            return;
        }
        if self.gpu.is_none() {
            let (Some(instance), Some(surface)) = (&self.instance, &popup.surface) else {
                return;
            };
            match Gpu::new(instance, surface) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(e) => {
                    log::error!("{e}");
                    self.close_popup();
                    return;
                }
            }
        }
        self.sync_card();
        let (Some(popup), Some(gpu)) = (self.popup.as_mut(), self.gpu.as_mut()) else {
            return;
        };
        let (action, again) = {
            let history = self.shared.history.lock().unwrap();
            popup.draw(gpu, &history)
        };
        if !popup.shown {
            popup.shown = true;
            log::info!("popup: shown");
        }
        if let Some(delay) = again {
            if delay.is_zero() {
                self.queue_redraw();
            } else {
                let _ = self.handle.insert_source(
                    Timer::from_duration(delay.max(Duration::from_millis(8))),
                    |_, _, state: &mut State| {
                        state.queue_redraw();
                        TimeoutAction::Drop
                    },
                );
            }
        }
        if let Some(action) = action {
            self.popup_action(action);
        }
    }

    /// Moves and sizes the card's subsurface to where the popup has it, when
    /// that has changed since last time.
    fn sync_card(&mut self) {
        let (Some(popup), Some(card), Some(layer), Some(gpu)) =
            (self.popup.as_mut(), &self.card, &self.layer, &self.gpu)
        else {
            return;
        };
        let Some(at) = popup.card else {
            return;
        };
        let now = (at, popup.card_size, popup.scale);
        if popup.synced == Some(now) {
            return;
        }
        popup.synced = Some(now);
        let (w, h) = popup.logical_size();
        card.subsurface
            .set_position(at.x.round() as i32, at.y.round() as i32);
        match &card.viewport {
            Some(v) => v.set_destination(w as i32, h as i32),
            // Without a viewport the scale is a whole number.
            None => card
                .surface
                .set_buffer_scale(popup.scale.round().max(1.0) as i32),
        }
        popup.configure_surface(gpu);
        // A subsurface's position belongs to its parent: it moves when the
        // layer surface next commits.
        layer.wl_surface().commit();
    }

    /// Gives the layer surface its transparent buffer: one pixel stretched
    /// over the output where there is a viewport, the output's size where not.
    fn attach_backdrop(&mut self, layer: &LayerSurface, w: u32, h: u32) {
        let (bw, bh) = match &self.layer_viewport {
            Some(v) => {
                v.set_destination(w as i32, h as i32);
                (1, 1)
            }
            None => (w as i32, h as i32),
        };
        let mut pool = match SlotPool::new((bw * bh * 4) as usize, &self.shm) {
            Ok(pool) => pool,
            Err(e) => {
                log::error!("popup: no shared memory for the backdrop: {e}");
                return;
            }
        };
        let buffer = match pool.create_buffer(bw, bh, bw * 4, wl_shm::Format::Argb8888) {
            Ok((buffer, canvas)) => {
                canvas.fill(0);
                buffer
            }
            Err(e) => {
                log::error!("popup: no backdrop buffer: {e}");
                return;
            }
        };
        let surface = layer.wl_surface();
        if buffer.attach_to(surface).is_err() {
            return;
        }
        surface.damage_buffer(0, 0, bw, bh);
        surface.commit();
        self.backdrop = Some((pool, buffer));
    }

    /// The popup's layer surface or its card.
    fn is_popup_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
            || self.card.as_ref().is_some_and(|c| &c.surface == surface)
    }

    fn popup_action(&mut self, action: PopupAction) {
        let auto_paste = self.popup.as_ref().is_some_and(|p| p.setup.auto_paste);
        match action {
            PopupAction::Paste(entry) => {
                self.close_popup();
                let service = LocalService(self.loop_tx.clone());
                service.set_entry(&entry, auto_paste);
            }
            PopupAction::PastePlain(text) => {
                self.close_popup();
                let _ = self.loop_tx.send(Request::Set {
                    payload: Payload::Text(text),
                    paste: true,
                });
            }
            PopupAction::Delete(id) => {
                self.shared.history.lock().unwrap().remove_by_id(id);
                self.queue_redraw();
            }
            PopupAction::Resize(w, h) => self.resize_popup(w, h, 0.0),
            PopupAction::ResizeLeft(w, h, dx) => self.resize_popup(w, h, dx),
            PopupAction::Dismiss => self.close_popup(),
        }
    }

    fn resize_popup(&mut self, w: f32, h: f32, dx: f32) {
        if let Some(popup) = self.popup.as_mut() {
            popup.resize(w, h, dx);
        }
        let _ = self.shared.events.send(ServiceEvent::PopupResized(w, h));
        self.queue_redraw();
    }

    fn popup_input(&mut self, event: egui::Event) {
        if let Some(popup) = self.popup.as_mut() {
            popup.events.push(event);
            self.queue_redraw();
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

// --- sctk handlers ---------------------------------------------------------

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // Integer scale, only when fractional scaling is not available.
        if self.popup_scale.is_some() {
            return;
        }
        if let (Some(layer), Some(popup)) = (&self.layer, self.popup.as_mut())
            && layer.wl_surface() == surface
        {
            // Applied to the card by `sync_card`; the backdrop has no detail
            // to lose.
            popup.scale = factor as f64;
            self.queue_redraw();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.close_popup();
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 || self.popup.is_none() {
            return;
        }
        self.attach_backdrop(layer, w, h);
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        if !popup.configured {
            log::info!(
                "popup: configured at {w}x{h}, {} ms after opening",
                popup.opened.elapsed().as_millis()
            );
        }
        popup.size = (w, h);
        popup.configured = true;
        if let Some(at) = popup.pointer {
            popup.place_at(at);
        }
        // The grace period may have run out before the first configure.
        if popup.opened.elapsed() >= ENTER_GRACE {
            popup.place_fallback();
        }
        self.queue_redraw();
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.adopt_seat(seat);
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.seat.as_ref() != Some(&seat) {
            return;
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self
                .seat_state
                .get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.handle.clone(),
                    Box::new(|state, _, event| state.key(event, true, true)),
                )
                .ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme_and_data(
                    qh,
                    &seat,
                    self.shm.wl_shm(),
                    cursor_surface,
                    ThemeSpec::default(),
                    EnterFrames(PointerData::new(seat.clone())),
                )
                .ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            // The last keyboard was unplugged; that is not focus going
            // elsewhere, so it must not close the popup.
            if let Some(popup) = self.popup.as_mut() {
                popup.keyboard_entered = false;
            }
            k.release();
        }
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl State {
    fn key(&mut self, event: KeyEvent, pressed: bool, repeat: bool) {
        let Some(popup) = self.popup.as_ref() else {
            return;
        };
        let modifiers = popup.modifiers;
        if let Some(key) = egui_key(event.keysym) {
            self.popup_input(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat,
                modifiers,
            });
        }
        if pressed
            && !modifiers.ctrl
            && !modifiers.alt
            && !modifiers.mac_cmd
            && let Some(text) = event.utf8
            && !text.chars().any(char::is_control)
        {
            self.popup_input(egui::Event::Text(text));
        }
    }
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if self.is_popup_surface(surface)
            && let Some(popup) = self.popup.as_mut()
        {
            popup.keyboard_entered = true;
            log::info!("popup: has the keyboard");
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Something else took the keyboard: a notification, a lock screen,
        // a click on another output. The popup goes, as on Windows.
        if self.is_popup_surface(surface) && self.popup.as_ref().is_some_and(|p| p.keyboard_entered)
        {
            log::info!("popup: lost the keyboard");
            self.close_popup();
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event, true, false);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event, true, true);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(event, false, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        m: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        if let Some(popup) = self.popup.as_mut() {
            popup.modifiers = egui::Modifiers {
                alt: m.alt,
                ctrl: m.ctrl,
                shift: m.shift,
                mac_cmd: false,
                command: m.ctrl,
            };
        }
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if !self.is_popup_surface(&event.surface) {
                continue;
            }
            let on_card = self
                .card
                .as_ref()
                .is_some_and(|c| c.surface == event.surface);
            let raw = egui::pos2(event.position.0 as f32, event.position.1 as f32);
            // Where the pointer is on the layer surface, which covers the
            // output, and on the card, which is where egui draws.
            let at = match self.popup.as_ref().and_then(|p| p.card) {
                Some(card) if on_card => raw + card.to_vec2(),
                _ => raw,
            };
            let mut pos = self.popup.as_ref().map_or(at, |p| p.to_card(at));
            let modifiers = self.popup.as_ref().map(|p| p.modifiers).unwrap_or_default();
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.pointer {
                        let _ = pointer.set_cursor(conn, CursorIcon::Default);
                    }
                    if let Some(popup) = self.popup.as_mut() {
                        log::info!(
                            "popup: pointer entered {} ms after opening",
                            popup.opened.elapsed().as_millis()
                        );
                        popup.pointer = Some(at);
                        popup.place_at(at);
                        pos = popup.to_card(at);
                    }
                    self.popup_input(egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Leave { .. } => self.popup_input(egui::Event::PointerGone),
                PointerEventKind::Motion { .. } => {
                    if let Some(popup) = self.popup.as_mut() {
                        popup.pointer = Some(at);
                    }
                    self.popup_input(egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Press { button, .. }
                | PointerEventKind::Release { button, .. } => {
                    let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                    // A click outside the card dismisses, as focus loss does on
                    // Windows.
                    let outside = self
                        .popup
                        .as_ref()
                        .and_then(|p| p.card_rect())
                        .is_some_and(|r| !r.contains(at));
                    if pressed && outside {
                        log::info!("popup: click outside the card");
                        self.close_popup();
                        return;
                    }
                    if let Some(button) = egui_button(button) {
                        self.popup_input(egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            modifiers,
                        });
                    }
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    // Discrete wheel clicks arrive as `absolute` too, in
                    // surface units, which is what egui wants as points.
                    self.popup_input(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-horizontal.absolute as f32, -vertical.absolute as f32),
                        modifiers,
                    });
                }
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for State {
    fn event(
        _: &mut State,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for State {
    fn event(
        state: &mut State,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(popup) = state.popup.as_mut()
        {
            popup.scale = scale as f64 / 120.0;
            state.queue_redraw();
        }
    }
}

impl Dispatch<WpViewporter, ()> for State {
    fn event(
        _: &mut State,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for State {
    fn event(
        _: &mut State,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_subcompositor!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);

/// The pointer's data, for a pointer whose enter is handled the moment it
/// arrives.
///
/// sctk holds a pointer's events until the `wl_pointer.frame` that ends them.
/// COSMIC sometimes sends the enter on a new surface with no frame after it
/// until the pointer next moves, and the popup places its card where the
/// pointer enters: with the mouse still, the card was centred instead. An
/// enter is whole on its own, so a frame of our own follows it; when the
/// compositor's frame does come, nothing is left pending and it does nothing.
pub struct EnterFrames(PointerData);

impl PointerDataExt for EnterFrames {
    fn pointer_data(&self) -> &PointerData {
        &self.0
    }
}

impl Dispatch<wl_pointer::WlPointer, EnterFrames> for State {
    fn event(
        state: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        data: &EnterFrames,
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let enter = matches!(event, wl_pointer::Event::Enter { .. });
        <SeatState as Dispatch<wl_pointer::WlPointer, EnterFrames, Self>>::event(
            state, pointer, event, data, conn, qh,
        );
        if enter {
            <SeatState as Dispatch<wl_pointer::WlPointer, EnterFrames, Self>>::event(
                state,
                pointer,
                wl_pointer::Event::Frame,
                data,
                conn,
                qh,
            );
        }
    }
}
delegate_layer!(State);
delegate_registry!(State);
