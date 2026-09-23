//! The tour: what tiling is, in six moves, each one drawn happening.
//!
//! A person coming from Windows doesn't get stuck because there are too many keys. They get
//! stuck because the desktop does something they didn't ask for — a second window opens and the
//! first one shrinks — and nothing on screen says why. Telling them the keys doesn't fix that;
//! showing them the screen rearranging itself does.
//!
//! So each lesson is a little stage of abstract tiles that moves on a loop while you read it,
//! and you step through with Enter. Abstract on purpose: real windows would mean driving the
//! desktop about while somebody is trying to learn it, and a tour that rearranges your actual
//! work is a tour nobody runs twice.
//!
//! Pure geometry, in a unit square, so what the tiles do is decided here and tested without a
//! compositor; `sheet.rs` paints it.

/// One lesson.
pub struct Lesson {
    pub title: &'static str,
    /// The keys it teaches, drawn as keycaps.
    pub keys: &'static [&'static str],
    pub says: &'static str,
}

pub const LESSONS: [Lesson; 6] = [
    Lesson {
        title: "Windows share the screen",
        keys: &["Super+Return"],
        says: "Open a second window and the first makes room for it. Nothing is hidden behind \
               anything else, so there is never a window you have to go looking for.",
    },
    Lesson {
        title: "Move between them",
        keys: &["Super+← → ↑ ↓"],
        says: "The keyboard goes to the window in that direction. The ring shows where it is.",
    },
    Lesson {
        title: "Move the window itself",
        keys: &["Super+Alt+← → ↑ ↓"],
        says: "The same keys with Alt held take the window with you: it swaps places with its \
               neighbour.",
    },
    Lesson {
        title: "Turn the layout",
        keys: &["Super+R"],
        says: "Rows become columns. Press it again and the layout goes back exactly as it was.",
    },
    Lesson {
        title: "Out of the way",
        keys: &["Super+M", "Super+H"],
        says: "One window into the code rain, or all of them at once. They keep running, and the \
               same keys bring them back.",
    },
    Lesson {
        title: "That's the shape of it",
        keys: &["Super+/"],
        says: "Every other key is one press away, and this tour is the first row of that list.",
    },
];

/// How long one lesson's animation takes before it repeats.
pub const LOOP: f64 = 3.2;
/// The share of the loop spent still, at each end, so a move reads as a move.
const REST: f32 = 0.3;

/// A tile on the stage, in a unit square: 0,0 top left to 1,1 bottom right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0 where it has gone into the code rain.
    pub alpha: f32,
}

/// What the stage shows at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub tiles: Vec<Tile>,
    /// Which tile the ring is around, if any.
    pub focus: Option<usize>,
    /// How much of the code-rain strip is showing, 0 to 1.
    pub rain: f32,
}

/// Eases a loop's progress into a move that rests at both ends: still, moves, still.
fn move_along(t: f32) -> f32 {
    let t = ((t - REST) / (1.0 - 2.0 * REST)).clamp(0.0, 1.0);
    // Smoothstep, so it sets off and arrives gently rather than snapping.
    t * t * (3.0 - 2.0 * t)
}

