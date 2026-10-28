//! The surfaces the Wayland thread draws in full with the GPU: one per output
//! for the capture overlay, one for the notice card. Each is a layer surface
//! or an xdg toplevel ([`Role`]), with its own wgpu surface, viewport and
//! preferred scale, and its own egui viewport in the context of its
//! [`Canvas`]. The compositor's events for them (configure, scale, keyboard,
//! pointer) arrive here from `handlers.rs` and become egui input.

use super::State;
use super::gpu::Gpu;
use super::overlay::Role;
use super::popup::egui_button;
use crate::capture::surfaces::buffer_size;
use smithay_client_toolkit::seat::pointer::{CursorIcon, PointerEvent, PointerEventKind};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerSurface,
};
use smithay_client_toolkit::shell::xdg::window::{Window, WindowDecorations};
use std::time::Instant;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use windup_core::capture::session::Outcome;
use windup_ui::{Mode, Palette};

pub(super) enum Shell {
    Layer(LayerSurface),
    Window(Window),
}

/// One surface Windup draws in full with the GPU: an output of the overlay,
/// or the notice card.
pub(super) struct Pane {
    /// Declared first, so it is dropped before the surface it draws to.
    pub(super) wgpu: Option<wgpu::Surface<'static>>,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) viewport: Option<WpViewport>,
    pub(super) fractional: Option<WpFractionalScaleV1>,
    /// Logical size from the last configure; zero until then.
    pub(super) size: (u32, u32),
    /// The size to use when the compositor leaves it to us.
    pub(super) fallback: (u32, u32),
    pub(super) scale: f64,
    pub(super) configured: bool,
    /// The buffer size and logical size the GPU surface was configured for.
    pub(super) synced: Option<((u32, u32), (u32, u32))>,
    pub(super) events: Vec<egui::Event>,
    pub(super) id: egui::ViewportId,
    /// The monitor of the picture it shows (the overlay's panes).
    pub(super) monitor: usize,
    pub(super) shown: bool,
    /// Held for its drop, which destroys the role object and the surface.
    pub(super) _shell: Shell,
}

impl Pane {
    pub(super) fn is(&self, surface: &wl_surface::WlSurface) -> bool {
        &self.surface == surface
    }

    pub(super) fn logical(&self) -> (u32, u32) {
        if self.size.0 == 0 || self.size.1 == 0 {
            self.fallback
        } else {
            self.size
        }
    }

    /// Without a viewport the buffer scale is a whole number.
    pub(super) fn effective_scale(&self) -> f64 {
        match self.viewport {
            Some(_) => self.scale,
            None => self.scale.round().max(1.0),
        }
    }

    /// Sizes the GPU surface and the viewport to the configured size when it
    /// or the scale changed. Applied by the next commit, which is the frame.
    pub(super) fn sync(&mut self, gpu: &Gpu) -> (u32, u32) {
        let logical = self.logical();
        let px = buffer_size(logical, self.effective_scale());
        if self.synced != Some((px, logical))
            && let Some(surface) = &self.wgpu
        {
            gpu.configure(surface, px.0, px.1);
            match &self.viewport {
                Some(v) => v.set_destination(logical.0 as i32, logical.1 as i32),
                None => self.surface.set_buffer_scale(self.effective_scale() as i32),
            }
            self.synced = Some((px, logical));
        }
        px
    }

    pub(super) fn raw_input(
        &mut self,
        time: f64,
        modifiers: egui::Modifiers,
        focused: bool,
    ) -> egui::RawInput {
        let (w, h) = self.logical();
        let mut raw = egui::RawInput {
            viewport_id: self.id,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w as f32, h as f32),
            )),
            time: Some(time),
            modifiers,
            events: std::mem::take(&mut self.events),
            focused,
            ..Default::default()
        };
        let info = raw.viewports.entry(self.id).or_default();
        info.native_pixels_per_point = Some(self.effective_scale() as f32);
        info.focused = Some(focused);
        raw
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // The wgpu surface before the wl_surface; the viewport and scale
        // objects before the surface they belong to. `_shell` goes last.
        self.wgpu = None;
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
    }
}

