//! What Slipstream's panels share, quick settings and the notification centre: a glass card at the
//! top right, just below the bar, that slides down into place as it opens. Only one panel, or the
//! explorer, is open at a time.

use crate::{Slipstream, motion::HYPR, paint::Painter};

/// Logical pixels per mockup pixel: the mockup is drawn for the laptop's 1.25×.
pub const MOCKUP_PX: f32 = 0.8;
/// The card's top right corner, in mockup pixels: 10 in from the screen's edge, and 48 down, which
/// is 8 below the bar.
pub const RIGHT: f32 = 10.0;
pub const TOP: f32 = 48.0;
/// Room around the card for its shadow, and more below it, where the shadow falls.
pub const MARGIN: f32 = 64.0;
pub const BELOW: f32 = 32.0;
pub const PADDING: f32 = 18.0;
/// Between the card's rows.
pub const GAP: f32 = 14.0;

pub const INK: u32 = 0xe7ebf1ff;
pub const AMBER: u32 = 0xffb547ff;
/// Text on amber.
pub const DARK: u32 = 0x1a1206ff;
/// Where the keyboard is, until the settings are read: the focused window's ring colour at the
/// same four fifths' opacity the window ring is drawn with. `Slipstream::panel_ring` hands the
/// chosen one to each panel.
pub const FOCUS: u32 = 0x42d3ffcc;

/// Opening: 200 ms from 10 px higher and transparent. Closing is instant.
const OPEN: f64 = 0.2;
const REDUCED_FADE: f64 = 0.08;

/// The card, `w` × `h` at (`x`, `y`), with its shadow. The mockup's glass is 92% opaque over a
/// blur; with nothing blurred behind it, it's nearly solid, as the explorer's is.
pub fn glass(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
    p.card(
        x,
        y,
        w,
        h,
        18.0,
        (30.0, 90.0, 0x000000b0),
        GLASS,
        GLASS_EDGE,
    );
}

/// Large panels' glass: the explorer, quick settings, the centre, the cards and the sheet.
pub const GLASS: u32 = 0x10131af8;
pub const GLASS_EDGE: u32 = 0xffffff1c;

/// The small surfaces that come and go by themselves: the toast, the slim card, notification
/// pop-ups and the volume display. Solid, so nothing behind shows through.
pub const NOTICE: u32 = 0x141925ff;
pub const NOTICE_EDGE: u32 = 0xffffff18;
pub const NOTICE_RADIUS: f32 = 12.0;
/// How far a notice's shadow reaches past its card: down by its offset and out by half its blur.
pub const NOTICE_SHADOW: (f32, f32, u32) = (20.0, 50.0, 0x000000aa);
pub const NOTICE_MARGIN: f32 = 48.0;

/// Text that teaches a key or says something quietly: 6.4:1 on the glass, 5.4:1 in a quiet
/// button.
pub const HINT: u32 = 0x8f98a8ff;
/// What an empty search box says.
pub const PLACEHOLDER: u32 = 0x7d8697ff;

/// A notice's card, `w` × `h` at (`x`, `y`), with its shadow.
pub fn notice(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
    let (dy, blur, shadow) = NOTICE_SHADOW;
    p.shadow(x, y, w, h, NOTICE_RADIUS, dy, blur, shadow);
    p.fill(x, y, w, h, NOTICE_RADIUS, NOTICE);
    p.border(x, y, w, h, NOTICE_RADIUS, 1.0, NOTICE_EDGE);
}

/// A footer's hint ending at `right`, centred on `centre`: its keycaps, then what they do.
/// Returns where it starts.
pub fn key_hint(p: &mut Painter, right: f32, centre: f32, keys: &[&str], label: &str) -> f32 {
    const GAP: f32 = 4.0;
    let style = crate::text::Style::new(crate::text::Face::Body, 13.0, HINT);
    let caps: f32 = keys
        .iter()
        .map(|key| crate::paint::keycap_width(key) + GAP)
        .sum();
    let label_w = if label.is_empty() {
        0.0
    } else {
        GAP + crate::text::width(label, &style)
    };
    let start = right - (caps - GAP) - label_w;
    let mut x = start;
    for key in keys {
        x += p.keycap(key, x, centre) + GAP;
    }
    if !label.is_empty() {
        p.text(label, x, centre, &style);
    }
    start
}

/// The keyboard's ring, just outside a control's box.
pub fn focus_ring(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, radius: f32, colour: u32) {
    p.border(
        x - 4.0,
        y - 4.0,
        w + 8.0,
        h + 8.0,
        radius + 4.0,
        2.0,
        colour,
    );
}

/// A panel `since` seconds after it opened: its opacity, and how far above its place it is, in
/// mockup pixels.
pub fn opening(since: f64, reduced_motion: bool) -> (f32, f64) {
    if reduced_motion {
        ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
    } else {
        let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
        (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
    }
}

impl Slipstream {
    /// Closes the explorer, quick settings, the notification centre and the shortcut sheet, before
    /// another opens.
    pub fn close_panels(&mut self) {
        if self.explorer.is_open() {
            self.explorer.close();
        }
        self.quick.close();
        self.centre.close();
        self.sheet.close();
        self.history.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG's contrast ratio between two colours, 0xRRGGBBAA, the alpha ignored.
    fn contrast(a: u32, b: u32) -> f64 {
        let luminance = |rgba: u32| {
            let [r, g, b, _] = rgba.to_be_bytes().map(|c| {
                let c = c as f64 / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn hint_ink_is_at_least_4_5_to_1() {
        let glass = 0x10131aff;
        // A quiet button: white at 7% over the glass.
        let quiet = 0x21232aff;
        assert!(contrast(HINT, glass) >= 4.5, "{}", contrast(HINT, glass));
        assert!(contrast(HINT, quiet) >= 4.5, "{}", contrast(HINT, quiet));
        assert!(contrast(PLACEHOLDER, glass) >= 4.5);
    }
}
