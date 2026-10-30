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
use crate::shortcut::Shortcut;

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
            let (modifiers, key) = cosmic_keys(&shortcut)?;
            Some(Wanted {
                modifiers,
                key,
                description: action.tool().display_name().to_owned(),
                args: format!("--{}", Command::for_action(action).as_str()),
            })
        })
        .collect()
}

/// COSMIC's modifier names and the keysym name for `shortcut`.
fn cosmic_keys(shortcut: &Shortcut) -> Option<(Vec<String>, String)> {
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
    Some((modifiers, xkb_keysym_name(&shortcut.key)?.to_owned()))
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
    /// The text of `description: Some("..")`, if it has one.
    description: Option<String>,
    /// Its span in the file: from the end of whatever comes before it (the
    /// map's brace, the comma before, a comment) to the end of its own comma,
    /// or of its action when it has none.
    span: (usize, usize),
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

fn same_mods(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<String> = a.iter().map(|m| m.to_ascii_lowercase()).collect();
    let mut b: Vec<String> = b.iter().map(|m| m.to_ascii_lowercase()).collect();
    a.sort();
    b.sort();
    a == b
}

fn same_keys(e: &Entry, w: &Wanted) -> bool {
    same_mods(&e.modifiers, &w.modifiers)
        && e.key
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case(&w.key))
}

// --- the recording's keys, for as long as it runs ---------------------------
//
// While a recording or a Steps session runs, its pause, stop and cancel keys
// (and Steps' step key) are bound the same way and taken out again when it
// ends. Those entries carry a tag in their
// description, so any a crash left behind are found at the next start and
// removed, and an entry of the user's own is never touched.

/// How a temporary entry's description starts.
pub const TEMPORARY_TAG: &str = "Windup while recording: ";

/// The arguments a temporary entry runs Windup with.
const TEMPORARY_ARGS: [&str; 4] = [
    "--record-pause",
    "--record-stop",
    "--record-cancel",
    "--step",
];

/// The empty map a missing file starts as.
const EMPTY_MAP: &str = "{\n}\n";

fn temporary_label(args: &str) -> &str {
    match args {
        "--record-pause" => "Pause",
        "--record-stop" => "Stop",
        "--record-cancel" => "Cancel",
        "--step" => "Take a step",
        other => other,
    }
}

/// Windup's own temporary entry: tagged, and running one of its recording
/// commands.
fn is_temporary(e: &Entry) -> bool {
    e.description
        .as_deref()
        .is_some_and(|d| d.starts_with(TEMPORARY_TAG))
        && TEMPORARY_ARGS.iter().any(|a| runs_windup(&e.action, a))
}

/// The file with temporary entries for `keys` (a shortcut, and the arguments
/// it runs Windup with, e.g. `--record-cancel`) put first in the map, or
/// `None` when every key is bound already: the user's binding stands, and the
/// Video key is stop already through Windup's own entry. `existing` is the
/// file's text, `None` when there is none. Nothing else in the file changes,
/// so [`remove_temporary`] gives back exactly the text it was given.
pub fn add_temporary(
    existing: Option<&str>,
    keys: &[(Shortcut, &str)],
    program: &str,
) -> Result<Option<String>, Unreadable> {
    let text = existing
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(EMPTY_MAP);
    let (entries, open, _) = parse_map(text)?;
    let mut added: Vec<Wanted> = Vec::new();
    for (shortcut, args) in keys {
        let Some((modifiers, key)) = cosmic_keys(shortcut) else {
            continue;
        };
        let w = Wanted {
            modifiers,
            key,
            description: format!("{TEMPORARY_TAG}{}", temporary_label(args)),
            args: (*args).to_owned(),
        };
        let taken = entries.iter().any(|e| same_keys(e, &w))
            || added.iter().any(|a| {
                a.key.eq_ignore_ascii_case(&w.key) && same_mods(&a.modifiers, &w.modifiers)
            });
        if !taken {
            added.push(w);
        }
    }
    if added.is_empty() {
        return Ok(None);
    }
    // Right after the brace, each with its own comma, so no comma of the
    // user's is added or needed.
    let body: String = added
        .iter()
        .map(|w| format!("\n    {},", render(w, program)))
        .collect();
    Ok(Some(format!("{}{body}{}", &text[..open], &text[open..])))
}

