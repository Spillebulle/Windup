//! The capture overlay on two fake monitors, side by side in one window, for
//! looking at: `cargo run -p windup-ui --example capture-view`.
//!
//! Monitor 0 sits left of the origin at 1x and lower down; monitor 1 is at
//! the origin at 2x (twice as many pixels as desktop units). Both pictures
//! are generated: a gradient desktop with a few light windows on it, and
//! fine stripes in the windows so the loupe has edges to show. Never a real
//! screenshot. Each outcome is printed and the session starts again.
//!
//! `WINDUP_EXAMPLE_SECS=3` closes the window after that many seconds.

use eframe::egui;
use egui::{Rect, UiBuilder, vec2};
use image::{Rgba, RgbaImage};
use windup_core::capture::session::{ModeAvailability, Outcome, Session};
use windup_core::capture::{DRect, DesktopShot, MonitorShot, WindowRect};
use windup_core::config::{EditorKeys, ScreenshotConfig, VideoConfig};
use windup_ui::capture::CaptureView;
use windup_ui::{Mode, Palette, apply_theme, install_fonts, metrics};

fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
    DRect { x, y, w, h }
}

/// The fake windows, in desktop units, front to back.
fn windows() -> Vec<WindowRect> {
    [
        (rect(-1100.0, 220.0, 620.0, 420.0), "Notes"),
        (rect(-380.0, 160.0, 760.0, 480.0), "Browser"),
        (rect(520.0, 90.0, 600.0, 560.0), "Terminal"),
    ]
    .into_iter()
    .map(|(rect, title)| WindowRect {
        rect,
        title: title.to_string(),
    })
    .collect()
}

/// A monitor's picture: what the desktop looks like at `layout`, `ppu`
/// pixels per desktop unit.
fn picture(layout: DRect, ppu: f32, windows: &[WindowRect]) -> RgbaImage {
    let (w, h) = ((layout.w * ppu) as u32, (layout.h * ppu) as u32);
    RgbaImage::from_fn(w, h, |px, py| {
        let x = layout.x + (px as f32 + 0.5) / ppu;
        let y = layout.y + (py as f32 + 0.5) / ppu;
        // Front to back: the first window holding the point is on top.
        for win in windows {
            let r = win.rect;
            if r.contains(x, y) {
                if y < r.y + 22.0 {
                    return Rgba([197, 190, 181, 255]);
                }
                // Lines of "text": one-unit stripes every four units.
                let line = ((y - r.y - 22.0) as i32 / 4) % 2 == 0;
                let margin = x > r.x + 16.0 && x < r.right() - 40.0;
                if line && margin && ((y - r.y) as i32 % 24) < 14 {
                    return Rgba([60, 64, 72, 255]);
                }
                return Rgba([220, 216, 210, 255]);
            }
        }
        let t = ((x + 1280.0) / 2560.0).clamp(0.0, 1.0);
        let s = (y / 800.0).clamp(0.0, 1.0);
        let mix = |a: f32, b: f32, k: f32| (a + (b - a) * k) as u8;
        Rgba([
            mix(60.0, 77.0, t),
            mix(79.0, 59.0, t * s),
            mix(99.0, 51.0, t),
            255,
        ])
    })
}

fn session() -> Session {
    let windows = windows();
    let monitors = [
        (rect(-1280.0, 80.0, 1280.0, 720.0), 1.0, "Left"),
        (rect(0.0, 0.0, 1280.0, 800.0), 2.0, "Right"),
    ]
    .into_iter()
    .map(|(layout, ppu, name)| MonitorShot {
        layout,
        image: picture(layout, ppu, &windows),
        name: name.to_string(),
        scale: 1.0,
        refresh_hz: None,
    })
    .collect();
    Session::new(
        DesktopShot { monitors },
        windows,
        &ScreenshotConfig::default(),
        &VideoConfig::default(),
        ModeAvailability::ALL,
    )
}

struct App {
    session: Session,
    view: CaptureView,
    keys: EditorKeys,
    palette: Palette,
    close_at: Option<f64>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(at) = self.close_at {
            let now = ctx.input(|i| i.time);
            if now >= at {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                // Nothing may be animating, and egui paints only when asked.
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(at - now));
            }
        }
        let p = self.palette;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.backdrop))
            .show(ctx, |ui| {
                // The desktop scaled to fit, one rect per monitor where its
                // layout puts it, each its own surface.
                let area = ui.max_rect().shrink(metrics::S3);
                let bounds = self.session.shot.bounds();
                let scale = (area.width() / bounds.w).min(area.height() / bounds.h);
                let mut outcome = None;
                for i in 0..self.session.shot.monitors.len() {
                    let l = self.session.shot.monitors[i].layout;
                    let surface = Rect::from_min_size(
                        area.min + vec2((l.x - bounds.x) * scale, (l.y - bounds.y) * scale),
                        vec2(l.w * scale, l.h * scale),
                    );
                    let out = ui
                        .scope_builder(UiBuilder::new().max_rect(surface), |ui| {
                            ui.set_clip_rect(surface);
                            self.view
                                .monitor_ui(ui, &mut self.session, i, &p, &self.keys)
                        })
                        .inner;
                    outcome = outcome.or(out);
                }
                if self.view.take_open_settings() {
                    println!("Settings asked for.");
                }
                if let Some(o) = outcome {
                    match &o {
                        Outcome::Copy(img) => println!("Copy {}x{}", img.width(), img.height()),
                        Outcome::Save(img) => println!("Save {}x{}", img.width(), img.height()),
                        Outcome::CopyAndSave(img) => {
                            println!("Copy and save {}x{}", img.width(), img.height())
                        }
                        Outcome::Record { rect, choice } => {
                            println!("Record {}x{} at {} fps", rect.w, rect.h, choice.fps)
                        }
                        Outcome::Cancel => println!("Cancel"),
                    }
                    println!("Starting again.");
                    self.session = session();
                    self.view = CaptureView::new();
                }
            });
    }
}

fn main() -> eframe::Result {
    println!(
        "Drag an area; Enter copies, Ctrl+S saves, Esc cancels, Z toggles the loupe, \
         the wheel sizes the tool."
    );
    let close_secs = std::env::var("WINDUP_EXAMPLE_SECS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Capture view")
            .with_inner_size([1320.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "windup-capture-view",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            let palette = Palette::dark(28.0);
            apply_theme(&cc.egui_ctx, palette, Mode::Dark);
            Ok(Box::new(App {
                session: session(),
                view: CaptureView::new(),
                keys: EditorKeys::default(),
                palette,
                close_at: close_secs,
            }))
        }),
    )
}
