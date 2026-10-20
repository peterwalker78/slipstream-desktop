//! Drawing cells as pixels: block and line characters by hand, other glyphs from a cache.

use super::*;

/// The desktop's backdrop colour, as behind every window.
pub(super) const BACKGROUND: [u8; 3] = [11, 14, 19];

/// Characters other than blocks and lines, rasterised once per size.
#[derive(Default)]
pub(super) struct GlyphCache {
    pub(super) px: f32,
    pub(super) bitmaps: HashMap<char, (fontdue::Metrics, Vec<u8>)>,
}

impl GlyphCache {
    pub(super) fn draw(
        &mut self,
        pixels: &mut [u8],
        width: usize,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        ch: char,
        rgb: [u8; 3],
        weight: f32,
    ) {
        let px = h as f32 * 0.72;
        if (self.px - px).abs() > 0.01 {
            self.bitmaps.clear();
            self.px = px;
        }
        let (ascent, descent) = text::line_metrics(Face::Mono, px);
        let (metrics, coverage) = self
            .bitmaps
            .entry(ch)
            .or_insert_with(|| text::glyph(Face::Mono, ch, px));
        let baseline = y0 as f32 + h as f32 / 2.0 + (ascent + descent) / 2.0;
        let left = x0 as i32 + (w as i32 - metrics.width as i32) / 2;
        let top = baseline.round() as i32 - metrics.height as i32 - metrics.ymin;
        for gy in 0..metrics.height {
            let py = top + gy as i32;
            if py < y0 as i32 || py >= (y0 + h) as i32 {
                continue;
            }
            for gx in 0..metrics.width {
                let px = left + gx as i32;
                let cover = coverage[gy * metrics.width + gx] as u32;
                if cover == 0 || px < x0 as i32 || px >= (x0 + w) as i32 {
                    continue;
                }
                let at = (py as usize * width + px as usize) * 4;
                let amount = cover as f32 / 255.0 * weight;
                for ((channel, &ink), &ground) in
                    pixels[at..at + 3].iter_mut().zip(&rgb).zip(&BACKGROUND)
                {
                    *channel = add_ink(*channel, ink, ground, amount);
                }
            }
        }
    }
}

pub(super) fn fill(
    pixels: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rgb: [u8; 3],
) {
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for pixel in pixels[start..start + w * 4].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
}

/// Adds `rgb` over a rectangle, `weight` of the way from the background (1 paints it solid).
pub(super) fn ink_fill(
    pixels: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rgb: [u8; 3],
    weight: f32,
) {
    // The step each channel takes is the same across the whole rectangle, so it's worked out
    // once, with the half that rounds it folded in.
    let step: [f32; 3] =
        std::array::from_fn(|c| (rgb[c] as f32 - BACKGROUND[c] as f32) * weight + 0.5);
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for pixel in pixels[start..start + w * 4].chunks_exact_mut(4) {
            for (channel, step) in pixel[..3].iter_mut().zip(step) {
                *channel = (*channel as f32 + step) as u8;
            }
        }
    }
}

/// `channel` plus `amount` of the step from the background to `ink`. A cell fading out and the
/// one fading in, drawn over the background, add up to a blend of the two.
pub(super) fn add_ink(channel: u8, ink: u8, ground: u8, amount: f32) -> u8 {
    // Adding a half and truncating rounds to nearest; the cast saturates at 0 and 255 on its own,
    // which spares the per-channel calls to `roundf` a baseline x86-64 build would otherwise make.
    (channel as f32 + (ink as f32 - ground as f32) * amount + 0.5) as u8
}

/// Draws a cell as `look` says: one cell, or two cross-fading between steps.
pub(super) fn paint_cell(
    pixels: &mut [u8],
    width: usize,
    layout: &Layout,
    glyphs: &mut GlyphCache,
    col: i32,
    row: i32,
    (from, to, along): Look,
) {
    let (w, h) = (layout.cell_w, layout.cell_h);
    let x0 = layout.origin.0 + col as usize * w;
    let y0 = layout.origin.1 + row as usize * h;
    fill(pixels, width, x0, y0, w, h, BACKGROUND);
    let weight = along as f32 / TWEEN_LEVELS as f32;
    if from != to {
        ink_cell(pixels, width, glyphs, (x0, y0, w, h), from, 1.0 - weight);
    }
    ink_cell(pixels, width, glyphs, (x0, y0, w, h), to, weight);
}

