//! The living wallpaper's first variation: the SLIPSTREAM logo, in FIGlet block letters, centred
//! on a dark screen and animated character by character through a cycle of effects.
//!
//! The effects are about slipstreams. Stream lines flow across the screen, bend clear of the logo,
//! brighten as they squeeze past it and ripple in its wake, and each leaves a fading streakline of
//! smoke behind it, kept in a feedback buffer. Letters arrive by drafting in behind the leaders,
//! riding a crosswind, or condensing behind a pressure wave that crosses the logo; they hold; then
//! they peel off into the wake or scatter into the flow.
//!
//! Now and then one stream line is a leader: amber, faster, and on the logo's own rows, it doesn't
//! bend round the letters but threads straight through them, and each letter glows as it passes.

use std::f32::consts::PI;

use super::{
    AMBER, BG, CYAN, Feedback, Frame, Grid, Layout, Variation, WHITE, gradient, hash01, hue_byte,
    hue_colour, mix, smoothstep,
};

// One cycle, in seconds of the animation clock.
const ARRIVE: f64 = 4.0;
const HOLD: f64 = 7.0;
const LEAVE: f64 = 3.0;
const GAP: f64 = 1.2;
const CYCLE: f64 = ARRIVE + HOLD + LEAVE + GAP;

/// The pressure wave: when it sets off, as a share of the arrival, and how long it takes to cross
/// the logo's box (and ten cells either side), in the same share. Letters condense behind it over
/// `CONDENSE` cells.
const SHOCK_AT: f32 = 0.3;
const SHOCK_CROSSES: f32 = 0.15;
const CONDENSE: f32 = 14.0;

/// How much of a step's light a streakline keeps: a little over a second to the floor at 30 steps
/// a second.
const SMOKE_DECAY: f32 = 0.93;
const SMOKE_FLOOR: f32 = 0.1;

