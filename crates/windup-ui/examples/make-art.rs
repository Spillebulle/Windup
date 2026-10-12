//! Writes the executable icon and the Windows installer's two pictures.
//!
//! ```sh
//! cargo run -p windup-ui --example make-art
//! ```
//!
//! Run by hand and commit what it writes:
//!
//! - `assets/icons/windup.ico`, compiled into `windup.exe` by
//!   `crates/windup-desktop/build.rs` and used by the MSI for Add or remove
//!   programs.
//! - `packaging/windows/banner.bmp` and `dialog.bmp`, WiX's `WixUIBannerBmp`
//!   and `WixUIDialogBmp`, seen only when somebody runs the `.msi` directly.
//!   The setup `.exe` installs silently behind Windup's own window.
//!
//! Nothing here invents a colour: the mark is [`windup_ui::art::mark`] in the
//! default dark palette (accent hue 28), the grounds are each theme's
//! `backdrop`, and the wordmark is the dark theme's `text_strong`. Change the
//! mark or the palette and re-run this to keep the files in step.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use std::io::Write;
use std::path::Path;
use windup_ui::art::{Image, mark};
use windup_ui::theme::Palette;

const ACCENT_HUE: f32 = 28.0;

/// The sizes the `.ico` carries. 256 is what Explorer's large views ask for.
const ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

/// Static SemiBold rather than the variable `Archivo.ttf`: `ab_glyph` ignores
/// variation axes and would draw the default master.
const ARCHIVO: &[u8] = include_bytes!("../../../assets/Archivo-SemiBold.ttf");
const WORDMARK: &str = "WINDUP";

/// WiX's sizes: the controls are 370x44 and 370x234 dialog units, which come
/// out at these pixel sizes.
const INSTALLER_BANNER: (u32, u32) = (493, 58);
const INSTALLER_DIALOG: (u32, u32) = (493, 312);

/// Where the banner may go dark. `ProgressDlg` draws its title over the left
/// of the strip in black, so the picture stays light until well past it.
const BANNER_SPLIT: u32 = 300;
/// Width of the dialog's dark sidebar. `WelcomeEulaDlg` starts its title and
/// licence at 173 px and `ExitDialog` at 180, so 168 clears both.
const DIALOG_SIDEBAR: u32 = 168;

const BLOCK_MARGIN: f32 = 12.0;
const SIDEBAR_MARGIN: f32 = 24.0;
const SIDEBAR_MARK: f32 = 88.0;
const SIDEBAR_GAP: f32 = 22.0;

/// The mark's side over the wordmark's cap height, and the gap between them as
/// a fraction of the cap height. The same proportions as the README banner
/// (`Design-Principles/banner.py`).
const MARK_PER_CAP: f32 = 1.2;
const GAP_PER_CAP: f32 = 0.46;
const TRACKING_EM: f32 = -0.031;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root is two levels above this crate");
    let dark = Palette::dark(ACCENT_HUE);
    let light = Palette::light(ACCENT_HUE);
    let font = FontRef::try_from_slice(ARCHIVO).expect("Archivo-SemiBold parses");

    let icons: Vec<Image> = ICON_SIZES
        .iter()
        .map(|&s| mark(s, dark.accent, dark.accent_ink))
        .collect();
    let ico = root.join("assets/icons/windup.ico");
    write_ico(&ico, &icons);
    println!("wrote {}", ico.display());

    let banner = root.join("packaging/windows/banner.bmp");
    write_bmp(&banner, &installer_banner(&font, &dark, &light));
    println!("wrote {}", banner.display());

    let dialog = root.join("packaging/windows/dialog.bmp");
    write_bmp(&dialog, &installer_dialog(&font, &dark, &light));
    println!("wrote {}", dialog.display());
}

/// Light on the left where MSI writes its heading, the mark and wordmark on a
/// dark block on the right.
fn installer_banner(font: &FontRef, dark: &Palette, light: &Palette) -> Image {
    let (w, h) = INSTALLER_BANNER;
    let mut image = Image::filled(w, h, light.backdrop);
    let block = Image::filled(w - BANNER_SPLIT, h, dark.backdrop);
    image.blit(&block, BANNER_SPLIT as i32, 0);

    let room_w = (w - BANNER_SPLIT) as f32 - 2.0 * BLOCK_MARGIN;
    let room_h = h as f32 - 2.0 * BLOCK_MARGIN;
    // Largest cap height at which the row fits the block.
    let mut cap = room_h / MARK_PER_CAP;
    let row = |cap: f32| cap * MARK_PER_CAP + cap * GAP_PER_CAP + word_width(font, cap);
    while row(cap) > room_w && cap > 4.0 {
        cap -= 0.25;
    }
    let side = (cap * MARK_PER_CAP).round();
    let x0 = BANNER_SPLIT as f32 + (w - BANNER_SPLIT) as f32 / 2.0 - row(cap) / 2.0;
    let cy = h as f32 / 2.0;
    image.blit(
        &mark(side as u32, dark.accent, dark.accent_ink),
        x0.round() as i32,
        (cy - side / 2.0).round() as i32,
    );
    let word = wordmark(font, cap, dark.text_strong);
    image.blit(
        &word,
        (x0 + side + cap * GAP_PER_CAP).round() as i32,
        (cy - word.height as f32 / 2.0).round() as i32,
    );
    image
}

