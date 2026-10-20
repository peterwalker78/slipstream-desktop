//! The grid of character cells, and where the logo's letters sit in it.

use super::*;

pub(super) static LOGO: &str = include_str!("../../../../assets/slipstream-logo.txt");
pub(super) static LOGO_STACKED: &str =
    include_str!("../../../../assets/slipstream-logo-stacked.txt");

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Cell {
    pub(super) ch: char,
    pub(super) rgb: [u8; 3],
}

pub(super) const BLANK: Cell = Cell {
    ch: ' ',
    rgb: BACKGROUND,
};

/// How a cell is drawn: fading from one cell to another, this many `TWEEN_LEVELS` of the way.
pub(super) type Look = (Cell, Cell, u8);

pub(super) struct Grid {
    pub(super) cols: i32,
    pub(super) rows: i32,
    pub(super) cells: Vec<Cell>,
}

impl Grid {
    pub(super) fn new(cols: i32, rows: i32) -> Self {
        Self {
            cols,
            rows,
            cells: vec![BLANK; (cols * rows).max(0) as usize],
        }
    }

    /// Empties every cell, ready for the next step, and sizes the grid to the layout. Cells are
    /// swapped in and out of the saver between steps, so the vector it comes back with may be the
    /// wrong length or none at all; it keeps whatever memory it has.
    pub(super) fn blank(&mut self, cols: i32, rows: i32) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize((cols * rows).max(0) as usize, BLANK);
    }

    /// The cell at (`x`, `y`), or a blank one where that's off the grid.
    #[cfg(test)]
    pub(super) fn at(&self, x: f32, y: f32) -> Cell {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            self.cells[(row * self.cols + col) as usize]
        } else {
            BLANK
        }
    }

    pub(super) fn put(&mut self, x: f32, y: f32, ch: char, rgb: [f32; 3]) {
        let (col, row) = (x.round() as i32, y.round() as i32);
        if (0..self.cols).contains(&col) && (0..self.rows).contains(&row) {
            self.cells[(row * self.cols + col) as usize] = Cell {
                ch,
                rgb: rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
            };
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Letter {
    pub(super) col: i32,
    pub(super) row: i32,
    pub(super) ch: char,
}

#[derive(Debug, Clone)]
pub(super) struct Layout {
    pub(super) cols: i32,
    pub(super) rows: i32,
    pub(super) cell_w: usize,
    pub(super) cell_h: usize,
    /// Where the grid starts in the buffer, centring it.
    pub(super) origin: (usize, usize),
    pub(super) letters: Vec<Letter>,
    /// The logo's column, row, width and height in cells.
    pub(super) logo: (i32, i32, i32, i32),
}

pub(super) fn art(text: &str) -> Vec<Vec<char>> {
    text.lines().map(|line| line.chars().collect()).collect()
}

pub(super) fn art_width(lines: &[Vec<char>]) -> usize {
    lines.iter().map(Vec::len).max().unwrap_or(1).max(1)
}

/// A terminal-like grid for a `w`×`h` pixel screen, with cells sized so the logo spans most of
/// its width. Narrow screens get the logo on two lines.
pub(super) fn layout(w: usize, h: usize) -> Option<Layout> {
    let full = art(LOGO);
    let mut cell_w = (w as f32 * 0.62 / art_width(&full) as f32).floor() as usize;
    let lines = if cell_w >= 5 {
        full
    } else {
        let stacked = art(LOGO_STACKED);
        cell_w = (w as f32 * 0.8 / art_width(&stacked) as f32).floor() as usize;
        stacked
    };
    let cell_w = cell_w.clamp(3, 18);
    let cell_h = cell_w * 2;
    let (cols, rows) = ((w / cell_w) as i32, (h / cell_h) as i32);
    if cols < 1 || rows < 1 {
        return None;
    }
    let (art_w, art_h) = (art_width(&lines) as i32, lines.len() as i32);
    let (col0, row0) = ((cols - art_w) / 2, (rows - art_h) / 2);
    let letters = lines
        .iter()
        .enumerate()
        .flat_map(|(r, line)| {
            line.iter()
                .enumerate()
                .filter(|(_, ch)| !ch.is_whitespace())
                .map(move |(c, &ch)| Letter {
                    col: col0 + c as i32,
                    row: row0 + r as i32,
                    ch,
                })
        })
        .collect();
    Some(Layout {
        cols,
        rows,
        cell_w,
        cell_h,
        origin: (
            (w - cols as usize * cell_w) / 2,
            (h - rows as usize * cell_h) / 2,
        ),
        letters,
        logo: (col0, row0, art_w, art_h),
    })
}

/// One Braille dot of the logo: where it is, in dots across and down the screen, and how far
/// across the logo it sits, 0 to 1.
#[derive(Debug, Clone, Copy)]
pub(super) struct LogoDot {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) across: f32,
}

/// Every Braille dot the logo's letters cover, with a part block covering only its part of the
/// cell.
pub(super) fn logo_dots(layout: &Layout) -> Vec<LogoDot> {
    let (lx, lw) = (layout.logo.0, layout.logo.2.max(1));
    let mut dots = Vec::new();
    for letter in &layout.letters {
        if letter.col < 0 || letter.row < 0 {
            continue;
        }
        let across = (letter.col - lx) as f32 / lw as f32;
        let lit = quarters(letter.ch).unwrap_or([true; 4]);
        for down in 0..4 {
            for side in 0..2 {
                if lit[down / 2 * 2 + side] {
                    dots.push(LogoDot {
                        x: letter.col as usize * 2 + side,
                        y: letter.row as usize * 4 + down,
                        across,
                    });
                }
            }
        }
    }
    dots
}
