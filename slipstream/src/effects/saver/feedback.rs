//! A picture carried from one step to the next through a motion, and quantised to cells.

use super::*;

/// A picture that persists between steps: each step starts from the last one, carried through a
/// motion and faded, and new ink is added on top. This is what makes a feedback variation feel
/// alive rather than looped: the motion is simple, and the picture it carries is not.
///
/// It holds light, not glyphs. Light is sampled between cells, so a motion of less than a cell a
/// step still moves it, and it only becomes characters in `draw`, quantised to a few levels and
/// hues, so a cell whose light fades smoothly changes its look only every few steps. That, not the
/// arithmetic, is what keeps the painting cheap.
#[derive(Default)]
pub(super) struct Feedback {
    pub(super) cols: i32,
    pub(super) rows: i32,
    /// 0..1 a cell.
    pub(super) light: Vec<f32>,
    /// Where on `palette` the ink in a cell came from, as 0..=255: stamped when ink lands and
    /// carried with it.
    pub(super) hue: Vec<u8>,
    pub(super) next_light: Vec<f32>,
    pub(super) next_hue: Vec<u8>,
}

/// How the last step's picture is carried into this one: zoomed and turned about a centre, then
/// shifted. Rows count double, so a turn is round on screen rather than in cells.
#[derive(Debug, Clone, Copy)]
pub(super) struct Motion {
    /// The centre of the zoom and the turn, in cells.
    pub(super) centre: (f32, f32),
    /// Above 1 the picture grows outwards from the centre each step; below 1 it is drawn in.
    pub(super) zoom: f32,
    /// Radians the picture turns each step.
    pub(super) turn: f32,
    /// Cells the picture moves each step, across and down.
    pub(super) shift: (f32, f32),
}

impl Motion {
    /// Where the picture that lands on (`x`, `y`) this step was on the last: the motion run
    /// backwards.
    pub(super) fn source(&self, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (
            x - self.shift.0 - self.centre.0,
            (y - self.shift.1 - self.centre.1) * 2.0,
        );
        let (sin, cos) = self.turn.sin_cos();
        let zoom = self.zoom.max(0.01);
        let (rx, ry) = ((dx * cos + dy * sin) / zoom, (dy * cos - dx * sin) / zoom);
        (self.centre.0 + rx, self.centre.1 + ry / 2.0)
    }
}

impl Feedback {
    /// Sizes the buffer to the grid and empties it.
    pub(super) fn reset(&mut self, cols: i32, rows: i32) {
        let len = (cols * rows).max(0) as usize;
        self.cols = cols;
        self.rows = rows;
        for buffer in [&mut self.light, &mut self.next_light] {
            buffer.clear();
            buffer.resize(len, 0.0);
        }
        for buffer in [&mut self.hue, &mut self.next_hue] {
            buffer.clear();
            buffer.resize(len, 0);
        }
    }

    /// Carries the picture one step: every cell takes the light from where `source` says it came
    /// from, sampled between cells, and fades by `decay`. The hue comes with it from the nearest
    /// cell.
    pub(super) fn advance(&mut self, decay: f32, source: impl Fn(f32, f32) -> (f32, f32)) {
        let (cols, rows) = (self.cols, self.rows);
        for row in 0..rows {
            for col in 0..cols {
                let (x, y) = source(col as f32, row as f32);
                let i = (row * cols + col) as usize;
                let (x0, y0) = (x.floor(), y.floor());
                let (fx, fy) = (x - x0, y - y0);
                let (c0, r0) = (x0 as i32, y0 as i32);
                let mut light = 0.0;
                let mut best = (0.0f32, 0u8);
                for (dc, dr, weight) in [
                    (0, 0, (1.0 - fx) * (1.0 - fy)),
                    (1, 0, fx * (1.0 - fy)),
                    (0, 1, (1.0 - fx) * fy),
                    (1, 1, fx * fy),
                ] {
                    let (c, r) = (c0 + dc, r0 + dr);
                    if weight <= 0.0 || !(0..cols).contains(&c) || !(0..rows).contains(&r) {
                        continue;
                    }
                    let j = (r * cols + c) as usize;
                    let lit = self.light[j] * weight;
                    light += lit;
                    if lit > best.0 {
                        best = (lit, self.hue[j]);
                    }
                }
                let light = light * decay;
                self.next_light[i] = if light < 0.004 { 0.0 } else { light };
                self.next_hue[i] = best.1;
            }
        }
        std::mem::swap(&mut self.light, &mut self.next_light);
        std::mem::swap(&mut self.hue, &mut self.next_hue);
    }

    /// Adds ink to the cell at (`x`, `y`): the brighter of what is there and `light` stays, and the
    /// new ink stamps its hue if it is at least as bright.
    pub(super) fn ink(&mut self, x: f32, y: f32, light: f32, hue: u8) {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if !(0..self.cols).contains(&col) || !(0..self.rows).contains(&row) {
            return;
        }
        let i = (row * self.cols + col) as usize;
        let light = light.clamp(0.0, 1.0);
        if light >= self.light[i] {
            self.light[i] = light;
            self.hue[i] = hue;
        }
    }

    /// Adds ink along the line from `from` to `to`, a cell at a time, brightest at `to`.
    pub(super) fn line(&mut self, from: (f32, f32), to: (f32, f32), light: f32, hue: u8) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let cells = dx.abs().max(dy.abs()).ceil().clamp(1.0, 64.0) as i32;
        for k in 0..=cells {
            let along = k as f32 / cells as f32;
            self.ink(
                from.0 + dx * along,
                from.1 + dy * along,
                light * (0.55 + 0.45 * along),
                hue,
            );
        }
    }

    /// The light in the cell at (`col`, `row`), and its hue.
    #[cfg(test)]
    pub(super) fn at(&self, col: i32, row: i32) -> (f32, u8) {
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            let i = (row * self.cols + col) as usize;
            (self.light[i], self.hue[i])
        } else {
            (0.0, 0)
        }
    }

    /// Draws the picture into `grid`: cells under `floor` stay blank, and the rest take one of
    /// `looks` by how bright they are (dimmest first) in the colour `colour` gives for their hue
    /// and that level.
    pub(super) fn draw(
        &self,
        grid: &mut Grid,
        floor: f32,
        looks: &[char],
        colour: impl Fn(u8, usize) -> [f32; 3],
    ) {
        let levels = looks.len();
        if levels == 0 {
            return;
        }
        for row in 0..self.rows.min(grid.rows) {
            for col in 0..self.cols.min(grid.cols) {
                let i = (row * self.cols + col) as usize;
                let light = self.light[i];
                if light < floor {
                    continue;
                }
                let level = (((light - floor) / (1.0 - floor).max(0.001)) * levels as f32) as usize;
                let level = level.min(levels - 1);
                grid.put(
                    col as f32,
                    row as f32,
                    looks[level],
                    colour(self.hue[i], level),
                );
            }
        }
    }
}

/// A hue on `palette` as stored in a `Feedback`, quantised to one of `steps` colours so the look
/// of a cell changes rarely.
pub(super) fn hue_colour(hue: u8, steps: u8) -> [f32; 3] {
    let steps = steps.max(1) as u16;
    let step = (hue as u16 * steps / 256) as f32;
    palette(step / steps as f32)
}

/// The byte a `Feedback` stores for a position on `palette`.
pub(super) fn hue_byte(at: f32) -> u8 {
    (at.rem_euclid(1.0) * 256.0).min(255.0) as u8
}