/// A dark sidebar with the mark stacked over the wordmark, light where MSI
/// writes the welcome and finish text.
fn installer_dialog(font: &FontRef, dark: &Palette, light: &Palette) -> Image {
    let (w, h) = INSTALLER_DIALOG;
    let mut image = Image::filled(w, h, light.backdrop);
    image.blit(&Image::filled(DIALOG_SIDEBAR, h, dark.backdrop), 0, 0);

    let room = DIALOG_SIDEBAR as f32 - 2.0 * SIDEBAR_MARGIN;
    let mut cap = SIDEBAR_MARK / MARK_PER_CAP;
    while word_width(font, cap) > room && cap > 4.0 {
        cap -= 0.25;
    }
    let word = wordmark(font, cap, dark.text_strong);
    let stack = SIDEBAR_MARK + SIDEBAR_GAP + word.height as f32;
    // A little above centre, where the eye expects a title.
    let top = (h as f32 * 0.42 - stack / 2.0).round();
    let cx = DIALOG_SIDEBAR as f32 / 2.0;
    image.blit(
        &mark(SIDEBAR_MARK as u32, dark.accent, dark.accent_ink),
        (cx - SIDEBAR_MARK / 2.0).round() as i32,
        top as i32,
    );
    image.blit(
        &word,
        (cx - word.width as f32 / 2.0).round() as i32,
        (top + SIDEBAR_MARK + SIDEBAR_GAP) as i32,
    );
    image
}

/// The em that gives `cap` pixels of cap height.
fn em_for_cap(font: &FontRef, cap: f32) -> f32 {
    let probe = font.as_scaled(PxScale::from(1000.0));
    let h_bounds = font
        .outline_glyph(probe.scaled_glyph('H'))
        .map(|g| g.px_bounds().height())
        .unwrap_or(700.0);
    cap * 1000.0 / h_bounds
}

fn word_width(font: &FontRef, cap: f32) -> f32 {
    let em = em_for_cap(font, cap);
    let scaled = font.as_scaled(PxScale::from(em));
    let tracking = TRACKING_EM * em;
    let mut x = 0.0;
    let mut last = None;
    for c in WORDMARK.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = last {
            x += scaled.kern(prev, id) + tracking;
        }
        x += scaled.h_advance(id);
        last = Some(id);
    }
    x
}

/// The wordmark set at `cap` pixels of cap height, cropped to its ink.
fn wordmark(font: &FontRef, cap: f32, ink: egui::Color32) -> Image {
    let em = em_for_cap(font, cap);
    let scaled = font.as_scaled(PxScale::from(em));
    let tracking = TRACKING_EM * em;
    let baseline = scaled.ascent().ceil();
    let width = (word_width(font, cap) + 4.0).ceil() as u32;
    let height = (scaled.ascent() - scaled.descent()).ceil() as u32 + 2;
    let mut coverage = vec![0f32; (width * height) as usize];

    let mut x = 1.0;
    let mut last = None;
    for c in WORDMARK.chars() {
        let id = scaled.glyph_id(c);
        if let Some(prev) = last {
            x += scaled.kern(prev, id) + tracking;
        }
        let glyph = id.with_scale_and_position(PxScale::from(em), point(x, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, v| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as u32) < width && (py as u32) < height {
                    let i = (py as u32 * width + px as u32) as usize;
                    coverage[i] = (coverage[i] + v).min(1.0);
                }
            });
        }
        x += scaled.h_advance(id);
        last = Some(id);
    }

    // Crop to the rows and columns that carry ink, so callers centre the
    // letters rather than the font's line box.
    let rows: Vec<u32> = (0..height)
        .filter(|&y| (0..width).any(|x| coverage[(y * width + x) as usize] > 0.0))
        .collect();
    let cols: Vec<u32> = (0..width)
        .filter(|&x| (0..height).any(|y| coverage[(y * width + x) as usize] > 0.0))
        .collect();
    let (y0, y1) = (rows[0], *rows.last().expect("ink"));
    let (x0, x1) = (cols[0], *cols.last().expect("ink"));
    let mut out = Image::new(x1 - x0 + 1, y1 - y0 + 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let a = coverage[(y * width + x) as usize];
            let i = (((y - y0) * out.width + (x - x0)) * 4) as usize;
            out.pixels[i..i + 4].copy_from_slice(&[
                ink.r(),
                ink.g(),
                ink.b(),
                (a * 255.0).round() as u8,
            ]);
        }
    }
    out
}

/// 24-bit bottom-up BMP, which is what WiX and MSI's dialogs read.
fn write_bmp(path: &Path, image: &Image) {
    let row = (image.width * 3).div_ceil(4) * 4;
    let data = row * image.height;
    let mut out = Vec::with_capacity(54 + data as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(image.width as i32).to_le_bytes());
    out.extend_from_slice(&(image.height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..image.height).rev() {
        let start = out.len();
        for x in 0..image.width {
            let i = ((y * image.width + x) * 4) as usize;
            let px = &image.pixels[i..i + 4];
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        out.resize(start + row as usize, 0);
    }
    std::fs::write(path, out).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

/// An `.ico` whose entries are PNG-compressed, which Windows has read since
/// Vista and the resource compiler accepts.
fn write_ico(path: &Path, images: &[Image]) {
    let pngs: Vec<Vec<u8>> = images.iter().map(png).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (image, data) in images.iter().zip(&pngs) {
        // 256 is written as 0 in the one-byte size fields.
        out.push((image.width % 256) as u8);
        out.push((image.height % 256) as u8);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in &pngs {
        out.extend_from_slice(data);
    }
    let mut file =
        std::fs::File::create(path).unwrap_or_else(|e| panic!("creating {}: {e}", path.display()));
    file.write_all(&out)
        .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

fn png(image: &Image) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut bytes),
        &image.pixels,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )
    .expect("PNG encoding of an in-memory image");
    bytes
}
