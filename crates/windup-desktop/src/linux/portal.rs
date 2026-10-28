//! The XDG desktop portals, for desktops whose compositor keeps the
//! protocols Windup would otherwise use to itself: GlobalShortcuts to bind
//! the tools' keys without a trip to the desktop's settings (KDE, GNOME 48+,
//! Hyprland), and RemoteDesktop to press Ctrl+V where there is no virtual
//! keyboard (GNOME, KDE).
//!
//! Both ask the user once. The RemoteDesktop grant is kept with a restore
//! token in the config directory, so the question is not asked again at the
//! next login on desktops that support persisting it.

use crate::capture::split::split_desktop;
use crate::capture::{CaptureError, dbus_absent};
use crate::linux::wayland::screencopy::output_layout_from_env;
use crate::platform::{HotkeysVia, Plan};
use ashpd::desktop::PersistMode;
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop};
use futures_util::StreamExt;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
use windup_core::Config;
use windup_core::Shortcut;
use windup_core::capture::geometry::DesktopShot;
use windup_core::hotkeys::{Action, bindings, portal_trigger};
use windup_core::ipc::Command;

/// Evdev codes, which is what the portal's keycode call takes.
const KEY_LEFTCTRL: i32 = 29;
const KEY_V: i32 = 47;

/// Where paste requests go: a thread that holds the RemoteDesktop session
/// open, because each new session would ask the user again.
static PASTER: Mutex<Option<Sender<()>>> = Mutex::new(None);

/// Asks for the config's shortcuts through the GlobalShortcuts portal when
/// nothing else delivers them. A desktop without the portal is left to its
/// own shortcut settings and `windup --popup`, except COSMIC, where
/// `cosmic::bind_once` writes them into those settings.
pub fn start_shortcuts(plan: &Plan, config: &Config) {
    if plan.hotkeys != HotkeysVia::Command {
        return;
    }
    let bindings = bindings(config);
    let spawned = std::thread::Builder::new()
        .name("portal-shortcuts".into())
        .spawn(move || {
            if let Err(e) = pollster::block_on(shortcuts(&bindings)) {
                log::info!(
                    "no GlobalShortcuts portal ({e}); bind `windup --popup` in the desktop's keyboard settings"
                );
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the shortcuts thread: {e}");
    }
}

async fn shortcuts(bindings: &[(Shortcut, Action)]) -> ashpd::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session().await?;
    let wanted: Vec<NewShortcut> = bindings
        .iter()
        .map(|(shortcut, action)| {
            let new = NewShortcut::new(
                Command::for_action(*action).as_str(),
                action.tool().display_name(),
            );
            match portal_trigger(shortcut) {
                Some(trigger) => new.preferred_trigger(trigger.as_str()),
                None => new,
            }
        })
        .collect();
    portal
        .bind_shortcuts(&session, &wanted, None)
        .await?
        .response()?;
    log::info!("shortcuts bound through the GlobalShortcuts portal");
    let mut activated = portal.receive_activated().await?;
    while let Some(event) = activated.next().await {
        // Back through our own socket, so a portal shortcut is handled
        // exactly like a desktop shortcut running `windup --popup`.
        if let Some(command) = Command::parse(event.shortcut_id())
            && let Err(e) = super::ipc::send(command)
        {
            log::warn!("portal shortcut not delivered: {e}");
        }
    }
    Ok(())
}

/// Presses Ctrl+V through the RemoteDesktop portal.
pub fn paste_ctrl_v() {
    let mut paster = PASTER.lock().unwrap();
    if let Some(tx) = paster.as_ref()
        && tx.send(()).is_ok()
    {
        return;
    }
    let (tx, rx) = channel();
    let _ = tx.send(());
    *paster = Some(tx);
    let spawned = std::thread::Builder::new()
        .name("portal-paste".into())
        .spawn(move || {
            let result = pollster::block_on(async {
                let (portal, session) = remote_session().await?;
                while rx.recv().is_ok() {
                    // Let the keys of the shortcut that asked for this come up.
                    std::thread::sleep(Duration::from_millis(120));
                    for (key, state) in [
                        (KEY_LEFTCTRL, KeyState::Pressed),
                        (KEY_V, KeyState::Pressed),
                        (KEY_V, KeyState::Released),
                        (KEY_LEFTCTRL, KeyState::Released),
                    ] {
                        portal.notify_keyboard_keycode(&session, key, state).await?;
                    }
                }
                ashpd::Result::Ok(())
            });
            if let Err(e) = result {
                log::warn!("RemoteDesktop portal unavailable or refused ({e}); the entry is on the clipboard, paste it with Ctrl+V");
            }
            *PASTER.lock().unwrap() = None;
        });
    if let Err(e) = spawned {
        log::warn!("could not start the paste thread: {e}");
    }
}

async fn remote_session<'a>() -> ashpd::Result<(
    RemoteDesktop<'a>,
    ashpd::desktop::Session<'a, RemoteDesktop<'a>>,
)> {
    let portal = RemoteDesktop::new().await?;
    let session = portal.create_session().await?;
    let token = token_path().and_then(|p| std::fs::read_to_string(p).ok());
    portal
        .select_devices(
            &session,
            DeviceType::Keyboard.into(),
            token.as_deref().map(str::trim),
            PersistMode::ExplicitlyRevoked,
        )
        .await?
        .response()?;
    let started = portal.start(&session, None).await?.response()?;
    if let (Some(path), Some(token)) = (token_path(), started.restore_token()) {
        let _ = std::fs::write(path, token);
    }
    Ok((portal, session))
}

