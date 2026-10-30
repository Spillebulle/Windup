//! Text for the PDF's one font: Helvetica in WinAnsiEncoding (cp1252), so
//! no font has to be embedded.

/// cp1252's 0x80 to 0x9F, in order. `None` is a code WinAnsi leaves unused.
const SPECIALS: [Option<char>; 32] = [
    Some('€'),
    None,
    Some('‚'),
    Some('ƒ'),
    Some('„'),
    Some('…'),
    Some('†'),
    Some('‡'),
    Some('ˆ'),
    Some('‰'),
    Some('Š'),
    Some('‹'),
    Some('Œ'),
    None,
    Some('Ž'),
    None,
    None,
    Some('‘'),
    Some('’'),
    Some('“'),
    Some('”'),
    Some('•'),
    Some('–'),
    Some('—'),
    Some('˜'),
    Some('™'),
    Some('š'),
    Some('›'),
    Some('œ'),
    None,
    Some('ž'),
    Some('Ÿ'),
];

/// `s` as WinAnsi bytes. A tab becomes a space, and a character the
/// encoding lacks (or a control character) becomes `?`.
pub fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\t' => b' ',
            ' '..='~' | '\u{a0}'..='\u{ff}' => c as u8,
            '\n' | '\r' => c as u8,
            c => SPECIALS
                .iter()
                .position(|&s| s == Some(c))
                .map_or(b'?', |i| 0x80 + i as u8),
        })
        .collect()
}

/// Helvetica's advance widths, in 1/1000 em, for WinAnsi codes 32 to 255.
/// Taken from Adobe's Helvetica.afm (Core 14 AFM files, version 002.000,
/// which Adobe distributes freely), each glyph at its WinAnsi code. The
/// codes WinAnsi leaves unused (127, 129, 141, 143, 144, 157) show a bullet
/// in PDF readers, so they carry its width.
#[rustfmt::skip]
const WIDTHS: [u16; 224] = [
    // 32: space ! " # $ % & ' ( ) * + , - . /
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    // 48: 0-9 : ; < = > ?
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    // 64: @ A-O
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    // 80: P-Z [ \ ] ^ _
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    // 96: ` a-o
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    // 112: p-z { | } ~ (127 unused)
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 350,
    // 128: Euro, -, quotesinglbase, florin, quotedblbase, ellipsis, dagger, daggerdbl,
    //      circumflex, perthousand, Scaron, guilsinglleft, OE, -, Zcaron, -
    556, 350, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 350, 611, 350,
    // 144: -, quoteleft, quoteright, quotedblleft, quotedblright, bullet, endash, emdash,
    //      tilde, trademark, scaron, guilsinglright, oe, -, zcaron, Ydieresis
    350, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 350, 500, 667,
    // 160: nbsp ¡ ¢ £ ¤ ¥ ¦ § ¨ © ª « ¬ soft hyphen ® ¯
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    // 176: ° ± ² ³ ´ µ ¶ · ¸ ¹ º » ¼ ½ ¾ ¿
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    // 192: À Á Â Ã Ä Å Æ Ç È É Ê Ë Ì Í Î Ï
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    // 208: Ð Ñ Ò Ó Ô Õ Ö × Ø Ù Ú Û Ü Ý Þ ß
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    // 224: à á â ã ä å æ ç è é ê ë ì í î ï
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    // 240: ð ñ ò ó ô õ ö ÷ ø ù ú û ü ý þ ÿ
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// The width of WinAnsi `bytes` in Helvetica at `size` pt. Codes below 32
/// draw nothing.
pub fn text_width(bytes: &[u8], size: f32) -> f32 {
    let units: u32 = bytes
        .iter()
        .filter(|&&b| b >= 32)
        .map(|&b| u32::from(WIDTHS[usize::from(b - 32)]))
        .sum();
    units as f32 * size / 1000.0
}

/// `text` as WinAnsi lines no wider than `width` at `size` pt. Lines break
/// at spaces and at every newline (a blank line stays); a word longer than a
/// line is broken between characters.
pub fn wrap(text: &str, size: f32, width: f32) -> Vec<Vec<u8>> {
    let space = text_width(b" ", size);
    let mut lines = Vec::new();
    for para in winansi(text).split(|&b| b == b'\n') {
        let mut line: Vec<u8> = Vec::new();
        let mut used = 0.0;
        for word in para.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
            let w: Vec<u8> = word.iter().copied().filter(|&b| b != b'\r').collect();
            let ww = text_width(&w, size);
            if !line.is_empty() && used + space + ww <= width {
                line.push(b' ');
                line.extend_from_slice(&w);
                used += space + ww;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if ww <= width {
                line = w;
                used = ww;
                continue;
            }
            // Longer than a line: as many characters as fit, then the next.
            used = 0.0;
            for b in w {
                let bw = text_width(&[b], size);
                if !line.is_empty() && used + bw > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0.0;
                }
                line.push(b);
                used += bw;
            }
        }
        lines.push(line);
    }
    lines
}
