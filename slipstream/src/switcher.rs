//! Alt+Tab's switcher: every open window as a tile, most recently used first, with the selection
//! moving on each Tab while Alt is held.
//!
//! The selection is the only thing that changes while Alt is down: no window is focused, restored
//! or scrolled to, so passing over a fullscreen game's neighbours or a minimised window leaves
//! them alone. Letting go of Alt commits the choice; Esc leaves everything as it was. The card is
//! drawn only once Alt has been held a moment, so a quick Alt+Tab flips without anything flashing
//! up.

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream,
    paint::{self, Painted, Painter},
    panel::{self, MOCKUP_PX},
    state::{window_app_id, window_title},
    text::{self, Face, Style},
};

/// How long Alt must still be held after the first Tab before the card is drawn, in seconds.
pub const APPEAR: f64 = 0.12;
/// The card's fade in, once it appears.
const FADE: f64 = 0.12;
/// Tiles in a row at most; past that the row scrolls to keep the selection in view.
pub const ACROSS: usize = 7;

// The card's measurements, in mockup pixels.
const TILE_W: f32 = 200.0;
const TILE_H: f32 = 128.0;
const TILE_GAP: f32 = 10.0;
const PADDING: f32 = 24.0;
/// Room around the card for its shadow.
const MARGIN: f32 = 64.0;
const ICON: f32 = 48.0;
const INSET: f32 = 14.0;
/// The code rain's green, for the glyph that marks a minimised window.
const RAIN: u32 = 0x3cf0c0ff;

/// What letting go of Alt does.
#[derive(Debug, PartialEq)]
pub enum Commit<W> {
    /// Go to the window's workspace and focus it.
    Focus(W),
    /// Bring the window back out of the code rain.
    Restore(W),
}

/// How the switcher ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Alt let go, or a tile clicked: the selection is committed.
    Release,
    /// Esc with Alt held: nothing changes.
    Escape,
}

/// What one tile shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub app_id: Option<String>,
    pub name: String,
    pub title: String,
    /// The workspace's name or number, or `None` for a minimised window, which gets the rain glyph.
    pub workspace: Option<String>,
}

/// Everything the painted card depends on.
#[derive(Clone, PartialEq)]
struct Look {
    tiles: Vec<Tile>,
    selected: usize,
    scroll: usize,
    across: usize,
    screen: Size<i32, Logical>,
    scale: f64,
    ring: u32,
}

pub struct Switcher<W> {
    /// Most recently used first, as they were when Alt+Tab began.
    windows: Vec<W>,
    selected: usize,
    /// The first tile in view.
    scroll: usize,
    /// When the first Tab was pressed, on wall time.
    started: f64,
    reduced_motion: bool,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// Each tile in view, by index, where it was drawn: logical pixels on the screen.
    hits: Vec<(usize, Rectangle<f64, Logical>)>,
}

impl<W: Clone + PartialEq> Switcher<W> {
    /// The first Tab (or, going `forward` false, Shift+Tab) over `windows`, most recently used
    /// first: the selection starts on the next one along, or the last. Nothing to switch between
    /// with fewer than two.
    pub fn start(windows: Vec<W>, forward: bool, now: f64, reduced_motion: bool) -> Option<Self> {
        if windows.len() < 2 {
            return None;
        }
        let selected = if forward { 1 } else { windows.len() - 1 };
        Some(Self {
            windows,
            selected,
            scroll: 0,
            started: now,
            reduced_motion,
            shown: None,
            painted: None,
            hits: Vec::new(),
        })
    }

    /// Another Tab, or Shift+Tab: the selection moves one along, wrapping.
    pub fn step(&mut self, forward: bool) {
        let count = self.windows.len();
        self.selected = if forward {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
    }

    pub fn selected(&self) -> &W {
        &self.windows[self.selected]
    }

    pub fn windows(&self) -> &[W] {
        &self.windows
    }

    /// Whether Alt has been held long enough for the card to be drawn.
    pub fn visible(&self, now: f64) -> bool {
        now - self.started >= APPEAR
    }

    /// The tile under a point in logical pixels on the screen, if the card is showing it.
    pub fn tile_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        self.hits
            .iter()
            .find(|(_, area)| area.contains(pos))
            .map(|(index, _)| *index)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.windows.len() {
            self.selected = index;
        }
    }

    /// `window` has closed. The selection stays on the same window, or moves to the one that took
    /// its place. Whether anything is left to switch to.
    pub fn forget(&mut self, window: &W) -> bool {
        let Some(at) = self.windows.iter().position(|w| w == window) else {
            return true;
        };
        self.windows.remove(at);
        if at < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.windows.len().saturating_sub(1));
        !self.windows.is_empty()
    }

    /// The switcher ending: with Alt's release, what to do with the selection, told whether a
    /// window is minimised; with Esc, nothing at all.
    pub fn finish(self, end: End, minimised: impl Fn(&W) -> bool) -> Option<Commit<W>> {
        if end == End::Escape {
            return None;
        }
        let window = self.windows.into_iter().nth(self.selected)?;
        Some(if minimised(&window) {
            Commit::Restore(window)
        } else {
            Commit::Focus(window)
        })
    }
}

