//! The clipboard popup as a layer-shell surface.
//!
//! The layer surface covers the whole output on the overlay layer, holding a
//! transparent shared-memory buffer, and the card is a subsurface of it placed
//! where the pointer is. Wayland never tells a client where the pointer is on
//! the screen, but it does say where it is when it enters one of the client's
//! surfaces, and a surface covering the output is entered wherever the pointer
//! happens to be.
//!
//! Only the card is drawn by the GPU. A GPU surface the size of the output
//! would have to be transparent, and some drivers (NVIDIA's Vulkan among
//! them) offer only opaque ones on Wayland: the whole screen went black. A
//! shared-memory buffer is transparent on every compositor and every driver.
//!
//! egui draws the same `windup_ui::PopupView` the Windows popup draws, through
//! `gpu.rs`.

use super::gpu::{Gpu, Painted, card_clear, paint};
use super::{Card, LocalService, Payload, Request, ServiceEvent, State};
use calloop::timer::{TimeoutAction, Timer};
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerSurface,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use wayland_client::Proxy;
use wayland_client::protocol::{wl_shm, wl_surface};
use windup_core::clipboard::ClipboardHistory;
use windup_core::placement::{self, Area};
use windup_ui::{Mode, Palette, PopupAction, apply_theme};

/// What the popup needs from the app's configuration.
#[derive(Clone)]
pub struct PopupSetup {
    pub palette: Palette,
    pub mode: Mode,
    pub size: (f32, f32),
    pub auto_paste: bool,
}

/// How long to wait for the pointer to enter before centring the card. A
/// compositor that only reports the pointer once it moves would otherwise
/// leave the popup invisible until then.
pub const ENTER_GRACE: Duration = Duration::from_millis(150);

/// One open popup.
pub struct Popup {
    pub setup: PopupSetup,
    pub surface: Option<wgpu::Surface<'static>>,
    /// Logical size of the layer surface, from its configure.
    pub size: (u32, u32),
    /// The card's position, size and scale as last given to the compositor,
    /// so the subsurface is moved and resized only when one of them changes.
    pub synced: Option<(Pos2, Vec2, f64)>,
    pub scale: f64,
    pub configured: bool,
    /// Where the card is, once known; `None` until the pointer enters or the
    /// grace period runs out.
    pub card: Option<Pos2>,
    pub card_size: Vec2,
    pub opened: Instant,
    pub events: Vec<egui::Event>,
    pub modifiers: egui::Modifiers,
    pub pointer: Option<Pos2>,
    pub keyboard_entered: bool,
    /// A frame has reached the screen.
    pub shown: bool,
    pub dirty: bool,
}

impl Popup {
    pub fn new(setup: PopupSetup) -> Self {
        let card_size = vec2(setup.size.0, setup.size.1);
        Self {
            setup,
            surface: None,
            size: (0, 0),
            synced: None,
            scale: 1.0,
            configured: false,
            card: None,
            card_size,
            opened: Instant::now(),
            events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            pointer: None,
            keyboard_entered: false,
            shown: false,
            dirty: true,
        }
    }

    fn area(&self) -> Area {
        Area {
            left: 0.0,
            top: 0.0,
            right: self.size.0 as f32,
            bottom: self.size.1 as f32,
        }
    }

    /// Puts the card at the pointer, once.
    pub fn place_at(&mut self, at: Pos2) {
        if self.card.is_none() && self.configured {
            let size = (self.card_size.x, self.card_size.y);
            let (x, y) = placement::place((at.x, at.y), self.area(), size);
            log::info!(
                "popup: placed at the pointer, {} ms after opening",
                self.opened.elapsed().as_millis()
            );
            self.card = Some(pos2(x, y));
            self.dirty = true;
        }
    }

    /// Centres the card when the pointer never entered.
    pub fn place_fallback(&mut self) {
        if self.card.is_none() && self.configured {
            let (x, y) = placement::centre(self.area(), (self.card_size.x, self.card_size.y));
            log::info!(
                "popup: no pointer {} ms after opening; centred",
                self.opened.elapsed().as_millis()
            );
            self.card = Some(pos2(x, y));
            self.dirty = true;
        }
    }

    pub fn card_rect(&self) -> Option<Rect> {
        self.card.map(|p| Rect::from_min_size(p, self.card_size))
    }

    /// The card's size in whole logical pixels, as the subsurface has it.
    pub fn logical_size(&self) -> (u32, u32) {
        (
            self.card_size.x.ceil().max(1.0) as u32,
            self.card_size.y.ceil().max(1.0) as u32,
        )
    }

    /// The card's size in buffer pixels.
    pub fn physical_size(&self) -> (u32, u32) {
        let (w, h) = self.logical_size();
        (
            ((w as f64) * self.scale).round().max(1.0) as u32,
            ((h as f64) * self.scale).round().max(1.0) as u32,
        )
    }

    /// Where a point on the layer surface is on the card.
    pub fn to_card(&self, at: Pos2) -> Pos2 {
        match self.card {
            Some(card) => at - card.to_vec2(),
            None => at,
        }
    }

    pub fn configure_surface(&self, gpu: &Gpu) {
        if let Some(surface) = &self.surface {
            let (width, height) = self.physical_size();
            gpu.configure(surface, width, height);
        }
    }

