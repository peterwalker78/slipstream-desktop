//! `prompt`: the logo printed at a terminal.
//!
//! A command is typed a character at a time with a block cursor after it, its output prints, and
//! a second command prints the logo as a banner, row by row. The cursor blinks under it for a
//! while, and then the screen clears from the top down, as `clear` would.
//!
//! One turn in three the typist's fingers slip on the first command: two letters the wrong way
//! round, a pause, rubbed out, and typed again.

use super::{BG, CYAN, Frame, Grid, Layout, MINT, Variation, WHITE, gradient, mix};

/// One turn, in seconds of the animation clock.
const TYPING: f64 = 2.8;
const BANNER: f64 = 1.4;
const HOLD: f64 = 8.0;
const CLEAR: f64 = 0.7;
const GAP: f64 = 1.0;
const TURN: f64 = TYPING + BANNER + HOLD + CLEAR + GAP;

/// The still picture reduced motion shows: everything printed, the cursor not blinking.
const STILL: f64 = TYPING + BANNER + HOLD / 2.0;

/// Characters a second, while a command is being typed.
const SPEED: f64 = 38.0;
/// The pause at the end of a typed line, before its output, and how long a line of output takes
/// to appear.
const RETURN: f64 = 0.25;
const OUTPUT: f64 = 0.18;

/// The slip: how many characters of the first command go in right, the two that follow typed the
/// wrong way round, and how long the typist stares at them before rubbing them out.
const SLIP_AT: usize = 9;
const SLIP_PAUSE: f64 = 0.35;
/// One turn in this many has the slip: the second, and every third after it.
const SLIPS_EVERY: u64 = 3;

/// Whether the typist slips in turn number `turn`.
fn slips(turn: u64) -> bool {
    turn % SLIPS_EVERY == 1
}

/// What a typed line reads `since` seconds after typing starts, and whether typing is still under
/// way. With `slip`, the characters at `SLIP_AT` go in swapped, are rubbed out and typed again.
fn typing(text: &str, since: f64, slip: bool) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    let keys = (since.max(0.0) * SPEED).round() as usize;
    if !slip || chars.len() < SLIP_AT + 2 {
        let shown = keys.min(chars.len());
        return (chars[..shown].iter().collect(), shown < chars.len());
    }
    // The keys in order: the start, the two swapped, a pause worth of none, two rub-outs, and
    // the rest from where it went wrong.
    let pause = (SLIP_PAUSE * SPEED) as usize;
    let wrong = [chars[SLIP_AT + 1], chars[SLIP_AT]];
    let mut line: Vec<char> = chars[..keys.min(SLIP_AT)].to_vec();
    let mut left = keys.saturating_sub(SLIP_AT);
    let typed_wrong = left.min(2);
    line.extend(&wrong[..typed_wrong]);
    left -= typed_wrong;
    left = left.saturating_sub(pause);
    let rubbed = left.min(2);
    line.truncate(line.len() - rubbed);
    left -= rubbed;
    let rest = left.min(chars.len() - SLIP_AT);
    line.extend(&chars[SLIP_AT..SLIP_AT + rest]);
    let under_way = line.len() < chars.len() || keys < SLIP_AT + 4 + pause;
    (line.into_iter().collect(), under_way)
}

/// The blink: on for this long, then off for it.
const BLINK: f64 = 0.55;

/// What is on the screen before the banner. Commands are typed; the rest prints at once.
const LINES: [(bool, &str); 5] = [
    (true, "$ slipstream --session"),
    (false, "  compositor ........... ready"),
    (false, "  outputs .............. eDP-1"),
    (false, "  effects ............... live"),
    (true, "$ slipstream --banner"),
];

/// When each line starts and finishes, in seconds from the top of the turn; with `slip`, the
/// first command takes longer.
fn schedule(slip: bool) -> [(f64, f64); LINES.len()] {
    let mut times = [(0.0, 0.0); LINES.len()];
    let mut at = 0.0;
    for (i, (typed, text)) in LINES.iter().enumerate() {
        let extra = if slip && i == 0 {
            4.0 / SPEED + SLIP_PAUSE
        } else {
            0.0
        };
        let takes = if *typed {
            text.chars().count() as f64 / SPEED + extra
        } else {
            OUTPUT
        };
        times[i] = (at, at + takes);
        at += takes + if *typed { RETURN } else { 0.0 };
    }
    times
}

#[derive(Default)]
pub struct Prompt;

