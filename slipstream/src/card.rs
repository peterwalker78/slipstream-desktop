//! The glass card the way out and the login offer are both drawn on: a title, a line or two of
//! prose, a list, an optional switch, and a footer of answers.
//!
//! One card, so the two never drift apart. The answers are buttons the pointer can press, each
//! carrying the key that does the same thing inside it — keyboard first, mouse always optional —
//! and each returns where it was drawn, so a click can find it again.

use smithay::utils::{Logical, Size};

use crate::{
    icons,
    paint::{Painted, Painter},
    panel::{self, AMBER, BELOW, DARK, INK, MARGIN, MOCKUP_PX},
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
pub const WIDTH: f32 = 560.0;
pub const PADDING: f32 = 24.0;
pub const ROW_H: f32 = 30.0;
pub const FOOT_H: f32 = 60.0;

/// The footer's buttons.
const BUTTON_H: f32 = 34.0;
const BUTTON_PAD: f32 = 14.0;
const BUTTON_GAP: f32 = 10.0;
/// Between the label and the key inside a button.
const KEY_GAP: f32 = 10.0;
/// The switch that says whether to write the layout down, and its row.
const CHECK: f32 = 18.0;
const CHECK_ROW: f32 = 26.0;

pub const DIM: u32 = 0x8f98a8ff;
pub const HINT: u32 = crate::panel::HINT;
/// A quiet button, and the same under the pointer.
const QUIET: u32 = 0xffffff12;
const QUIET_LIT: u32 = 0xffffff26;
const EDGE: u32 = 0xffffff24;
/// The amber button under the pointer.
const AMBER_LIT: u32 = 0xffc978ff;
/// The key hint inside the amber button.
const ON_AMBER: u32 = 0x1a1206a0;

/// An answer on a card: a button to click, and a key that does the same thing. Enter always
/// presses `Go` and Esc always `Stay`, whichever card is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The primary answer: log out, go anyway, reopen.
    Go,
    /// Esc: leave everything alone.
    Stay,
    /// Another ten seconds.
    Wait,
    /// The switch: write the layout down.
    Remember,
    /// One of the card's rows, by its place in `rows`: the rows of a card that asks for a choice.
    Row(usize),
}

/// One line of the card.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub title: String,
    /// The note at the right, and its colour.
    pub note: Option<(String, u32)>,
}

/// One of the card's answers, as it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Btn {
    pub which: Button,
    pub label: String,
    /// The key that does the same thing, drawn inside the button.
    pub key: String,
    /// Filled amber: the one Enter presses.
    pub primary: bool,
}

/// A card in the middle of the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub title: String,
    pub note: String,
    pub rows: Vec<Row>,
    /// "and 3 more", when there are more windows than rows.
    pub more: Option<String>,
    /// The switch, on the card that still has a choice to make.
    pub remember: Option<bool>,
    pub buttons: Vec<Btn>,
    /// What the pointer is over, so it lights up.
    pub hover: Option<Button>,
    /// The row the keyboard is on, and the colour of its ring, on a card that asks for a choice.
    /// Its rows are then clickable, and the chosen one is drawn lit.
    pub selected: Option<(usize, u32)>,
}

/// A clickable box, in mockup pixels from the painted surface's corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub which: Button,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Hit {
    /// The middle of the box, in mockup pixels from the painted surface's corner: where a test
    /// aims a click, since nothing else needs to know where a hit sits.
    #[cfg(test)]
    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// A button, filled amber when it's the one Enter presses and quiet glass otherwise, with the key
