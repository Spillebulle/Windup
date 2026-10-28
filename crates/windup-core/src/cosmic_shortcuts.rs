//! Windup's shortcuts in COSMIC's own list of custom shortcuts.
//!
//! COSMIC has no GlobalShortcuts portal, so an application cannot ask for a
//! key. What it has is a file the compositor watches,
//! `~/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom`: a RON
//! map from a key binding to an action, the same file COSMIC Settings writes
//! when a custom shortcut is added by hand. Adding an entry there binds the
//! key at once, and it shows in Settings where it can be changed or removed.
//!
//! The file belongs to the user, so this never rewrites it: it reads the
//! entries, and appends Windup's before the closing brace, only for a key
//! nothing is bound to and a command nothing runs yet. A file it cannot read
//! is left alone.
//!
//! The format is `cosmic-settings-config`'s `Shortcuts` (MPL-2.0; nothing of
//! it is copied here): `{ (modifiers: [Super, Shift], key: "v",
//! description: Some("…")): Spawn("command"), … }`.

use crate::config::Config;
use crate::hotkeys::{bindings, xkb_keysym_name};
use crate::ipc::Command;

/// A shortcut Windup wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// COSMIC's modifier names: `Super`, `Ctrl`, `Alt`, `Shift`.
    pub modifiers: Vec<String>,
    /// An xkb keysym name.
    pub key: String,
    /// The name COSMIC Settings shows for it.
    pub description: String,
    /// The arguments after the program, e.g. `--popup`.
    pub args: String,
}

/// The shortcuts the config binds, one per enabled tool.
pub fn wanted(config: &Config) -> Vec<Wanted> {
    bindings(config)
        .into_iter()
        .filter_map(|(shortcut, action)| {
            let m = &shortcut.modifiers;
            let modifiers = [
                (m.super_key, "Super"),
                (m.ctrl, "Ctrl"),
                (m.alt, "Alt"),
                (m.shift, "Shift"),
            ]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| name.to_owned())
            .collect();
            Some(Wanted {
                modifiers,
                key: xkb_keysym_name(&shortcut.key)?.to_owned(),
                description: action.tool().display_name().to_owned(),
                args: format!("--{}", Command::for_action(action).as_str()),
            })
        })
        .collect()
}

const MARKER_HEADER: &str =
    "Windup offered these shortcuts to COSMIC, once each. Delete this file to have it check again.";
const MARKER_LINE: &str = "offered ";

/// The arguments Windup has already offered to COSMIC, from the marker file's
/// text (`None` when there is no file). A marker from before the capture tools
/// lists nothing: it stands for the two shortcuts that existed then.
pub fn offered(marker: Option<&str>) -> Vec<String> {
    let Some(text) = marker else {
        return Vec::new();
    };
    let listed: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix(MARKER_LINE))
        .map(|a| a.trim().to_owned())
        .collect();
    if listed.is_empty() {
        vec!["--popup".into(), "--paste-plain".into()]
    } else {
        listed
    }
}

/// The wanted shortcuts not offered yet.
pub fn pending(wanted: Vec<Wanted>, offered: &[String]) -> Vec<Wanted> {
    wanted
        .into_iter()
        .filter(|w| !offered.contains(&w.args))
        .collect()
}

/// The marker file's text once `offered` have been offered.
pub fn marker_text(offered: &[String]) -> String {
    let mut out = format!(
        "{MARKER_HEADER}
"
    );
    for a in offered {
        out.push_str(&format!(
            "{MARKER_LINE}{a}
"
        ));
    }
    out
}

/// What happened to one wanted shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Added,
    /// Something already runs Windup with these arguments, on whatever key.
    AlreadyBound,
    /// The key does something else; the user's choice stands.
    KeyTaken,
}

/// Why the file was left alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable(pub String);

/// One binding in the file, as far as this module needs it.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    modifiers: Vec<String>,
    key: Option<String>,
    action: String,
}

