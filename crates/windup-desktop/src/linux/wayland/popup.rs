//! The clipboard popup as a layer-shell surface.
//!
//! The surface covers the whole output on the overlay layer and is
//! transparent; the card is drawn inside it where the pointer is. Wayland
//! never tells a client where the pointer is on the screen, but it does say
//! where it is when it enters one of the client's surfaces, and a surface
//! covering the output is entered wherever the pointer happens to be.
//!
//! egui runs here exactly as eframe runs it, through `egui-wgpu`, and draws
//! the same `windup_ui::PopupView` the Windows popup draws.

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use std::time::{Duration, Instant};
use windup_core::clipboard::ClipboardHistory;
use windup_core::placement::{self, Area};
use windup_ui::{Mode, Palette, PopupAction, PopupView, apply_theme};

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

/// GPU objects and the egui context, kept for the life of the app so the
/// second popup opens without the cost of the first. The renderer and the
/// context belong together: the renderer holds the textures the context has
/// already uploaded.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub renderer: egui_wgpu::Renderer,
    pub ctx: egui::Context,
    pub view: PopupView,
}

impl Gpu {
    /// Creates everything for a first surface.
    pub fn new(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'static>,
    ) -> Result<(Self, wgpu::CompositeAlphaMode), String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("no graphics adapter for the popup: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("windup popup"),
            ..Default::default()
        }))
        .map_err(|e| format!("no graphics device for the popup: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        // egui blends in linear space and expects a non-sRGB target, as eframe
        // chooses one.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("the surface supports no format")?;
        let alpha = pick_alpha(&caps.alpha_modes);
        let renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        let ctx = egui::Context::default();
        windup_ui::install_fonts(&ctx);

        Ok((
            Self {
                device,
                queue,
                format,
                renderer,
                ctx,
                view: PopupView::new(),
            },
            alpha,
        ))
    }
}

fn pick_alpha(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode::*;
    // egui's output is premultiplied; the transparent margin must stay
    // transparent or the whole output goes black.
    [PreMultiplied, PostMultiplied, Inherit]
        .into_iter()
        .find(|m| modes.contains(m))
        .unwrap_or(Auto)
}

/// One open popup.
pub struct Popup {
    pub setup: PopupSetup,
    pub surface: Option<wgpu::Surface<'static>>,
    pub alpha: wgpu::CompositeAlphaMode,
    /// Logical size of the layer surface, from its configure.
    pub size: (u32, u32),
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
    pub dirty: bool,
}

impl Popup {
    pub fn new(setup: PopupSetup) -> Self {
        let card_size = vec2(setup.size.0, setup.size.1);
        Self {
            setup,
            surface: None,
            alpha: wgpu::CompositeAlphaMode::Auto,
            size: (0, 0),
            scale: 1.0,
            configured: false,
            card: None,
            card_size,
            opened: Instant::now(),
            events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            pointer: None,
            keyboard_entered: false,
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
            self.card = Some(pos2(x, y));
            self.dirty = true;
        }
    }

    /// Centres the card when the pointer never entered.
    pub fn place_fallback(&mut self) {
        if self.card.is_none() && self.configured {
            let (x, y) = placement::centre(self.area(), (self.card_size.x, self.card_size.y));
            self.card = Some(pos2(x, y));
            self.dirty = true;
        }
    }

    pub fn card_rect(&self) -> Option<Rect> {
        self.card.map(|p| Rect::from_min_size(p, self.card_size))
    }

    pub fn physical_size(&self) -> (u32, u32) {
        (
            ((self.size.0 as f64) * self.scale).round().max(1.0) as u32,
            ((self.size.1 as f64) * self.scale).round().max(1.0) as u32,
        )
    }

    pub fn configure_surface(&self, gpu: &Gpu) {
        let Some(surface) = &self.surface else {
            return;
        };
        let (width, height) = self.physical_size();
        surface.configure(
            &gpu.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: gpu.format,
                width,
                height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: self.alpha,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
    }

    /// Runs one egui frame and presents it. Returns what the user did, and
    /// how soon egui wants to be run again.
    pub fn draw(
        &mut self,
        gpu: &mut Gpu,
        history: &ClipboardHistory,
    ) -> (Option<PopupAction>, Option<Duration>) {
        self.dirty = false;
        let Some(surface) = &self.surface else {
            return (None, None);
        };
        let logical = vec2(self.size.0 as f32, self.size.1 as f32);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, logical)),
            time: Some(self.opened.elapsed().as_secs_f64()),
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        let info = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.native_pixels_per_point = Some(self.scale as f32);
        info.focused = Some(true);

        let mut action = None;
        let card = self.card_rect();
        let setup = &self.setup;
        let view = &mut gpu.view;
        let output = gpu.ctx.run(raw, |ctx| {
            apply_theme(ctx, setup.palette, setup.mode);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if let Some(rect) = card {
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.set_clip_rect(rect);
                            action = view.ui(ui, history, &setup.palette);
                        });
                    }
                });
        });

        let ppp = output.pixels_per_point;
        let prims = gpu.ctx.tessellate(output.shapes, ppp);
        let (width, height) = self.physical_size();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: ppp,
        };

        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.configure_surface(gpu);
                self.dirty = true;
                return (action, Some(Duration::ZERO));
            }
            Err(e) => {
                log::warn!("popup frame skipped: {e}");
                return (action, Some(Duration::from_millis(16)));
            }
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        for (id, delta) in &output.textures_delta.set {
            gpu.renderer
                .update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("windup popup"),
            });
        let extra =
            gpu.renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &prims, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("windup popup"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            gpu.renderer.render(&mut pass, &prims, &screen);
        }
        gpu.queue
            .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        frame.present();
        for id in &output.textures_delta.free {
            gpu.renderer.free_texture(id);
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
