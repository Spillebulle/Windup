//! The popup as an ordinary window of its own, for Wayland desktops without
//! layer-shell (GNOME). Wayland cannot hide a window, and a window that is
//! never shown never draws, so Windup keeps none open: each popup is a short
//! eframe run that returns when it closes. The compositor places it, which
//! on GNOME is the middle of the screen. The capture notice opens the same
//! way.

use super::wayland::PopupSetup;
use eframe::egui;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windup_core::clipboard::ClipboardHistory;
use windup_core::ipc::Command;
use windup_ui::{Mode, Palette, PopupAction, PopupView, apply_theme, install_fonts, notice};

/// What the popup leaves for the host to do once its window is gone.
#[derive(Default)]
pub struct Closed {
    /// A pick to paste. Done after the window closes, so focus is back on
    /// the window the paste is for.
    pub action: Option<PopupAction>,
    /// The size the popup was resized to.
    pub size: Option<(f32, f32)>,
    /// Commands that arrived while it was open, other than closing it.
    pub later: Vec<Command>,
}

/// Shows the popup and returns when it closes.
pub fn run(
    setup: &PopupSetup,
    history: &Arc<Mutex<ClipboardHistory>>,
    commands: &Receiver<Command>,
) -> Closed {
    log::info!("popup window: opening");
    let mut closed = Closed::default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Clipboard History")
            .with_app_id(crate::APP_ID)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_active(true)
            .with_inner_size([setup.size.0, setup.size.1]),
        run_and_return: true,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Windup",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(PopupWindow {
                setup,
                history,
                commands,
                view: PopupView::new(),
                shown: false,
                focused_once: false,
                closed: &mut closed,
            }))
        }),
    );
    if let Err(e) = result {
        log::error!("the popup window failed: {e}");
    }
    log::info!("popup window: closed");
    closed
}

struct PopupWindow<'a> {
    setup: &'a PopupSetup,
    history: &'a Arc<Mutex<ClipboardHistory>>,
    commands: &'a Receiver<Command>,
    view: PopupView,
    shown: bool,
    focused_once: bool,
    closed: &'a mut Closed,
}

impl eframe::App for PopupWindow<'_> {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let close = || ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        apply_theme(ctx, self.setup.palette, self.setup.mode);

        // The shortcut again closes it, as on Windows.
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Popup => close(),
                other => self.closed.later.push(other),
            }
        }

        if !self.shown {
            self.shown = true;
            log::info!("popup window: shown");
        }

        let mut action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let history = self.history.lock().unwrap();
                action = self.view.ui(ui, &history, &self.setup.palette);
            });

        // Focus moving elsewhere dismisses it. Not before it ever had focus:
        // the compositor may take a moment to give it.
        match ctx.input(|i| i.viewport().focused) {
            Some(true) => self.focused_once = true,
            Some(false) if self.focused_once => close(),
            _ => {}
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close();
        }

        match action {
            Some(PopupAction::Delete(id)) => {
                self.history.lock().unwrap().remove_by_id(id);
            }
            // The compositor decides where the window is, so growing to the
            // left is growing.
            Some(PopupAction::Resize(w, h) | PopupAction::ResizeLeft(w, h, _)) => {
                self.closed.size = Some((w, h));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
            Some(PopupAction::Dismiss) => close(),
            Some(pick @ (PopupAction::Paste(_) | PopupAction::PastePlain(_))) => {
                self.closed.action = Some(pick);
                close();
            }
            None => {}
        }

        // Keeps reading commands while nothing moves.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// The notice card as a small window of its own, with OK (and "Open folder"
/// when `action` names a saved file); Enter and Esc close it too. Returns,
/// once it has closed, the commands that arrived meanwhile.
pub fn notice(
    message: &str,
    action: Option<&std::path::Path>,
    palette: Palette,
    mode: Mode,
    commands: &Receiver<Command>,
) -> Vec<Command> {
    log::info!("notice window: opening");
    let size = notice_size(message, &palette);
    let mut later = Vec::new();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Windup notice")
            .with_app_id(crate::APP_ID)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_active(true)
            .with_resizable(false)
            .with_inner_size(size),
        run_and_return: true,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Windup",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(NoticeWindow {
                message,
                action,
                palette,
                mode,
                commands,
                focused_once: false,
                later: &mut later,
            }))
        }),
    );
    if let Err(e) = result {
        log::error!("the notice window failed: {e}");
    }
    log::info!("notice window: closed");
    later
}

/// The card's size for `message`, from a context of its own: the window's
/// does not exist before the window does.
fn notice_size(message: &str, palette: &Palette) -> egui::Vec2 {
    let ctx = egui::Context::default();
    install_fonts(&ctx);
    // Fonts are ready after one pass.
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    notice::size(&ctx, message, palette)
}

struct NoticeWindow<'a> {
    message: &'a str,
    action: Option<&'a std::path::Path>,
    palette: Palette,
    mode: Mode,
    commands: &'a Receiver<Command>,
    focused_once: bool,
    later: &'a mut Vec<Command>,
}

impl eframe::App for NoticeWindow<'_> {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        apply_theme(ctx, self.palette, self.mode);
        self.later.extend(self.commands.try_iter());
        let mut close = false;
        let label = self.action.map(|_| crate::video::outcome::OPEN_FOLDER);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
            .show(ctx, |ui| {
                match notice::show(ui, &self.palette, self.message, label) {
                    Some(notice::NoticeReply::Action) => {
                        if let Some(file) = self.action {
                            crate::video::outcome::open_folder(file);
                        }
                        close = true;
                    }
                    Some(notice::NoticeReply::Dismissed) => close = true,
                    None => {}
                }
            });
        // Focus moving elsewhere puts it away, as it does the popup.
        match ctx.input(|i| i.viewport().focused) {
            Some(true) => self.focused_once = true,
            Some(false) if self.focused_once => close = true,
            _ => {}
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