/// The file without Windup's temporary entries, or `None` when it has none.
pub fn remove_temporary(text: &str) -> Result<Option<String>, Unreadable> {
    let (entries, _) = parse(text)?;
    let spans: Vec<(usize, usize)> = entries
        .iter()
        .filter(|e| is_temporary(e))
        .map(|e| e.span)
        .collect();
    if spans.is_empty() {
        return Ok(None);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in spans {
        out.push_str(&text[at..start]);
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(Some(out))
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

    /// `(modifiers: [..], key: "..", description: Some(".."), ...)`
    fn binding(&mut self) -> Result<Binding, Unreadable> {
        self.expect('(')?;
        let mut modifiers = Vec::new();
        let mut key = None;
        let mut description = None;
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
                "description" => description = some_string(self.value()?),
                _ => {
                    self.value()?;
                }
            }
            if !self.eat(',') {
                self.expect(')')?;
                break;
            }
        }
        Ok((modifiers, key, description))
    }
}

/// A binding's modifiers, key and description.
type Binding = (Vec<String>, Option<String>, Option<String>);

/// The string in `Some("..")`; `None` for `None` or anything else.
fn some_string(value: &str) -> Option<String> {
    let mut r = Reader { text: value, at: 0 };
    if r.ident().ok()? != "Some" || !r.eat('(') {
        return None;
    }
    let s = r.string().ok()?;
    r.eat(')').then_some(s)
}

/// The entries, and where the map's closing brace is.
fn parse(text: &str) -> Result<(Vec<Entry>, usize), Unreadable> {
    parse_map(text).map(|(entries, _, close)| (entries, close))
}