/// Between two numbers.
fn between(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// Between two tiles, corner by corner.
fn tween(from: Tile, to: Tile, t: f32) -> Tile {
    Tile {
        x: between(from.x, to.x, t),
        y: between(from.y, to.y, t),
        w: between(from.w, to.w, t),
        h: between(from.h, to.h, t),
        alpha: between(from.alpha, to.alpha, t),
    }
}

/// A tile filling the whole stage.
fn whole() -> Tile {
    Tile {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        alpha: 1.0,
    }
}

/// The left and right halves, with a gap between them like the real tiling's.
fn halves() -> (Tile, Tile) {
    const GAP: f32 = 0.02;
    let w = 0.5 - GAP / 2.0;
    (
        Tile {
            x: 0.0,
            y: 0.0,
            w,
            h: 1.0,
            alpha: 1.0,
        },
        Tile {
            x: 0.5 + GAP / 2.0,
            y: 0.0,
            w,
            h: 1.0,
            alpha: 1.0,
        },
    )
}

/// The top and bottom halves.
fn stacked() -> (Tile, Tile) {
    const GAP: f32 = 0.02;
    let h = 0.5 - GAP / 2.0;
    (
        Tile {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h,
            alpha: 1.0,
        },
        Tile {
            x: 0.0,
            y: 0.5 + GAP / 2.0,
            w: 1.0,
            h,
            alpha: 1.0,
        },
    )
}

/// What lesson `step` shows, `t` of the way through its loop (0 to 1).
pub fn stage(step: usize, t: f32) -> Stage {
    let t = t.clamp(0.0, 1.0);
    let moved = move_along(t);
    let (left, right) = halves();
    match step {
        // A second window opens and the first makes room.
        0 => Stage {
            tiles: vec![tween(whole(), left, moved), {
                let mut arriving = right;
                arriving.alpha = moved;
                arriving
            }],
            focus: Some(1),
            rain: 0.0,
        },
        // The ring moves across.
        1 => Stage {
            tiles: vec![left, right],
            focus: Some(if moved < 0.5 { 0 } else { 1 }),
            rain: 0.0,
        },
        // The windows swap places, the focused one going with the keyboard.
        2 => Stage {
            tiles: vec![tween(left, right, moved), tween(right, left, moved)],
            focus: Some(0),
            rain: 0.0,
        },
        // Rows become columns.
        3 => {
            let (top, bottom) = stacked();
            Stage {
                tiles: vec![tween(left, top, moved), tween(right, bottom, moved)],
                focus: Some(0),
                rain: 0.0,
            }
        }
        // Both windows pour off to the right, into the rain.
        4 => {
            let gone = |from: Tile| Tile {
                x: 1.0,
                w: 0.04,
                alpha: 0.0,
                ..from
            };
            Stage {
                tiles: vec![
                    tween(left, gone(left), moved),
                    tween(right, gone(right), moved),
                ],
                focus: None,
                rain: moved,
            }
        }
        // Nothing moving: the last card is the one that points at the list.
        _ => Stage {
            tiles: vec![left, right],
            focus: None,
            rain: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lesson_draws_something() {
        for step in 0..LESSONS.len() {
            for tenth in 0..=10 {
                let stage = stage(step, tenth as f32 / 10.0);
                assert!(!stage.tiles.is_empty(), "lesson {step} shows nothing");
                for tile in &stage.tiles {
                    assert!(
                        tile.w >= 0.0 && tile.h >= 0.0,
                        "lesson {step} has a tile inside out: {tile:?}"
                    );
                    assert!((0.0..=1.0).contains(&tile.alpha), "{tile:?}");
                }
                if let Some(focus) = stage.focus {
                    assert!(focus < stage.tiles.len(), "the ring is on no tile");
                }
            }
        }
    }

    #[test]
    fn a_move_rests_at_both_ends() {
        // Still while the words are read, then the move, then still again.
        assert_eq!(move_along(0.0), 0.0);
        assert_eq!(move_along(REST), 0.0);
        assert_eq!(move_along(1.0 - REST), 1.0);
        assert_eq!(move_along(1.0), 1.0);
        assert!(move_along(0.5) > 0.0 && move_along(0.5) < 1.0);
    }

    #[test]
    fn the_second_window_arrives_as_the_first_makes_room() {
        let start = stage(0, 0.0);
        assert_eq!(start.tiles[0].w, 1.0, "one window fills the screen");
        assert_eq!(start.tiles[1].alpha, 0.0, "and the second isn't there yet");
        let end = stage(0, 1.0);
        assert!(end.tiles[0].w < 0.55, "the first has made room");
        assert_eq!(end.tiles[1].alpha, 1.0, "and the second has arrived");
    }

    #[test]
    fn turning_the_layout_swaps_rows_for_columns() {
        let before = stage(3, 0.0);
        assert!(before.tiles[0].h > before.tiles[0].w, "side by side");
        let after = stage(3, 1.0);
        assert!(after.tiles[0].w > after.tiles[0].h, "stacked");
    }

    #[test]
    fn hiding_takes_the_windows_off_to_the_rain() {
        let after = stage(4, 1.0);
        assert!(after.tiles.iter().all(|tile| tile.alpha == 0.0));
        assert_eq!(after.rain, 1.0, "the rain has taken their place");
        assert_eq!(after.focus, None);
    }

    #[test]
    fn the_last_lesson_holds_still() {
        assert_eq!(stage(LESSONS.len() - 1, 0.0), stage(LESSONS.len() - 1, 1.0));
    }
}
