//! Where the bar sits on its screen. Everything that keeps clear of the bar or hangs from it reads
//! it from here: the tiling area and so maximised windows, the docked pane and its frame, the
//! panels that drop from under it, the streams down the side, and the bar's own click targets.

use slipstream_config::BarStyle;

use super::HEIGHT;
use crate::{
    layout::{OUTER_GAP, Rect},
    panel,
};

/// Every size here is in logical pixels.
///
/// How far a floating strip's top edge sits below the screen's top edge.
pub const STRIP_TOP: i32 = 7;
/// How far in from the screen's sides a floating strip's ends are: a pixel outside the windows'
/// outer edges below it.
const STRIP_SIDE: i32 = OUTER_GAP - 1;
/// How far inside the bar's picture a floating strip is drawn, top and bottom: it is that much
/// thinner than the bar along the edge, with what's on it still centred.
const STRIP_INSET: i32 = 1;
/// How round a floating strip's corners are, in logical pixels.
pub const STRIP_RADIUS: f32 = 8.0;

/// The bar along the screen's top edge, or a strip floating clear of the edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Place {
    pub floating: bool,
}

impl Place {
    pub fn of(style: BarStyle) -> Self {
        Self {
            floating: style == BarStyle::FloatingStrip,
        }
    }

    /// How far in from each side the bar's ends are.
    pub fn side(self) -> i32 {
        if self.floating { STRIP_SIDE } else { 0 }
    }

    /// Where the bar's visible top edge is, below the screen's top edge.
    pub fn top(self) -> i32 {
        if self.floating { STRIP_TOP } else { 0 }
    }

    /// How far inside its picture the bar is drawn, top and bottom.
    pub fn inset(self) -> i32 {
        if self.floating { STRIP_INSET } else { 0 }
    }

    /// Where the bar's visible bottom edge is, from the screen's top: windows tile from here,
    /// keeping their outer gap, and whatever hangs from the bar starts here.
    pub fn foot(self) -> i32 {
        self.top() + HEIGHT - 2 * self.inset()
    }

    /// How much lower the foot is than the bar along the edge has it.
    pub fn drop(self) -> i32 {
        self.foot() - HEIGHT
    }

    /// The box the bar's picture fills on a screen `width` wide, from the screen's top-left
    /// corner: the bar itself, and around a floating strip its inset.
    pub fn rect(self, width: i32) -> Rect {
        Rect {
            x: self.side(),
            y: self.top() - self.inset(),
            w: (width - 2 * self.side()).max(1),
            h: HEIGHT,
        }
    }

    /// The part of `screen` below the bar, where windows tile.
    pub fn below(self, screen: Rect) -> Rect {
        Rect {
            y: screen.y + self.foot(),
            h: (screen.h - self.foot()).max(1),
            ..screen
        }
    }

    /// The bar's material, which the docked pane's frame carries on down. A floating strip is
    /// the panels' glass.
    pub fn material(self) -> Material {
        if self.floating {
            Material {
                fill: panel::GLASS,
                edge: panel::GLASS_EDGE,
            }
        } else {
            Material {
                fill: EDGE_FILL,
                edge: panel::DIVIDER,
            }
        }
    }
}

/// The fill of the bar along the screen's edge: a near-black deeper than the dark chips', nearly
/// opaque.
const EDGE_FILL: u32 = 0x07080beb;

/// What the bar is made of, as 0xRRGGBBAA colours: its fill, and its edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Material {
    pub fill: u32,
    pub edge: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 1536,
        y: 0,
        w: 1536,
        h: 960,
    };

    #[test]
    fn a_floating_strip_is_thinner_and_a_pixel_wider_than_the_windows_below_it() {
        let strip = Place::of(BarStyle::FloatingStrip);
        let bar = strip.rect(SCREEN.w);
        let windows = strip.below(SCREEN).inset(OUTER_GAP);
        assert_eq!(strip.foot() - strip.top(), HEIGHT - 2, "two pixels thinner");
        assert_eq!(
            windows.x - SCREEN.x,
            bar.x + 1,
            "a pixel past the left ends"
        );
        assert_eq!(windows.x + windows.w - SCREEN.x, bar.x + bar.w - 1);
        // Its picture keeps the bar's height, so what's on it is laid out the same, centred.
        assert_eq!(bar.h, HEIGHT);
        assert_eq!(strip.top() - bar.y, bar.y + bar.h - strip.foot());
    }

    /// The visible bottom edge of the bar in `place` on a screen at `scale`, in device pixels:
    /// where its picture's edge lands, as it is drawn.
    fn visible_foot(place: Place, scale: f64) -> i64 {
        (place.foot() as f64 * scale).round() as i64
    }

    #[test]
    fn windows_keep_the_same_gap_below_the_bar_in_every_style_and_scale() {
        for style in [BarStyle::Plain, BarStyle::LitEdge, BarStyle::FloatingStrip] {
            let place = Place::of(style);
            for scale in [1.0, 1.25, 1.5] {
                let windows = place.below(SCREEN).inset(OUTER_GAP);
                let top = ((windows.y - SCREEN.y) as f64 * scale).round() as i64;
                assert_eq!(
                    top - visible_foot(place, scale),
                    (OUTER_GAP as f64 * scale).round() as i64,
                    "{style:?} at {scale}"
                );
                assert_eq!(windows.y - SCREEN.y - place.foot(), OUTER_GAP);
            }
        }
    }

    #[test]
    fn the_bar_along_the_edge_is_where_it_always_was() {
        for style in [BarStyle::Plain, BarStyle::LitEdge] {
            let place = Place::of(style);
            assert_eq!(
                place.rect(1536),
                Rect {
                    x: 0,
                    y: 0,
                    w: 1536,
                    h: HEIGHT
                }
            );
            assert_eq!(place.foot(), HEIGHT);
            assert_eq!(place.drop(), 0);
            assert_eq!(place.below(SCREEN).y, HEIGHT);
        }
    }
}
