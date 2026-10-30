//! Steps exports: a web page, a PDF, Markdown with its pictures, or the
//! pictures alone. Numbers come from a step's place in the list, so a
//! deleted step leaves no gap.

mod base64;
mod html;
mod markdown;
mod pdf;
mod winansi;

use std::path::{Path, PathBuf};

use crate::capture::output::file_name_ext;

use super::model::StepsDoc;
use super::options::{ExportFormat, MarkerStyle};
use super::render::render_step;

pub use base64::base64;
pub use html::{escape_html, html};
pub use markdown::{escape_markdown, markdown};
pub use pdf::{PdfImage, jpeg_for_pdf, pdf, text_width, winansi, wrap};

pub const NOTHING_TO_EXPORT: &str = "There are no steps to export.";

/// One step ready to write: its number, its caption and its picture as PNG.
pub struct Item {
    pub number: usize,
    pub caption: String,
    pub png: Vec<u8>,
}

/// Renders the session in `dir` and writes it to `dest`: a file for Html and
/// Pdf, a folder (created) for Markdown and Images. Returns what was written.
pub fn export(
    dir: &Path,
    format: ExportFormat,
    style: &MarkerStyle,
    dest: &Path,
) -> Result<PathBuf, String> {
    let doc = StepsDoc::load(dir)?;
    if doc.steps.is_empty() {
        return Err(NOTHING_TO_EXPORT.to_string());
    }
    // A folder export would merge into whatever is there.
    if matches!(format, ExportFormat::Markdown | ExportFormat::Images) && !is_free_folder(dest) {
        return Err(FOLDER_NOT_EMPTY.to_string());
    }
    // Each step rendered with its marker, one at a time, so only the encoded
    // pictures are held.
    let shot = |i: usize| -> Result<image::RgbaImage, String> {
        let step = &doc.steps[i];
        let img = image::open(dir.join(&step.file))
            .map_err(|e| format!("cannot read {}: {e}", step.file))?
            .to_rgba8();
        Ok(render_step(&img, step, i + 1, style))
    };

    if format == ExportFormat::Pdf {
        let pages = (0..doc.steps.len())
            .map(|i| {
                Ok((
                    i + 1,
                    doc.steps[i].caption.clone(),
                    jpeg_for_pdf(&shot(i)?)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if let Some(parent) = dest.parent() {
            make_dir(parent)?;
        }
        write(dest, &pdf(TITLE, &pages))?;
        return Ok(dest.to_path_buf());
    }
    let items = (0..doc.steps.len())
        .map(|i| {
            Ok(Item {
                number: i + 1,
                caption: doc.steps[i].caption.clone(),
                png: encode_png(&shot(i)?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    match format {
        ExportFormat::Html => {
            if let Some(parent) = dest.parent() {
                make_dir(parent)?;
            }
            write(dest, html(TITLE, &items).as_bytes())?;
        }
        ExportFormat::Markdown => {
            let images = dest.join("images");
            make_dir(&images)?;
            write_pictures(&images, &items)?;
            write(&dest.join("steps.md"), markdown(TITLE, &items).as_bytes())?;
        }
        ExportFormat::Images => {
            make_dir(dest)?;
            write_pictures(dest, &items)?;
        }
        ExportFormat::Pdf => unreachable!("written above"),
    }
    Ok(dest.to_path_buf())
}

const TITLE: &str = "Steps";

pub const FOLDER_NOT_EMPTY: &str = "That folder is not empty.";

/// Nothing there yet, or a folder with nothing in it. A file counts as in use.
fn is_free_folder(dest: &Path) -> bool {
    match std::fs::read_dir(dest) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => !dest.exists(),
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn make_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))
}

/// `01.png` onwards, in list order.
fn write_pictures(folder: &Path, items: &[Item]) -> Result<(), String> {
    for item in items {
        write(&folder.join(format!("{:02}.png", item.number)), &item.png)?;
    }
    Ok(())
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Default, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("cannot encode a picture: {e}"))?;
    Ok(out)
}

/// The name an export is saved under. Markdown and Images are folders, so
/// they get no extension.
pub fn export_name(pattern: &str, when: jiff::Zoned, format: ExportFormat) -> String {
    let ext = match format {
        ExportFormat::Html => "html",
        ExportFormat::Pdf => "pdf",
        ExportFormat::Markdown | ExportFormat::Images => "",
    };
    let name = file_name_ext(pattern, when, ext, "Steps");
    // A folder has no extension: drop the dot `file_name_ext` puts before it.
    if ext.is_empty() {
        name.strip_suffix('.').unwrap_or(&name).to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::model::{Button, Step, StepCause};
    use image::{Rgba, RgbaImage};
    use jiff::civil::date;

    fn when() -> jiff::Zoned {
        date(2026, 10, 30)
            .at(14, 25, 1, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
    }

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("windup-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn picture(n: u8) -> RgbaImage {
        RgbaImage::from_pixel(64, 48, Rgba([n * 40, 90, 200, 255]))
    }

    fn step(file: &str, caption: &str, at: [f32; 2]) -> Step {
        Step {
            file: file.into(),
            cause: StepCause::Click(Button::Left),
            pointer: Some(at),
            scale: 1.0,
            monitor: "DISPLAY1".into(),
            caption: caption.into(),
            at_ms: 1,
        }
    }

    /// A session folder with three pictures.
    fn session(tag: &str) -> (PathBuf, StepsDoc) {
        let dir = temp(tag);
        let mut doc = StepsDoc::new(&when());
        for (i, at) in [[10.0, 10.0], [30.0, 20.0], [50.0, 30.0]]
            .into_iter()
            .enumerate()
        {
            let file = doc.next_file();
            picture(i as u8 + 1).save(dir.join(&file)).unwrap();
            doc.push(step(&file, "", at));
        }
        doc.save(&dir).unwrap();
        (dir, doc)
    }

    fn items(captions: &[&str]) -> Vec<Item> {
        captions
            .iter()
            .enumerate()
            .map(|(i, c)| Item {
                number: i + 1,
                caption: c.to_string(),
                png: vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3],
            })
            .collect()
    }

    #[test]
    fn html_has_one_img_per_step_and_no_script() {
        let page = html("Steps", &items(&["", "", ""]));
        assert_eq!(page.matches("<img").count(), 3);
        assert!(page.contains("data:image/png;base64,iVBORw0KGgo"));
        assert!(!page.contains("<script"));
        assert!(page.contains("<style>"));
    }

    #[test]
    fn html_escapes_captions() {
        let page = html("Steps", &items(&["<b>&", "a\nb"]));
        assert!(page.contains("&lt;b&gt;&amp;"));
        assert!(page.contains("a<br>b"));
        assert!(!page.contains("<b>&"));
    }

    #[test]
    fn export_numbers_follow_the_list_not_the_files() {
        let (dir, mut doc) = session("numbers");
        doc.remove(1);
        doc.save(&dir).unwrap();
        let style = MarkerStyle::default();

        let page = dir.join("out.html");
        export(&dir, ExportFormat::Html, &style, &page).unwrap();
        let text = std::fs::read_to_string(&page).unwrap();
        assert!(text.contains("Step 1") && text.contains("Step 2"));
        assert!(!text.contains("Step 3"));

        let out = dir.join("pics");
        export(&dir, ExportFormat::Images, &style, &out).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["01.png", "02.png"]);
        let got = image::open(out.join("02.png")).unwrap().to_rgba8();
        let old = image::open(dir.join("03.png")).unwrap().to_rgba8();
        let want = render_step(&old, &doc.steps[1], 2, &style);
        assert_eq!(got, want);
    }

    #[test]
    fn markdown_links_its_images_and_escapes_captions() {
        let (dir, mut doc) = session("markdown");
        doc.set_caption(1, "*not bold*");
        doc.save(&dir).unwrap();
        let out = dir.join("md");
        let made = export(&dir, ExportFormat::Markdown, &MarkerStyle::default(), &out).unwrap();
        assert_eq!(made, out);
        let text = std::fs::read_to_string(out.join("steps.md")).unwrap();
        assert!(text.contains("![Step 2](images/02.png)"));
        assert!(text.contains(r"\*not bold\*"));
        assert!(out.join("images/01.png").exists());
    }

    #[test]
    fn a_folder_in_use_is_refused_and_an_empty_one_accepted() {
        let (dir, _) = session("folder");
        let style = MarkerStyle::default();
        for format in [ExportFormat::Markdown, ExportFormat::Images] {
            let used = dir.join(format!("used-{}", format.arg()));
            std::fs::create_dir_all(&used).unwrap();
            std::fs::write(used.join("keep.txt"), "x").unwrap();
            let err = export(&dir, format, &style, &used).unwrap_err();
            assert_eq!(err, FOLDER_NOT_EMPTY);
            let names: Vec<_> = std::fs::read_dir(&used).unwrap().collect();
            assert_eq!(names.len(), 1, "nothing was written");

            let empty = dir.join(format!("empty-{}", format.arg()));
            std::fs::create_dir_all(&empty).unwrap();
            export(&dir, format, &style, &empty).unwrap();
            assert!(
                empty
                    .join(if format == ExportFormat::Images {
                        "01.png"
                    } else {
                        "steps.md"
                    })
                    .exists()
            );
        }
        // A file may be overwritten.
        let page = dir.join("a.html");
        std::fs::write(&page, "old").unwrap();
        export(&dir, ExportFormat::Html, &style, &page).unwrap();
        assert_ne!(std::fs::read_to_string(&page).unwrap(), "old");
    }

    #[test]
    fn an_empty_doc_exports_nothing() {
        let dir = temp("empty");
        StepsDoc::new(&when()).save(&dir).unwrap();
        let err = export(
            &dir,
            ExportFormat::Html,
            &MarkerStyle::default(),
            &dir.join("x.html"),
        )
        .unwrap_err();
        assert_eq!(err, NOTHING_TO_EXPORT);
        assert!(!dir.join("x.html").exists());
    }

    #[test]
    fn export_name_per_format() {
        assert_eq!(
            export_name("Steps {date} {time}", when(), ExportFormat::Html),
            "Steps 2026-10-30 142501.html"
        );
        assert_eq!(
            export_name("Steps {date} {time}", when(), ExportFormat::Images),
            "Steps 2026-10-30 142501"
        );
        assert_eq!(
            export_name("Steps {date} {time}", when(), ExportFormat::Markdown),
            "Steps 2026-10-30 142501"
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(escape_html("\"'<>&"), "&quot;&#39;&lt;&gt;&amp;");
        assert_eq!(escape_markdown("a_b [c] #d"), r"a\_b \[c\] \#d");
    }
}