/// An egui context and renderer for one capture or one notice.
pub(super) struct Canvas {
    pub(super) ctx: egui::Context,
    pub(super) renderer: Option<egui_wgpu::Renderer>,
    pub(super) epoch: Instant,
    pub(super) palette: Palette,
    pub(super) mode: Mode,
}

impl Canvas {
    pub(super) fn new(palette: Palette, mode: Mode) -> Self {
        let ctx = egui::Context::default();
        windup_ui::install_fonts(&ctx);
        Self {
            ctx,
            renderer: None,
            epoch: Instant::now(),
            palette,
            mode,
        }
    }

    pub(super) fn time(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

/// Which pane a surface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneRef {
    Overlay(usize),
    Notice,
}

impl State {
    /// A surface of `role`. `size` is a card's; `None` covers `output`.
    pub(super) fn new_pane(
        &mut self,
        role: Role,
        output: Option<&wl_output::WlOutput>,
        size: Option<(u32, u32)>,
        fallback: (u32, u32),
        scale: f64,
        id: egui::ViewportId,
    ) -> Result<Pane, String> {
        let surface = self.compositor.create_surface(&self.qh);
        let shell = match role {
            Role::Layer => {
                let shell = self.layer_shell.as_ref().ok_or("no layer-shell")?;
                let layer = shell.create_layer_surface(
                    &self.qh,
                    surface.clone(),
                    Layer::Overlay,
                    Some(if size.is_some() {
                        "windup-notice"
                    } else {
                        "windup-capture"
                    }),
                    output,
                );
                match size {
                    // Unanchored: the compositor centres it.
                    Some((w, h)) => layer.set_size(w, h),
                    None => {
                        layer.set_anchor(
                            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                        );
                        layer.set_exclusive_zone(-1);
                        layer.set_size(0, 0);
                    }
                }
                layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
                Shell::Layer(layer)
            }
            Role::Fullscreen => {
                let shell = self.xdg_shell.as_ref().ok_or("no xdg-shell")?;
                let window =
                    shell.create_window(surface.clone(), WindowDecorations::None, &self.qh);
                window.set_title(if size.is_some() {
                    "Windup notice"
                } else {
                    "Windup capture"
                });
                window.set_app_id(crate::APP_ID);
                match size {
                    Some((w, h)) => {
                        window.set_min_size(Some((w, h)));
                        window.set_max_size(Some((w, h)));
                    }
                    None => window.set_fullscreen(output),
                }
                Shell::Window(window)
            }
        };
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &self.qh, ()));
        // Fractional scale only with a viewport to apply it.
        let fractional = match (&viewport, &self.fractional) {
            (Some(_), Some(f)) => Some(f.get_fractional_scale(&surface, &self.qh, ())),
            _ => None,
        };
        // The first commit, with no buffer: asks for the configure.
        match &shell {
            Shell::Layer(l) => l.commit(),
            Shell::Window(w) => w.commit(),
        }
        let mut pane = Pane {
            wgpu: None,
            surface: surface.clone(),
            viewport,
            fractional,
            size: (0, 0),
            fallback,
            scale,
            configured: false,
            synced: None,
            events: Vec::new(),
            id,
            monitor: 0,
            shown: false,
            _shell: shell,
        };
        pane.wgpu = Some(self.wgpu_surface(&surface)?);
        Ok(pane)
    }

    pub(super) fn pane_of(&self, surface: &wl_surface::WlSurface) -> Option<PaneRef> {
        if let Some(o) = &self.overlay
            && let Some(i) = o.panes.iter().position(|p| p.is(surface))
        {
            return Some(PaneRef::Overlay(i));
        }
        self.notice
            .as_ref()
            .filter(|n| n.pane.is(surface))
            .map(|_| PaneRef::Notice)
    }

    fn pane_mut(&mut self, which: PaneRef) -> Option<&mut Pane> {
        match which {
            PaneRef::Overlay(i) => self.overlay.as_mut()?.panes.get_mut(i),
            PaneRef::Notice => self.notice.as_mut().map(|n| &mut n.pane),
        }
    }

    /// Whether key presses go to a pane rather than the popup.
    pub(super) fn panes_want_keys(&self) -> bool {
        self.overlay.is_some() || self.notice.is_some()
    }

    pub(super) fn pane_configured(
        &mut self,
        surface: &wl_surface::WlSurface,
        size: Option<(u32, u32)>,
    ) {
        let Some(which) = self.pane_of(surface) else {
            return;
        };
        if let Some(pane) = self.pane_mut(which) {
            if let Some(size) = size.filter(|s| s.0 > 0 && s.1 > 0) {
                pane.size = size;
            }
            pane.configured = true;
        }
        self.queue_pane_redraw();
    }

    /// The compositor closed a pane's surface.
    pub(super) fn pane_closed(&mut self, surface: &wl_surface::WlSurface) {
        match self.pane_of(surface) {
            Some(PaneRef::Overlay(_)) => self.end_capture(Outcome::Cancel, false, None),
            Some(PaneRef::Notice) => self.close_notice(),
            None => {}
        }
    }

    /// A preferred scale, fractional (`fractional`) or whole (`surface`).
    pub(super) fn pane_scale(
        &mut self,
        surface: Option<&wl_surface::WlSurface>,
        fractional: Option<&WpFractionalScaleV1>,
        scale: f64,
    ) -> bool {
        let mine = |p: &Pane| match (surface, fractional) {
            (_, Some(f)) => p.fractional.as_ref() == Some(f),
            // A whole scale counts only where no fractional one comes.
            (Some(s), None) => p.is(s) && p.fractional.is_none(),
            _ => false,
        };
        let pane = if let Some(o) = self.overlay.as_mut()
            && let Some(p) = o.panes.iter_mut().find(|p| mine(p))
        {
            Some(p)
        } else {
            self.notice
                .as_mut()
                .map(|n| &mut n.pane)
                .filter(|p| mine(p))
        };
        let Some(pane) = pane else {
            return false;
        };
        pane.scale = scale;
        self.queue_pane_redraw();
        true
    }

    pub(super) fn pane_keyboard(&mut self, surface: &wl_surface::WlSurface, entered: bool) {
        match self.pane_of(surface) {
            Some(PaneRef::Overlay(i)) => {
                if let Some(o) = self.overlay.as_mut() {
                    if entered {
                        log::info!("capture: output {i} has the keyboard");
                        o.keyboard = Some(i);
                    } else if o.keyboard == Some(i) {
                        o.keyboard = None;
                    }
                }
                self.queue_pane_redraw();
            }
            Some(PaneRef::Notice) => match self.notice.as_mut() {
                Some(n) if entered => n.had_keyboard = true,
                // Something else took the keyboard: the notice goes, as
                // the popup does.
                Some(n) if n.had_keyboard => self.close_notice(),
                _ => {}
            },
            None => {}
        }
    }

    /// A key, for the pane with the keyboard (else the first).
    pub(super) fn pane_key(&mut self, key: egui::Key, pressed: bool, repeat: bool) {
        let modifiers = self.pane_modifiers;
        let event = egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers,
        };
        let pane = match (self.overlay.as_mut(), self.notice.as_mut()) {
            (Some(o), _) => {
                let i = o.keyboard.unwrap_or(0);
                o.panes.get_mut(i)
            }
            (None, Some(n)) => Some(&mut n.pane),
            (None, None) => None,
        };
        if let Some(pane) = pane {
            pane.events.push(event);
            self.queue_pane_redraw();
        }
    }

    pub(super) fn pane_pointer(&mut self, which: PaneRef, event: &PointerEvent) {
        let pos = egui::pos2(event.position.0 as f32, event.position.1 as f32);
        let modifiers = self.pane_modifiers;
        if let (PaneRef::Overlay(i), Some(o)) = (which, self.overlay.as_mut()) {
            match event.kind {
                // The next frame sets the cursor this surface wants.
                PointerEventKind::Enter { .. } => {
                    o.pointer = Some(i);
                    o.cursor = None;
                }
                PointerEventKind::Leave { .. } if o.pointer == Some(i) => o.pointer = None,
                _ => {}
            }
        }
        if which == PaneRef::Notice
            && matches!(event.kind, PointerEventKind::Enter { .. })
            && let Some(pointer) = &self.pointer
        {
            let _ = pointer.set_cursor(&self.conn, CursorIcon::Default);
        }
        let egui_event = match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                Some(egui::Event::PointerMoved(pos))
            }
            PointerEventKind::Leave { .. } => Some(egui::Event::PointerGone),
            PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
                // A press on one output does not close a menu or popover
                // the bar has open on another: the view has no way to be
                // told. A known limit; Esc or a press on the bar's own
                // output closes it.
                egui_button(button).map(|button| egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: matches!(event.kind, PointerEventKind::Press { .. }),
                    modifiers,
                })
            }
            PointerEventKind::Axis {
                horizontal,
                vertical,
                ..
            } => Some(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-horizontal.absolute as f32, -vertical.absolute as f32),
                modifiers,
            }),
        };
        if let Some(e) = egui_event
            && let Some(pane) = self.pane_mut(which)
        {
            pane.events.push(e);
            self.queue_pane_redraw();
        }
    }

    pub(super) fn queue_pane_redraw(&mut self) {
        if self.pane_redraw_queued || !self.panes_want_keys() {
            return;
        }
        self.pane_redraw_queued = true;
        self.handle.insert_idle(|state| {
            state.pane_redraw_queued = false;
            state.draw_capture();
            state.draw_notice();
        });
    }

    /// Makes the GPU device from `surface` if there is none yet.
    pub(super) fn ensure_gpu(&mut self, surface: &wgpu::Surface<'static>) -> Result<(), String> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let instance = self.instance.as_ref().ok_or("no wgpu instance")?;
        self.gpu = Some(Gpu::new(instance, surface)?);
        Ok(())
    }
}