/// that does the same thing inside it. Returns where it was drawn, so it can be clicked.
fn paint_button(p: &mut Painter, button: &Btn, x: f32, y: f32, w: f32, lit: bool) -> Hit {
    let (fill, ink, key_ink) = match (button.primary, lit) {
        (true, false) => (AMBER, DARK, ON_AMBER),
        (true, true) => (AMBER_LIT, DARK, ON_AMBER),
        (false, false) => (QUIET, INK, HINT),
        (false, true) => (QUIET_LIT, INK, DIM),
    };
    p.fill(x, y, w, BUTTON_H, 9.0, fill);
    if !button.primary {
        p.border(x, y, w, BUTTON_H, 9.0, 1.0, EDGE);
    }
    let label = Style::new(Face::BodyBold, 14.0, ink);
    let key = Style::new(Face::Mono, 12.0, key_ink);
    let middle = y + BUTTON_H / 2.0;
    let label_w = text::width(&button.label, &label);
    p.text(&button.label, x + BUTTON_PAD, middle, &label);
    p.text(
        &button.key,
        x + BUTTON_PAD + label_w + KEY_GAP,
        middle,
        &key,
    );
    Hit {
        which: button.which,
        x,
        y,
        w,
        h: BUTTON_H,
    }
}

/// How wide a button has to be for its label and its key.
fn button_width(button: &Btn) -> f32 {
    let label = Style::new(Face::BodyBold, 14.0, INK);
    let key = Style::new(Face::Mono, 12.0, HINT);
    2.0 * BUTTON_PAD + text::width(&button.label, &label) + KEY_GAP + text::width(&button.key, &key)
}

/// The height of a card's contents, in mockup pixels.
pub fn measure(card: &Card, note_lines: usize) -> f32 {
    let rows = card.rows.len() as f32 + if card.more.is_some() { 1.0 } else { 0.0 };
    let list = if rows > 0.0 { 14.0 + rows * ROW_H } else { 0.0 };
    let switch = if card.remember.is_some() {
        16.0 + CHECK_ROW + 20.0
    } else {
        0.0
    };
    PADDING + 30.0 + 8.0 + note_lines as f32 * 22.0 + list + switch + 10.0 + FOOT_H
}

