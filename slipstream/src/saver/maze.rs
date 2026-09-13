//! `maze`: a walk through a maze, seen first hand, to the logo on the wall at its end.
//!
//! Each turn builds a new maze (a depth-first carve, so it has long winding corridors), picks the
//! dead end furthest from the start, and walks the way there, turning smoothly at the corners. The
//! view is ray cast a column of Braille dots at a time and drawn as fine lines of dots: the walls'
//! edges, the joints between their blocks and the joints of the floor, dimmed with distance. The wall at the end of the way carries the logo, so it
//! can be seen down the last corridor, and the walk stops where the letters on it stand exactly on
//! the logo's own cells. Then the maze fades and the letters stay.

use std::f32::consts::{PI, TAU};

use super::{
    AMBER, BG, CYAN, Frame, Grid, Layout, MINT, Variation, braille, gradient, hash01, mix,
};

/// One turn, in seconds of the animation clock.
const IN: f64 = 1.0;
const WALK: f64 = 27.0;
const ARRIVE: f64 = 2.0;
const HOLD: f64 = 2.5;
const OUT: f64 = 1.0;
const TURN: f64 = IN + WALK + ARRIVE + HOLD + OUT;

/// Cells in the maze across and down. The map it is cast in has a square for every cell and every
/// wall between them.
const CELLS: (usize, usize) = (8, 6);

/// Half the view's width, as an angle: a little wider than a person's eye, as games have it.
const HALF_VIEW: f32 = 0.58;

/// How far round a corner the turn starts and finishes, in map squares.
const CORNER: f32 = 0.6;

/// Courses of blocks a wall is built of, from floor to ceiling.
const COURSES: usize = 4;

/// The most of a wall's face the dither lights, near the eye.
const FACE: f32 = 0.2;

/// A 4×4 ordered dither: the order in which a block's dots light as it grows brighter.
const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Map squares beyond which nothing is drawn.
const FAR: f32 = 9.0;

/// The logo's width on the end wall, as a share of the wall.
const BANNER: f32 = 0.92;

struct Map {
    w: usize,
    h: usize,
    solid: Vec<bool>,
}

impl Map {
    fn solid(&self, x: i32, y: i32) -> bool {
        x < 0
            || y < 0
            || x as usize >= self.w
            || y as usize >= self.h
            || self.solid[y as usize * self.w + x as usize]
    }
}

/// Carves a maze with a depth-first walk from the top-left cell, and returns the map and the way
/// from that cell to the dead end furthest from it, as cells.
fn carve(seed: u64) -> (Map, Vec<(usize, usize)>) {
    let (cw, ch) = CELLS;
    let (w, h) = (cw * 2 + 1, ch * 2 + 1);
    let mut solid = vec![true; w * h];
    let mut seen = vec![false; cw * ch];
    let mut parent = vec![usize::MAX; cw * ch];
    let mut depth = vec![0usize; cw * ch];
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    solid[w + 1] = false;
    let mut draws = 0u64;
    while let Some(&(x, y)) = stack.last() {
        let mut options = Vec::with_capacity(4);
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx >= 0
                && ny >= 0
                && (nx as usize) < cw
                && (ny as usize) < ch
                && !seen[ny as usize * cw + nx as usize]
            {
                options.push((nx as usize, ny as usize));
            }
        }
        if options.is_empty() {
            stack.pop();
            continue;
        }
        draws += 1;
        let pick = (hash01(seed ^ draws.wrapping_mul(0x9e37_79b9_7f4a_7c15)) * options.len() as f32)
            as usize;
        let (nx, ny) = options[pick.min(options.len() - 1)];
        seen[ny * cw + nx] = true;
        parent[ny * cw + nx] = y * cw + x;
        depth[ny * cw + nx] = depth[y * cw + x] + 1;
        solid[(y + ny + 1) * w + (x + nx + 1)] = false;
        solid[(ny * 2 + 1) * w + nx * 2 + 1] = false;
        stack.push((nx, ny));
    }
    // Every cell's way back to the start is its chain of parents, so the furthest cell is a dead
    // end, and its chain is the way.
    let far = (0..cw * ch).max_by_key(|&i| depth[i]).unwrap_or(0);
    let mut way = vec![];
    let mut at = far;
    while at != usize::MAX {
        way.push((at % cw, at / cw));
        at = parent[at];
    }
    way.reverse();
    (Map { w, h, solid }, way)
}