/// One stream line in this many is a leader.
const LEADERS: f32 = 1.0 / 45.0;
/// A leader's speed in columns a second, and how long a letter glows after it has passed.
const LEADER_SPEED: f32 = 30.0;
const LEADER_GLOW: f32 = 16.0;
#[derive(Debug, Clone, Copy, PartialEq)]
enum Arrive {
    /// Leaders on the right fly in first; the rest draft in behind them, trailing streaks.
    Draft,
    /// Letters ride curving stream lines in from anywhere down the left edge.
    Crosswind,
    /// A pressure wave crosses the logo, and letters condense out of the air behind it.
    Shock,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Leave {
    /// The trailing edge peels away first, tumbling into the wake.
    Wake,
    /// Letters break into specks and drift off with the flow.
    Scatter,
}

fn effects(cycle: u64) -> (Arrive, Leave) {
    const ARRIVALS: [Arrive; 3] = [Arrive::Draft, Arrive::Crosswind, Arrive::Shock];
    const LEAVINGS: [Leave; 2] = [Leave::Wake, Leave::Scatter];
    (
        ARRIVALS[(cycle % 3) as usize],
        LEAVINGS[(cycle % 2) as usize],
    )
}

/// A stream line flowing left to right: its head's column, the row it started on, its speed in
/// columns a second and its length. A leader goes straight through the logo instead of round it.
#[derive(Debug, Clone)]
struct Flow {
    x: f32,
    row: f32,
    speed: f32,
    len: usize,
    leader: bool,
}

fn spawn_flow(layout: &Layout, seed: u64, anywhere: bool) -> Flow {
    // Leaders only come in from the left edge, so one never appears out of nowhere mid-screen.
    if !anywhere && hash01(seed ^ 0x1ead) < LEADERS {
        let (_, ly, _, lh) = layout.logo;
        return Flow {
            row: (ly + (hash01(seed ^ 0x2ead) * lh as f32) as i32) as f32,
            x: -12.0,
            speed: LEADER_SPEED,
            len: 12,
            leader: true,
        };
    }
    let speed = 6.0 + hash01(seed) * 18.0;
    let len = 3 + (speed / 3.0) as usize;
    Flow {
        row: hash01(seed ^ 0x5157) * layout.rows as f32,
        x: if anywhere {
            hash01(seed ^ 0x0a11) * layout.cols as f32
        } else {
            -(len as f32) - hash01(seed ^ 0x0b22) * 30.0
        },
        speed,
        len,
        leader: false,
    }
}

/// How hard a stream line that started on row `base` is squeezed past the logo at column `x`, from
/// 0 (free stream) to 1 (the tightest point, level with the logo's middle).
fn squeeze(layout: &Layout, base: f32, x: f32) -> f32 {
    let (lx, ly, lw, lh) = (
        layout.logo.0 as f32,
        layout.logo.1 as f32,
        layout.logo.2 as f32,
        layout.logo.3 as f32,
    );
    let off = base - (ly + lh / 2.0);
    let clear = lh / 2.0 + 4.0;
    if off.abs() >= clear {
        return 0.0;
    }
    let along = (x - (lx - 14.0)) / (lw + 28.0);
    if !(0.0..=1.0).contains(&along) {
        return 0.0;
    }
    (along * PI).sin()
}

/// Where a stream line that started on row `base` runs at column `x`: pushed clear of the logo
/// while it passes, and rippling in the wake behind it.
fn flow_row(layout: &Layout, base: f32, x: f32, clock: f32) -> f32 {
    let (lx, ly, lw, lh) = (
        layout.logo.0 as f32,
        layout.logo.1 as f32,
        layout.logo.2 as f32,
        layout.logo.3 as f32,
    );
    let off = base - (ly + lh / 2.0);
    let clear = lh / 2.0 + 4.0;
    let mut y = base;
    if off.abs() < clear {
        let along = ((x - (lx - 14.0)) / (lw + 28.0)).clamp(0.0, 1.0);
        let push = (clear - off.abs()) * (along * PI).sin();
        y += if off < 0.0 { -push } else { push };
    }
    let behind = x - (lx + lw);
    if behind > 0.0 && off.abs() < clear + 3.0 {
        y += 1.6 * (-behind / 30.0).exp() * (x * 0.45 + clock * 4.0 + base).sin();
    }
    y
}

/// Where the pressure wave's front is at `u` of the way through the arrival, in columns, if it
/// has set off.
fn shock_front(layout: &Layout, u: f32) -> Option<f32> {
    let (lx, _, lw, _) = layout.logo;
    let along = (u - SHOCK_AT) / SHOCK_CROSSES;
    (along >= 0.0).then(|| (lx - 10) as f32 + (lw + 20) as f32 * along)
}

/// The screen as characters, `elapsed` seconds into the variation. `clock` drives the wake's
/// ripple.
fn compose(grid: &mut Grid, layout: &Layout, flows: &[Flow], elapsed: f64, clock: f32) {
    let (lx, ly, lw, lh) = layout.logo;

    // Stream lines, dim, behind everything. Squeezing past the logo they brighten, as air speeds
    // up round an obstacle; a leader goes straight through in amber.
    let mut leaders: Vec<(f32, f32)> = Vec::new();
    for flow in flows {
        if flow.leader {
            leaders.push((flow.x, flow.row));
            for k in 0..flow.len {
                let fade = 1.0 - k as f32 / flow.len as f32;
                let rgb = mix(BG, mix(AMBER, WHITE, 0.3 * fade), 0.15 + 0.6 * fade);
                grid.put(flow.x - k as f32, flow.row, '━', rgb);
            }
            continue;
        }
        let wake_row = (flow.row - (ly as f32 + lh as f32 / 2.0)).abs() < lh as f32 / 2.0 + 7.0;
        for k in 0..flow.len {
            let x = flow.x - k as f32;
            let y = flow_row(layout, flow.row, x, clock);
            let slope = y - flow_row(layout, flow.row, x - 1.0, clock);
            let ch = if wake_row && x > (lx + lw) as f32 {
                '~'
            } else if slope > 0.4 {
                '╲'
            } else if slope < -0.4 {
                '╱'
            } else {
                '─'
            };
            let fade = 1.0 - k as f32 / flow.len as f32;
            let light = (0.08 + 0.3 * fade) * (1.0 + 0.6 * squeeze(layout, flow.row, x));
            grid.put(x, y, ch, mix(BG, CYAN, light));
        }
    }

    let cycle = (elapsed / CYCLE).floor().max(0.0) as u64;
    let t = elapsed.rem_euclid(CYCLE);
    let (arrive, leave) = effects(cycle);
    // Trails go down first, so letters always sit on top of them.
    let mut trails: Vec<(f32, f32, char, [f32; 3])> = Vec::new();
    let mut marks: Vec<(f32, f32, char, [f32; 3])> = Vec::new();

    if t < ARRIVE && arrive == Arrive::Shock {
        // The front: a bright wall a couple of cells thick, taller than the logo, with the air
        // behind it still shimmering.
        if let Some(head) = shock_front(layout, (t / ARRIVE) as f32) {
            if head < (lx + lw + 10) as f32 {
                for row in ly - 3..ly + lh + 3 {
                    let off = (row - ly) as f32 - lh as f32 / 2.0;
                    let edge = 1.0 - off.abs() / (lh as f32 / 2.0 + 3.0);
                    let light = 0.35 + 0.65 * edge.max(0.0);
                    // Bowed like a shock standing off a nose: the middle leads, the ends trail.
                    let x = (head - 0.08 * off * off).round();
                    trails.push((x, row as f32, '█', mix(BG, WHITE, light)));
                    trails.push((x - 1.0, row as f32, '▓', mix(BG, WHITE, light * 0.8)));
                    trails.push((x - 2.0, row as f32, '▒', mix(BG, CYAN, light * 0.6)));
                    trails.push((x - 3.0, row as f32, '░', mix(BG, CYAN, light * 0.4)));
                }
            }
        }
    }

    for (i, letter) in layout.letters.iter().enumerate() {
        let seed = hash01(cycle.wrapping_mul(1_000_003).wrapping_add(i as u64));
        let across = (letter.col - lx) as f32 / lw.max(1) as f32;
        let (hx, hy) = (letter.col as f32, letter.row as f32);
        let home = gradient(across);
        if t < ARRIVE {
            let u = (t / ARRIVE) as f32;
            match arrive {
                Arrive::Draft => {
                    let p = ((u - ((1.0 - across) * 0.5 + seed * 0.1)) / 0.38).clamp(0.0, 1.0);
                    if p <= 0.0 {
                        continue;
                    }
                    let e = 1.0 - (1.0 - p).powi(3);
                    let start = -12.0 - seed * 30.0;
                    let x = start + (hx - start) * e;
                    if p < 1.0 {
                        let trail = ((1.0 - e) * 14.0) as i32 + 1;
                        for s in 1..=trail {
                            let light = 0.75 * (1.0 - s as f32 / trail as f32);
                            trails.push((x - s as f32, hy, '─', mix(BG, CYAN, light)));
                        }
                    }
                    marks.push((x, hy, letter.ch, mix(WHITE, home, e)));
                }
                Arrive::Crosswind => {
                    let p = ((u - seed * 0.45) / 0.5).clamp(0.0, 1.0);
                    if p <= 0.0 {
                        continue;
                    }
                    let e = smoothstep(p);
                    let (sx, sy) = (
                        -8.0 - seed * 40.0,
                        hash01(i as u64 ^ cycle.rotate_left(17)) * layout.rows as f32,
                    );
                    let x = sx + (hx - sx) * e;
                    let y = sy + (hy - sy) * smoothstep(e) + (e * PI).sin() * (seed - 0.5) * 6.0;
                    if p < 1.0 {
                        for s in 1..=3 {
                            let light = 0.5 * (1.0 - s as f32 / 4.0);
                            trails.push((x - s as f32, y, '─', mix(BG, CYAN, light)));
                        }
                    }
                    marks.push((x, y, letter.ch, mix(CYAN, home, e)));
                }
                Arrive::Shock => {
                    let Some(head) = shock_front(layout, u) else {
                        continue;
                    };
                    let behind = head - hx;
                    if behind < 1.0 {
                        continue;
                    }
                    // Out of the air behind the front: a haze, thicker, then the letter, white hot
                    // and cooling to its colour.
                    let condensed = behind / CONDENSE;
                    let ch = match condensed {
                        c if c < 0.25 => '░',
                        c if c < 0.5 => '▒',
                        c if c < 0.75 => '▓',
                        _ => letter.ch,
                    };
                    let cooled = ((behind - CONDENSE) / 60.0).clamp(0.0, 1.0);
                    marks.push((hx, hy, ch, mix(WHITE, home, cooled)));
                }
            }
        } else if t < ARRIVE + HOLD {
            marks.push((hx, hy, letter.ch, home));
        } else if t < ARRIVE + HOLD + LEAVE {
            let u = ((t - ARRIVE - HOLD) / LEAVE) as f32;
            match leave {
                Leave::Wake => {
                    let p = ((u - ((1.0 - across) * 0.55 + seed * 0.08)) / 0.4).clamp(0.0, 1.0);
                    let e = p * p;
                    let x = hx + e * (layout.cols as f32 - hx + 25.0);
                    let y = hy + (e * PI * 2.0 + seed * 6.0).sin() * e * 2.5;
                    if e > 0.0 {
                        let trail = (e * 8.0) as i32 + 1;
                        for s in 1..=trail {
                            let light = 0.6 * (1.0 - s as f32 / (trail + 1) as f32);
                            trails.push((x - s as f32, y, '─', mix(BG, CYAN, light)));
                        }
                    }
                    let ch = if e > 0.25 { '~' } else { letter.ch };
                    marks.push((x, y, ch, mix(home, CYAN, e)));
                }
                Leave::Scatter => {
                    let p = ((u - seed * 0.55) / 0.35).clamp(0.0, 1.0);
                    if p >= 1.0 {
                        continue;
                    }
                    let x = hx + p * p * 30.0 * (0.5 + seed);
                    let y = hy + (p * 3.0 + seed * 9.0).sin() * p * 1.5;
                    let ch = if p < 0.2 {
                        letter.ch
                    } else if p < 0.6 {
                        '•'
                    } else {
                        '·'
                    };
                    marks.push((x, y, ch, mix(home, mix(BG, CYAN, 0.4), p)));
                }
            }
        }
    }
    // A leader lights the letters on its row as it threads through them.
    for (x, y, _, rgb) in &mut marks {
        for &(head, row) in &leaders {
            let behind = head - *x;
            if (*y - row).abs() < 0.5 && (0.0..LEADER_GLOW).contains(&behind) {
                *rgb = mix(
                    *rgb,
                    mix(AMBER, WHITE, 0.6),
                    0.85 * (1.0 - behind / LEADER_GLOW),
                );
            }
        }
    }
    for (x, y, ch, rgb) in trails.into_iter().chain(marks) {
        grid.put(x, y, ch, rgb);
    }
}

/// The variation itself: the stream lines it carries, their smoke, and where its cycle has got to.
#[derive(Default)]
pub struct Slipstream {
    flows: Vec<Flow>,
    /// Counts the flows spawned so far, so each gets its own seed.
    spawned: u64,
    /// Animation-clock seconds since it took over, for the wake's ripple.
    clock: f32,
    /// The streaklines the flows leave, fading where they were laid down.
    smoke: Feedback,
}

impl Variation for Slipstream {
    fn id(&self) -> &'static str {
        "slipstream"
    }

