//! The GPU side of everything the Wayland thread draws: one device for the
//! life of the app, and putting one egui frame on one surface.
//!
//! egui runs here exactly as eframe runs it, through `egui-wgpu`.

use std::time::Instant;
use windup_ui::PopupView;

/// GPU objects and the popup's egui context, kept for the life of the app so
/// the second popup opens without the cost of the first. The renderer and the
/// context belong together: the renderer holds the textures the context has
/// already uploaded. The capture overlay and the notice share the device but
/// bring a context and renderer of their own (`overlay::Canvas`).
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    /// Chosen once, with the adapter, for every popup after it: a surface
    /// configured with `Auto` is opaque on most Wayland drivers.
    pub alpha: wgpu::CompositeAlphaMode,
    pub renderer: egui_wgpu::Renderer,
    pub ctx: egui::Context,
    pub view: PopupView,
    pub epoch: Instant,
}

impl Gpu {
    /// Creates everything for a first surface.
    pub fn new(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'static>,
    ) -> Result<Self, String> {
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
        let info = adapter.get_info();
        log::info!(
            "popup: {} on {:?}, alpha modes {:?}",
            info.name,
            info.backend,
            caps.alpha_modes
        );
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

        Ok(Self {
            device,
            queue,
            format,
            alpha,
            renderer,
            ctx,
            view: PopupView::new(),
            epoch: Instant::now(),
        })
    }

    /// Sizes `surface` to `width` x `height` buffer pixels.
    pub fn configure(&self, surface: &wgpu::Surface<'static>, width: u32, height: u32) {
        surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width,
                height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: self.alpha,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
    }
}

fn pick_alpha(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode::*;
    // egui's output is premultiplied; with an opaque surface the card's
    // rounded corners show square, in the card's own colour.
    [PreMultiplied, PostMultiplied, Inherit]
        .into_iter()
        .find(|m| modes.contains(m))
        .unwrap_or(Auto)
}

/// Whether a surface configured with `mode` shows what is behind it.
pub fn transparent(mode: wgpu::CompositeAlphaMode) -> bool {
    use wgpu::CompositeAlphaMode::*;
    matches!(mode, PreMultiplied | PostMultiplied | Inherit)
}

/// What became of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Painted {
    Shown,
    /// The surface is outdated: configure it again and draw again.
    Reconfigure,
    /// No frame this time; try again shortly.
    Skipped,
}

/// The colour a card clears to around its rounded corners: transparent, or
/// where the surface cannot be, the card's own colour, so the corners show
/// square rather than black.
pub fn card_clear(alpha: wgpu::CompositeAlphaMode, card: egui::Color32) -> wgpu::Color {
    if transparent(alpha) {
        return wgpu::Color::TRANSPARENT;
    }
    let [r, g, b, _] = card.to_normalized_gamma_f32();
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: 1.0,
    }
}

/// Puts one egui frame (`output`, from `ctx`) on `surface`, configured at
/// `size` buffer pixels. The frame's texture changes reach `renderer` even
/// when no frame can be shown: a context drawing several surfaces hands each
/// change out once, so a change dropped here would be lost to all of them.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    surface: &wgpu::Surface<'static>,
    output: &mut egui::FullOutput,
    size: (u32, u32),
    clear: wgpu::Color,
) -> Painted {
    let textures = std::mem::take(&mut output.textures_delta);
    for (id, delta) in &textures.set {
        renderer.update_texture(device, queue, *id, delta);
    }
    let painted = present(device, queue, renderer, ctx, surface, output, size, clear);
    for id in &textures.free {
        renderer.free_texture(id);
    }
    painted
}

#[allow(clippy::too_many_arguments)]
fn present(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    surface: &wgpu::Surface<'static>,
    output: &mut egui::FullOutput,
    size: (u32, u32),
    clear: wgpu::Color,
) -> Painted {
    let ppp = output.pixels_per_point;
    let prims = ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [size.0, size.1],
        pixels_per_point: ppp,
    };
    let frame = match surface.get_current_texture() {
        Ok(frame) => frame,
        Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
            return Painted::Reconfigure;
        }
        Err(e) => {
            log::warn!("frame skipped: {e}");
            return Painted::Skipped;
        }
    };
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("windup"),
    });
    let extra = renderer.update_buffers(device, queue, &mut encoder, &prims, &screen);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("windup"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &prims, &screen);
    }
    queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
    frame.present();
    Painted::Shown
}