impl Variation for Prompt {
    fn id(&self) -> &'static str {
        "prompt"
    }

    fn reset(&mut self, _layout: &Layout, _seed: u64) {}

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let t = if frame.still {
            STILL
        } else {
            frame.elapsed.rem_euclid(TURN)
        };
        if t >= TYPING + BANNER + HOLD + CLEAR {
            return;
        }
        let (lx, ly, lw, lh) = layout.logo;
        // The lines sit above the logo, with a blank line between, and the last prompt below it.
        let top = ly - LINES.len() as i32 - 1;
        if top < 0 {
            return;
        }
        // Clearing takes the screen away from the top down.
        let cleared = if t < TYPING + BANNER + HOLD {
            -1.0
        } else {
            let along = ((t - TYPING - BANNER - HOLD) / CLEAR) as f32;
            top as f32 + along * (ly + lh + 2 - top) as f32
        };

        let slip = !frame.still && slips((frame.elapsed / TURN).floor() as u64);
        for (i, ((typed, text), (from, to))) in LINES.iter().zip(schedule(slip)).enumerate() {
            let row = top + i as i32;
            if cleared >= 0.0 && (row as f32) < cleared {
                continue;
            }
            if t < from {
                break;
            }
            // A typed line arrives a character at a time; output prints at once.
            let (line, typing) = if *typed {
                let (line, typing) = typing(text, t - from, slip && i == 0);
                (line, typing && t < to + 0.001)
            } else {
                (text.to_string(), false)
            };
            // In a line of output the value at the end is mint and the rest of it is dim.
            let value_at = if *typed {
                usize::MAX
            } else {
                text.rfind(' ').map_or(usize::MAX, |at| at + 1)
            };
            let mut shown = 0;
            for (col, ch) in line.chars().enumerate() {
                let rgb = if col >= value_at {
                    MINT
                } else if *typed {
                    mix(BG, WHITE, 0.92)
                } else {
                    mix(BG, CYAN, 0.42)
                };
                grid.put((lx + col as i32) as f32, row as f32, ch, rgb);
                shown = col + 1;
            }
            // The cursor sits at the end of the line being typed.
            if typing {
                grid.put(
                    (lx + shown as i32) as f32,
                    row as f32,
                    '█',
                    mix(BG, WHITE, 0.75),
                );
            }
        }

        // The banner prints row by row once the last command has been entered.
        if t < TYPING {
            return;
        }
        let printed = if t < TYPING + BANNER {
            ((t - TYPING) / BANNER * lh as f64).ceil() as i32
        } else {
            lh
        };
        for letter in &layout.letters {
            let row = letter.row - ly;
            if row >= printed {
                continue;
            }
            if cleared >= 0.0 && (letter.row as f32) < cleared {
                continue;
            }
            let across = (letter.col - lx) as f32 / lw.max(1) as f32;
            grid.put(
                letter.col as f32,
                letter.row as f32,
                letter.ch,
                gradient(across),
            );
        }

        // The prompt waiting under the banner, with the cursor blinking on it.
        if t < TYPING + BANNER {
            return;
        }
        let row = ly + lh + 1;
        if cleared >= 0.0 && (row as f32) < cleared {
            return;
        }
        grid.put(lx as f32, row as f32, '$', mix(BG, WHITE, 0.92));
        let lit = frame.still || (t - TYPING - BANNER).rem_euclid(BLINK * 2.0) < BLINK;
        if lit {
            grid.put((lx + 2) as f32, row as f32, '█', mix(BG, WHITE, 0.75));
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    fn prompt(elapsed: f64, still: bool) -> (Grid, Layout) {
        let layout = layout(1920, 1200).unwrap();
        let grid = composed(&mut Prompt, &layout, elapsed, still);
        (grid, layout)
    }

    /// What a row of the grid reads, without the blanks either side of it (the lines start at
    /// the logo's own left edge, not at the edge of the screen).
    fn line(grid: &Grid, row: i32) -> String {
        let text: String = (0..grid.cols)
            .map(|col| grid.at(col as f32, row as f32).ch)
            .collect();
        text.trim().to_string()
    }

    #[test]
    fn every_line_is_typed_and_printed_inside_the_typing() {
        for slip in [false, true] {
            let last = schedule(slip).last().copied().unwrap();
            assert!(
                last.1 < TYPING,
                "the last line finishes at {} of {TYPING}s",
                last.1
            );
        }
    }

    #[test]
    fn a_slip_is_typed_rubbed_out_and_put_right() {
        let text = LINES[0].1;
        let at = |keys: f64| typing(text, keys / SPEED + 0.001, true).0;
        assert_eq!(at(9.0), "$ slipstr");
        assert_eq!(at(11.0), "$ slipstrae", "the fingers slip");
        let pause = (SLIP_PAUSE * SPEED) as usize as f64;
        assert_eq!(at(11.0 + pause), "$ slipstrae", "and it sits there");
        assert_eq!(at(12.0 + pause), "$ slipstra");
        assert_eq!(at(13.0 + pause), "$ slipstr", "rubbed out");
        assert_eq!(at(15.0 + pause), "$ slipstrea");
        let (done, typing) = self::typing(text, 10.0, true);
        assert_eq!(done, text, "and put right");
        assert!(!typing);
        assert_eq!(self::typing(text, 10.0, false), (text.to_string(), false));
        // A third of turns, never the first, so a fresh screen starts clean.
        let slipped = (0..300).filter(|&turn| slips(turn)).count();
        assert_eq!(slipped, 100);
        assert!(!slips(0));
    }

    #[test]
    fn the_first_command_is_typed_a_character_at_a_time() {
        let (early, layout) = prompt(0.1, false);
        let row = layout.logo.1 - LINES.len() as i32 - 1;
        let typed = line(&early, row);
        assert!(
            typed.starts_with("$ sl") && typed.len() < LINES[0].1.len(),
            "part of the way through the first command: {typed:?}"
        );
        assert!(typed.ends_with('█'), "the cursor is at the end of it");
        let (done, _) = prompt(TYPING + BANNER + 1.0, false);
        assert_eq!(line(&done, row), LINES[0].1.trim());
    }

    #[test]
    fn the_banner_prints_after_the_commands_and_the_screen_clears() {
        let (typing, layout) = prompt(TYPING * 0.5, false);
        assert_eq!(at_home(&typing, &layout), 0, "no banner while it types");
        let (printed, layout) = prompt(TYPING + BANNER + HOLD / 2.0, false);
        assert_eq!(
            at_home(&printed, &layout),
            layout.letters.len(),
            "the whole banner is printed"
        );
        let (gap, _) = prompt(TURN - 0.1, false);
        assert_eq!(drawn(&gap), 0, "the gap is an empty screen");
    }

    #[test]
    fn reduced_motion_shows_it_all_printed_with_the_cursor_still() {
        let (first, layout) = prompt(0.0, true);
        let (second, _) = prompt(13.0, true);
        assert_eq!(first.cells, second.cells, "nothing blinks");
        assert_eq!(at_home(&first, &layout), layout.letters.len());
    }
}
