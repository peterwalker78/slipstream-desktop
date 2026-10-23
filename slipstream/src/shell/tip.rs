//! Tips: what something says when the pointer rests on it, on a small card of a line or a few. A
//! tip waits for the pointer to settle on one thing, so sweeping across several shows nothing. The
//! streams of code rain have one (`stream_tip.rs`), and so do the bar's modules.
//!
//! A tip appears and goes at once, with nothing animated, so reduced motion has nothing to change.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    paint,
    panel::{self, DESIGN_PX},
    text::{self, Face, Style},
};

/// How long the pointer stays on something before its tip shows.
pub const REST: Duration = Duration::from_millis(500);

/// The card's padding, in design pixels.
const PAD: f32 = 14.0;

/// Between a tip and the bar, and the least room kept from the screen's edges, in logical
/// pixels.
const GAP: f64 = 8.0;
/// The widest a line of a bar tip runs, in design pixels, before it is cut short.
const BAR_TIP_LINE: f32 = 560.0;

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

    /// What the pointer is on, tip or no tip yet.
    pub fn over(&self) -> Option<&K> {
        self.over.as_ref().map(|(on, _)| on)
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

/// A tip hanging under something on the bar: its text, a line or a few, on a card centred under
/// it and kept on the screen. It is painted again only when what it says changes.
#[derive(Default)]
pub struct BarTip {
    painted: Option<(String, paint::Painted)>,
}

impl BarTip {
    /// The card saying `text` under `under`, on a screen `screen_w` logical pixels wide whose
    /// bar is `bar_h` tall.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        text: &str,
        under: Rectangle<f64, Logical>,
        (screen_w, bar_h): (i32, i32),
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let stale = self
            .painted
            .as_ref()
            .is_none_or(|(said, painted)| said != text || painted.scale != scale);
        if stale {
            let style = Style::new(Face::Body, 14.0, panel::PROSE);
            let lines: Vec<String> = text
                .lines()
                .map(|line| text::ellipsize(line, &style, BAR_TIP_LINE))
                .collect();
            let lines: Vec<Line> = lines
                .iter()
                .map(|line| Line {
                    text: line,
                    style,
                    height: 20.0,
                })
                .collect();
            self.painted = card(&lines, scale).map(|painted| (text.to_string(), painted));
        }
        let (_, painted) = self.painted.as_ref()?;
        let m = (panel::NOTICE_MARGIN * DESIGN_PX) as f64;
        let w = painted.logical.w as f64 - 2.0 * m;
        let x = under_bar(under.loc.x, under.size.w, w, screen_w as f64);
        painted.element(
            renderer,
            Point::from((x - m, bar_h as f64 + GAP - m)),
            alpha,
        )
    }
}

/// Where a card `w` wide goes under something at `x`, `width` wide: centred under it, and kept
/// `GAP` clear of the screen's sides.
fn under_bar(x: f64, width: f64, w: f64, screen_w: f64) -> f64 {
    let centred = x + width / 2.0 - w / 2.0;
    centred.min(screen_w - GAP - w).max(GAP)
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
        assert_eq!(hover.over(), Some(&0));
        // Moving about on the same thing doesn't start the wait again.
        hover.point(Some(0), 340.0, start + REST / 2);
        assert_eq!(hover.showing(start + REST), Some(&0));
        // Over to the next: it waits again, and the first one's tip has gone.
        hover.point(Some(1), 340.0, start + REST);
        assert_eq!(hover.showing(start + REST), None);
        assert_eq!(hover.showing(start + REST * 2), Some(&1));
    }

    #[test]
    fn a_bar_tip_is_centred_under_its_button_and_kept_on_the_screen() {
        assert_eq!(under_bar(700.0, 100.0, 200.0, 1536.0), 650.0);
        assert_eq!(under_bar(1500.0, 30.0, 200.0, 1536.0), 1536.0 - GAP - 200.0);
        assert_eq!(under_bar(0.0, 30.0, 200.0, 1536.0), GAP);
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
