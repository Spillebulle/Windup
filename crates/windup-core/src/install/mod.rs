//! First-run installation: the parts that can be decided and tested without
//! installing anything.
//!
//! On Windows the release ships `windup-setup-<version>-<arch>.exe`, which is
//! `windup.exe` with the MSI appended ([`payload`]). Started with no arguments
//! it finds that payload and becomes an installer instead of the app: Windup's
//! own window, `msiexec` run silently underneath, Windup started when it is
//! done. The window lives in `windup-ui::setup`, the Win32 calls in
//! `windup-desktop`'s `setup.rs`, and everything that is a pure decision lives
//! here where a test can reach it.
//!
//! Linux and macOS will not use this. They install through the system's own
//! package formats (`.deb`, `.rpm`, `.dmg`), so their release step is a
//! packaging script under `packaging/<os>/` rather than a branch of this
//! module. See `docs/packaging.md`.

pub mod payload;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Asks for the installer explicitly, for testing a setup build by hand.
/// A double-clicked setup file has no arguments; the payload decides.
pub const SETUP_FLAG: &str = "--install";

/// The folder under Program Files the MSI installs into. Pinned against
/// `packaging/windows/windup.wxs` by a test: setup starts Windup from here once
/// the package is in, so a rename there must be a rename here.
pub const INSTALL_FOLDER: &str = "Windup";

/// The executable inside [`INSTALL_FOLDER`].
pub const EXECUTABLE: &str = if cfg!(windows) {
    "windup.exe"
} else {
    "windup"
};

/// Whether this launch is the installer: asked for by flag, or carrying a
/// package on its own end.
pub fn is_setup<I: IntoIterator<Item = String>>(args: I, carries_payload: bool) -> bool {
    carries_payload || args.into_iter().skip(1).any(|a| a == SETUP_FLAG)
}