    /// Runs one egui frame and presents it. Returns what the user did, and
    /// how soon egui wants to be run again.
    pub fn draw(
        &mut self,
        gpu: &mut Gpu,
        history: &ClipboardHistory,
    ) -> (Option<PopupAction>, Option<Duration>) {
        self.dirty = false;
        let (Some(surface), Some(_)) = (&self.surface, self.card) else {
            return (None, None);
        };
        let (w, h) = self.logical_size();
        let logical = vec2(w as f32, h as f32);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, logical)),
            // One clock for the life of the context, which outlives popups.
            time: Some(gpu.epoch.elapsed().as_secs_f64()),
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        let info = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.native_pixels_per_point = Some(self.scale as f32);
        info.focused = Some(true);

        let mut action = None;
        let card = Rect::from_min_size(Pos2::ZERO, self.card_size);
        let setup = &self.setup;
        let view = &mut gpu.view;
        let mut output = gpu.ctx.run(raw, |ctx| {
            apply_theme(ctx, setup.palette, setup.mode);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                        ui.set_clip_rect(card);
                        action = view.ui(ui, history, &setup.palette);
                    });
                });
        });

        let clear = card_clear(gpu.alpha, self.setup.palette.dock);
        match paint(
            &gpu.device,
            &gpu.queue,
            &mut gpu.renderer,
            &gpu.ctx,
            surface,
            &mut output,
            self.physical_size(),
            clear,
        ) {
            Painted::Shown => {}
            Painted::Reconfigure => {
                self.configure_surface(gpu);
                self.dirty = true;
                return (action, Some(Duration::ZERO));
            }
            Painted::Skipped => return (action, Some(Duration::from_millis(16))),
        }

        let again = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .filter(|d| *d < Duration::from_secs(5));
        (action, again)
    }

    /// Applies a resize the popup asked for: the card grows from its top-left
    /// corner, or from its top-right one when dragged by the left grip.
    pub fn resize(&mut self, w: f32, h: f32, dx: f32) {
        self.card_size = vec2(w, h);
        if let Some(card) = self.card.as_mut() {
            card.x += dx;
        }
        self.dirty = true;
    }
}

/// An egui key for an XKB keysym, for the keys the popup uses.
pub fn egui_key(sym: smithay_client_toolkit::seat::keyboard::Keysym) -> Option<egui::Key> {
    use egui::Key;
    use smithay_client_toolkit::seat::keyboard::Keysym as K;
    Some(match sym {
        K::Escape => Key::Escape,
        K::Return | K::KP_Enter => Key::Enter,
        K::Tab | K::ISO_Left_Tab => Key::Tab,
        K::BackSpace => Key::Backspace,
        K::Delete | K::KP_Delete => Key::Delete,
        K::Up | K::KP_Up => Key::ArrowUp,
        K::Down | K::KP_Down => Key::ArrowDown,
        K::Left | K::KP_Left => Key::ArrowLeft,
        K::Right | K::KP_Right => Key::ArrowRight,
        K::Home | K::KP_Home => Key::Home,
        K::End | K::KP_End => Key::End,
        K::Page_Up | K::KP_Page_Up => Key::PageUp,
        K::Page_Down | K::KP_Page_Down => Key::PageDown,
        K::space => Key::Space,
        K::Insert => Key::Insert,
        other => {
            let c = other.key_char()?;
            return Key::from_name(&c.to_ascii_uppercase().to_string());
        }
    })
}

/// The egui pointer button for a Linux input event code.
pub fn egui_button(code: u32) -> Option<egui::PointerButton> {
    match code {
        0x110 => Some(egui::PointerButton::Primary),
        0x111 => Some(egui::PointerButton::Secondary),
        0x112 => Some(egui::PointerButton::Middle),
        0x113 => Some(egui::PointerButton::Extra1),
        0x114 => Some(egui::PointerButton::Extra2),
        _ => None,
    }
}

// --- the popup on the Wayland thread -----------------------------------------

impl State {
    pub(super) fn open_popup(&mut self, setup: PopupSetup) {
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

    pub(super) fn wgpu_surface(
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

    pub(super) fn close_popup(&mut self) {
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

    pub(super) fn queue_redraw(&mut self) {
        if self.redraw_queued || self.popup.is_none() {
            return;
        }
        self.redraw_queued = true;
        self.handle.insert_idle(|state| {
            state.redraw_queued = false;
            state.redraw();
        });
    }

    pub(super) fn redraw(&mut self) {
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
    pub(super) fn sync_card(&mut self) {
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
    pub(super) fn attach_backdrop(&mut self, layer: &LayerSurface, w: u32, h: u32) {
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
    pub(super) fn is_popup_surface(&self, surface: &wl_surface::WlSurface) -> bool {
        self.layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
            || self.card.as_ref().is_some_and(|c| &c.surface == surface)
    }

    pub(super) fn popup_action(&mut self, action: PopupAction) {
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

    pub(super) fn resize_popup(&mut self, w: f32, h: f32, dx: f32) {
        if let Some(popup) = self.popup.as_mut() {
            popup.resize(w, h, dx);
        }
        let _ = self.shared.events.send(ServiceEvent::PopupResized(w, h));
        self.queue_redraw();
    }

    pub(super) fn popup_input(&mut self, event: egui::Event) {
        if let Some(popup) = self.popup.as_mut() {
            popup.events.push(event);
            self.queue_redraw();
        }
    }
}