    fn reset(&mut self, layout: &Layout, seed: u64) {
        self.spawned = seed;
        self.clock = 0.0;
        let count = (layout.rows as f32 * 0.35) as usize;
        self.flows = (0..count)
            .map(|_| {
                self.spawned += 1;
                spawn_flow(layout, self.spawned, true)
            })
            .collect();
        self.smoke.reset(layout.cols, layout.rows);
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        self.clock += frame.dt;
        for flow in &mut self.flows {
            flow.x += flow.speed * frame.dt;
            if flow.x - flow.len as f32 > layout.cols as f32 {
                self.spawned += 1;
                *flow = spawn_flow(layout, self.spawned, false);
            }
        }
        if frame.dt > 0.0 {
            // Each line lays its smoke where it runs now, and the smoke stays put and fades.
            self.smoke.advance(SMOKE_DECAY, |x, y| (x, y));
            for flow in &self.flows {
                let hue = hue_byte(if flow.leader { 0.0 } else { 2.0 / 3.0 });
                for k in 0..flow.len {
                    let x = flow.x - k as f32;
                    let y = if flow.leader {
                        flow.row
                    } else {
                        flow_row(layout, flow.row, x, self.clock)
                    };
                    let light = if flow.leader { 0.6 } else { 0.32 };
                    self.smoke.ink(x, y, light, hue);
                }
            }
        }
        // Thin lines, not shaded cells: smoke, a few shades of the line's own colour.
        self.smoke
            .draw(grid, SMOKE_FLOOR, &['─', '─', '─'], |hue, level| {
                mix(BG, hue_colour(hue, 6), [0.07, 0.12, 0.18][level])
            });
        // Reduced motion: the logo simply sits there, mid-hold.
        let elapsed = if frame.still {
            ARRIVE + HOLD / 2.0
        } else {
            frame.elapsed
        };
        compose(grid, layout, &self.flows, elapsed, self.clock);
    }

