//! Text in the mockup's fonts, rasterised with fontdue straight into tiny-skia pixmaps. The fonts
//! are embedded, so the binary needs nothing installed; their OFL licences are in `assets/fonts/`.
//! A character they don't have comes from an installed face (`fallback.rs`), chosen the same way
//! for measuring and for drawing, so text is laid out exactly as it's drawn.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use resvg::tiny_skia::Pixmap;

use crate::fallback;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Atkinson Hyperlegible, the mockup's `--body`.
    Body,
    // The explorer's selected rows and bullet time's workspace numbers use these.
    #[allow(dead_code)]
    BodyBold,
    /// JetBrains Mono, the mockup's `--mono`.
    Mono,
    MonoBold,
    /// Chakra Petch Bold, the mockup's `--disp`.
    Display,
}

static FONTS: OnceLock<Vec<Font>> = OnceLock::new();

fn font(face: Face) -> &'static Font {
    let fonts = FONTS.get_or_init(|| {
        [
            &include_bytes!("../../assets/fonts/AtkinsonHyperlegible-Regular.ttf")[..],
            &include_bytes!("../../assets/fonts/AtkinsonHyperlegible-Bold.ttf")[..],
            &include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf")[..],
            &include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf")[..],
            &include_bytes!("../../assets/fonts/ChakraPetch-Bold.ttf")[..],
        ]
        .into_iter()
        .map(|data| Font::from_bytes(data, FontSettings::default()).expect("embedded fonts parse"))
        .collect()
    });
    &fonts[face as usize]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub face: Face,
    /// Size in pixels.
    pub px: f32,
    /// Straight (not premultiplied) RGBA.
    pub color: [u8; 4],
    /// Extra space after each character, in ems (CSS `letter-spacing`).
    pub tracking: f32,
    /// Equal-width digits, so a clock doesn't jiggle (CSS `tabular-nums`).
    pub tabular: bool,
}

impl Style {
    /// `rgba` is written as in CSS: 0xRRGGBBAA.
    pub fn new(face: Face, px: f32, rgba: u32) -> Self {
        Self {
            face,
            px,
            color: rgba.to_be_bytes(),
            tracking: 0.0,
            tabular: false,
        }
    }

    pub fn scaled(&self, factor: f32) -> Self {
        Self {
            px: self.px * factor,
            ..*self
        }
    }
}

/// Where a character's glyph comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    /// The style's own embedded face, which has it (or nothing installed does, and it draws as
    /// the face's missing-glyph box).
    Embedded,
    /// An installed face's glyph.
    Fallback(usize, ttf_parser::GlyphId),
    /// A face that may have it is still loading: its space, and nothing in it yet.
    Blank,
}

/// Calls `glyph(ch, x, source)` with each character's pen position, and returns the total advance.
fn walk(text: &str, style: &Style, mut glyph: impl FnMut(char, f32, Source)) -> f32 {
    let font = font(style.face);
    let digit = font.metrics('0', style.px).advance_width;
    let mut x = 0.0;
    let mut previous = None;
    for ch in text.chars() {
        let own = font.lookup_glyph_index(ch) != 0 || ch.is_whitespace() || ch.is_control();
        let found = if own {
            fallback::Found::Absent
        } else {
            fallback::find(ch, style.px)
        };
        match found {
            fallback::Found::Glyph { slot, id, advance } => {
                glyph(ch, x, Source::Fallback(slot, id));
                x += advance;
                previous = None;
            }
            fallback::Found::Pending => {
                glyph(ch, x, Source::Blank);
                x += font.metrics(ch, style.px).advance_width;
                previous = None;
            }
            fallback::Found::Absent => {
                if let Some(previous) = previous {
                    x += font.horizontal_kern(previous, ch, style.px).unwrap_or(0.0);
                }
                let advance = font.metrics(ch, style.px).advance_width;
                if style.tabular && ch.is_ascii_digit() {
                    glyph(ch, x + (digit - advance) / 2.0, Source::Embedded);
                    x += digit;
                } else {
                    glyph(ch, x, Source::Embedded);
                    x += advance;
                }
                previous = Some(ch);
            }
        }
        x += style.tracking * style.px;
    }
    x
}

/// One character's coverage bitmap at `px` pixels, for drawing text cell by cell.
pub fn glyph(face: Face, ch: char, px: f32) -> (fontdue::Metrics, Vec<u8>) {
    font(face).rasterize(ch, px)
}

/// A face's ascent and descent (negative) at `px` pixels.
pub fn line_metrics(face: Face, px: f32) -> (f32, f32) {
    font(face)
        .horizontal_line_metrics(px)
        .map_or((px * 0.8, -px * 0.2), |line| (line.ascent, line.descent))
}

