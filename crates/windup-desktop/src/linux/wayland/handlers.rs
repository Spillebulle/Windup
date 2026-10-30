//! The sctk and protocol handlers of the Wayland thread's state: what the
//! compositor says about seats, outputs, surfaces, keys and the pointer, sent
//! on to the clipboard popup, the capture overlay or the notice.

use super::State;
use super::popup::{ENTER_GRACE, egui_button, egui_key};
use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerData, PointerDataExt, PointerEvent, PointerEventKind, PointerHandler,
    ThemeSpec,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::xdg::window::{Window, WindowConfigure, WindowHandler};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_subcompositor, delegate_xdg_shell,
    delegate_xdg_window, registry_handlers,
};
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // A pane takes it only when it has no fractional scale.
        if self.pane_scale(Some(surface), None, factor as f64) {
            return;
        }
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

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Only the recording bar asks for one, when it is hidden for a shot.
        self.recbar_concealed(Some(surface));
    }

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
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.pane_of(layer.wl_surface()).is_some() {
            self.pane_closed(layer.wl_surface());
        } else {
            self.close_popup();
        }
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
        if self.pane_of(layer.wl_surface()).is_some() {
            self.pane_configured(layer.wl_surface(), Some((w, h)));
            return;
        }
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

impl WindowHandler for State {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        self.pane_closed(window.wl_surface());
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let size = match configure.new_size {
            (Some(w), Some(h)) => Some((w.get(), h.get())),
            _ => None,
        };
        self.pane_configured(window.wl_surface(), size);
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
        if self.panes_want_keys() {
            if let Some(key) = egui_key(event.keysym) {
                self.pane_key(key, pressed, repeat);
            }
            return;
        }
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
        if self.pane_of(surface).is_some() {
            self.pane_keyboard(surface, true);
            return;
        }
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
        if self.pane_of(surface).is_some() {
            self.pane_keyboard(surface, false);
            return;
        }
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
        let modifiers = egui::Modifiers {
            alt: m.alt,
            ctrl: m.ctrl,
            shift: m.shift,
            mac_cmd: false,
            command: m.ctrl,
        };
        self.pane_modifiers = modifiers;
        if let Some(popup) = self.popup.as_mut() {
            popup.modifiers = modifiers;
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
            if let Some(pane) = self.pane_of(&event.surface) {
                self.pane_pointer(pane, event);
                continue;
            }
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
        proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && state.pane_scale(None, Some(proxy), scale as f64 / 120.0)
        {
            return;
        }
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
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
delegate_registry!(State);
