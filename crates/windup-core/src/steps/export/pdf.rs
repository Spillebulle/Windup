//! A minimal PDF 1.4 writer: one A4 page per step with its heading, its
//! picture as a JPEG and its caption, which continues on text-only pages
//! when it does not fit.

use image::RgbaImage;

pub use super::winansi::{text_width, winansi, wrap};

/// A picture ready for a page: baseline JPEG bytes and its size in pixels.
pub struct PdfImage {
    pub jpeg: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// `img` as a JPEG at quality 90, its transparency laid onto white (a PDF
/// JPEG has no alpha).
pub fn jpeg_for_pdf(img: &RgbaImage) -> Result<PdfImage, String> {
    let mut rgb = Vec::with_capacity(img.as_raw().len() / 4 * 3);
    for p in img.pixels() {
        let a = u32::from(p[3]);
        for &c in &p.0[..3] {
            rgb.push(((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(
            &rgb,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("cannot encode a picture: {e}"))?;
    Ok(PdfImage {
        jpeg,
        w: img.width(),
        h: img.height(),
    })
}

// A4 in points, and the layout on it.
const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 48.0;
const TEXT_W: f32 = PAGE_W - 2.0 * MARGIN;
const IMAGE_MAX_H: f32 = 560.0;
const HEADING: f32 = 14.0;
const CAPTION: f32 = 11.0;
const LEADING: f32 = 14.0;
/// Between the heading and the picture, and the picture and the caption.
const GAP: f32 = 10.0;
const TOP: f32 = PAGE_H - MARGIN;

// The fixed objects; pages, contents and pictures follow from 5.
const CATALOG: usize = 1;
const PAGES: usize = 2;
const FONT: usize = 3;
const INFO: usize = 4;

/// One page per step: `Step N`, the picture fitted to the text width (at
/// most 560 pt high) and the caption under it. Caption lines that do not
/// fit go on text-only pages after it.
pub fn pdf(title: &str, steps: &[(usize, String, PdfImage)]) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![Vec::new(); INFO];
    let mut kids = Vec::new();
    for (number, caption, image) in steps {
        let scale = (TEXT_W / image.w.max(1) as f32).min(IMAGE_MAX_H / image.h.max(1) as f32);
        let (w, h) = (image.w as f32 * scale, image.h as f32 * scale);
        let mut content = Vec::new();
        let heading = format!("Step {number}");
        text_line(&mut content, HEADING, TOP - HEADING, heading.as_bytes());
        let bottom = TOP - HEADING - GAP - h;
        let draw = format!(
            "q {} 0 0 {} {} {} cm /Im1 Do Q\n",
            num(w),
            num(h),
            num(MARGIN),
            num(bottom)
        );
        content.extend_from_slice(draw.as_bytes());

        let lines = if caption.trim().is_empty() {
            Vec::new()
        } else {
            wrap(caption, CAPTION, TEXT_W)
        };
        let mut picture = Some(image);
        let mut y = bottom - GAP - CAPTION;
        for line in lines {
            if y < MARGIN {
                // This page is full: the rest goes on a text-only one.
                let full = std::mem::take(&mut content);
                add_page(&mut objects, &mut kids, full, picture.take());
                y = TOP - CAPTION;
            }
            text_line(&mut content, CAPTION, y, &line);
            y -= LEADING;
        }
        add_page(&mut objects, &mut kids, content, picture);
    }

    objects[CATALOG - 1] = format!("<< /Type /Catalog /Pages {PAGES} 0 R >>").into_bytes();
    let refs: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    objects[PAGES - 1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        refs.join(" "),
        refs.len()
    )
    .into_bytes();
    objects[FONT - 1] =
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec();
    let mut info = b"<< /Title ".to_vec();
    info.extend_from_slice(&info_string(title));
    info.extend_from_slice(b" /Producer (Windup) >>");
    objects[INFO - 1] = info;
    assemble(&objects)
}

/// Adds a page drawing `content`, with `image` as `/Im1` when given.
fn add_page(
    objects: &mut Vec<Vec<u8>>,
    kids: &mut Vec<usize>,
    content: Vec<u8>,
    image: Option<&PdfImage>,
) {
    let page = objects.len() + 1;
    let contents = page + 1;
    let xobject = match image {
        Some(_) => format!(" /XObject << /Im1 {} 0 R >>", page + 2),
        None => String::new(),
    };
    objects.push(
        format!(
            "<< /Type /Page /Parent {PAGES} 0 R /MediaBox [0 0 {} {}] \
/Resources << /Font << /F1 {FONT} 0 R >>{xobject} >> /Contents {contents} 0 R >>",
            num(PAGE_W),
            num(PAGE_H)
        )
        .into_bytes(),
    );
    objects.push(stream("", &content));
    if let Some(image) = image {
        let dict = format!(
            " /Type /XObject /Subtype /Image /Width {} /Height {} \
/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
            image.w, image.h
        );
        objects.push(stream(&dict, &image.jpeg));
    }
    kids.push(page);
}

/// A stream object: `extra` dictionary entries, then the bytes as they are.
fn stream(extra: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<<{extra} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// One line of Helvetica at the left margin, its baseline at `y`.
fn text_line(out: &mut Vec<u8>, size: f32, y: f32, text: &[u8]) {
    let at = format!("BT\n/F1 {} Tf\n{} {} Td\n", num(size), num(MARGIN), num(y));
    out.extend_from_slice(at.as_bytes());
    out.extend_from_slice(&literal(text));
    out.extend_from_slice(b" Tj\nET\n");
}

/// A literal string: `(`, `)` and `\` escaped, and line ends as escapes so
/// a reader cannot normalise them.
fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'('];
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b => out.push(b),
        }
    }
    out.push(b')');
    out
}

/// The document title: a literal string when ASCII, else UTF-16BE behind a
/// byte order mark, which readers accept for any Info string.
fn info_string(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        return literal(s.as_bytes());
    }
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04X}"));
    }
    out.push('>');
    out.into_bytes()
}

