//! The session document: `steps.json` beside the numbered PNGs of one
//! recording. The review window is handed only the folder, so `load` refuses
//! any file name that could point outside it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// The document's name inside a session folder.
pub const DOC_FILE: &str = "steps.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// What made the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepCause {
    Hotkey,
    Click(Button),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// `01.png`, relative to the session folder.
    pub file: String,
    pub cause: StepCause,
    /// In the image's pixels; none for a hotkey picture.
    pub pointer: Option<[f32; 2]>,
    /// Image pixels per point, used to size the marker.
    pub scale: f32,
    pub monitor: String,
    pub caption: String,
    /// Milliseconds since the session started.
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepsDoc {
    pub version: u32,
    pub started: String,
    /// The next file number, so a number is never reused after a delete.
    pub next: u32,
    pub steps: Vec<Step>,
}

impl StepsDoc {
    pub fn new(started: &jiff::Zoned) -> StepsDoc {
        StepsDoc {
            version: 1,
            started: started.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
            next: 1,
            steps: Vec::new(),
        }
    }

    /// `01.png` ... `99.png`, `100.png`; advances the counter.
    pub fn next_file(&mut self) -> String {
        let name = format!("{:02}.png", self.next);
        self.next += 1;
        name
    }

    pub fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    pub fn remove(&mut self, i: usize) -> Option<Step> {
        (i < self.steps.len()).then(|| self.steps.remove(i))
    }

    pub fn set_caption(&mut self, i: usize, text: &str) {
        if let Some(step) = self.steps.get_mut(i) {
            step.caption = text.to_string();
        }
    }

    pub fn load(dir: &Path) -> Result<StepsDoc, String> {
        let text = std::fs::read_to_string(dir.join(DOC_FILE))
            .map_err(|e| format!("cannot read {DOC_FILE}: {e}"))?;
        let doc: StepsDoc =
            serde_json::from_str(&text).map_err(|e| format!("{DOC_FILE} is broken: {e}"))?;
        for step in &doc.steps {
            if !is_step_file(&step.file) {
                return Err(format!("{DOC_FILE} names a bad file: {:?}", step.file));
            }
        }
        Ok(doc)
    }

    /// Writes `steps.json.tmp`, then renames it over `steps.json`.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = dir.join(format!("{DOC_FILE}.tmp"));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(DOC_FILE))
    }
}

/// Two or more digits and `.png`: nothing that could leave the folder.
fn is_step_file(name: &str) -> bool {
    name.strip_suffix(".png")
        .is_some_and(|stem| stem.len() >= 2 && stem.bytes().all(|b| b.is_ascii_digit()))
}

/// `2026-10-30 142501`.
pub fn session_dir_name(when: &jiff::Zoned) -> String {
    when.strftime("%Y-%m-%d %H%M%S").to_string()
}

/// Creates the session folder under `root`, adding ` (2)` and so on when the
/// name is taken.
pub fn new_session_dir(root: &Path, when: &jiff::Zoned) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let base = session_dir_name(when);
    let mut n = 1u32;
    loop {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base} ({n})")
        };
        let path = root.join(name);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Where sessions live while they are being recorded and reviewed.
pub fn steps_root() -> Option<PathBuf> {
    Config::cache_dir().map(|d| d.join("steps"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn when() -> jiff::Zoned {
        date(2026, 10, 30)
            .at(14, 25, 1, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
    }

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("windup-steps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn step(file: String) -> Step {
        Step {
            file,
            cause: StepCause::Click(Button::Left),
            pointer: Some([10.0, 20.0]),
            scale: 1.5,
            monitor: "DISPLAY1".into(),
            caption: String::new(),
            at_ms: 5,
        }
    }

    #[test]
    fn files_are_never_reused_even_after_a_reload() {
        let dir = temp("reuse");
        let mut doc = StepsDoc::new(&when());
        for _ in 0..3 {
            let f = doc.next_file();
            doc.push(step(f));
        }
        assert_eq!(doc.steps[2].file, "03.png");
        doc.remove(1);
        doc.save(&dir).unwrap();
        let mut back = StepsDoc::load(&dir).unwrap();
        let files: Vec<_> = back.steps.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["01.png", "03.png"]);
        assert_eq!(back.next_file(), "04.png");
        assert!(!dir.join("steps.json.tmp").exists());
    }

    #[test]
    fn the_hundredth_file_is_100_png() {
        let mut doc = StepsDoc::new(&when());
        doc.next = 100;
        assert_eq!(doc.next_file(), "100.png");
        assert_eq!(doc.next, 101);
    }

    #[test]
    fn a_doc_round_trips_through_json() {
        let mut doc = StepsDoc::new(&when());
        let mut a = step("01.png".into());
        a.cause = StepCause::Click(Button::Right);
        a.pointer = None;
        a.caption = "Open it".into();
        doc.push(a);
        doc.push(step("02.png".into()));
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<StepsDoc>(&json).unwrap(), doc);
        assert_eq!(doc.version, 1);
        assert_eq!(doc.started, "2026-10-30T14:25:01");
    }

    #[test]
    fn captions_and_removal_stay_in_range() {
        let mut doc = StepsDoc::new(&when());
        doc.push(step("01.png".into()));
        doc.set_caption(0, "Hi");
        doc.set_caption(5, "ignored");
        assert_eq!(doc.steps[0].caption, "Hi");
        assert!(doc.remove(3).is_none());
        assert!(doc.remove(0).is_some());
    }

    #[test]
    fn session_dir_name_is_date_and_time() {
        assert_eq!(session_dir_name(&when()), "2026-10-30 142501");
    }

    #[test]
    fn a_second_folder_in_the_same_second_gets_a_number() {
        let root = temp("twice");
        let first = new_session_dir(&root, &when()).unwrap();
        let second = new_session_dir(&root, &when()).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert!(second.to_string_lossy().ends_with(" (2)"));
    }

    #[test]
    fn load_refuses_a_file_outside_the_folder() {
        for bad in [
            "../x.png",
            "/etc/passwd",
            "a/b.png",
            "1.png",
            "01.PNG",
            "..\\x.png",
        ] {
            let dir = temp("outside");
            let mut doc = StepsDoc::new(&when());
            doc.push(step(bad.into()));
            doc.save(&dir).unwrap();
            assert!(StepsDoc::load(&dir).is_err(), "{bad}");
        }
    }

    #[test]
    fn load_reports_a_broken_json() {
        let dir = temp("broken");
        std::fs::write(dir.join(DOC_FILE), "{ not json").unwrap();
        let err = StepsDoc::load(&dir).unwrap_err();
        assert!(err.contains("broken"), "{err}");
        assert!(StepsDoc::load(&dir.join("missing")).is_err());
    }
}
