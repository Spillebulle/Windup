//! Guards on what a release depends on being true.
//!
//! The release workflow publishes `CHANGELOG.md`'s section for the tag as the
//! release notes, and the README's download table links to files by version.
//! Both are wrong in a way nobody notices until a tag is pushed, unless this
//! catches them first, on every CI run.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root is two levels up")
        .to_path_buf()
}

fn read(name: &str) -> String {
    let path = repo_root().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// One version's section: from `## <version>` (alone, or followed by a space
/// and a date) to the next `## `. tools/release-notes.sh and
/// tools/release.ps1 state the same rule.
fn section(text: &str, version: &str) -> Option<String> {
    let mut lines = text.lines();
    lines.by_ref().find(|line| {
        line.strip_prefix("## ")
            .is_some_and(|rest| rest == version || rest.starts_with(&format!("{version} ")))
    })?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    Some(body.join("\n").trim().to_string())
}

#[test]
fn the_changelog_describes_this_version() {
    let version = env!("CARGO_PKG_VERSION");
    let body = section(&read("CHANGELOG.md"), version).unwrap_or_else(|| {
        panic!(
            "CHANGELOG.md has no `## {version}` section. The release workflow \
             publishes it as the release notes, so there would be nothing to publish."
        )
    });
    assert!(
        body.lines().any(|l| l.trim_start().starts_with("- ")),
        "the `## {version}` section of CHANGELOG.md has no bullet points"
    );
}

/// A changelog whose newest section is the previous version would publish
/// the previous version's notes.
#[test]
fn this_version_is_the_newest_entry() {
    let version = env!("CARGO_PKG_VERSION");
    let text = read("CHANGELOG.md");
    let first = text
        .lines()
        .find_map(|line| line.strip_prefix("## "))
        .expect("CHANGELOG.md has no version sections");
    let named = first.split_whitespace().next().unwrap_or(first);
    assert_eq!(
        named, version,
        "CHANGELOG.md's newest section is `{named}` but this build is `{version}`"
    );
}

/// Every file the release publishes, as `.github/workflows/release.yml` and
/// `packaging/*/package.sh` name them. A new platform's files are added here
/// with its matrix row. `{v}` is the version.
const ASSETS: &[&str] = &[
    "windup-setup-{v}-x64.exe",
    "windup-setup-{v}-arm64.exe",
    "windup-{v}-x64.msi",
    "windup-{v}-arm64.msi",
    "windup-{v}-x86_64-pc-windows-msvc.zip",
    "windup-{v}-aarch64-pc-windows-msvc.zip",
    "windup_{v}_amd64.deb",
    "windup-{v}-1.x86_64.rpm",
    "windup-bin-{v}-1-x86_64.pkg.tar.zst",
    "Windup-{v}-x86_64.AppImage",
    "windup-{v}-x86_64-unknown-linux-gnu.tar.gz",
];

const DOWNLOADS: &str = "https://github.com/Spillebulle/Windup/releases/download/";

/// The README names this release and links to each of its files. The links
/// carry the version, so they cannot be written once and left.
#[test]
fn the_readme_links_to_every_file_of_this_release() {
    let version = env!("CARGO_PKG_VERSION");
    let text = read("README.md");
    assert!(
        text.contains(&format!("**Windup {version}.**")),
        "README.md's Install section does not say `**Windup {version}.**`"
    );
    for asset in ASSETS {
        let link = format!("{DOWNLOADS}v{version}/{}", asset.replace("{v}", version));
        assert!(text.contains(&link), "README.md does not link to {link}");
    }
}

/// And to nothing else under the downloads path: no row left naming the
/// previous version or a file that is no longer built.
#[test]
fn the_readme_has_no_download_link_that_is_not_a_file_of_this_release() {
    let version = env!("CARGO_PKG_VERSION");
    let text = read("README.md");
    let wanted: Vec<String> = ASSETS.iter().map(|a| a.replace("{v}", version)).collect();
    for (at, _) in text.match_indices(DOWNLOADS) {
        let url: String = text[at + DOWNLOADS.len()..]
            .chars()
            .take_while(|c| !matches!(c, ')' | ' ' | '\n' | '"' | '>'))
            .collect();
        let (tag, name) = url
            .split_once('/')
            .unwrap_or_else(|| panic!("{DOWNLOADS}{url} names no file"));
        assert_eq!(tag, format!("v{version}"), "README.md links to {url}");
        assert!(
            wanted.iter().any(|w| w == name),
            "README.md links to `{name}`, which this release does not publish"
        );
    }
}

#[test]
fn a_section_stops_at_the_next_version() {
    let text = "# Changelog\n\n## 0.2.0 2026-10-12\n\n- new thing\n\n## 0.1.0\n\n- old thing\n";
    assert_eq!(section(text, "0.2.0").as_deref(), Some("- new thing"));
    assert_eq!(section(text, "0.1.0").as_deref(), Some("- old thing"));
    assert_eq!(section(text, "9.9.9"), None);
    assert_eq!(section("## 0.1.01\n\n- x\n", "0.1.0"), None);
}