pub fn paint(card: &Card, scale: f64) -> (Option<Painted>, Vec<Hit>) {
    let title = Style {
        tracking: 0.02,
        ..Style::new(Face::Display, 27.0, INK)
    };
    let note = Style::new(Face::Body, 15.0, DIM);
    let name = Style::new(Face::BodyBold, 15.0, INK);
    let where_ = Style::new(Face::Body, 14.0, 0x9aa3b2ff);
    let inner = WIDTH - 2.0 * PADDING;
    let lines = text::wrap(&card.note, &note, inner);
    let height = measure(card, lines.len());
    // Room around the card for its shadow, and more below it, where the shadow falls.
    let logical = Size::<i32, Logical>::from((
        ((WIDTH + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * MARGIN + BELOW) * MOCKUP_PX).ceil() as i32,
    ));
    let mut hits = Vec::new();
    let painted = Painted::new(logical, scale, |p| {
        p.f *= MOCKUP_PX;
        let (fx, fy) = (MARGIN, MARGIN);
        panel::glass(p, fx, fy, WIDTH, height);
        let x0 = fx + PADDING;
        let mut y = fy + PADDING;
        // An amber bar down the title, as the toast has: this is the one card that ends things.
        p.fill(fx + 1.0, fy + PADDING, 3.0, 30.0, 1.5, AMBER);
        p.text(&card.title, x0 + 10.0, y + 15.0, &title);
        y += 30.0 + 8.0;
        for line in &lines {
            p.text(line, x0, y + 11.0, &note);
            y += 22.0;
        }
        if !card.rows.is_empty() || card.more.is_some() {
            y += 14.0;
            for (index, row) in card.rows.iter().enumerate() {
                if let Some((chosen, ring)) = card.selected {
                    let lit = card.hover == Some(Button::Row(index));
                    let (rx, rw) = (x0 - 10.0, inner + 20.0);
                    if index == chosen {
                        p.fill(rx, y + 1.0, rw, ROW_H - 2.0, 7.0, QUIET_LIT);
                        p.border(rx, y + 1.0, rw, ROW_H - 2.0, 7.0, 1.5, ring);
                    } else if lit {
                        p.fill(rx, y + 1.0, rw, ROW_H - 2.0, 7.0, QUIET);
                    }
                    hits.push(Hit {
                        which: Button::Row(index),
                        x: rx,
                        y,
                        w: rw,
                        h: ROW_H,
                    });
                }
                let mut room = inner;
                if let Some((text, colour)) = &row.note {
                    let style = Style::new(Face::Mono, 12.0, *colour);
                    let w = text::width(text, &style);
                    p.text(text, x0 + inner - w, y + ROW_H / 2.0, &style);
                    room -= w + 14.0;
                }
                let name_w = text::width(&row.name, &name);
                p.text(&row.name, x0, y + ROW_H / 2.0, &name);
                if !row.title.is_empty() && room - name_w > 60.0 {
                    let title = text::ellipsize(&row.title, &where_, room - name_w - 12.0);
                    p.text(&title, x0 + name_w + 12.0, y + ROW_H / 2.0, &where_);
                }
                y += ROW_H;
            }
            if let Some(more) = &card.more {
                p.text(more, x0, y + ROW_H / 2.0, &note);
                y += ROW_H;
            }
        }
        if let Some(on) = card.remember {
            y += 16.0;
            hits.push(paint_switch(
                p,
                on,
                x0,
                y,
                inner,
                card.hover == Some(Button::Remember),
            ));
            // The footer is measured from the bottom, so nothing follows the switch here.
        }
        // The footer: the answers, in the same order everywhere, the one Enter presses at the
        // right where the eye ends up.
        let foot_y = fy + height - FOOT_H;
        p.fill(fx, foot_y, WIDTH, 1.0, 0.0, 0xffffff10);
        let button_y = foot_y + (FOOT_H - BUTTON_H) / 2.0;
        let mut right = fx + WIDTH - PADDING;
        for button in card.buttons.iter().rev() {
            let w = button_width(button);
            right -= w;
            hits.push(paint_button(
                p,
                button,
                right,
                button_y,
                w,
                card.hover == Some(button.which),
            ));
            right -= BUTTON_GAP;
        }
    });
    (painted, hits)
}

/// The switch that says whether to write the layout down, with what it means under it. The whole
/// row is clickable, not just the box, because a 18-pixel target is a mouse-only cruelty.
fn paint_switch(p: &mut Painter, on: bool, x: f32, y: f32, inner: f32, lit: bool) -> Hit {
    let label = Style::new(Face::Body, 15.0, INK);
    let key = Style::new(Face::Mono, 12.0, if lit { DIM } else { HINT });
    let under = Style::new(Face::Body, 12.0, HINT);
    let box_y = y + (CHECK_ROW - CHECK) / 2.0;
    if on {
        p.fill(x, box_y, CHECK, CHECK, 5.0, AMBER);
        p.icon(icons::TICK, x + 2.0, box_y + 2.0, CHECK - 4.0, Some(DARK));
    } else {
        p.fill(
            x,
            box_y,
            CHECK,
            CHECK,
            5.0,
            if lit { QUIET_LIT } else { QUIET },
        );
        p.border(x, box_y, CHECK, CHECK, 5.0, 1.0, EDGE);
    }
    let middle = y + CHECK_ROW / 2.0;
    let text_x = x + CHECK + 12.0;
    let words = "Restore this layout next time";
    p.text(words, text_x, middle, &label);
    // The key sits beside the words, as it does inside a button, not adrift at the card's edge.
    p.text(
        "R",
        text_x + text::width(words, &label) + KEY_GAP,
        middle,
        &key,
    );
    p.text(
        "Your apps come back on their workspaces next time. Not what they had open.",
        x + CHECK + 12.0,
        y + CHECK_ROW + 10.0,
        &under,
    );
    Hit {
        which: Button::Remember,
        x,
        y,
        w: inner,
        h: CHECK_ROW,
    }
}
