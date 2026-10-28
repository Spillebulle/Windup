//! Screenshot output: file names, unique paths and encoding.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use jiff::Zoned;

use crate::config::ImageFormat;

/// Expands `{date}` / `{time}`, makes the result a legal Windows file name and
/// appends the extension.
pub fn file_name(pattern: &str, when: Zoned, format: ImageFormat) -> String {
    let date = when.strftime("%Y-%m-%d").to_string();
    let time = when.strftime("%H%M%S").to_string();
    let expanded = pattern.replace("{date}", &date).replace("{time}", &time);
    let cleaned: String = expanded
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_ascii_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = cleaned.trim_end_matches(['.', ' ']);
    let stem = if stem.is_empty() { "Screenshot" } else { stem };
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
    };
    format!("{stem}.{ext}")
}

/// `dir/name`, or `dir/stem (2).ext`, `(3)`, ... when the name is taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (2u32..)
        .map(|n| match &ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        })
        .find(|p| !p.exists())
        .expect("an unused name exists")
}

/// `<Pictures>/Windup`, where screenshots are saved by default.
pub fn default_dir() -> Option<PathBuf> {
    directories::UserDirs::new()?
        .picture_dir()
        .map(|p| p.join("Windup"))
}

/// Encodes to PNG, or to JPEG at quality 90 with transparency flattened onto black.
pub fn encode(img: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    match format {
        ImageFormat::Png => {
            let encoder = image::codecs::png::PngEncoder::new(&mut out);
            let _ = img.write_with_encoder(encoder);
        }
        ImageFormat::Jpeg => {
            let mut rgb = image::RgbImage::new(img.width(), img.height());
            for (x, y, p) in img.enumerate_pixels() {
                let a = u32::from(p[3]);
                let mix = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
            }
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
            let _ = rgb.write_with_encoder(encoder);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Zoned {
        jiff::civil::date(2026, 10, 28)
            .at(17, 47, 12, 0)
            .in_tz("UTC")
            .unwrap()
    }

    #[test]
    fn default_pattern_names_the_file() {
        assert_eq!(
            file_name("Screenshot {date} {time}", at(), ImageFormat::Png),
            "Screenshot 2026-10-28 174712.png"
        );
        assert_eq!(file_name("x", at(), ImageFormat::Jpeg), "x.jpg");
    }

    #[test]
    fn illegal_characters_become_dashes() {
        assert_eq!(file_name("a:b", at(), ImageFormat::Png), "a-b.png");
        assert_eq!(
            file_name("a<>\"/\\|?*b\u{1}", at(), ImageFormat::Png),
            "a--------b-.png"
        );
    }

    #[test]
    fn trailing_dots_and_empty_patterns() {
        assert_eq!(file_name("shot. . ", at(), ImageFormat::Png), "shot.png");
        assert_eq!(file_name("", at(), ImageFormat::Png), "Screenshot.png");
        assert_eq!(file_name("...", at(), ImageFormat::Jpeg), "Screenshot.jpg");
    }

    #[test]
    fn unique_path_numbers_taken_names() {
        let dir = std::env::temp_dir().join(format!("windup-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (2).png"));
        std::fs::write(dir.join("a (2).png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (3).png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn png_round_trips() {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 128]));
        let bytes = encode(&img, ImageFormat::Png);
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back, img);
    }

    #[test]
    fn jpeg_flattens_transparency_onto_black() {
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 0]));
        let bytes = encode(&img, ImageFormat::Jpeg);
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let p = back.get_pixel(4, 4);
        assert!(p.0.iter().all(|&c| c < 8), "{p:?}");
    }
}
