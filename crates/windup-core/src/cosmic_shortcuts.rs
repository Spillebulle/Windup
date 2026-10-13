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

/// A shortcut Windup wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// COSMIC's modifier names: `Super`, `Ctrl`, `Alt`, `Shift`.
    pub modifiers: &'static [&'static str],
    /// An xkb keysym name.
    pub key: &'static str,
    /// The name COSMIC Settings shows for it.
    pub description: &'static str,
    /// The arguments after the program, e.g. `--popup`.
    pub args: &'static str,
}

/// Super+V and Super+Shift+V, as on Windows.
pub const WANTED: [Wanted; 2] = [
    Wanted {
        modifiers: &["Super"],
        key: "v",
        description: "Clipboard history",
        args: "--popup",
    },
    Wanted {
        modifiers: &["Super", "Shift"],
        key: "v",
        description: "Paste plain text",
        args: "--paste-plain",
    },
];

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
        let outcome = if entries.iter().any(|e| runs_windup(&e.action, w.args)) {
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
        ron_string(w.key),
        ron_string(w.description),
        ron_string(&command(program, w.args)),
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
            .is_some_and(|k| k.eq_ignore_ascii_case(w.key))
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

    fn added(o: &[Outcome]) -> usize {
        o.iter().filter(|o| **o == Outcome::Added).count()
    }

    #[test]
    fn a_missing_file_gets_both() {
        let (new, outcomes) = merge(None, &WANTED, "windup").unwrap();
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
        let (new, outcomes) = merge(Some("{}"), &WANTED, "windup").unwrap();
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
        let (new, outcomes) = merge(Some(SETTINGS), &WANTED, "windup").unwrap();
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
        let (new, _) = merge(Some(text), &WANTED, "windup").unwrap();
        let new = new.unwrap();
        assert!(new.contains("Spawn(\"cosmic-term\"),\n    (modifiers: [Super], key: \"v\""));
        assert_eq!(parse(&new).unwrap().0.len(), 3);
    }

    #[test]
    fn a_key_the_user_bound_is_not_taken() {
        let text = "{ (modifiers: [Super], key: \"V\"): Spawn(\"cliphist-menu\") }";
        let (new, outcomes) = merge(Some(text), &WANTED, "windup").unwrap();
        assert_eq!(outcomes, [Outcome::KeyTaken, Outcome::Added]);
        assert!(new.unwrap().contains("--paste-plain"));
    }

    #[test]
    fn modifier_order_does_not_matter() {
        let text = "{ (modifiers: [Shift, Super], key: \"v\"): Disable }";
        let (_, outcomes) = merge(Some(text), &WANTED, "windup").unwrap();
        assert_eq!(outcomes, [Outcome::Added, Outcome::KeyTaken]);
    }

    #[test]
    fn windup_already_bound_elsewhere_is_left_alone() {
        let text = r#"{
            (modifiers: [Ctrl, Alt], key: "v"): Spawn("/opt/Windup.AppImage --popup"),
            (modifiers: [Super, Shift], key: "v"): Spawn("windup --paste-plain"),
        }"#;
        let (new, outcomes) = merge(Some(text), &WANTED, "windup").unwrap();
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
        let (new, outcomes) = merge(Some(text), &WANTED, "windup").unwrap();
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
            assert!(merge(Some(text), &WANTED, "windup").is_err(), "{text}");
        }
    }

    #[test]
    fn an_appimage_path_with_a_space_is_quoted() {
        assert_eq!(command("windup", "--popup"), "windup --popup");
        assert_eq!(
            command("/home/a b/Windup.AppImage", "--popup"),
            "'/home/a b/Windup.AppImage' --popup"
        );
        let (new, _) = merge(None, &WANTED[..1], "/home/a b/it's.AppImage").unwrap();
        assert!(
            new.unwrap()
                .contains(r#"Spawn("'/home/a b/it'\\''s.AppImage' --popup")"#)
        );
    }
}
