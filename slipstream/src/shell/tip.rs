//! Tips: what something says when the pointer rests on it, on a small card of a line or a few. A
//! tip waits for the pointer to settle on one thing, so sweeping across several shows nothing. The
//! streams of code rain have one (`stream_tip.rs`).
//!
//! A tip appears and goes at once, with nothing animated, so reduced motion has nothing to change.

use std::time::{Duration, Instant};

use smithay::utils::{Logical, Size};

use crate::{
    paint,
    panel::{self, DESIGN_PX},
    text::{self, Style},
};

/// How long the pointer stays on something before its tip shows.
pub const REST: Duration = Duration::from_millis(500);

/// The card's padding, in design pixels.
const PAD: f32 = 14.0;


/// What the pointer is on, since when, and at what height.
pub struct Hover<K> {
    over: Option<(K, Instant)>,
    pub y: f64,
}

impl<K: PartialEq> Hover<K> {
    pub fn new() -> Self {
        Self { over: None, y: 0.0 }
    }

    /// The pointer is on `over`, or on nothing, `y` logical pixels down the screen. Moving about
    /// on the same thing keeps its wait, or its tip, and the tip can follow the pointer down.
    pub fn point(&mut self, over: Option<K>, y: f64, at: Instant) {
        let Some(key) = over else {
            self.over = None;
            return;
        };
        if self.over.as_ref().is_none_or(|(on, _)| *on != key) {
            self.over = Some((key, at));
        }
        self.y = y;
    }

    /// What the pointer was on went: it has to come back for a tip to show again.
    pub fn leave(&mut self) {
        self.over = None;
    }

    /// What the tip shows for at `at`: the thing the pointer has been on for `REST`.
    pub fn showing(&self, at: Instant) -> Option<&K> {
        self.over
            .as_ref()
            .filter(|(_, since)| at.saturating_duration_since(*since) >= REST)
            .map(|(on, _)| on)
    }
}

/// One line of a tip: its text, its style, and the height it takes, in design pixels.
pub struct Line<'a> {
    pub text: &'a str,
    pub style: Style,
    pub height: f32,
}

/// A tip's card: `lines`, one under another, on a notice's solid card, since it is read at a
/// glance over whatever is under it. The card is `NOTICE_MARGIN` bigger than that on every side,
/// for its shadow.
pub fn card(lines: &[Line], scale: f64) -> Option<paint::Painted> {
    let width = lines
        .iter()
        .map(|line| text::width(line.text, &line.style))
        .fold(0.0, f32::max)
        + 2.0 * PAD;
    let height = 2.0 * PAD + lines.iter().map(|line| line.height).sum::<f32>();
    let m = panel::NOTICE_MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((width + 2.0 * m) * DESIGN_PX).ceil() as i32,
        ((height + 2.0 * m) * DESIGN_PX).ceil() as i32,
    ));
    paint::Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        panel::notice(p, m, m, width, height);
        let mut y = m + PAD;
        for line in lines {
            p.text(line.text, m + PAD, y + line.height / 2.0, &line.style);
            y += line.height;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tip_waits_for_the_pointer_to_rest_on_one_thing() {
        let start = Instant::now();
        let mut hover = Hover::new();
        hover.point(Some(0), 300.0, start);
        assert_eq!(hover.showing(start + REST / 2), None, "only passing over");
        // Moving about on the same thing doesn't start the wait again.
        hover.point(Some(0), 340.0, start + REST / 2);
        assert_eq!(hover.showing(start + REST), Some(&0));
        // Over to the next: it waits again, and the first one's tip has gone.
        hover.point(Some(1), 340.0, start + REST);
        assert_eq!(hover.showing(start + REST), None);
        assert_eq!(hover.showing(start + REST * 2), Some(&1));
    }

    #[test]
    fn a_tip_goes_when_the_pointer_leaves_or_its_thing_does() {
        let start = Instant::now();
        let mut hover = Hover::new();
        hover.point(Some(0), 300.0, start);
        hover.point(None, 300.0, start + REST);
        assert_eq!(hover.showing(start + REST * 2), None);

        hover.point(Some(0), 300.0, start);
        hover.leave();
        assert_eq!(hover.showing(start + REST * 2), None);
        // Coming back is a fresh wait.
        hover.point(Some(0), 300.0, start + REST * 2);
        assert_eq!(hover.showing(start + REST * 2), None);
    }
}
