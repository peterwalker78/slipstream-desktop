//! What a stream says when the pointer rests on it (a tip, `tip.rs`): the app's name, and the
//! memory, processor time and network traffic behind its brightness, speed and rain, as numbers.
//! It sits just left of its stream at the pointer's height, and goes when the pointer leaves, the
//! stream goes, or the rain does.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::Point,
};

use crate::{
    layout::Rect,
    paint,
    panel::{self, DESIGN_PX},
    text::{Face, Style},
    tip::{self, Hover, Line},
    usage::Figures,
};

/// The card's two lines' heights, in design pixels.
const NAME_LINE: f32 = 22.0;
const FIGURES_LINE: f32 = 20.0;

/// Between the card and its stream, and the least room kept from the screen's edges, in logical
/// pixels.
const GAP: f64 = 8.0;

const MIB: f64 = 1024.0 * 1024.0;

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

    /// The card for `name`'s stream in `column`, saying what `figures` it is using, below `top`
    /// on `screen`. It is painted again only when what it says changes, which is at most once a
    /// second.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        (name, figures): (&str, Option<Figures>),
        column: Rect,
        (screen, top): (Rect, i32),
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
    let name = Line {
        text: name,
        style: Style::new(Face::BodyBold, 15.0, panel::BRIGHT),
        height: NAME_LINE,
    };
    let figures = line.map(|line| Line {
        text: line,
        style: Style {
            tabular: true,
            ..Style::new(Face::Mono, 13.0, panel::PROSE)
        },
        height: FIGURES_LINE,
    });
    tip::card(
        &[Some(name), figures]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        scale,
    )
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
