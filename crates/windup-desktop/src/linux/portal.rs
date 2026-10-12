//! The XDG desktop portals, for desktops whose compositor keeps the
//! protocols Windup would otherwise use to itself: GlobalShortcuts to bind
//! Super+V without a trip to the desktop's settings (KDE, GNOME 48+,
//! Hyprland), and RemoteDesktop to press Ctrl+V where there is no virtual
//! keyboard (GNOME, KDE).
//!
//! Both ask the user once. The RemoteDesktop grant is kept with a restore
//! token in the config directory, so the question is not asked again at the
//! next login on desktops that support persisting it.

use crate::platform::{HotkeysVia, Plan};
use ashpd::desktop::PersistMode;
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop};
use futures_util::StreamExt;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;
use windup_core::Config;
use windup_core::ipc::Command;

/// Evdev codes, which is what the portal's keycode call takes.
const KEY_LEFTCTRL: i32 = 29;
const KEY_V: i32 = 47;

/// Where paste requests go: a thread that holds the RemoteDesktop session
/// open, because each new session would ask the user again.
static PASTER: Mutex<Option<Sender<()>>> = Mutex::new(None);

/// Asks for Super+V and Super+Shift+V through the GlobalShortcuts portal
/// when nothing else delivers them. A desktop without the portal (COSMIC
/// today) is left to its own shortcut settings and `windup --popup`.
pub fn start_shortcuts(plan: &Plan) {
    if plan.hotkeys != HotkeysVia::Command {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("portal-shortcuts".into())
        .spawn(|| {
            if let Err(e) = pollster::block_on(shortcuts()) {
                log::info!(
                    "no GlobalShortcuts portal ({e}); bind `windup --popup` in the desktop's keyboard settings"
                );
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the shortcuts thread: {e}");
    }
}

async fn shortcuts() -> ashpd::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session().await?;
    let wanted = [
        NewShortcut::new(Command::Popup.as_str(), "Open clipboard history")
            .preferred_trigger("LOGO+v"),
        NewShortcut::new(Command::PastePlain.as_str(), "Paste as plain text")
            .preferred_trigger("LOGO+SHIFT+v"),
    ];
    portal
        .bind_shortcuts(&session, &wanted, None)
        .await?
        .response()?;
    log::info!("Super+V bound through the GlobalShortcuts portal");
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