/// The Wayland cursor for egui's.
pub(super) fn wl_cursor(icon: egui::CursorIcon) -> CursorIcon {
    use egui::CursorIcon as E;
    match icon {
        E::Crosshair => CursorIcon::Crosshair,
        E::Move | E::AllScroll => CursorIcon::Move,
        E::Grab => CursorIcon::Grab,
        E::Grabbing => CursorIcon::Grabbing,
        E::PointingHand => CursorIcon::Pointer,
        E::Text => CursorIcon::Text,
        E::NotAllowed | E::NoDrop => CursorIcon::NotAllowed,
        E::ResizeHorizontal | E::ResizeColumn => CursorIcon::EwResize,
        E::ResizeVertical | E::ResizeRow => CursorIcon::NsResize,
        E::ResizeNeSw => CursorIcon::NeswResize,
        E::ResizeNwSe => CursorIcon::NwseResize,
        E::ResizeEast => CursorIcon::EResize,
        E::ResizeWest => CursorIcon::WResize,
        E::ResizeNorth => CursorIcon::NResize,
        E::ResizeSouth => CursorIcon::SResize,
        E::ResizeNorthEast => CursorIcon::NeResize,
        E::ResizeNorthWest => CursorIcon::NwResize,
        E::ResizeSouthEast => CursorIcon::SeResize,
        E::ResizeSouthWest => CursorIcon::SwResize,
        _ => CursorIcon::Default,
    }
}
