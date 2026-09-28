//! Super+T held: gravity's arrangements side by side, each drawn from the workspace's own windows.
//!
//! A quick Super+T turns gravity on or off. Keep Super down and a strip of the arrangements
//! appears — tiling, grid, centre, wide, spotlight — with the one in force selected. Each further
//! T, or ← and →, moves along it, and the workspace takes that arrangement there and then, so
//! what the strip points at is what the screen shows. Letting go of Super keeps it; Esc puts back
//! what was there before Super+T.
//!
//! Like Alt+Tab's switcher, the strip is drawn only once Super has been held a moment, so a quick
//! press flips gravity without anything flashing up.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    gravity::Rung,
    layout::Rect,
    paint::{self, Painted, Painter},
    panel::{self, MOCKUP_PX},
    text::{self, Face, Style},
};

/// The arrangements on the strip, in order.
pub const CHOICES: [Rung; 5] = [
    Rung::Tiling,
    Rung::Grid,
    Rung::Centre,
    Rung::Wide,
    Rung::Spotlight,
];

/// How long Super must still be held after Super+T before the strip is drawn, in seconds.
pub const APPEAR: f64 = 0.2;
/// The strip's fade in, once it appears.
const FADE: f64 = 0.12;

// The strip's measurements, in mockup pixels.
const TILE_W: f32 = 168.0;
const TILE_GAP: f32 = 10.0;
const INSET: f32 = 14.0;
const LABEL_H: f32 = 34.0;
const PADDING: f32 = 24.0;
const FOOT_H: f32 = 40.0;
/// Room around the strip for its shadow.
const MARGIN: f32 = 64.0;
/// Its lower edge above the bottom of the screen.
const BOTTOM: f32 = 96.0;

/// One window in a picture of an arrangement: where it sits, as fractions of the tiling area,
/// and whether it's the one the arrangement is centred on.
pub type Pane = ([f32; 4], bool);

/// Super+T while it's held.
pub struct Arrange<W> {
    selected: usize,
    /// When Super+T was pressed, on wall time.
    started: f64,
    /// The window the arrangements are centred on, and its workspace.
    pub window: W,
    pub workspace: usize,
    /// What Esc puts back on the workspace: its gravity, and its maximised window.
    pub before: (crate::gravity::Gravity<W>, Option<W>),
    reduced_motion: bool,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// Each tile, by index, where it was drawn: logical pixels on the screen.
    hits: Vec<(usize, Rectangle<f64, Logical>)>,
}

impl<W> Arrange<W> {
    /// Super+T, with `selected` already in force.
    pub fn start(
        selected: Rung,
        window: W,
        workspace: usize,
        before: (crate::gravity::Gravity<W>, Option<W>),
        now: f64,
        reduced_motion: bool,
    ) -> Self {
        Self {
            selected: CHOICES.iter().position(|r| *r == selected).unwrap_or(0),
            started: now,
            window,
            workspace,
            before,
            reduced_motion,
            shown: None,
            painted: None,
            hits: Vec::new(),
        }
    }

    /// Another T, or an arrow: the selection moves one along, wrapping. Returns the new choice.
    pub fn step(&mut self, forward: bool) -> Rung {
        let count = CHOICES.len();
        self.selected = if forward {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
        CHOICES[self.selected]
    }

    /// A tile clicked: that arrangement, if there is one at `index`.
    pub fn select(&mut self, index: usize) -> Option<Rung> {
        let rung = *CHOICES.get(index)?;
        self.selected = index;
        Some(rung)
    }

    /// Whether Super has been held long enough for the strip to be drawn.
    pub fn visible(&self, now: f64) -> bool {
        now - self.started >= APPEAR
    }

    /// The tile under a point in logical pixels on the screen, if the strip is showing it.
    pub fn tile_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        self.hits
            .iter()
            .find(|(_, area)| area.contains(pos))
            .map(|(index, _)| *index)
    }

    /// The strip, centred near the bottom of a screen `screen` big, with a picture of each
    /// arrangement in `pictures`, in the order of `CHOICES`.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        ring: u32,
        pictures: Vec<Vec<Pane>>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let look = Look {
            pictures,
            selected: self.selected,
            aspect: screen.h as f32 / screen.w.max(1) as f32,
            scale,
            ring,
        };
        if self.shown.as_ref() != Some(&look) {
            self.painted = paint(&look);
            self.shown = Some(look);
        }
        let painted = self.painted.as_ref()?;
        let at = Point::<f64, Logical>::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            (screen.h - painted.logical.h + ((MARGIN - BOTTOM) * MOCKUP_PX) as i32).max(0) as f64,
        ));
        let tile_h = self.shown.as_ref().map_or(0.0, tile_h);
        self.hits = (0..CHOICES.len())
            .map(|index| {
                let x = MARGIN + PADDING + index as f32 * (TILE_W + TILE_GAP);
                let y = MARGIN + PADDING;
                (
                    index,
                    Rectangle::new(
                        (at.x + (x * MOCKUP_PX) as f64, at.y + (y * MOCKUP_PX) as f64).into(),
                        ((TILE_W * MOCKUP_PX) as f64, (tile_h * MOCKUP_PX) as f64).into(),
                    ),
                )
            })
            .collect();
        let alpha = if self.reduced_motion {
            1.0
        } else {
            ((now - self.started - APPEAR) / FADE).clamp(0.0, 1.0) as f32
        };
        painted.element(renderer, at, alpha)
    }
}

