//! Being the installer: what `windup-setup-<version>-<arch>.exe` does when it
//! is double-clicked.
//!
//! The setup file is this same binary with the MSI on its end
//! (`windup_core::install::payload`). `main` sees the payload before anything
//! else and hands over to [`run`], which lifts the MSI out to the temporary
//! directory, opens Windup's own window, and on Install runs msiexec silently
//! and elevated, then starts the installed Windup and closes.
//!
//! Only Windows ships a setup executable. The non-Windows twins below exist so
//! the crate compiles everywhere; they report that this route is Windows-only.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;
use windup_core::install::{self, Command, Step, installed_path, payload};
use windup_ui::setup::{SetupAction, WINDOW};
use windup_ui::{Mode, Palette, apply_theme, install_fonts, metrics};

/// How often the window looks for news from the worker while it works.
const TICK: Duration = Duration::from_millis(125);

/// The accent setup draws in. It runs before any config exists, so it uses
/// the default rather than the user's.
const ACCENT_HUE: f32 = 28.0;

pub fn run() -> eframe::Result<()> {
    let (step, package, version) = match unpack_payload() {
        Ok((package, version)) => (Step::Ready, Some(package), version),
        Err(why) => (Step::Failed(why), None, String::new()),
    };
    let palette = Palette::of(Mode::Dark, ACCENT_HUE);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Install Windup")
            .with_icon(windup_ui::art::window_icon(&palette))
            .with_inner_size(WINDOW)
            .with_resizable(false)
            .with_maximize_button(false),
        ..Default::default()
    };
    eframe::run_native(
        "Windup setup",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx, palette, Mode::Dark);
            Ok(Box::new(Setup {
                step,
                package,
                version,
                palette,
                news: None,
                log: None,
            }))
        }),
    )
}

/// Lift the MSI off the end of this executable into the temporary directory,
/// named after this file, and read the version out of that name.
fn unpack_payload() -> Result<(PathBuf, String), String> {
    let unreadable = |e: std::io::Error| format!("Windup could not read its own setup file: {e}");
    let exe = std::env::current_exe().map_err(unreadable)?;
    let bytes = std::fs::read(&exe).map_err(unreadable)?;
    let package = payload::read(&bytes).ok_or_else(|| {
        "This setup file carries no package. Download it again from the releases page.".to_string()
    })?;

    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "windup-setup".to_string());
    let to = std::env::temp_dir().join(format!("{stem}.msi"));
    std::fs::write(&to, package).map_err(|e| {
        format!(
            "Windup could not write the package to {}: {e}",
            to.display()
        )
    })?;
    Ok((to, install::version_from_stem(&stem).unwrap_or_default()))
}

struct Setup {
    step: Step,
    /// The unpacked MSI. `None` when the window opened only to say why there
    /// is nothing to install.
    package: Option<PathBuf>,
    version: String,
    palette: Palette,
    news: Option<Receiver<Step>>,
    log: Option<PathBuf>,
}

impl Setup {
    fn begin(&mut self) {
        let Some(package) = self.package.clone() else {
            return;
        };
        let (tx, rx) = channel();
        self.news = Some(rx);
        self.step = Step::AskingPermission;
        // On a thread, so the window keeps painting for as long as msiexec
        // takes.
        std::thread::spawn(move || {
            let command = Command::for_package(&package);
            let running = tx.clone();
            let result = elevated_msiexec(&command, &move || {
                let _ = running.send(Step::Installing);
            });
            if let Err(why) = result {
                let _ = tx.send(Step::Failed(why));
                return;
            }
            let _ = tx.send(Step::Starting);
            let started = installed_path(std::env::var("ProgramFiles").ok().as_deref())
                .ok_or_else(|| "Program Files is not set".to_string())
                .and_then(|exe| {
                    std::process::Command::new(&exe)
                        .spawn()
                        .map(|_| ())
                        .map_err(|e| format!("{}: {e}", exe.display()))
                });
            match started {
                Ok(()) => {
                    let _ = tx.send(Step::Finished);
                }
                Err(why) => {
                    log::warn!("installed, but could not start Windup: {why}");
                    let _ = tx.send(Step::Installed);
                }
            }
        });
    }

    fn poll(&mut self) {
        let Some(news) = self.news.as_ref() else {
            return;
        };
        loop {
            match news.try_recv() {
                Ok(step) => {
                    if matches!(step, Step::Failed(_)) {
                        self.log = self.package.as_deref().map(|p| Command::for_package(p).log);
                    }
                    self.step = step;
                }
                Err(TryRecvError::Disconnected) => {
                    self.news = None;
                    return;
                }
                Err(TryRecvError::Empty) => return,
            }
        }
    }
}

impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.step.holds_work() {
            ctx.request_repaint_after(TICK);
        }
        if self.step == Step::Finished {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Closing the window while msiexec runs would leave it running with
        // nothing on screen to say so.
        if ctx.input(|i| i.viewport().close_requested()) && self.step.holds_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        let p = self.palette;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.window)
                    .inner_margin(egui::Margin::same(metrics::S5 as i8)),
            )
            .show(ctx, |ui| {
                match windup_ui::setup::show(ui, &p, &self.step, &self.version, self.log.as_deref())
                {
                    Some(SetupAction::Install) => self.begin(),
                    Some(SetupAction::Close) => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    None => {}
                }
            });
    }
}

/// Run msiexec elevated and wait for it.
///
/// `ShellExecuteExW` with the `runas` verb, because a per-machine MSI needs
/// administrator rights and `/qn` has no UI in which a plain spawn could say
/// so. This is the one UAC prompt the install shows. `started` is called once
/// consent has been given and msiexec is running.
#[cfg(target_os = "windows")]
fn elevated_msiexec(command: &Command, started: &dyn Fn()) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = wide("runas".as_ref());
    let file = wide(&command.program);
    let parameters = wide(command.parameters().as_ref());

    // SAFETY: SHELLEXECUTEINFOW is plain data; zeroed leaves every unused
    // pointer null.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // NOCLOSEPROCESS hands back a handle to wait on; without it there is no
    // way to know whether the install worked.
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_HIDE;

    // SAFETY: every pointer in `info` outlives the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // Almost always the consent prompt being declined.
        return Err(
            "Windows did not allow the installation to start. It may have been declined at the permission prompt."
                .to_string(),
        );
    }
    if info.hProcess.is_null() {
        return Err("Windows did not report the installer's progress.".to_string());
    }
    started();

    // SAFETY: hProcess is non-null and owned here until closed.
    let code = unsafe {
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 || read == 0 {
            return Err("Windup could not tell whether the installation finished.".to_string());
        }
        code
    };
    install::outcome(code)
}

#[cfg(not(target_os = "windows"))]
fn elevated_msiexec(_command: &Command, _started: &dyn Fn()) -> Result<(), String> {
    Err("The setup file only installs on Windows. Use your system's package instead.".to_string())
}