/// The shortest turn from angle `a` to angle `b`.
fn turn_between(a: f32, b: f32) -> f32 {
    (b - a + PI).rem_euclid(TAU) - PI
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The walk: a line through map squares, with the heading along each piece.
struct Walk {
    points: Vec<(f32, f32)>,
    /// Distance along the walk at each point.
    along: Vec<f32>,
    headings: Vec<f32>,
}

impl Walk {
    fn new(points: Vec<(f32, f32)>) -> Self {
        let mut along = vec![0.0];
        let mut headings = vec![];
        for pair in points.windows(2) {
            let (dx, dy) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
            along.push(along.last().copied().unwrap_or(0.0) + (dx * dx + dy * dy).sqrt());
            headings.push(dy.atan2(dx));
        }
        Walk {
            points,
            along,
            headings,
        }
    }

    fn length(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// Where the walk is, and which way it faces, `s` squares along.
    fn at(&self, s: f32) -> ((f32, f32), f32) {
        if self.headings.is_empty() {
            return (self.points.first().copied().unwrap_or((1.5, 1.5)), 0.0);
        }
        let s = s.clamp(0.0, self.length());
        let piece = (1..self.along.len())
            .find(|&k| s <= self.along[k])
            .unwrap_or(self.along.len() - 1)
            - 1;
        let from = self.points[piece];
        let into = (s - self.along[piece]) / (self.along[piece + 1] - self.along[piece]).max(1e-6);
        let to = self.points[piece + 1];
        let place = (
            from.0 + (to.0 - from.0) * into,
            from.1 + (to.1 - from.1) * into,
        );
        let mut heading = self.headings[piece];
        // Into the corner behind, and towards the corner ahead.
        if piece > 0 {
            let past = s - self.along[piece];
            if past < CORNER {
                let before = self.headings[piece - 1];
                let share = smooth(0.5 + past / (2.0 * CORNER));
                heading = before + turn_between(before, heading) * share;
            }
        }
        if piece + 1 < self.headings.len() {
            let left = self.along[piece + 1] - s;
            if left < CORNER {
                let next = self.headings[piece + 1];
                let share = smooth(0.5 - left / (2.0 * CORNER));
                heading += turn_between(heading, next) * share;
            }
        }
        (place, heading)
    }
}

#[derive(Default)]
pub struct Maze {
    seed: u64,
    turn: Option<u64>,
    map: Option<Map>,
    walk: Option<Walk>,
    /// The square the logo is on, and the side of it that shows it, as a unit step out of the wall.
    banner: ((i32, i32), (i32, i32)),
    /// The logo as Braille dots, and its size in dots.
    logo: Vec<bool>,
    logo_size: (usize, usize),
    /// The view in Braille dots, two across and four down a cell: how bright each dot is and in
    /// what colour, and what each column's ray struck.
    w: usize,
    h: usize,
    light: Vec<f32>,
    ink: Vec<[f32; 3]>,
    struck: Vec<Hit>,
}

/// What a column's ray struck.
#[derive(Clone, Copy)]
struct Hit {
    distance: f32,
    square: (i32, i32),
    /// The side of the square that was struck, as a unit step out of it.
    face: (i32, i32),
    /// The plane the face lies in, which is the same all along a straight wall.
    plane: (u8, i32),
    /// Where along the plane, in map squares.
    along: f32,
    /// How far across the face, 0 at its left as the eye sees it.
    u: f32,
    ray: (f32, f32),
}

impl Maze {
    fn start(&mut self, layout: &Layout, turn: u64) {
        let (map, way) = carve(self.seed ^ turn.wrapping_mul(0xa24b_aed4_963e_e407) ^ 0x3a2e);
        let centre = |(x, y): (usize, usize)| ((x * 2 + 1) as f32 + 0.5, (y * 2 + 1) as f32 + 0.5);
        let mut points: Vec<(f32, f32)> = way.iter().map(|&cell| centre(cell)).collect();
        // The last step into the dead end faces its end wall. Stop where the wall fills the logo's
        // width: the focal length over the logo's width, both in dots.
        let last = points.len() - 1;
        let (end, before) = (points[last], points[last.saturating_sub(1)]);
        // Not `signum`, which is 1 for 0.
        let sign = |v: f32| {
            if v > 0.0 {
                1.0
            } else if v < 0.0 {
                -1.0
            } else {
                0.0
            }
        };
        let (dx, dy) = (sign(end.0 - before.0), sign(end.1 - before.1));
        let focal = self.focal(layout);
        let stop = focal * BANNER / (layout.logo.2.max(1) * 2) as f32;
        points[last] = (end.0 + dx * (0.5 - stop), end.1 + dy * (0.5 - stop));
        let wall = ((end.0 + dx).floor() as i32, (end.1 + dy).floor() as i32);
        self.banner = (wall, (-dx as i32, -dy as i32));
        self.walk = Some(Walk::new(points));
        self.map = Some(map);
        self.turn = Some(turn);

        // The logo's letters are block characters, so each of its cells is a top half, a bottom
        // half or both: two dots across and two down for each half.
        let (lx, ly, lw, lh) = layout.logo;
        let (pw, ph) = (lw.max(1) as usize * 2, lh.max(1) as usize * 4);
        self.logo = vec![false; pw * ph];
        for letter in &layout.letters {
            let (x, y) = (
                (letter.col - lx) as usize * 2,
                (letter.row - ly) as usize * 4,
            );
            if x + 1 >= pw || y + 3 >= ph {
                continue;
            }
            let (top, bottom) = match letter.ch {
                '▀' => (true, false),
                '▄' => (false, true),
                _ => (true, true),
            };
            for down in 0..4 {
                let on = if down < 2 { top } else { bottom };
                for across in 0..2 {
                    self.logo[(y + down) * pw + x + across] |= on;
                }
            }
        }
        self.logo_size = (pw, ph);
    }

    /// The distance, in dots, from the eye to a screen on which one dot across is one dot's worth
    /// of view. Dots are as tall as they are wide, since a cell is twice as tall as it is wide.
    fn focal(&self, layout: &Layout) -> f32 {
        layout.cols.max(1) as f32 / HALF_VIEW.tan()
    }

    /// Lights the dot at (`x`, `y`) at `amount` in `rgb`, keeping the brighter of what is there.
    fn plot(&mut self, x: usize, y: f32, amount: f32, rgb: [f32; 3]) {
        if y < 0.0 || amount <= 0.0 {
            return;
        }
        let y = y as usize;
        if x >= self.w || y >= self.h {
            return;
        }
        let i = y * self.w + x;
        if amount > self.light[i] {
            self.light[i] = amount;
            self.ink[i] = rgb;
        }
    }

    /// Lights a line of dots straight down column `x`, from `top` to `bottom`.
    fn upright(&mut self, x: usize, top: f32, bottom: f32, amount: f32, rgb: [f32; 3]) {
        let (top, bottom) = (top.max(0.0), bottom.min(self.h as f32));
        let mut y = top.floor();
        while y < bottom {
            self.plot(x, y, amount, rgb);
            y += 1.0;
        }
    }

    /// Casts one ray from `place` into the map.
    fn cast_ray(map: &Map, place: (f32, f32), ray: (f32, f32), right: (f32, f32)) -> Hit {
        // Digital differential analysis through the map's squares.
        let (mut mx, mut my) = (place.0.floor() as i32, place.1.floor() as i32);
        let delta = (
            if ray.0 == 0.0 {
                f32::INFINITY
            } else {
                (1.0 / ray.0).abs()
            },
            if ray.1 == 0.0 {
                f32::INFINITY
            } else {
                (1.0 / ray.1).abs()
            },
        );
        let step = (
            if ray.0 < 0.0 { -1 } else { 1 },
            if ray.1 < 0.0 { -1 } else { 1 },
        );
        let mut next = (
            if ray.0 < 0.0 {
                (place.0 - mx as f32) * delta.0
            } else {
                (mx as f32 + 1.0 - place.0) * delta.0
            },
            if ray.1 < 0.0 {
                (place.1 - my as f32) * delta.1
            } else {
                (my as f32 + 1.0 - place.1) * delta.1
            },
        );
        let mut across_x = true;
        for _ in 0..64 {
            if next.0 < next.1 {
                next.0 += delta.0;
                mx += step.0;
                across_x = true;
            } else {
                next.1 += delta.1;
                my += step.1;
                across_x = false;
            }
            if map.solid(mx, my) {
                break;
            }
        }
        let distance = if across_x {
            next.0 - delta.0
        } else {
            next.1 - delta.1
        }
        .max(0.05);
        let along = if across_x {
            place.1 + distance * ray.1
        } else {
            place.0 + distance * ray.0
        };
        let mut u = along - along.floor();
        if (if across_x { right.1 } else { right.0 }) < 0.0 {
            u = 1.0 - u;
        }
        let (face, plane) = if across_x {
            ((-step.0, 0), (0, mx + (1 - step.0) / 2))
        } else {
            ((0, -step.1), (1, my + (1 - step.1) / 2))
        };
        Hit {
            distance,
            square: (mx, my),
            face,
            plane,
            along,
            u,
            ray,
        }
    }

    /// Draws the view from `place`, facing `heading`, with the maze at `light` and the logo at
    /// `banner_light`. The maze is drawn as lines of dots: the edges of the walls where they meet
    /// the floor and the ceiling, turn a corner or stand in front of one another, the joints
    /// between the blocks they are built of, and the joints of the floor, each hidden where a wall
    /// stands in front of it.
    fn cast(
        &mut self,
        layout: &Layout,
        place: (f32, f32),
        heading: f32,
        light: f32,
        banner_light: f32,
    ) {
        let (w, h) = (
            layout.cols.max(0) as usize * 2,
            layout.rows.max(0) as usize * 4,
        );
        self.w = w;
        self.h = h;
        self.light.clear();
        self.light.resize(w * h, 0.0);
        self.ink.clear();
        self.ink.resize(w * h, BG);
        let Some(map) = self.map.take() else {
            return;
        };
        let focal = self.focal(layout);
        let (lx, ly, lw, lh) = layout.logo;
        // The centre of the view is the middle of the logo, so the letters on the end wall land on
        // its cells.
        let centre = (
            (lx as f32 + lw as f32 / 2.0) * 2.0,
            (ly as f32 + lh as f32 / 2.0) * 4.0,
        );
        let facing = (heading.cos(), heading.sin());
        let right = (-heading.sin(), heading.cos());

        let mut struck = std::mem::take(&mut self.struck);
        struck.clear();
        for col in 0..w {
            let across = (col as f32 + 0.5 - centre.0) / focal;
            let ray = (facing.0 + right.0 * across, facing.1 + right.1 * across);
            struck.push(Self::cast_ray(&map, place, ray, right));
        }

        let near_colour = mix(MINT, CYAN, 0.35);
        let fog = |distance: f32| (1.0 - distance / FAR).clamp(0.0, 1.0).powi(2);
        let wall_ink = |distance: f32| {
            let fog = fog(distance);
            (
                mix(CYAN, near_colour, fog),
                (0.3 + 0.7 * fog.sqrt()) * light,
            )
        };
        let span = |hit: &Hit| {
            let height = focal / hit.distance;
            (centre.1 - height / 2.0, centre.1 + height / 2.0)
        };

        for col in 0..w {
            let hit = struck[col];
            let (top, bottom) = span(&hit);
            let (ink, bright) = wall_ink(hit.distance);

            // The floor's and the ceiling's joints: a dot wherever the square the floor or ceiling
            // under it lies in differs from the one under the dot beside it or below it.
            if light > 0.0 {
                let square_at = |x: usize, y: f32| {
                    let ray = struck[x.min(w - 1)].ray;
                    let far = focal * 0.5 / (y + 0.5 - centre.1).abs().max(0.01);
                    (
                        far,
                        ((place.0 + ray.0 * far) * 2.0).floor(),
                        ((place.1 + ray.1 * far) * 2.0).floor(),
                    )
                };
                for row in (0..h).filter(|&row| {
                    let y = row as f32 + 0.5;
                    y < top || y >= bottom
                }) {
                    let y = row as f32;
                    let below = y + 0.5 >= bottom;
                    let (far, fx, fy) = square_at(col, y);
                    if far > FAR {
                        continue;
                    }
                    let (_, rx, ry) = square_at(col + 1, y);
                    let beside = col + 1 < w && (rx != fx || ry != fy);
                    let (_, dx, dy) = square_at(col, if below { y + 1.0 } else { y - 1.0 });
                    let next = dx != fx || dy != fy;
                    if beside || next {
                        let fade = 1.0 - far / FAR;
                        let (tone, amount) = if below { (AMBER, 0.9) } else { (CYAN, 0.4) };
                        self.plot(col, y, fade * amount * light, tone);
                    }
                }
            }

            if hit.distance >= FAR {
                continue;
            }

            // The logo on the end wall, dot for dot.
            let is_banner = hit.square == self.banner.0 && hit.face == self.banner.1;
            if is_banner && banner_light > 0.0 {
                let (pw, ph) = self.logo_size;
                let height = bottom - top;
                let shape = BANNER * ph as f32 / pw as f32;
                let bu = (hit.u - (1.0 - BANNER) / 2.0) / BANNER;
                let px = (bu * pw as f32).floor();
                if px >= 0.0 && (px as usize) < pw {
                    let mut y = top.max(0.0).floor();
                    while y < bottom.min(h as f32) {
                        let bv = ((y + 0.5 - top) / height - 0.5) / shape + 0.5;
                        let py = (bv * ph as f32).floor();
                        if py >= 0.0
                            && (py as usize) < ph
                            && self.logo[py as usize * pw + px as usize]
                        {
                            let amount = banner_light.min(0.45 + fog(hit.distance));
                            self.plot(col, y, amount, gradient(px / pw as f32));
                        }
                        y += 1.0;
                    }
                }
            }

            if light <= 0.0 {
                continue;
            }
            // The face itself, shaded with a sparse dither: denser near, and denser on walls that
            // run across the view than along it, so walls read as solid and fall away into the dark.
            let facing_share = if hit.plane.0 == 0 { 1.0 } else { 0.6 };
            let density = FACE * facing_share * fog(hit.distance).sqrt();
            let mut y = top.max(0.0).floor();
            while y < bottom.min(h as f32) {
                let threshold = (BAYER[(y as usize % 4) * 4 + col % 4] as f32 + 0.5) / 16.0;
                if threshold < density {
                    self.plot(col, y, bright * 0.35, ink);
                }
                y += 1.0;
            }
            // Where the wall meets the floor and the ceiling, joined to the next column so a
            // steep edge stays a line.
            let next = (col + 1 < w).then(|| struck[col + 1]);
            let same_plane = next.is_some_and(|n| n.plane == hit.plane && n.face == hit.face);
            let (next_top, next_bottom) = match next {
                Some(n) if same_plane => span(&n),
                _ => (top, bottom),
            };
            self.upright(
                col,
                top.min(next_top).floor(),
                top.max(next_top).floor() + 1.0,
                bright,
                ink,
            );
            self.upright(
                col,
                bottom.min(next_bottom).floor() - 1.0,
                bottom.max(next_bottom).floor(),
                bright,
                ink,
            );
            // The courses of blocks along the wall, fainter than its edges.
            for course in 1..COURSES {
                let share = course as f32 / COURSES as f32;
                let (here, there) = (
                    top + (bottom - top) * share,
                    next_top + (next_bottom - next_top) * share,
                );
                self.upright(
                    col,
                    here.min(there).floor(),
                    here.max(there).floor() + 1.0,
                    bright * 0.3,
                    ink,
                );
            }
            let Some(next) = next else {
                continue;
            };
            if !same_plane || (next.distance - hit.distance).abs() > 0.4 {
                // A corner or one wall in front of another: an upright edge on the nearer side.
                let (x, near) = if next.distance < hit.distance {
                    (col + 1, next)
                } else {
                    (col, hit)
                };
                let (top, bottom) = span(&near);
                let (ink, bright) = wall_ink(near.distance);
                self.upright(x, top, bottom, bright, ink);
            } else if next.along.floor() != hit.along.floor() {
                // The joint between two blocks of a straight wall, fainter than its edges.
                let (x, near) = if next.distance < hit.distance {
                    (col + 1, next)
                } else {
                    (col, hit)
                };
                let (top, bottom) = span(&near);
                let (ink, bright) = wall_ink(near.distance);
                self.upright(x, top + 1.0, bottom - 1.0, bright * 0.45, ink);
            }
        }
        self.struck = struck;
        self.map = Some(map);
    }

    /// Puts the view into the grid as Braille patterns, each cell in the colour of its brightest
    /// dots.
    fn draw(&self, grid: &mut Grid) {
        let cols = self.w / 2;
        for row in 0..self.h / 4 {
            for col in 0..cols {
                let (mut dots, mut most, mut rgb) = (0u8, 0.0f32, [0.0f32; 3]);
                for down in 0..4 {
                    for across in 0..2 {
                        let i = (row * 4 + down) * self.w + col * 2 + across;
                        let amount = self.light[i];
                        if amount < 1.0 / 16.0 {
                            continue;
                        }
                        dots |= 1 << (down * 2 + across);
                        if amount > most {
                            most = amount;
                            rgb = self.ink[i];
                        }
                    }
                }
                if dots == 0 {
                    continue;
                }
                // Stepped, so a line creeping across the cells changes each only now and then.
                let level = (most * 16.0).round() / 16.0;
                grid.put(col as f32, row as f32, braille(dots), mix(BG, rgb, level));
            }
        }
    }
}

impl Variation for Maze {
    fn id(&self) -> &'static str {
        "maze"
    }

    fn reset(&mut self, _layout: &Layout, seed: u64) {
        self.seed = seed;
        self.turn = None;
    }

    fn step_hz(&self) -> f64 {
        20.0
    }

    fn compose(&mut self, frame: Frame<'_>, grid: &mut Grid) {
        let layout = frame.layout;
        let (t, turn) = if frame.still {
            (IN + WALK * 0.35, 0)
        } else {
            (
                frame.elapsed.rem_euclid(TURN),
                (frame.elapsed / TURN).floor().max(0.0) as u64,
            )
        };
        if self.turn != Some(turn) || self.w != layout.cols.max(0) as usize * 2 {
            self.start(layout, turn);
        }
        let Some(walk) = &self.walk else {
            return;
        };
        let walked = smooth(((t - IN) / WALK) as f32);
        let (place, heading) = walk.at(walked * walk.length());
        let (light, banner_light, letters) = if t < IN {
            ((t / IN) as f32, (t / IN) as f32, 0.0)
        } else if t < IN + WALK {
            (1.0, 1.0, 0.0)
        } else if t < IN + WALK + ARRIVE {
            let share = ((t - IN - WALK) / ARRIVE) as f32;
            (1.0 - share, 1.0, share)
        } else {
            let out = ((TURN - t) / OUT).min(1.0) as f32;
            (0.0, 0.0, out)
        };
        self.cast(layout, place, heading, light, banner_light);
        self.draw(grid);
        if letters > 0.0 {
            let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
            for letter in &layout.letters {
                let across = (letter.col - lx) as f32 / lw as f32;
                grid.put(
                    letter.col as f32,
                    letter.row as f32,
                    letter.ch,
                    mix(BG, gradient(across), letters.max(banner_light)),
                );
            }
        }
    }

    fn turn(&self) -> Option<f64> {
        Some(TURN)
    }

    fn preview_at(&self) -> f64 {
        IN + WALK * 0.3
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::saver::{
        layout,
        tests::{at_home, composed, drawn},
    };

    #[test]
    fn the_maze_is_connected_and_the_way_runs_through_open_squares() {
        for seed in 0..20 {
            let (map, way) = carve(seed);
            assert!(way.len() > 8, "a long way: {}", way.len());
            assert_eq!(way[0], (0, 0));
            for pair in way.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let between = ((a.0 + b.0 + 1) as i32, (a.1 + b.1 + 1) as i32);
                assert!(!map.solid(between.0, between.1), "the wall between is open");
                assert_eq!(a.0.abs_diff(b.0) + a.1.abs_diff(b.1), 1);
            }
        }
    }

    #[test]
    fn it_walks_the_maze_and_arrives_at_the_logo() {
        let layout = layout(1920, 1200).unwrap();
        let mut maze = Maze::default();
        maze.reset(&layout, 5);
        let walking = composed(&mut maze, &layout, IN + WALK * 0.5, false);
        assert!(drawn(&walking) > (layout.cols * layout.rows) as usize / 4);
        let arrived = composed(&mut maze, &layout, TURN - OUT - 0.5, false);
        assert_eq!(at_home(&arrived, &layout), layout.letters.len());
        // Just before the letters take over, the logo on the end wall already stands on them.
        let mut ahead = Maze::default();
        ahead.reset(&layout, 5);
        let _ = composed(&mut ahead, &layout, 0.0, false);
        let facing = composed(&mut ahead, &layout, IN + WALK - 0.01, false);
        let on_logo = layout
            .letters
            .iter()
            .filter(|l| facing.at(l.col as f32, l.row as f32).ch != ' ')
            .count();
        assert!(
            on_logo > layout.letters.len() * 8 / 10,
            "{on_logo} of {}",
            layout.letters.len()
        );
    }

    #[test]
    fn the_heading_turns_smoothly_round_corners() {
        let walk = Walk::new(vec![(1.5, 1.5), (3.5, 1.5), (3.5, 3.5)]);
        let (_, before) = walk.at(1.0);
        let (_, at) = walk.at(2.0);
        let (_, after) = walk.at(3.0);
        assert!(before.abs() < 1e-3);
        assert!(
            (at - FRAC_PI_2 / 2.0).abs() < 0.05,
            "half way round at the corner: {at}"
        );
        assert!((after - FRAC_PI_2).abs() < 1e-3);
    }

    #[test]
    fn reduced_motion_stands_still_in_the_maze() {
        let layout = layout(1920, 1200).unwrap();
        let mut maze = Maze::default();
        maze.reset(&layout, 5);
        let first = composed(&mut maze, &layout, 0.0, true);
        let second = composed(&mut maze, &layout, 30.0, true);
        assert_eq!(first.cells, second.cells);
        assert!(drawn(&first) > 100);
    }
}