/// The new file, if anything was added, and what happened to each of
/// `wanted`. `existing` is the file's text, `None` when there is none.
/// `program` is how the command names Windup: `windup`, or an AppImage's path.
pub fn merge(
    existing: Option<&str>,
    wanted: &[Wanted],
    program: &str,
) -> Result<(Option<String>, Vec<Outcome>), Unreadable> {
    let text = existing.unwrap_or("");
    let (entries, close) = if text.trim().is_empty() {
        (Vec::new(), None)
    } else {
        let (entries, close) = parse(text)?;
        (entries, Some(close))
    };

    let mut added = Vec::new();
    let mut outcomes = Vec::new();
    for w in wanted {
        let outcome = if entries.iter().any(|e| runs_windup(&e.action, &w.args)) {
            Outcome::AlreadyBound
        } else if entries.iter().any(|e| same_keys(e, w)) {
            Outcome::KeyTaken
        } else {
            added.push(render(w, program));
            Outcome::Added
        };
        outcomes.push(outcome);
    }
    if added.is_empty() {
        return Ok((None, outcomes));
    }

    let body: String = added.iter().map(|e| format!("    {e},\n")).collect();
    let new = match close {
        None => format!("{{\n{body}}}\n"),
        Some(close) => {
            // Before the closing brace, after a comma the last entry may lack.
            let before = text[..close].trim_end();
            let comma = if before.ends_with('{') || before.ends_with(',') {
                ""
            } else {
                ","
            };
            format!("{before}{comma}\n{body}{}", &text[close..])
        }
    };
    Ok((Some(new), outcomes))
}

/// The command for `args`, quoted for the shell COSMIC runs it with.
pub fn command(program: &str, args: &str) -> String {
    let plain = program
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        format!("{program} {args}")
    } else {
        format!("'{}' {args}", program.replace('\'', r"'\''"))
    }
}

fn render(w: &Wanted, program: &str) -> String {
    format!(
        "(modifiers: [{}], key: {}, description: Some({})): Spawn({})",
        w.modifiers.join(", "),
        ron_string(&w.key),
        ron_string(&w.description),
        ron_string(&command(program, &w.args)),
    )
}

fn ron_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A `Spawn` of Windup with `args`, by any path.
fn runs_windup(action: &str, args: &str) -> bool {
    let action = action.trim();
    let Some(inner) = action
        .strip_prefix("Spawn")
        .map(str::trim_start)
        .and_then(|a| a.strip_prefix('('))
    else {
        return false;
    };
    let lower = inner.to_ascii_lowercase();
    lower.contains("windup")
        && inner
            .split_whitespace()
            .any(|w| w.trim_matches(['"', ')']) == args)
}

fn same_keys(e: &Entry, w: &Wanted) -> bool {
    let mut a: Vec<String> = e.modifiers.iter().map(|m| m.to_ascii_lowercase()).collect();
    let mut b: Vec<String> = w.modifiers.iter().map(|m| m.to_ascii_lowercase()).collect();
    a.sort();
    b.sort();
    a == b
        && e.key
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case(&w.key))
}

// --- a reader for the little of RON this file uses ---------------------------

struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Reader<'a> {
    fn err<T>(&self, what: &str) -> Result<T, Unreadable> {
        Err(Unreadable(format!("{what} at byte {}", self.at)))
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    /// Skips whitespace and `//` and `/* */` comments.
    fn skip(&mut self) {
        loop {
            let rest = &self.text[self.at..];
            let trimmed = rest.trim_start();
            self.at += rest.len() - trimmed.len();
            if trimmed.starts_with("//") {
                self.at += trimmed.find('\n').unwrap_or(trimmed.len());
            } else if trimmed.starts_with("/*") {
                self.at += trimmed.find("*/").map_or(trimmed.len(), |i| i + 2);
            } else {
                return;
            }
        }
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip();
        if self.peek() == Some(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), Unreadable> {
        if self.eat(c) {
            Ok(())
        } else {
            self.err(&format!("expected '{c}'"))
        }
    }

    fn ident(&mut self) -> Result<String, Unreadable> {
        self.skip();
        let rest = &self.text[self.at..];
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if len == 0 {
            return self.err("expected a name");
        }
        self.at += len;
        Ok(rest[..len].to_string())
    }

    fn string(&mut self) -> Result<String, Unreadable> {
        self.skip();
        if self.peek() != Some('"') {
            return self.err("expected a string");
        }
        self.at += 1;
        let mut out = String::new();
        let mut chars = self.text[self.at..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.at += i + 1;
                    return Ok(out);
                }
                '\\' => {
                    if let Some((_, e)) = chars.next() {
                        out.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            e => e,
                        });
                    }
                }
                c => out.push(c),
            }
        }
        self.err("unterminated string")
    }

    /// Any value, skipped over; its text is returned.
    fn value(&mut self) -> Result<&'a str, Unreadable> {
        self.skip();
        let start = self.at;
        let mut depth = 0usize;
        loop {
            match self.peek() {
                None => return self.err("unexpected end"),
                Some('"') => {
                    self.string()?;
                    continue;
                }
                Some('(' | '[' | '{') => depth += 1,
                Some(')' | ']' | '}') => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Some(',') if depth == 0 => break,
                _ => {}
            }
            self.at += self.peek().map_or(1, char::len_utf8);
            // A group closing at depth 0 ends the value: `Spawn("..")` is a
            // name and one group, `[..]` a group alone.
            if depth == 0 && matches!(self.text[..self.at].chars().last(), Some(')' | ']' | '}')) {
                break;
            }
        }
        Ok(self.text[start..self.at].trim())
    }

    /// `(modifiers: [..], key: "..", ...)`
    fn binding(&mut self) -> Result<(Vec<String>, Option<String>), Unreadable> {
        self.expect('(')?;
        let mut modifiers = Vec::new();
        let mut key = None;
        loop {
            if self.eat(')') {
                break;
            }
            let field = self.ident()?;
            self.expect(':')?;
            match field.as_str() {
                "modifiers" => {
                    self.expect('[')?;
                    while !self.eat(']') {
                        modifiers.push(self.ident()?);
                        if !self.eat(',') {
                            self.expect(']')?;
                            break;
                        }
                    }
                }
                "key" => key = Some(self.string()?),
                _ => {
                    self.value()?;
                }
            }
            if !self.eat(',') {
                self.expect(')')?;
                break;
            }
        }
        Ok((modifiers, key))
    }
}

