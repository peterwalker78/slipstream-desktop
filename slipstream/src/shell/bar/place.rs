//! Where the bar sits on its screen. Everything that keeps clear of the bar or hangs from it reads
//! it from here: the tiling area and so maximised windows, the docked pane and its frame, the
//! panels that drop from under it, the streams down the side, and the bar's own click targets.

use slipstream_config::BarStyle;

use super::HEIGHT;
use crate::{
    layout::{OUTER_GAP, Rect},
    panel,
};

/// How far a floating strip sits below the screen's top edge, in logical pixels.
pub const STRIP_TOP: i32 = 8;
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

    /// How far in from each side the bar's ends are: a floating strip's ends line up with the
    /// windows' outer edges below it.
    pub fn side(self) -> i32 {
        if self.floating { OUTER_GAP } else { 0 }
    }

    /// How far below the screen's top edge the bar starts.
    pub fn top(self) -> i32 {
        if self.floating { STRIP_TOP } else { 0 }
    }

    /// Where the bar ends going down, from the screen's top: windows tile from here, keeping
    /// their outer gap, and whatever hangs from the bar starts here.
    pub fn foot(self) -> i32 {
        self.top() + HEIGHT
    }

    /// How much lower the foot is than the bar along the edge has it.
    pub fn drop(self) -> i32 {
        self.foot() - HEIGHT
    }

    /// The bar's box on a screen `width` wide, from the screen's top-left corner.
    pub fn rect(self, width: i32) -> Rect {
        Rect {
            x: self.side(),
            y: self.top(),
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

    /// The bar's material, its fill and its edge, which the docked pane's frame carries on down.
    /// A floating strip is the panels' glass.
    pub fn material(self) -> (u32, u32) {
        if self.floating {
            (panel::GLASS, panel::GLASS_EDGE)
        } else {
            (panel::CHIP | 0xe6, panel::DIVIDER)
        }
    }
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
    fn windows_tile_a_gap_below_the_strip_and_line_up_with_its_ends() {
        let strip = Place::of(BarStyle::FloatingStrip);
        let bar = strip.rect(SCREEN.w);
        let area = strip.below(SCREEN);
        assert_eq!((bar.x, bar.y, bar.h), (OUTER_GAP, STRIP_TOP, HEIGHT));
        assert_eq!(
            area.y - SCREEN.y,
            bar.y + bar.h,
            "the area starts at the strip's foot"
        );
        // The windows keep their outer gap inside the area.
        let windows = area.inset(OUTER_GAP);
        assert_eq!(windows.y - SCREEN.y, bar.y + bar.h + OUTER_GAP);
        assert_eq!(windows.x - SCREEN.x, bar.x, "the left ends line up");
        assert_eq!(
            windows.x + windows.w - SCREEN.x,
            bar.x + bar.w,
            "and the right ones"
        );
        assert_eq!(area.y + area.h, SCREEN.y + SCREEN.h);
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
