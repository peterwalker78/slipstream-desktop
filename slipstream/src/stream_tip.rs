//! What a stream says when the pointer rests on it: the app's name, and the memory, processor time
//! and network traffic behind its brightness, speed and rain, as numbers. It waits for the
//! pointer to settle on one stream, so sweeping across them shows nothing, and sits just left of
//! its stream at the pointer's height. It goes when the pointer leaves, the stream goes, or the
//! rain does.
//!
//! It appears and goes at once, with nothing animated, so reduced motion has nothing to change.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Size},
};

use crate::{
    layout::Rect,
    paint,
    panel::{self, DESIGN_PX},
    text::{self, Face, Style},
    usage::Figures,
};

/// How long the pointer stays on a stream before its tip shows.
pub const REST: Duration = Duration::from_millis(500);

/// The card's padding and its two lines' heights, in design pixels.
const PAD: f32 = 14.0;
const NAME_LINE: f32 = 22.0;
const FIGURES_LINE: f32 = 20.0;

/// Between the card and its stream, and the least room kept from the screen's edges, in logical
/// pixels.
const GAP: f64 = 8.0;

const MIB: f64 = 1024.0 * 1024.0;

/// Which stream the pointer is on, since when, and at what height.
pub struct Hover<K> {
    over: Option<(K, Instant)>,
    y: f64,
}

impl<K: PartialEq> Hover<K> {
    pub fn new() -> Self {
        Self { over: None, y: 0.0 }
    }

    /// The pointer is on `over`'s stream, or on none, `y` logical pixels down the screen. Moving
    /// along the same stream keeps its wait, or its tip, and the tip follows the pointer down.
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

    /// The rain went: the pointer has to come back for a tip to show again.
    pub fn leave(&mut self) {
        self.over = None;
    }

    /// The stream whose tip shows at `at`: the one the pointer has been on for `REST`.
    pub fn showing(&self, at: Instant) -> Option<&K> {
        self.over
            .as_ref()
            .filter(|(_, since)| at.saturating_duration_since(*since) >= REST)
            .map(|(on, _)| on)
    }
}

/// The hover, and the card last painted for it.
pub struct Tip<K> {
    pub hover: Hover<K>,
    painted: Option<(String, Option<String>, paint::Painted)>,
}

impl<K: PartialEq> Tip<K> {
    pub fn new() -> Self {
        Self {
            hover: Hover::new(),
            painted: None,
        }
    }

    /// The card for `name`'s stream in `column`, below `top` on `screen`. It is painted again
    /// only when what it says changes, which is at most once a second.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        name: &str,
        figures: Option<Figures>,
        column: Rect,
        screen: Rect,
        top: i32,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let line = figures.map(describe);
        let stale = self.painted.as_ref().is_none_or(|(was, said, painted)| {
            was != name || *said != line || painted.scale != scale
        });
        if stale {
            self.painted = paint_card(name, line.as_deref(), scale)
                .map(|painted| (name.to_string(), line, painted));
        }
        let (.., painted) = self.painted.as_ref()?;
        let m = (panel::NOTICE_MARGIN * DESIGN_PX) as f64;
        let card = (
            painted.logical.w as f64 - 2.0 * m,
            painted.logical.h as f64 - 2.0 * m,
        );
        let (x, y) = place(column, screen, top, self.hover.y, card);
        painted.element(renderer, Point::from((x - m, y - m)), alpha)
    }
}

/// Where a card `size` logical pixels big goes for the stream in `column`, with the pointer `y`
/// down the screen: just left of the stream, centred on the pointer's height, and kept below
/// `top` and clear of the screen's foot and left edge.
fn place(column: Rect, screen: Rect, top: i32, y: f64, size: (f64, f64)) -> (f64, f64) {
    let (w, h) = size;
    let x = (column.x as f64 - GAP - w).max(screen.x as f64 + GAP);
    let highest = (screen.y + top) as f64 + GAP;
    let lowest = ((screen.y + screen.h) as f64 - GAP - h).max(highest);
    (x, (y - h / 2.0).clamp(highest, lowest))
}

/// What an app is using, as a line: "312 MB memory · 4% CPU · 1.2 MB/s down · 40 KB/s up".
fn describe(figures: Figures) -> String {
    format!(
        "{} memory · {}% CPU · {} down · {} up",
        memory(figures.memory),
        figures.cpu.clamp(0.0, 100.0).round() as u32,
        rate(figures.received),
        rate(figures.sent),
    )
}

/// "312 MB", or "1.4 GB" from a gigabyte up.
fn memory(bytes: u64) -> String {
    let mb = bytes as f64 / MIB;
    if mb.round() >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{} MB", mb.round() as u64)
    }
}

/// "40 KB/s", or "1.2 MB/s" from a megabyte a second up.
fn rate(bytes_a_second: f32) -> String {
    let kb = bytes_a_second.max(0.0) as f64 / 1024.0;
    if kb.round() >= 1024.0 {
        format!("{:.1} MB/s", kb / 1024.0)
    } else {
        format!("{} KB/s", kb.round() as u64)
    }
}