pub fn width(text: &str, style: &Style) -> f32 {
    walk(text, style, |_, _, _| {})
}

/// The baseline that centres a line of `style` on `centre`, as CSS centres a line box.
pub fn baseline(style: &Style, centre: f32) -> f32 {
    match font(style.face).horizontal_line_metrics(style.px) {
        Some(line) => centre + (line.ascent + line.descent) / 2.0,
        None => centre + style.px * 0.35,
    }
}

/// `text`, cut short with an ellipsis if it's wider than `room`.
pub fn ellipsize(text: &str, style: &Style, room: f32) -> String {
    if width(text, style) <= room {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.pop().is_some() {
        let candidate = format!("{}…", chars.iter().collect::<String>().trim_end());
        if width(&candidate, style) <= room {
            return candidate;
        }
    }
    String::new()
}

/// `text` broken at spaces into lines no wider than `room`. A single word wider than that gets a
/// line to itself.
pub fn wrap(text: &str, style: &Style, room: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let longer = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || width(&longer, style) <= room {
            line = longer;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draws `text` from `x` with its baseline at `y`, blended over what's in `pixmap`. Returns its
/// width.
pub fn draw(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, style: &Style) -> f32 {
    let font = font(style.face);
    walk(text, style, |ch, pen, source| {
        let left = (x + pen).round() as i32;
        match source {
            Source::Embedded => {
                let (metrics, coverage) = font.rasterize(ch, style.px);
                let top = y.round() as i32 - metrics.height as i32 - metrics.ymin;
                let area = (left + metrics.xmin, top, metrics.width, metrics.height);
                blend(pixmap, area, &coverage, style.color);
            }
            Source::Fallback(slot, id) => {
                fallback::with_raster(slot, id, style.px, |raster| {
                    let top = y.round() as i32 - raster.height as i32 - raster.ymin;
                    let area = (left + raster.xmin, top, raster.width, raster.height);
                    blend(pixmap, area, &raster.coverage, style.color);
                });
            }
            Source::Blank => {}
        }
    })
}

/// Lays `coverage`, `width` by `height` with its top-left corner at (`left`, `top`), over `pixmap`
/// in `color`.
fn blend(
    pixmap: &mut Pixmap,
    (left, top, width, height): (i32, i32, usize, usize),
    coverage: &[u8],
    [r, g, b, a]: [u8; 4],
) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let pixels = pixmap.data_mut();
    for row in 0..height {
        let py = top + row as i32;
        if !(0..h).contains(&py) {
            continue;
        }
        for col in 0..width {
            let px = left + col as i32;
            let cover = coverage[row * width + col] as u32;
            if cover == 0 || !(0..w).contains(&px) {
                continue;
            }
            // Premultiplied "over": the pixmap stores premultiplied RGBA.
            let alpha = cover * a as u32 / 255;
            let keep = 255 - alpha;
            let at = (py * w + px) as usize * 4;
            for (i, channel) in [r, g, b].into_iter().enumerate() {
                pixels[at + i] =
                    ((channel as u32 * alpha + pixels[at + i] as u32 * keep + 127) / 255) as u8;
            }
            pixels[at + 3] = ((alpha * 255 + pixels[at + 3] as u32 * keep + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabular_digits_keep_a_clock_the_same_width() {
        let clock = Style {
            tabular: true,
            ..Style::new(Face::Body, 16.0, 0xffffffff)
        };
        assert_eq!(width("11:11", &clock), width("08:08", &clock));
    }

    #[test]
    fn long_text_is_cut_short_with_an_ellipsis() {
        let style = Style::new(Face::Body, 14.0, 0xffffffff);
        let title = "A very long window title that will not fit in the room available";
        let short = ellipsize(title, &style, 120.0);
        assert!(short.ends_with('…'));
        assert!(width(&short, &style) <= 120.0);
        assert_eq!(ellipsize("Files", &style, 120.0), "Files");
    }

    #[test]
    fn wrapped_lines_fit_and_keep_every_word() {
        let style = Style::new(Face::Body, 16.0, 0xffffffff);
        let body = "Firefox got lighter and drifted to the edge of the workspace.";
        let lines = wrap(body, &style, 150.0);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| width(line, &style) <= 150.0));
        assert_eq!(lines.join(" "), body);
        assert!(wrap("", &style, 150.0).is_empty());
    }

    #[test]
    fn drawing_covers_pixels_in_the_text_colour() {
        let mut pixmap = Pixmap::new(60, 30).unwrap();
        let style = Style::new(Face::MonoBold, 20.0, 0xff0000ff);
        draw(&mut pixmap, "W", 5.0, 22.0, &style);
        let solid = pixmap.data().chunks_exact(4).find(|pixel| pixel[3] == 255);
        assert_eq!(solid, Some(&[255, 0, 0, 255][..]));
    }
}