/// Everything the strip's picture depends on.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    pictures: Vec<Vec<Pane>>,
    selected: usize,
    /// The screen's height over its width, which each picture keeps.
    aspect: f32,
    scale: f64,
    ring: u32,
}

/// A tile's height: the picture at the screen's shape, and the name under it.
fn tile_h(look: &Look) -> f32 {
    INSET + picture_h(look) + LABEL_H
}

fn picture_h(look: &Look) -> f32 {
    ((TILE_W - 2.0 * INSET) * look.aspect).clamp(60.0, 140.0)
}

fn name(rung: Rung) -> &'static str {
    match rung {
        Rung::Tiling => "Tiling",
        Rung::Grid => "Grid",
        Rung::Centre => "Centre",
        Rung::Wide => "Wide",
        Rung::Spotlight => "Spotlight",
        Rung::Orbit => "Orbit",
        Rung::Distant => "Distant",
    }
}

fn paint(look: &Look) -> Option<Painted> {
    let count = CHOICES.len() as f32;
    let card_w = 2.0 * PADDING + count * TILE_W + (count - 1.0) * TILE_GAP;
    let tile_h = tile_h(look);
    let card_h = 2.0 * PADDING + tile_h + FOOT_H;
    let logical = Size::<i32, Logical>::from((
        ((card_w + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
        ((card_h + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * look.scale).round().max(1.0) as i32,
        (logical.h as f64 * look.scale).round().max(1.0) as i32,
    );
    let f = look.scale as f32 * MOCKUP_PX;
    let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;
    panel::glass(&mut p, MARGIN, MARGIN, card_w, card_h);

    let label = Style::new(Face::Body, 14.0, panel::INK);
    let (pw, ph) = (TILE_W - 2.0 * INSET, picture_h(look));
    for (index, rung) in CHOICES.iter().enumerate() {
        let x = MARGIN + PADDING + index as f32 * (TILE_W + TILE_GAP);
        let y = MARGIN + PADDING;
        if index == look.selected {
            p.fill(x, y, TILE_W, tile_h, 12.0, (look.ring & 0xffffff00) | 0x1c);
            panel::focus_ring(&mut p, x, y, TILE_W, tile_h, 12.0, look.ring);
        } else {
            p.fill(x, y, TILE_W, tile_h, 12.0, 0xffffff08);
        }
        let (px, py) = (x + INSET, y + INSET);
        for &([nx, ny, nw, nh], centre) in look.pictures.get(index).into_iter().flatten() {
            // A hair of gap between panes, whatever the real gaps come to at this size.
            let (wx, wy) = (px + nx * pw + 1.0, py + ny * ph + 1.0);
            let (ww, wh) = ((nw * pw - 2.0).max(2.0), (nh * ph - 2.0).max(2.0));
            let fill = if centre {
                (look.ring & 0xffffff00) | 0x99
            } else {
                0xffffff26
            };
            p.fill(wx, wy, ww, wh, 3.0, fill);
        }
        let text = name(*rung);
        let w = text::width(text, &label);
        p.text(
            text,
            x + (TILE_W - w) / 2.0,
            y + INSET + ph + LABEL_H / 2.0,
            &label,
        );
    }

    // The keys, right-aligned along the foot, as on every panel.
    let right = MARGIN + card_w - PADDING;
    let centre = MARGIN + PADDING + tile_h + FOOT_H / 2.0 + 4.0;
    let start = panel::key_hint(&mut p, right, centre, &["Esc"], "put it back");
    let start = panel::key_hint(&mut p, start - 20.0, centre, &["Super"], "let go to keep");
    panel::key_hint(&mut p, start - 20.0, centre, &["T", "←", "→"], "choose");

    Some(Painted {
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
        scale: look.scale,
    })
}

/// A picture of `rects` in `area`, as fractions of it, with `centre` marked.
pub fn picture<W: PartialEq>(rects: &[(W, Rect)], area: Rect, centre: Option<&W>) -> Vec<Pane> {
    let (w, h) = (area.w.max(1) as f32, area.h.max(1) as f32);
    rects
        .iter()
        .map(|(window, rect)| {
            (
                [
                    (rect.x - area.x) as f32 / w,
                    (rect.y - area.y) as f32 / h,
                    rect.w as f32 / w,
                    rect.h as f32 / h,
                ],
                centre == Some(window),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gravity::Gravity;

    fn arrange() -> Arrange<&'static str> {
        Arrange::start(Rung::Centre, "a", 0, (Gravity::default(), None), 0.0, false)
    }

    #[test]
    fn tapping_walks_the_strip_and_wraps() {
        let mut arrange = arrange();
        assert_eq!(arrange.step(true), Rung::Wide);
        assert_eq!(arrange.step(true), Rung::Spotlight);
        assert_eq!(arrange.step(true), Rung::Tiling);
        assert_eq!(arrange.step(false), Rung::Spotlight);
    }

    #[test]
    fn nothing_shows_for_a_quick_press() {
        let arrange = arrange();
        assert!(!arrange.visible(APPEAR / 2.0));
        assert!(arrange.visible(APPEAR));
    }

    #[test]
    fn a_picture_is_in_fractions_of_the_area() {
        let area = Rect {
            x: 100,
            y: 50,
            w: 1000,
            h: 500,
        };
        let rects = [(
            "a",
            Rect {
                x: 600,
                y: 50,
                w: 500,
                h: 250,
            },
        )];
        assert_eq!(
            picture(&rects, area, Some(&"a")),
            vec![([0.5, 0.0, 0.5, 0.5], true)]
        );
    }
}