/// The entries, and where the map's closing brace is.
fn parse(text: &str) -> Result<(Vec<Entry>, usize), Unreadable> {
    let mut r = Reader { text, at: 0 };
    r.expect('{')?;
    let mut entries = Vec::new();
    loop {
        r.skip();
        if r.peek() == Some('}') {
            break;
        }
        let (modifiers, key) = r.binding()?;
        r.expect(':')?;
        let action = r.value()?.to_string();
        entries.push(Entry {
            modifiers,
            key,
            action,
        });
        if !r.eat(',') {
            r.skip();
            if r.peek() != Some('}') {
                return r.err("expected ',' or '}'");
            }
        }
    }
    let close = r.at;
    r.at += 1;
    r.skip();
    if r.at != text.len() {
        return r.err("text after the map");
    }
    Ok((entries, close))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// Clipboard history and paste plain text, the shortcuts of 0.1.x.
    fn two() -> Vec<Wanted> {
        wanted(&Config::default())[..2].to_vec()
    }

    #[test]
    fn wanted_follows_the_config() {
        let w = wanted(&Config::default());
        assert!(
            w.iter().any(|w| w.args == "--capture"
                && w.key == "s"
                && w.modifiers == ["Super", "Shift"])
        );
        assert!(
            w.iter()
                .any(|w| w.args == "--screenshot" && w.key == "Print" && w.modifiers.is_empty())
        );
        let mut c = Config::default();
        c.screenshot.enabled = false;
        assert!(wanted(&c).iter().all(|w| w.args != "--screenshot"));
    }

    #[test]
    fn no_marker_offers_everything() {
        assert!(offered(None).is_empty());
        assert_eq!(pending(wanted(&Config::default()), &offered(None)).len(), 4);
    }

    #[test]
    fn an_old_marker_means_the_first_two_were_offered() {
        let old =
            "Windup added its shortcuts to COSMIC's once. Delete this file to have it check again.
";
        let left = pending(wanted(&Config::default()), &offered(Some(old)));
        let args: Vec<&str> = left.iter().map(|w| w.args.as_str()).collect();
        assert_eq!(args, ["--capture", "--screenshot"]);
    }

    #[test]
    fn the_marker_round_trips() {
        let args: Vec<String> = ["--popup", "--capture"].map(String::from).to_vec();
        assert_eq!(offered(Some(&marker_text(&args))), args);
        let all: Vec<String> = wanted(&Config::default())
            .into_iter()
            .map(|w| w.args)
            .collect();
        assert!(pending(wanted(&Config::default()), &all).is_empty());
    }

    fn added(o: &[Outcome]) -> usize {
        o.iter().filter(|o| **o == Outcome::Added).count()
    }

    #[test]
    fn a_missing_file_gets_both() {
        let (new, outcomes) = merge(None, &two(), "windup").unwrap();
        let new = new.unwrap();
        assert_eq!(added(&outcomes), 2);
        assert_eq!(
            new,
            "{\n    (modifiers: [Super], key: \"v\", description: Some(\"Clipboard history\")): Spawn(\"windup --popup\"),\n    (modifiers: [Super, Shift], key: \"v\", description: Some(\"Paste plain text\")): Spawn(\"windup --paste-plain\"),\n}\n"
        );
        // And what was written reads back.
        let (entries, _) = parse(&new).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].modifiers, ["Super", "Shift"]);
    }

    #[test]
    fn an_empty_map_gets_both() {
        let (new, outcomes) = merge(Some("{}"), &two(), "windup").unwrap();
        assert_eq!(added(&outcomes), 2);
        assert!(
            new.unwrap()
                .starts_with("{\n    (modifiers: [Super], key: \"v\"")
        );
    }

    /// As COSMIC Settings writes it, the user's own shortcut kept.
    const SETTINGS: &str = r#"{
    (
        modifiers: [
            Super,
        ],
        key: "t",
        description: Some("Terminal"),
    ): Spawn("cosmic-term"),
}"#;

    #[test]
    fn the_users_shortcuts_are_kept_and_ours_appended() {
        let (new, outcomes) = merge(Some(SETTINGS), &two(), "windup").unwrap();
        let new = new.unwrap();
        assert_eq!(added(&outcomes), 2);
        assert!(new.starts_with(&SETTINGS[..SETTINGS.len() - 1]));
        let (entries, _) = parse(&new).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].action, "Spawn(\"cosmic-term\")");
        assert_eq!(entries[0].key.as_deref(), Some("t"));
    }

    #[test]
    fn a_last_entry_without_a_comma_gets_one() {
        let text = "{\n    (modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\")\n}";
        let (new, _) = merge(Some(text), &two(), "windup").unwrap();
        let new = new.unwrap();
        assert!(new.contains("Spawn(\"cosmic-term\"),\n    (modifiers: [Super], key: \"v\""));
        assert_eq!(parse(&new).unwrap().0.len(), 3);
    }

    #[test]
    fn a_key_the_user_bound_is_not_taken() {
        let text = "{ (modifiers: [Super], key: \"V\"): Spawn(\"cliphist-menu\") }";
        let (new, outcomes) = merge(Some(text), &two(), "windup").unwrap();
        assert_eq!(outcomes, [Outcome::KeyTaken, Outcome::Added]);
        assert!(new.unwrap().contains("--paste-plain"));
    }

    #[test]
    fn modifier_order_does_not_matter() {
        let text = "{ (modifiers: [Shift, Super], key: \"v\"): Disable }";
        let (_, outcomes) = merge(Some(text), &two(), "windup").unwrap();
        assert_eq!(outcomes, [Outcome::Added, Outcome::KeyTaken]);
    }

    #[test]
    fn windup_already_bound_elsewhere_is_left_alone() {
        let text = r#"{
            (modifiers: [Ctrl, Alt], key: "v"): Spawn("/opt/Windup.AppImage --popup"),
            (modifiers: [Super, Shift], key: "v"): Spawn("windup --paste-plain"),
        }"#;
        let (new, outcomes) = merge(Some(text), &two(), "windup").unwrap();
        assert_eq!(outcomes, [Outcome::AlreadyBound, Outcome::AlreadyBound]);
        assert_eq!(new, None);
    }

    #[test]
    fn comments_and_other_fields_are_read_past() {
        let text = r#"// mine
{
    /* keycode too */
    (modifiers: [Super], keycode: 47, key: "semicolon"): System(Launcher), // launcher
    (modifiers: [], key: "Print"): System(Screenshot),
}
"#;
        let (new, outcomes) = merge(Some(text), &two(), "windup").unwrap();
        assert_eq!(added(&outcomes), 2);
        assert!(new.unwrap().ends_with("}\n"));
    }

    #[test]
    fn a_file_that_does_not_read_is_left_alone() {
        for text in [
            "{ (modifiers: [Super], key: \"v\"): Spawn(\"x\")",
            "[1, 2]",
            "{ oops }",
        ] {
            assert!(merge(Some(text), &two(), "windup").is_err(), "{text}");
        }
    }

    #[test]
    fn an_appimage_path_with_a_space_is_quoted() {
        assert_eq!(command("windup", "--popup"), "windup --popup");
        assert_eq!(
            command("/home/a b/Windup.AppImage", "--popup"),
            "'/home/a b/Windup.AppImage' --popup"
        );
        let (new, _) = merge(None, &two()[..1], "/home/a b/it's.AppImage").unwrap();
        assert!(
            new.unwrap()
                .contains(r#"Spawn("'/home/a b/it'\\''s.AppImage' --popup")"#)
        );
    }
}
