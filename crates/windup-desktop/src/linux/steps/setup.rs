//! The one-time setup that lets Windup read clicks on Wayland where no udev
//! rule does (the AppImage): the user joins the `input` group. That group
//! can read the keyboard too, which the settings say before offering it,
//! and it only counts from the next login.

use std::process::Command;

/// The group whose members may read every input device.
pub const INPUT_GROUP: &str = "input";

/// Where distributions keep usermod: sbin on Debian and older Fedora, bin
/// where sbin was merged into it (Arch, newer Fedora).
const USERMOD_PATHS: [&str; 3] = ["/usr/sbin/usermod", "/usr/bin/usermod", "/sbin/usermod"];

/// The name of the user with `uid`, from the text of `/etc/passwd`.
pub fn user_name(etc_passwd: &str, uid: u32) -> Option<String> {
    etc_passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        (fields.nth(1)?.parse() == Ok(uid)).then(|| name.to_owned())
    })
}

/// The command that adds `user` to the input group. A name that could reach
/// another field of `/etc/group` or a path, or read as an option, is refused.
pub fn usermod_args(user: &str) -> Result<[String; 4], String> {
    let bad = user.is_empty()
        || user.starts_with('-')
        || user
            .chars()
            .any(|c| matches!(c, ':' | '/' | ',') || c.is_whitespace() || c.is_control());
    if bad {
        return Err(format!("\"{user}\" cannot be added to a group."));
    }
    Ok(["usermod", "-aG", INPUT_GROUP, user].map(String::from))
}

/// Adds this user to the input group through pkexec, which asks for the
/// password. `Err("Cancelled.")` when the user said no.
pub fn join_input_group() -> Result<(), String> {
    let uid = rustix::process::getuid().as_raw();
    let user = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| user_name(&p, uid))
        .ok_or("Your user name was not found.")?;
    let [usermod, args @ ..] = usermod_args(&user)?;
    // By path: /usr/sbin is not on an ordinary user's PATH on Debian, and
    // pkexec looks the program up in the caller's.
    let usermod = USERMOD_PATHS
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map_or(usermod, |p| (*p).to_owned());
    let status = Command::new("pkexec")
        .arg(usermod)
        .args(&args)
        .status()
        .map_err(|e| format!("pkexec could not be started: {e}"))?;
    match status.code() {
        Some(0) => Ok(()),
        // pkexec: the dialog was dismissed, or the password refused.
        Some(126 | 127) => Err("Cancelled.".into()),
        Some(code) => Err(format!("usermod failed (exit code {code}).")),
        None => Err("usermod was stopped.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usermod_adds_the_user_to_input() {
        assert_eq!(
            usermod_args("anna").unwrap(),
            ["usermod", "-aG", "input", "anna"]
        );
        for bad in ["an:na", "../anna", "anna/x", "", "-anna", "an na"] {
            assert!(usermod_args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_user_name_comes_from_etc_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      anna:x:1000:1000:Anna:/home/anna:/bin/bash\n\
                      bo:x:10000:10000::/home/bo:/bin/sh\n";
        assert_eq!(user_name(passwd, 1000).as_deref(), Some("anna"));
        assert_eq!(user_name(passwd, 0).as_deref(), Some("root"));
        // 1000 is not a prefix match for 10000.
        assert_eq!(user_name(passwd, 10000).as_deref(), Some("bo"));
        assert_eq!(user_name(passwd, 1001), None);
        assert_eq!(user_name("", 1000), None);
    }
}
