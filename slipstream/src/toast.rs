//! Short answers to what was just done, low in the middle of the screen: an amber title over a
//! line or two of text. They share their place with the volume and brightness display, so a new
//! one of either replaces the other. A toast rises in over 250 ms and stays long enough to read:
//! 2.6 s at least, longer for more words.

use crate::motion::REDUCED_FADE;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    motion::HYPR,
    paint::{self, Painter},
    panel::{self, DESIGN_PX},
    text::{self, Face, Style},
};

// Sizes in design pixels.
const WIDTH: f32 = 440.0;
/// The shortest and longest a toast stays, and the time per word in between.
const SHOWN: f64 = 2.6;
const SHOWN_MOST: f64 = 7.0;
const PER_WORD: f64 = 0.3;
const FADE: f64 = 0.25;

struct Painted {
    scale: f64,
    buffer: MemoryRenderBuffer,
    logical: Size<i32, Logical>,
    device: (i32, i32),
}

pub struct Toast {
    /// Title, body, when it appeared on the animation clock, and how long it stays.
    message: Option<(String, String, f64, f64)>,
    painted: Option<Painted>,
    pub reduced_motion: bool,
}

impl Toast {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.painted = None;
    }

    pub fn new(reduced_motion: bool) -> Self {
        Self {
            message: None,
            painted: None,
            reduced_motion,
        }
    }

    pub fn show(&mut self, title: &str, body: &str, now: f64) {
        // The title only. Titles are fixed wording and counts; a body can name a window, and for
        // an app without a desktop entry that name is its title: a folder, a document, a URL.
        tracing::info!(title, "toast");
        self.message = Some((title.to_string(), body.to_string(), now, dwell(title, body)));
        self.painted = None;
    }

    /// Takes the toast off: the volume or brightness display has taken its place.
    pub fn clear(&mut self) {
        self.message = None;
        self.painted = None;
    }

    /// The toast for a screen `size` logical pixels across, while one is showing.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        now: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (title, body, at, shown) = self.message.as_ref()?;
        let (age, shown) = (now - at, *shown);
        if !(0.0..shown).contains(&age) {
            self.message = None;
            self.painted = None;
            return None;
        }
        if self
            .painted
            .as_ref()
            .is_none_or(|painted| painted.scale != scale)
        {
            self.painted = paint(title, body, scale);
        }
        let painted = self.painted.as_ref()?;
        let fade = if self.reduced_motion {
            REDUCED_FADE
        } else {
            FADE
        };
        let alpha = (age / fade).min((shown - age) / fade).clamp(0.0, 1.0);
        let rise = if self.reduced_motion {
            0.0
        } else {
            8.0 * (1.0 - HYPR.at((age / FADE).min(1.0)))
        };
        // The painted area holds the shadow too; the card itself keeps its place.
        let margin = (panel::NOTICE_MARGIN * DESIGN_PX) as f64;
        let location = Point::<f64, Logical>::from((
            ((size.w - painted.logical.w) / 2) as f64,
            (size.h - painted.logical.h) as f64 + margin
                - (crate::osd::BOTTOM as f64 - rise) * DESIGN_PX as f64,
        ))
        .to_physical(scale)
        .to_i32_round::<i32>()
        .to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            &painted.buffer,
            Some(alpha as f32),
            Some(Rectangle::from_size(
                (painted.device.0 as f64, painted.device.1 as f64).into(),
            )),
            Some(painted.logical),
            Kind::Unspecified,
        )
        .ok()
    }
}

/// How long a toast of `title` and `body` stays: time to read it, within bounds.
fn dwell(title: &str, body: &str) -> f64 {
    let words = title.split_whitespace().count() + body.split_whitespace().count();
    (1.0 + PER_WORD * words as f64).clamp(SHOWN, SHOWN_MOST)
}

/// Where the middle of a one-line toast sits on a screen `height` logical pixels tall: where a
/// snip flies to before its toast appears.
pub fn one_line_centre(height: f64) -> f64 {
    height - (crate::osd::BOTTOM + card_height(1, 0.0) / 2.0) as f64 * DESIGN_PX as f64
}

/// The toast's words.
fn prose() -> Style {
    Style::new(Face::Body, 16.0, panel::PROSE)
}

/// The room the words' lines have in a card `width` wide.
fn prose_width(width: f32) -> f32 {
    width - PAD - 18.0
}

/// From the card's left edge to its words.
pub const PAD: f32 = 22.0;
/// A line of the toast's words.
const LINE: f32 = 24.0;

/// How tall a toast's card is with `lines` of words, and `extra` more under them.
pub fn card_height(lines: usize, extra: f32) -> f32 {
    16.0 + 20.0 + 4.0 + lines as f32 * LINE + extra + 16.0
}

/// A toast's card, `width` × `height` with its corner at (`x`, `y`): the notice's shadow and
/// card with an amber left edge, the title in amber capitals, and `lines` of words under it.
pub fn paint_card(
    p: &mut Painter,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    title: &str,
    lines: &[String],
) {
    let heading = Style {
        tracking: 0.1,
        ..Style::new(Face::BodyBold, 13.0, panel::AMBER)
    };
    let prose = prose();
    let radius = panel::NOTICE_RADIUS;
    let (dy, blur, shadow) = panel::NOTICE_SHADOW;
    p.shadow(x, y, width, height, radius, dy, blur, shadow);
    p.fill(x, y, width, height, radius, panel::AMBER);
    p.fill(x + 4.0, y, width - 4.0, height, radius - 3.0, panel::NOTICE);
    p.border(x, y, width, height, radius, 1.0, panel::NOTICE_EDGE);
    p.text(&title.to_uppercase(), x + PAD, y + 16.0 + 10.0, &heading);
    let top = y + 16.0 + 20.0 + 4.0;
    for (i, line) in lines.iter().enumerate() {
        p.text(line, x + PAD, top + i as f32 * LINE + 12.0, &prose);
    }
}

fn paint(title: &str, body: &str, scale: f64) -> Option<Painted> {
    let lines = text::wrap(body, &prose(), prose_width(WIDTH));
    let height = card_height(lines.len(), 0.0);
    let m = panel::NOTICE_MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((WIDTH + 2.0 * m) * DESIGN_PX).ceil() as i32,
        ((height + 2.0 * m) * DESIGN_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * scale).round() as i32,
        (logical.h as f64 * scale).round() as i32,
    );
    let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * DESIGN_PX)?;
    paint_card(&mut p, m, m, WIDTH, height, title, &lines);
    Some(Painted {
        scale,
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_message_grows_the_card_rather_than_overflowing() {
        let short = paint("Gravity", "On.", 1.25).unwrap();
        let long = paint(
            "Already the centre",
            "Konsole can’t get any heavier, and this sentence goes on long enough to wrap twice \
             over in a card this narrow. Super+[ makes it lighter.",
            1.25,
        )
        .unwrap();
        assert_eq!(short.logical.w, long.logical.w);
        assert!(long.logical.h > short.logical.h);
    }

    #[test]
    fn a_toast_stays_long_enough_to_read() {
        assert_eq!(dwell("Copied", "42"), SHOWN);
        let long = dwell(
            "Setting not saved",
            "The settings file couldn't be read or written, so the change lasts until you log out.",
        );
        assert!(long > SHOWN && long <= SHOWN_MOST, "{long}");
    }
}