/// Blocks and lines are drawn as exact rectangles, so rows join up as they do in a terminal.
pub(super) fn ink_cell(
    pixels: &mut [u8],
    width: usize,
    glyphs: &mut GlyphCache,
    (x0, y0, w, h): (usize, usize, usize, usize),
    cell: Cell,
    weight: f32,
) {
    let rgb = cell.rgb;
    let (half_w, half_h) = (w / 2, h / 2);
    if let Some(lit) = quarters(cell.ch) {
        if lit == [true; 4] {
            ink_fill(pixels, width, x0, y0, w, h, rgb, weight);
            return;
        }
        for (at, _) in lit.iter().enumerate().filter(|(_, on)| **on) {
            let (x, qw) = if at % 2 == 1 {
                (x0 + half_w, w - half_w)
            } else {
                (x0, half_w)
            };
            let (y, qh) = if at >= 2 {
                (y0 + half_h, h - half_h)
            } else {
                (y0, half_h)
            };
            ink_fill(pixels, width, x, y, qw, qh, rgb, weight);
        }
        return;
    }
    match cell.ch {
        ' ' => {}
        // Shades fill the cell at a fraction of the ink, which is what they look like in a
        // terminal and several times cheaper than a glyph.
        '░' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.25),
        '▒' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.5),
        '▓' => ink_fill(pixels, width, x0, y0, w, h, rgb, weight * 0.75),
        '─' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h,
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        // A thin line a third of the way up or down the cell, so a line that climbs slowly can
        // step a third of a row at a time rather than a whole one.
        '▔' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h - h / 3,
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        '▁' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h + h / 3 - (h / 14).max(1),
            w,
            (h / 14).max(1),
            rgb,
            weight,
        ),
        '━' => ink_fill(
            pixels,
            width,
            x0,
            y0 + half_h - h / 12,
            w,
            (h / 6).max(2),
            rgb,
            weight,
        ),
        // Braille patterns are drawn as their dots, two across and four down: a picture made of
        // them has four times the grid's detail, and the monospace font has none of them.
        ch @ '\u{2801}'..='\u{28ff}' => {
            let bits = ch as u32 - 0x2800;
            let (dot_w, dot_h) = ((w * 3 / 10).max(1), (h * 3 / 20).max(1));
            for (bit, (across, down)) in BRAILLE_DOTS.iter().enumerate() {
                if bits & (1 << bit) != 0 {
                    let x = x0 + (w * (1 + 2 * across)) / 4 - dot_w / 2;
                    let y = y0 + (h * (1 + 2 * down)) / 8 - dot_h / 2;
                    ink_fill(pixels, width, x, y, dot_w, dot_h, rgb, weight);
                }
            }
        }
        ch => glyphs.draw(pixels, width, x0, y0, w, h, ch, rgb, weight),
    }
}

/// Where each of a Braille pattern's eight dots sits, by bit: which of the two columns and which
/// of the four rows.
pub(super) const BRAILLE_DOTS: [(usize, usize); 8] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (0, 3),
    (1, 3),
];

/// The Braille pattern with a dot at each of (`across` 0..2, `down` 0..4) set in `dots`, a bit a
/// dot, row by row from the top left.
pub(super) fn braille(dots: u8) -> char {
    let mut bits = 0u32;
    for (bit, (across, down)) in BRAILLE_DOTS.iter().enumerate() {
        if dots & (1 << (down * 2 + across)) != 0 {
            bits |= 1 << bit;
        }
    }
    char::from_u32(0x2800 + bits).unwrap_or(' ')
}

/// The quarters of its cell a block character covers: top left, top right, bottom left, bottom
/// right. The logo is drawn in these, and anything else is not a block.
pub(super) fn quarters(ch: char) -> Option<[bool; 4]> {
    Some(match ch {
        '█' => [true, true, true, true],
        '▀' => [true, true, false, false],
        '▄' => [false, false, true, true],
        '▌' => [true, false, true, false],
        '▐' => [false, true, false, true],
        '▘' => [true, false, false, false],
        '▝' => [false, true, false, false],
        '▖' => [false, false, true, false],
        '▗' => [false, false, false, true],
        '▚' => [true, false, false, true],
        '▞' => [false, true, true, false],
        '▛' => [true, true, true, false],
        '▜' => [true, true, false, true],
        '▙' => [true, false, true, true],
        '▟' => [false, true, true, true],
        _ => return None,
    })
}