fn token_path() -> Option<std::path::PathBuf> {
    Config::config_dir().map(|d| d.join("remote-desktop-token"))
}

/// A still of the whole desktop through the Screenshot portal (GNOME, and
/// anywhere else with a portal and no better route). The first time, the
/// desktop asks the user whether Windup may take screenshots and remembers the
/// answer; a refusal comes back as `Denied`. The portal returns one image of
/// the whole desktop, which is cut up by the compositor's output layout.
///
/// Blocks until the portal answers, which includes the user answering that
/// first-time dialog: call it from a thread of its own, never the hotkey or
/// event thread.
pub fn screenshot() -> Result<DesktopShot, CaptureError> {
    use ashpd::desktop::screenshot::Screenshot;
    let layout = output_layout_from_env()?;
    let response = pollster::block_on(async {
        Screenshot::request()
            .interactive(false)
            .modal(false)
            .send()
            .await?
            .response()
    });
    let shot = match response {
        Ok(shot) => shot,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            return Err(CaptureError::Denied);
        }
        Err(ashpd::Error::Portal(ashpd::PortalError::NotAllowed(_))) => {
            return Err(CaptureError::Denied);
        }
        Err(e) if absent(&e) => {
            log::debug!("no screenshot portal: {e}");
            return Err(CaptureError::Unsupported);
        }
        Err(e) => return Err(CaptureError::Failed(e.to_string())),
    };
    let path = shot
        .uri()
        .to_file_path()
        .map_err(|()| CaptureError::Failed(format!("not a file: {}", shot.uri())))?;
    let img = image::open(&path);
    // GNOME leaves the file in ~/Pictures; this one is ours to remove.
    let _ = std::fs::remove_file(&path);
    let img = img.map_err(|e| CaptureError::Failed(e.to_string()))?;
    Ok(split_desktop(img.to_rgba8(), &layout))
}

/// Whether the error means there is no portal to ask: no session bus, no
/// portal service, or a portal without the interface.
fn absent(e: &ashpd::Error) -> bool {
    match e {
        ashpd::Error::PortalNotFound(_) => true,
        ashpd::Error::Zbus(z) | ashpd::Error::Portal(ashpd::PortalError::ZBus(z)) => zbus_absent(z),
        _ => false,
    }
}

fn zbus_absent(e: &zbus::Error) -> bool {
    use zbus::fdo::Error as Fdo;
    match e {
        zbus::Error::Address(_)
        | zbus::Error::InputOutput(_)
        | zbus::Error::Handshake(_)
        | zbus::Error::Connection(..) => true,
        zbus::Error::MethodError(name, ..) => dbus_absent(name.as_str()),
        zbus::Error::FDO(f) => matches!(
            **f,
            Fdo::ServiceUnknown(_)
                | Fdo::NameHasNoOwner(_)
                | Fdo::UnknownMethod(_)
                | Fdo::UnknownInterface(_)
                | Fdo::UnknownObject(_)
        ),
        _ => false,
    }
}

/// Starts reading whether the desktop asks for less motion: GNOME's
/// `org.gnome.desktop.interface enable-animations`, through the Settings
/// portal (KDE's and others' portals may not carry it). The answer arrives
/// on the receiver; none arrives when there is no portal or no such key, and
/// the caller then takes "no".
pub fn ask_reduced_motion() -> Receiver<bool> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("portal-motion".into())
        .spawn(move || {
            let read = pollster::block_on(async {
                use ashpd::desktop::settings::Settings;
                Settings::new()
                    .await?
                    .read::<bool>("org.gnome.desktop.interface", "enable-animations")
                    .await
            });
            match read {
                Ok(animations) => {
                    let _ = tx.send(!animations);
                }
                Err(e) => log::debug!("no animation setting from the portal: {e}"),
            }
        });
    if let Err(e) = spawned {
        log::debug!("could not start the motion thread: {e}");
    }
    rx
}
