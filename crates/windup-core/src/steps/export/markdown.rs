//! A `steps.md` that links its pictures in `images/`.

use super::Item;

/// A backslash before each of ``\ ` * _ [ ] < > #``.
pub fn escape_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\`*_[]<>#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn markdown(title: &str, items: &[Item]) -> String {
    let mut out = format!("# {}\n", escape_markdown(title));
    for item in items {
        let n = item.number;
        out.push_str(&format!(
            "\n## Step {n}\n\n![Step {n}](images/{n:02}.png)\n"
        ));
        if !item.caption.is_empty() {
            // A hard break keeps a caption's own lines apart.
            let lines: Vec<String> = item.caption.lines().map(escape_markdown).collect();
            out.push_str(&format!("\n{}\n", lines.join("  \n")));
        }
    }
    out
}