/// The entries, where the map's opening brace ends, and where its closing
/// brace is.
fn parse_map(text: &str) -> Result<(Vec<Entry>, usize, usize), Unreadable> {
    let mut r = Reader { text, at: 0 };
    r.expect('{')?;
    let open = r.at;
    let mut entries = Vec::new();
    loop {
        r.skip();
        if r.peek() == Some('}') {
            break;
        }
        // Back over the white space only, so a comment before it stays.
        let start = text[..r.at].trim_end().len();
        let (modifiers, key, description) = r.binding()?;
        r.expect(':')?;
        let action = r.value()?.to_string();
        let mut end = r.at;
        let comma = r.eat(',');
        if comma {
            end = r.at;
        }
        entries.push(Entry {
            modifiers,
            key,
            action,
            description,
            span: (start, end),
        });
        if !comma {
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
    Ok((entries, open, close))
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
            w.iter()
                .any(|w| w.args == "--capture" && w.key == "Print" && w.modifiers.is_empty())
        );
        assert!(
            w.iter().any(|w| w.args == "--screenshot"
                && w.key == "s"
                && w.modifiers == ["Ctrl", "Shift"])
        );
        assert!(w.iter().any(|w| w.args == "--record"
            && w.key == "r"
            && w.modifiers == ["Super", "Shift"]
            && w.description == "Video"));
        let mut c = Config::default();
        c.screenshot.enabled = false;
        assert!(wanted(&c).iter().all(|w| w.args != "--screenshot"));
    }

    #[test]
    fn steps_is_offered_to_cosmic() {
        let w = wanted(&Config::default());
        assert!(w.iter().any(|w| w.args == "--steps"
            && w.key == "g"
            && w.modifiers == ["Super", "Shift"]
            && w.description == "Steps"));
        // The step key is for the session only.
        assert!(w.iter().all(|w| w.args != "--step"));
    }

    #[test]
    fn no_marker_offers_everything() {
        assert!(offered(None).is_empty());
        assert_eq!(pending(wanted(&Config::default()), &offered(None)).len(), 6);
    }

    #[test]
    fn an_old_marker_means_the_first_two_were_offered() {
        let old =
            "Windup added its shortcuts to COSMIC's once. Delete this file to have it check again.
";
        let left = pending(wanted(&Config::default()), &offered(Some(old)));
        let args: Vec<&str> = left.iter().map(|w| w.args.as_str()).collect();
        assert_eq!(args, ["--capture", "--screenshot", "--record", "--steps"]);
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

    fn recording_keys() -> Vec<(Shortcut, &'static str)> {
        let c = Config::default();
        vec![
            (c.video.pause_shortcut.clone(), "--record-pause"),
            (c.video.shortcut.clone(), "--record-stop"),
            (
                Shortcut::new(crate::ShortcutModifiers::NONE, crate::shortcut::ESCAPE),
                "--record-cancel",
            ),
        ]
    }

    #[test]
    fn temporary_entries_come_and_go_leaving_the_file_byte_identical() {
        let windup_bound = merge(Some(SETTINGS), &wanted(&Config::default()), "windup")
            .unwrap()
            .0
            .unwrap();
        for text in [
            SETTINGS,
            "{}",
            "{\n}\n",
            "// mine\n{ (modifiers: [Super], key: \"t\"): Spawn(\"cosmic-term\") }  \n",
            windup_bound.as_str(),
        ] {
            let added = add_temporary(Some(text), &recording_keys(), "windup")
                .unwrap()
                .unwrap();
            assert_ne!(added, text);
            let (entries, _) = parse(&added).unwrap();
            assert!(entries.iter().any(|e| e.key.as_deref() == Some("Escape")
                && e.modifiers.is_empty()
                && runs_windup(&e.action, "--record-cancel")));
            assert!(
                entries
                    .iter()
                    .any(|e| e.key.as_deref() == Some("p")
                        && runs_windup(&e.action, "--record-pause"))
            );
            assert_eq!(
                remove_temporary(&added).unwrap().as_deref(),
                Some(text),
                "{text}"
            );
            // Nothing tagged: nothing to do.
            assert_eq!(remove_temporary(text).unwrap(), None);
        }
    }

    #[test]
    fn steps_temporary_keys_include_step() {
        let c = Config::default();
        let super_shift_x = c.steps.step_shortcut.clone();
        let keys = [
            (super_shift_x, "--step"),
            (c.video.pause_shortcut.clone(), "--record-pause"),
            (
                Shortcut::new(crate::ShortcutModifiers::NONE, crate::shortcut::ESCAPE),
                "--record-cancel",
            ),
        ];
        for base in [SETTINGS, "{\n}\n"] {
            let added = add_temporary(Some(base), &keys, "windup").unwrap().unwrap();
            let (entries, _) = parse(&added).unwrap();
            assert!(
                entries
                    .iter()
                    .any(|e| e.key.as_deref() == Some("x") && runs_windup(&e.action, "--step")),
                "{added}"
            );
            assert_eq!(remove_temporary(&added).unwrap().as_deref(), Some(base));
        }
    }

    #[test]
    fn a_temporary_key_never_shadows_one_already_bound() {
        // Windup's own Video binding already stops a recording on its key.
        let bound = merge(None, &wanted(&Config::default()), "windup")
            .unwrap()
            .0
            .unwrap();
        let added = add_temporary(Some(&bound), &recording_keys(), "windup")
            .unwrap()
            .unwrap();
        let (entries, _) = parse(&added).unwrap();
        let r: Vec<_> = entries
            .iter()
            .filter(|e| e.key.as_deref() == Some("r"))
            .collect();
        assert_eq!(r.len(), 1);
        assert!(runs_windup(&r[0].action, "--record"));
        assert_eq!(remove_temporary(&added).unwrap().as_deref(), Some(&*bound));
    }

    #[test]
    fn a_missing_file_gets_an_empty_map_back() {
        let added = add_temporary(None, &recording_keys(), "windup")
            .unwrap()
            .unwrap();
        assert_eq!(parse(&added).unwrap().0.len(), 3);
        assert_eq!(remove_temporary(&added).unwrap().as_deref(), Some("{\n}\n"));
    }

    #[test]
    fn stale_tagged_entries_are_cleaned_and_the_users_kept() {
        // A crash left Windup's temporary entries, and COSMIC Settings has
        // rewritten the file in its own layout since.
        let stale = r#"{
    (
        modifiers: [
            Super,
        ],
        key: "t",
        description: Some("Terminal"),
    ): Spawn("cosmic-term"),
    (
        modifiers: [],
        key: "Escape",
        description: Some("Windup while recording: Cancel"),
    ): Spawn("windup --record-cancel"),
    (
        modifiers: [
            Super,
            Shift,
        ],
        key: "p",
        description: Some("Windup while recording: Pause"),
    ): Spawn("/opt/Windup.AppImage --record-pause")
}"#;
        let cleaned = remove_temporary(stale).unwrap().unwrap();
        let (entries, _) = parse(&cleaned).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "Spawn(\"cosmic-term\")");
        assert!(cleaned.starts_with(&SETTINGS[..SETTINGS.len() - 1]));
        assert_eq!(remove_temporary(&cleaned).unwrap(), None);
    }

    #[test]
    fn the_users_own_windup_entries_are_never_removed() {
        // Same command, but not tagged: the user's.
        let text = r#"{
    (modifiers: [], key: "Escape", description: Some("Mine")): Spawn("windup --record-cancel"),
    (modifiers: [Super], key: "x", description: Some("Windup while recording: Cancel")): Spawn("other-tool"),
}"#;
        assert_eq!(remove_temporary(text).unwrap(), None);
        assert!(remove_temporary("{ oops }").is_err());
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