/// The version in a setup file's name: `windup-setup-0.1.0-x64` gives `0.1.0`.
/// Only ever displayed, so a name that is not shaped like that gives nothing.
pub fn version_from_stem(stem: &str) -> Option<String> {
    stem.split('-')
        .find(|part| {
            part.split('.').count() == 3
                && part.split('.').all(|n| !n.is_empty())
                && part.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(str::to_string)
}

/// Where the MSI puts `windup.exe`, given Program Files. Injected rather than
/// read from the environment so it can be tested off Windows.
pub fn installed_path(program_files: Option<&str>) -> Option<PathBuf> {
    let root = program_files.filter(|r| !r.is_empty())?;
    Some(PathBuf::from(root).join(INSTALL_FOLDER).join(EXECUTABLE))
}

/// The `msiexec` invocation for a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Where msiexec writes its verbose log, so a failure has something to show.
    pub log: PathBuf,
}

impl Command {
    /// Install `package` with no Windows Installer UI (`/qn`), never rebooting
    /// on its own (`/norestart`), logging verbosely beside the package.
    /// The MSI keeps one `UpgradeCode`, so `/i` over an older Windup replaces it.
    pub fn for_package(package: &Path) -> Self {
        let log = package.with_extension("log");
        Self {
            program: OsString::from("msiexec"),
            args: vec![
                OsString::from("/i"),
                package.as_os_str().to_owned(),
                OsString::from("/qn"),
                OsString::from("/norestart"),
                OsString::from("/l*v"),
                log.as_os_str().to_owned(),
            ],
            log,
        }
    }

    /// The arguments as one line, for `ShellExecuteExW`.
    ///
    /// Paths are quoted and switches never are. A path under
    /// `C:\Users\Some One\` needs its quotes, and msiexec parses its own
    /// command line and does not recognise a quoted switch: Muster shipped
    /// `"/i"` once, and msiexec showed its usage dialog on the elevated desktop
    /// where nobody could see it, and waited for ever.
    pub fn parameters(&self) -> String {
        self.args
            .iter()
            .map(|a| {
                let text = a.to_string_lossy();
                if text.starts_with('/') {
                    text.into_owned()
                } else {
                    format!("\"{text}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How far setup has got, for the window to draw.
///
/// A silent msiexec reports no progress, so this names the step and never
/// invents a percentage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Waiting for the Install button. Setup was double-clicked; nothing goes
    /// onto the machine until the person says so.
    Ready,
    /// The UAC consent prompt is up, or about to be.
    AskingPermission,
    /// msiexec is running.
    Installing,
    /// Installed, and Windup is being started.
    Starting,
    /// Installed and started; the window closes itself.
    Finished,
    /// Installed, but Windup could not be started. A success: the package went
    /// in, and the Start menu has it.
    Installed,
    /// It did not work. The window says why and names the log.
    Failed(String),
}

impl Step {
    pub fn label(&self) -> String {
        match self {
            Self::Ready => "Windup will be installed for everyone on this PC.".to_string(),
            Self::AskingPermission => "Asking Windows for permission to install...".to_string(),
            Self::Installing => "Installing...".to_string(),
            Self::Starting => "Starting Windup...".to_string(),
            Self::Finished => "Done.".to_string(),
            Self::Installed => "Windup is installed. Start it from the Start menu.".to_string(),
            Self::Failed(why) => why.clone(),
        }
    }

    /// Whether work is under way, so the window may not be dismissed: one that
    /// vanished mid-install would leave msiexec running with nothing to say so.
    pub fn holds_work(&self) -> bool {
        !matches!(
            self,
            Self::Ready | Self::Installed | Self::Finished | Self::Failed(_)
        )
    }

    /// How full the bar is, where that can be said. `None` while msiexec runs,
    /// because it reports nothing; the bar draws an empty track.
    pub fn progress(&self) -> Option<f32> {
        match self {
            Self::Ready => Some(0.0),
            Self::AskingPermission => Some(0.25),
            Self::Installing | Self::Failed(_) => None,
            Self::Starting => Some(0.95),
            Self::Finished | Self::Installed => Some(1.0),
        }
    }
}

/// What a finished msiexec's exit code means.
pub fn outcome(code: u32) -> Result<(), String> {
    match code {
        0 => Ok(()),
        // "A restart is required to finish", which /norestart asks for rather
        // than performing. The files are in place.
        3010 => Ok(()),
        1602 => Err("The installation was cancelled.".to_string()),
        1618 => Err(
            "Another installation is already in progress. Wait for it to finish and try again."
                .to_string(),
        ),
        other => Err(format!(
            "Windows Installer stopped with error {other}. Nothing was changed."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_payload_or_the_flag_makes_setup() {
        assert!(is_setup(args(&["windup-setup-0.1.0-x64.exe"]), true));
        assert!(is_setup(args(&["windup.exe", SETUP_FLAG]), false));
        assert!(!is_setup(args(&["windup.exe"]), false));
        assert!(!is_setup(args(&[]), false));
        // The program name is never read as the flag.
        assert!(!is_setup(args(&[SETUP_FLAG]), false));
    }

    #[test]
    fn the_version_comes_out_of_the_setup_file_name() {
        assert_eq!(
            version_from_stem("windup-setup-0.1.0-x64").as_deref(),
            Some("0.1.0")
        );
        assert_eq!(
            version_from_stem("windup-setup-12.3.45-arm64").as_deref(),
            Some("12.3.45")
        );
        assert_eq!(version_from_stem("windup-setup"), None);
        assert_eq!(version_from_stem("windup"), None);
        assert_eq!(version_from_stem("windup-1..2"), None);
    }

    #[test]
    fn the_install_is_silent_and_never_reboots() {
        let cmd = Command::for_package(Path::new("C:\\Temp\\windup-0.1.0-x64.msi"));
        let flat: Vec<String> = cmd
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for switch in ["/i", "/qn", "/norestart", "/l*v"] {
            assert!(flat.contains(&switch.to_string()), "{switch} in {flat:?}");
        }
        for loud in ["/qb", "/qf", "/qr", "/passive"] {
            assert!(!flat.iter().any(|a| a == loud), "{loud} in {flat:?}");
        }
        assert_eq!(cmd.log, PathBuf::from("C:\\Temp\\windup-0.1.0-x64.log"));
    }

    #[test]
    fn a_path_is_quoted_and_a_switch_is_not() {
        let line = Command::for_package(Path::new("C:\\Users\\Some One\\windup.msi")).parameters();
        assert!(line.starts_with("/i "), "{line}");
        assert!(
            line.contains("\"C:\\Users\\Some One\\windup.msi\""),
            "{line}"
        );
        assert!(
            line.contains("\"C:\\Users\\Some One\\windup.log\""),
            "{line}"
        );
        for switch in ["/i", "/qn", "/norestart", "/l*v"] {
            assert!(
                !line.contains(&format!("\"{switch}\"")),
                "{switch} quoted: {line}"
            );
        }
    }

    #[test]
    fn being_unable_to_start_windup_is_not_a_failed_install() {
        assert!(!Step::Installed.holds_work());
        assert_eq!(Step::Installed.progress(), Some(1.0));
        let said = Step::Installed.label().to_lowercase();
        for word in ["fail", "could not", "error", "sorry", "problem"] {
            assert!(!said.contains(word), "{said:?} reads as a failure");
        }
    }

    #[test]
    fn a_working_step_holds_the_window_and_claims_no_progress_it_cannot_see() {
        for step in [Step::AskingPermission, Step::Installing, Step::Starting] {
            assert!(step.holds_work(), "{step:?}");
        }
        for step in [
            Step::Ready,
            Step::Finished,
            Step::Installed,
            Step::Failed("x".into()),
        ] {
            assert!(!step.holds_work(), "{step:?}");
        }
        assert_eq!(Step::Installing.progress(), None);
    }

    #[test]
    fn exit_codes_that_mean_installed_are_successes() {
        assert!(outcome(0).is_ok());
        assert!(outcome(3010).is_ok());
        assert!(outcome(1602).is_err());
        assert!(outcome(1603).unwrap_err().contains("1603"));
    }

    /// Read as components: a backslash is not a separator off Windows.
    #[test]
    fn the_installed_path_is_program_files_and_the_packages_folder() {
        let path = installed_path(Some("root")).expect("a path");
        let parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(parts, ["root", INSTALL_FOLDER, EXECUTABLE]);
        assert_eq!(installed_path(None), None);
        assert_eq!(installed_path(Some("")), None);
    }

    // --- pins against the packaging file -----------------------------------

    /// Windup's own MSI identity. A `.wxs` copied from a sibling app brings
    /// that app's UpgradeCode, and Windows then treats the two as one product:
    /// Muster 0.0.1 did exactly that with Umber's.
    const UPGRADE_CODE: &str = "f9aca005-ce23-4a9f-bd1b-ce170bf28696";

    fn wxs() -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/windup.wxs"),
        )
        .expect("packaging/windows/windup.wxs")
    }

    #[test]
    fn the_package_carries_windups_own_upgrade_code() {
        assert!(
            wxs().contains(&format!("UpgradeCode=\"{UPGRADE_CODE}\"")),
            "windup.wxs does not carry Windup's UpgradeCode {UPGRADE_CODE}"
        );
    }

    #[test]
    fn no_guid_in_the_package_is_used_twice() {
        let wxs = wxs();
        let mut guids: Vec<&str> = wxs
            .split(['"', '\''])
            .filter(|t| {
                t.len() == 36
                    && t.chars().enumerate().all(|(i, c)| match i {
                        8 | 13 | 18 | 23 => c == '-',
                        _ => c.is_ascii_hexdigit(),
                    })
            })
            .collect();
        let before = guids.len();
        assert!(before >= 3, "expected the package to carry GUIDs");
        guids.sort_unstable();
        guids.dedup();
        assert_eq!(guids.len(), before, "windup.wxs uses one GUID twice");
    }

    /// The two FFmpeg components. A component's GUID is its identity across
    /// upgrades: changing one makes Windows treat the file as a different
    /// component, so it is pinned like the UpgradeCode.
    #[test]
    fn the_ffmpeg_components_keep_their_guids() {
        let wxs = wxs();
        for (id, guid) in [
            ("Ffmpeg", "67424759-2b1e-4242-951c-2573a3aa2da6"),
            ("FfmpegLicence", "67a3c020-8956-4ddb-ae5f-f227bfd93768"),
        ] {
            let wanted = format!("<Component Id=\"{id}\" Guid=\"{guid}\">");
            assert!(wxs.contains(&wanted), "windup.wxs lacks {wanted}");
        }
    }

    #[test]
    fn the_install_folder_is_the_one_the_package_uses() {
        let wanted = format!("<Directory Id=\"INSTALLFOLDER\" Name=\"{INSTALL_FOLDER}\" />");
        assert!(wxs().contains(&wanted), "windup.wxs lacks {wanted}");
    }
}