/// The card: the app's name, and under it what it is using when that has been measured. A
/// notice's solid card, since it is read at a glance over the rain.
fn paint_card(name: &str, line: Option<&str>, scale: f64) -> Option<paint::Painted> {
    let name_style = Style::new(Face::BodyBold, 15.0, panel::BRIGHT);
    let figures_style = Style {
        tabular: true,
        ..Style::new(Face::Mono, 13.0, panel::PROSE)
    };
    let width = text::width(name, &name_style)
        .max(line.map_or(0.0, |line| text::width(line, &figures_style)))
        + 2.0 * PAD;
    let height = 2.0 * PAD + NAME_LINE + if line.is_some() { FIGURES_LINE } else { 0.0 };
    let m = panel::NOTICE_MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((width + 2.0 * m) * DESIGN_PX).ceil() as i32,
        ((height + 2.0 * m) * DESIGN_PX).ceil() as i32,
    ));
    paint::Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        panel::notice(p, m, m, width, height);
        let (x, y) = (m + PAD, m + PAD);
        p.text(name, x, y + NAME_LINE / 2.0, &name_style);
        if let Some(line) = line {
            p.text(line, x, y + NAME_LINE + FIGURES_LINE / 2.0, &figures_style);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rain::{Rain, stream_hit};

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 1536,
        h: 960,
    };
    const TOP: i32 = 40;

    #[test]
    fn a_tip_waits_for_the_pointer_to_rest_on_one_stream() {
        let start = Instant::now();
        let mut hover = Hover::new();
        hover.point(Some(0), 300.0, start);
        assert_eq!(hover.showing(start + REST / 2), None, "only passing over");
        // Moving along the same stream doesn't start the wait again.
        hover.point(Some(0), 340.0, start + REST / 2);
        assert_eq!(hover.showing(start + REST), Some(&0));
        // Over to the next stream: it waits again, and the first one's tip has gone.
        hover.point(Some(1), 340.0, start + REST);
        assert_eq!(hover.showing(start + REST), None);
        assert_eq!(hover.showing(start + REST * 2), Some(&1));
    }

    #[test]
    fn a_tip_goes_when_the_pointer_leaves_or_the_rain_hides() {
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

    #[test]
    fn the_streams_and_only_the_streams_bring_up_a_tip() {
        let first = Rain::column(0, SCREEN, TOP);
        let second = Rain::column(1, SCREEN, TOP);
        let y = 500.0;
        assert_eq!(stream_hit(2, first.x as f64 + 5.0, y, SCREEN, TOP), Some(0));
        assert_eq!(
            stream_hit(2, second.x as f64 + 5.0, y, SCREEN, TOP),
            Some(1)
        );
        // The gap between them, the bar above them, and the tiles to their left are not.
        let gap = (second.x + second.w) as f64 + 1.0;
        assert!(gap < first.x as f64);
        assert_eq!(stream_hit(2, gap, y, SCREEN, TOP), None);
        assert_eq!(stream_hit(2, first.x as f64 + 5.0, 10.0, SCREEN, TOP), None);
        assert_eq!(stream_hit(2, second.x as f64 - 20.0, y, SCREEN, TOP), None);
    }

    #[test]
    fn the_tip_sits_left_of_its_stream_at_the_pointer_s_height() {
        let column = Rain::column(1, SCREEN, TOP);
        let (x, y) = place(column, SCREEN, TOP, 500.0, (300.0, 60.0));
        assert_eq!(x, column.x as f64 - GAP - 300.0);
        assert_eq!(y, 470.0);
        // Never under the bar, nor off the foot of the screen.
        let (_, y) = place(column, SCREEN, TOP, TOP as f64 + 2.0, (300.0, 60.0));
        assert_eq!(y, TOP as f64 + GAP);
        let (_, y) = place(column, SCREEN, TOP, 958.0, (300.0, 60.0));
        assert_eq!(y, 960.0 - GAP - 60.0);
        // Following its stream as the streams shift.
        let (moved, _) = place(
            Rain::column(0, SCREEN, TOP),
            SCREEN,
            TOP,
            500.0,
            (300.0, 60.0),
        );
        assert!(moved > x);
    }

    #[test]
    fn the_numbers_read_as_a_person_would_say_them() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(memory(312 * MB), "312 MB");
        assert_eq!(memory(MB / 3), "0 MB");
        assert_eq!(memory(1023 * MB + MB * 3 / 4), "1.0 GB", "not 1024 MB");
        assert_eq!(memory(1434 * MB), "1.4 GB");
        assert_eq!(rate(0.0), "0 KB/s");
        assert_eq!(rate(40.0 * 1024.0), "40 KB/s");
        assert_eq!(rate(1.2 * 1024.0 * 1024.0), "1.2 MB/s");
        let figures = Figures {
            memory: 312 * MB,
            cpu: 3.6,
            sent: 40.0 * 1024.0,
            received: 1.2 * 1024.0 * 1024.0,
        };
        assert_eq!(
            describe(figures),
            "312 MB memory · 4% CPU · 1.2 MB/s down · 40 KB/s up"
        );
    }
}