/// Tiles in a row on a screen `screen_w` logical pixels wide, for `count` windows.
fn across(screen_w: i32, count: usize) -> usize {
    let room = screen_w as f32 / MOCKUP_PX - 2.0 * PADDING - 40.0 + TILE_GAP;
    let fits = (room / (TILE_W + TILE_GAP)).floor().max(1.0) as usize;
    count.min(ACROSS).min(fits).max(1)
}

/// The first tile in view: `scroll` moved no further than it takes to show `selected`.
fn scroll_to_show(scroll: usize, selected: usize, count: usize, across: usize) -> usize {
    let scroll = if selected < scroll {
        selected
    } else if selected >= scroll + across {
        selected + 1 - across
    } else {
        scroll
    };
    scroll.min(count.saturating_sub(across))
}

impl<W> Switcher<W> {
    /// The card, centred on a screen `screen` big, with `tiles` for the windows in order. `icon`
    /// finds an app's icon at a size in device pixels, and is asked only when the card repaints.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        ring: u32,
        tiles: Vec<Tile>,
        icon: &mut dyn FnMut(&str, u32) -> Option<Pixmap>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let across = across(screen.w, tiles.len());
        self.scroll = scroll_to_show(self.scroll, self.selected, tiles.len(), across);
        let look = Look {
            tiles,
            selected: self.selected,
            scroll: self.scroll,
            across,
            screen,
            scale,
            ring,
        };
        if self.shown.as_ref() != Some(&look) {
            self.painted = paint(&look, icon);
            self.shown = Some(look);
        }
        let painted = self.painted.as_ref()?;
        let at = Point::<f64, Logical>::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        // Where each tile in view landed, for clicks.
        self.hits = (0..across)
            .map(|column| {
                let x = MARGIN + PADDING + column as f32 * (TILE_W + TILE_GAP);
                let y = MARGIN + PADDING;
                (
                    self.scroll + column,
                    Rectangle::new(
                        (at.x + (x * MOCKUP_PX) as f64, at.y + (y * MOCKUP_PX) as f64).into(),
                        ((TILE_W * MOCKUP_PX) as f64, (TILE_H * MOCKUP_PX) as f64).into(),
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

fn paint(look: &Look, icon: &mut dyn FnMut(&str, u32) -> Option<Pixmap>) -> Option<Painted> {
    let card_w = 2.0 * PADDING + look.across as f32 * TILE_W + (look.across - 1) as f32 * TILE_GAP;
    let card_h = 2.0 * PADDING + TILE_H;
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

    let name_style = Style::new(Face::Body, 14.0, panel::INK);
    let title_style = Style::new(Face::Body, 12.0, 0x8f98a8ff);
    let chip_style = Style::new(Face::Mono, 11.0, 0xc4cad6ff);
    let icon_px = (ICON * f).round() as u32;
    let visible = look
        .tiles
        .iter()
        .enumerate()
        .skip(look.scroll)
        .take(look.across);
    for (column, (index, tile)) in visible.enumerate() {
        let x = MARGIN + PADDING + column as f32 * (TILE_W + TILE_GAP);
        let y = MARGIN + PADDING;
        if index == look.selected {
            p.fill(x, y, TILE_W, TILE_H, 12.0, (look.ring & 0xffffff00) | 0x1c);
            panel::focus_ring(&mut p, x, y, TILE_W, TILE_H, 12.0, look.ring);
        } else {
            p.fill(x, y, TILE_W, TILE_H, 12.0, 0xffffff08);
        }
        match tile.app_id.as_deref().and_then(|id| icon(id, icon_px)) {
            Some(pixmap) => p.image(&pixmap, x + INSET, y + INSET),
            None => placeholder(&mut p, &tile.name, x + INSET, y + INSET),
        }
        // Top right: the workspace chip, or the rain glyph for a minimised window.
        let right = x + TILE_W - INSET;
        match &tile.workspace {
            Some(label) => {
                let label = text::ellipsize(label, &chip_style, 80.0);
                let w = text::width(&label, &chip_style) + 14.0;
                p.fill(right - w, y + INSET, w, 20.0, 5.0, 0xffffff12);
                p.text(&label, right - w + 7.0, y + INSET + 10.0, &chip_style);
            }
            None => {
                for (i, (dy, h)) in [(0.0, 13.0), (5.0, 9.0), (2.0, 12.0)]
                    .into_iter()
                    .enumerate()
                {
                    p.fill(
                        right - 14.0 + i as f32 * 6.0,
                        y + INSET + dy,
                        2.5,
                        h,
                        1.2,
                        RAIN,
                    );
                }
            }
        }
        let room = TILE_W - 2.0 * INSET;
        let name = text::ellipsize(&tile.name, &name_style, room);
        p.text(&name, x + INSET, y + 86.0, &name_style);
        let title = text::ellipsize(&tile.title, &title_style, room);
        p.text(&title, x + INSET, y + 107.0, &title_style);
    }
    Some(Painted {
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
        scale: look.scale,
    })
}

/// A coloured square with the app's initial, for an app with no icon.
fn placeholder(p: &mut Painter, name: &str, x: f32, y: f32) {
    const COLOURS: [u32; 6] = [
        0x3cf0c0ff, 0x33ccffff, 0xffb547ff, 0xff7a93ff, 0xa78bfaff, 0x7fe3ffff,
    ];
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    p.fill(
        x + 4.0,
        y + 4.0,
        ICON - 8.0,
        ICON - 8.0,
        11.0,
        COLOURS[hash as usize % 6],
    );
    let initial: String = name.chars().next().into_iter().collect();
    let style = Style::new(Face::MonoBold, 22.0, 0x10131aff);
    let w = text::width(&initial, &style);
    p.text(&initial, x + (ICON - w) / 2.0, y + ICON / 2.0, &style);
}

impl Slipstream {
    /// The switcher's card, once Alt has been held long enough, for a screen `screen` big.
    pub fn switcher_element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let now = self.wall();
        let mut switcher = self.switcher.take()?;
        let element = if switcher.visible(now) {
            let tiles = switcher
                .windows()
                .iter()
                .map(|window| Tile {
                    app_id: window_app_id(window),
                    name: self.stream_name(window),
                    title: window_title(window),
                    workspace: (!self.rain.contains(window)).then(|| {
                        self.workspaces
                            .find(window)
                            .map(|index| self.workspaces.label(index))
                            .unwrap_or_default()
                    }),
                })
                .collect();
            let ring = self.panel_ring();
            let explorer = &mut self.explorer;
            switcher.element(renderer, screen, scale, now, ring, tiles, &mut |id, px| {
                explorer.app_icon(id, px)
            })
        } else {
            None
        };
        self.switcher = Some(switcher);
        element
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Switcher<&'static str> {
        Switcher::start(vec!["a", "b", "c"], true, 10.0, false).unwrap()
    }

    #[test]
    fn a_quick_flip_draws_nothing() {
        let switcher = three();
        assert!(!switcher.visible(10.05), "released within 60 ms");
        assert!(switcher.visible(10.2), "held past the moment it takes");
        assert_eq!(
            switcher.finish(End::Release, |_| false),
            Some(Commit::Focus("b")),
            "the flip still lands on the previous window"
        );
        assert!(
            Switcher::start(vec!["a"], true, 0.0, false).is_none(),
            "nothing to switch to"
        );
    }

    #[test]
    fn escape_restores_nothing() {
        let mut switcher = three();
        switcher.step(true);
        assert_eq!(*switcher.selected(), "c");
        assert_eq!(switcher.finish(End::Escape, |w| *w == "c"), None);
    }

    #[test]
    fn a_minimised_window_is_restored_only_on_release() {
        let mut switcher = three();
        // Tabbing past the minimised "b" and on round changes nothing but the selection.
        switcher.step(true);
        switcher.step(true);
        switcher.step(false);
        assert_eq!(*switcher.selected(), "c");
        assert_eq!(
            three().finish(End::Release, |w| *w == "b"),
            Some(Commit::Restore("b"))
        );
        assert_eq!(
            switcher.finish(End::Release, |w| *w == "b"),
            Some(Commit::Focus("c"))
        );
        let backwards = Switcher::start(vec!["a", "b", "c"], false, 0.0, false).unwrap();
        assert_eq!(
            *backwards.selected(),
            "c",
            "Shift+Tab starts at the far end"
        );
    }

    #[test]
    fn seven_across_then_scrolls() {
        assert_eq!(across(1536, 9), ACROSS);
        assert_eq!(across(1536, 3), 3);
        assert!(across(1024, 9) < ACROSS, "a narrow screen takes fewer");
        let mut scroll = 0;
        for selected in 0..ACROSS {
            scroll = scroll_to_show(scroll, selected, 9, ACROSS);
            assert_eq!(scroll, 0);
        }
        scroll = scroll_to_show(scroll, 7, 9, ACROSS);
        assert_eq!(scroll, 1);
        scroll = scroll_to_show(scroll, 8, 9, ACROSS);
        assert_eq!(scroll, 2);
        scroll = scroll_to_show(scroll, 4, 9, ACROSS);
        assert_eq!(scroll, 2, "still in view");
        assert_eq!(scroll_to_show(scroll, 0, 9, ACROSS), 0, "wrapping round");
    }

    #[test]
    fn a_closed_window_leaves_the_selection_where_it_was() {
        let mut switcher = three();
        switcher.step(true);
        assert!(switcher.forget(&"a"));
        assert_eq!(*switcher.selected(), "c");
        assert!(switcher.forget(&"c"));
        assert_eq!(
            *switcher.selected(),
            "b",
            "the selected one gone: the one before"
        );
        assert!(!switcher.forget(&"b"));
    }
}