/// A number with at most two decimals and no trailing zeros.
fn num(x: f32) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The file: header, objects from 1, the xref table and the trailer.
fn assemble(objects: &[Vec<u8>]) -> Vec<u8> {
    // High bytes on the second line mark the file as binary.
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    let trailer = format!(
        "trailer\n<< /Size {size} /Root {CATALOG} 0 R /Info {INFO} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    );
    out.extend_from_slice(trailer.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::export::export;
    use crate::steps::model::{Button, Step, StepCause, StepsDoc};
    use crate::steps::options::{ExportFormat, MarkerStyle};
    use image::Rgba;

    fn img() -> PdfImage {
        jpeg_for_pdf(&RgbaImage::from_pixel(64, 48, Rgba([40, 90, 200, 255]))).unwrap()
    }

    fn three() -> Vec<(usize, String, PdfImage)> {
        vec![
            (1, "Click Start".into(), img()),
            (2, String::new(), img()),
            (3, "Type a name\nand press Enter".into(), img()),
        ]
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).rposition(|w| w == needle)
    }

    /// Every xref entry points at its own `n 0 obj`, and `startxref` at the
    /// table. Returns the number of objects.
    fn check_xref(b: &[u8]) -> usize {
        let at = rfind(b, b"\nxref\n").expect("an xref table") + 1;
        let rest = text(&b[at..]);
        let mut lines = rest.lines();
        assert_eq!(lines.next(), Some("xref"));
        let head: Vec<usize> = lines
            .next()
            .unwrap()
            .split(' ')
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(head[0], 0);
        let count = head[1];
        assert_eq!(lines.next(), Some("0000000000 65535 f "));
        for n in 1..count {
            let line = lines.next().unwrap();
            assert_eq!(line.len(), 19, "entry {n}: {line:?}");
            assert!(line.ends_with(" 00000 n "));
            let off: usize = line[..10].parse().unwrap();
            assert!(
                b[off..].starts_with(format!("{n} 0 obj").as_bytes()),
                "object {n} is not at {off}"
            );
        }
        // Each entry is exactly 20 bytes, line end included.
        let table = find(&b[at..], b"trailer").unwrap();
        assert_eq!(
            table,
            "xref\n".len() + format!("0 {count}\n").len() + 20 * count
        );
        let sx = rfind(b, b"startxref\n").unwrap() + "startxref\n".len();
        let num: usize = text(&b[sx..]).lines().next().unwrap().parse().unwrap();
        assert_eq!(num, at);
        assert!(text(&b[at..]).contains(&format!("/Size {count}")));
        count
    }

    #[test]
    fn pdf_starts_and_ends_right() {
        let b = pdf("T", &three());
        assert!(b.starts_with(b"%PDF-1.4\n"));
        assert!(b.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let b = pdf("T", &three());
        // Catalog, pages, font, info, then a page, its content and its picture per step.
        assert_eq!(check_xref(&b), 1 + 4 + 3 * 3);
    }

    #[test]
    fn one_page_per_step() {
        let s = text(&pdf("T", &three()));
        assert!(s.contains("/Count 3"));
        assert_eq!(s.matches("/Filter /DCTDecode").count(), 3);
        assert!(s.contains("/Width 64 /Height 48"));
        assert!(s.contains("/ColorSpace /DeviceRGB /BitsPerComponent 8"));
        assert!(s.contains("/BaseFont /Helvetica /Encoding /WinAnsiEncoding"));
        assert!(s.contains("/MediaBox [0 0 595 842]"));
        assert!(s.contains("(Step 3) Tj"));
        assert!(s.contains("(and press Enter) Tj"));
        assert!(s.contains("/Title (T)"));
    }

    #[test]
    fn the_picture_is_fitted_to_the_text_width() {
        let s = text(&pdf("T", &three()));
        // 64 x 48 at 499 pt wide is 374.25 pt high.
        assert!(s.contains("499 0 0 374.25 48 "), "{s}");
        let tall = jpeg_for_pdf(&RgbaImage::from_pixel(10, 100, Rgba([0, 0, 0, 255]))).unwrap();
        let s = text(&pdf("T", &[(1, String::new(), tall)]));
        // Capped at 560 pt high: 56 pt wide.
        assert!(s.contains("56 0 0 560 48 "), "{s}");
    }

    #[test]
    fn winansi_maps_and_substitutes() {
        assert_eq!(winansi("café €5 — 漢"), b"caf\xe9 \x805 \x97 ?".to_vec());
        assert_eq!(winansi("a\tb"), b"a b".to_vec());
        assert_eq!(
            winansi("‘’“”•–™Ÿ"),
            b"\x91\x92\x93\x94\x95\x96\x99\x9f".to_vec()
        );
        assert_eq!(winansi("\u{a0}ÿ"), b"\xa0\xff".to_vec());
        assert_eq!(winansi("\u{80}\u{1}"), b"??".to_vec());
    }

    #[test]
    fn helvetica_widths() {
        assert!((text_width(b"Hello", 10.0) - 22.78).abs() < 0.01);
        assert!((text_width(b" ", 1000.0) - 278.0).abs() < 0.01);
        assert!((text_width(b"\x97", 1000.0) - 1000.0).abs() < 0.01);
        assert!((text_width(b"\xe9", 1000.0) - 556.0).abs() < 0.01);
    }

    #[test]
    fn wrapped_lines_fit() {
        let lines = wrap(&"word ".repeat(80), 11.0, 499.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(text_width(l, 11.0) <= 499.0);
        }
        assert_eq!(wrap("a\nb", 11.0, 499.0).len(), 2);
        assert_eq!(
            wrap("a\n\nb", 11.0, 499.0),
            [b"a".to_vec(), vec![], b"b".to_vec()]
        );
        let long = wrap(&"m".repeat(100), 11.0, 100.0);
        assert!(long.len() > 1);
        assert_eq!(long.concat(), b"m".repeat(100));
        for l in &long {
            assert!(text_width(l, 11.0) <= 100.0);
        }
    }

    #[test]
    fn parentheses_and_backslashes_are_escaped() {
        let s = text(&pdf("T", &[(1, r"a(b)\c".into(), img())]));
        assert!(s.contains(r"(a\(b\)\\c) Tj"));
    }

    #[test]
    fn a_long_caption_continues_on_the_next_page() {
        let b = pdf("T", &[(1, "word ".repeat(2000), img())]);
        let s = text(&b);
        assert!(s.contains("/Count 2") || s.contains("/Count 3"));
        assert_eq!(s.matches("/Filter /DCTDecode").count(), 1);
        check_xref(&b);
        // Every caption line sits inside the bottom margin.
        for line in s.lines().filter(|l| l.ends_with(" Td")) {
            let y: f32 = line.split(' ').nth(1).unwrap().parse().unwrap();
            assert!(y >= 48.0, "{line}");
        }
    }

    #[test]
    fn export_writes_a_pdf_file() {
        let dir = std::env::temp_dir().join(format!("windup-export-pdf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let when = jiff::civil::date(2026, 10, 30)
            .at(14, 25, 1, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap();
        let mut doc = StepsDoc::new(&when);
        for caption in ["Open the menu", "Choose (Save)"] {
            let file = doc.next_file();
            RgbaImage::from_pixel(64, 48, Rgba([200, 90, 40, 255]))
                .save(dir.join(&file))
                .unwrap();
            doc.push(Step {
                file,
                cause: StepCause::Click(Button::Left),
                pointer: Some([20.0, 20.0]),
                scale: 1.0,
                monitor: "DISPLAY1".into(),
                caption: caption.into(),
                at_ms: 1,
            });
        }
        doc.save(&dir).unwrap();
        let dest = dir.join("out").join("Steps.pdf");
        let made = export(&dir, ExportFormat::Pdf, &MarkerStyle::default(), &dest).unwrap();
        assert_eq!(made, dest);
        let b = std::fs::read(&dest).unwrap();
        assert!(b.starts_with(b"%PDF-1.4\n"));
        check_xref(&b);
        let s = text(&b);
        assert!(s.contains("/Count 2"));
        assert!(s.contains(r"(Choose \(Save\)) Tj"));
    }
}