    fn turn(&self) -> Option<f64> {
        Some(CYCLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saver::layout;

    /// The grid this variation draws `elapsed` seconds in, with no stream lines and nothing
    /// moving.
    fn composed(layout: &Layout, elapsed: f64) -> Grid {
        let mut grid = Grid::new(layout.cols, layout.rows);
        compose(&mut grid, layout, &[], elapsed, 0.0);
        grid
    }

    #[test]
    fn every_effect_brings_the_letters_in_holds_them_and_takes_them_away() {
        let layout = layout(1920, 1200).unwrap();
        let at_home = |grid: &Grid| {
            layout
                .letters
                .iter()
                .filter(|letter| {
                    grid.cells[(letter.row * layout.cols + letter.col) as usize].ch == letter.ch
                })
                .count()
        };
        for cycle in 0..6u64 {
            let start = cycle as f64 * CYCLE;
            let (arrive, leave) = effects(cycle);
            assert_eq!(
                at_home(&composed(&layout, start + 0.001)),
                0,
                "{arrive:?} starts with no letters"
            );
            assert!(
                at_home(&composed(&layout, start + ARRIVE * 0.5)) > 0,
                "{arrive:?} has letters arriving half way"
            );
            assert_eq!(
                at_home(&composed(&layout, start + ARRIVE + 1.0)),
                layout.letters.len(),
                "the logo holds after {arrive:?}"
            );
            assert_eq!(
                at_home(&composed(&layout, start + CYCLE - 0.1)),
                0,
                "{leave:?} leaves nothing behind"
            );
        }
    }

    #[test]
    fn letters_condense_behind_the_pressure_wave_and_not_ahead_of_it() {
        let layout = layout(1920, 1200).unwrap();
        let (lx, _, lw, _) = layout.logo;
        // The turn whose arrival is the shock, as the front crosses the middle of the logo.
        let cycle = (0..6).find(|&c| effects(c).0 == Arrive::Shock).unwrap();
        let u = SHOCK_AT + SHOCK_CROSSES * (10.0 + lw as f32 / 2.0) / (lw + 20) as f32;
        let grid = composed(&layout, cycle as f64 * CYCLE + u as f64 * ARRIVE);
        let middle = lx + lw / 2;
        let ahead = layout
            .letters
            .iter()
            .filter(|letter| letter.col > middle + 1)
            .filter(|letter| grid.at(letter.col as f32, letter.row as f32).ch != ' ')
            .count();
        let behind = layout
            .letters
            .iter()
            .filter(|letter| letter.col < middle - CONDENSE as i32)
            .filter(|letter| grid.at(letter.col as f32, letter.row as f32).ch == letter.ch)
            .count();
        assert_eq!(ahead, 0, "nothing has condensed ahead of the front");
        assert!(behind > 100, "well behind it the letters are out: {behind}");
    }

    #[test]
    fn a_leader_threads_the_logo_and_lights_the_letters_it_passes() {
        let layout = layout(1920, 1200).unwrap();
        let (lx, ly, lw, _) = layout.logo;
        let row = ly + 4;
        let head = (lx + lw / 2) as f32;
        let leader = Flow {
            x: head,
            row: row as f32,
            speed: LEADER_SPEED,
            len: 12,
            leader: true,
        };
        let hold = ARRIVE + HOLD / 2.0;
        let mut lit = Grid::new(layout.cols, layout.rows);
        compose(&mut lit, &layout, &[leader], hold, 0.0);
        let plain = composed(&layout, hold);
        let glowing = layout
            .letters
            .iter()
            .filter(|letter| {
                let (x, y) = (letter.col as f32, letter.row as f32);
                lit.at(x, y).rgb != plain.at(x, y).rgb
            })
            .collect::<Vec<_>>();
        assert!(!glowing.is_empty(), "some letters glow");
        assert!(
            glowing
                .iter()
                .all(|letter| letter.row == row && (letter.col as f32) <= head),
            "only on its row, behind its head"
        );
        // Leaders are rare, and never appear mid-screen.
        let spawned = (0..20_000u64)
            .map(|seed| spawn_flow(&layout, seed, false))
            .filter(|flow| flow.leader)
            .count();
        assert!((200..700).contains(&spawned), "{spawned} leaders in 20,000");
        assert!((0..2000u64).all(|seed| !spawn_flow(&layout, seed, true).leader));
    }

    #[test]
    fn stream_lines_bend_clear_of_the_logo_and_leave_far_rows_alone() {
        let layout = layout(1920, 1200).unwrap();
        let (lx, ly, lw, lh) = layout.logo;
        let middle = lx as f32 + lw as f32 / 2.0;
        let through_centre = flow_row(&layout, ly as f32 + lh as f32 / 2.0, middle, 0.0);
        assert!(through_centre >= (ly + lh) as f32 || through_centre < ly as f32);
        assert_eq!(flow_row(&layout, 1.0, middle, 0.0), 1.0);
    }
}
