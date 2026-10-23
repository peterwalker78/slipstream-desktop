//! The glass card the way out and the login offer are both drawn on: a title, a line or two of
//! prose, a list, an optional switch, and a footer of answers.
//!
//! One card, so they never drift apart. The answers are buttons the pointer can press, each
//! carrying the key that does the same thing inside it — keyboard first, mouse always optional —
//! and each returns where it was drawn, so a click can find it again.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    utils::{Logical, Point, Size},
};

use crate::{
    icons,
    paint::{Painted, Painter},
    panel::{self, BELOW, DESIGN_PX, HINT, INK, MARGIN},
    text::{self, Face, Style},
};

// Sizes in design pixels.
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

/// A box to type into, under the rows, with a line under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// What it asks for, shown in it while it is empty.
    pub label: String,
    /// What it shows of what is typed: only dots for a password.
    pub shown: String,
    /// The line under it, and whether it is a warning.
    pub line: Option<(String, bool)>,
}

/// A card in the middle of the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub title: String,
    pub note: String,
    pub rows: Vec<Row>,
    /// "and 3 more", when there are more windows than rows.
    pub more: Option<String>,
    /// Something to type, on the card that asks for a password.
    pub field: Option<Field>,
    /// The switch, on the card that still has a choice to make.
    pub remember: Option<bool>,
    pub buttons: Vec<Btn>,
    /// What the pointer is over, so it lights up.
    pub hover: Option<Button>,
    /// The row the keyboard is on, and the colour of its ring, on a card that asks for a choice.
    /// Its rows are then clickable, and the chosen one is drawn lit.
    pub selected: Option<(usize, u32)>,
}

/// A clickable box, in design pixels from the painted surface's corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub which: Button,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Hit {
    /// The middle of the box, in design pixels from the painted surface's corner: where a test
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
fn paint_button(
    p: &mut Painter,
    button: &Btn,
    (x, y, w): (f32, f32, f32),
    lit: bool,
    accent: u32,
) -> Hit {
    let (ink, key_ink) = if button.primary {
        let fill = if lit {
            panel::lighter(accent, 0.2)
        } else {
            accent
        };
        p.fill(x, y, w, BUTTON_H, panel::ROW_RADIUS, fill);
        (
            panel::ink_on(fill),
            (panel::ink_on(fill) & 0xffffff00) | 0xa0,
        )
    } else {
        panel::quiet_button(p, x, y, w, BUTTON_H, panel::ROW_RADIUS, lit);
        (INK, if lit { INK } else { HINT })
    };
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

/// The height of a card's contents, in design pixels.
pub fn measure(card: &Card, note_lines: usize) -> f32 {
    let rows = card.rows.len() as f32 + if card.more.is_some() { 1.0 } else { 0.0 };
    let list = if rows > 0.0 { 14.0 + rows * ROW_H } else { 0.0 };
    let switch = if card.remember.is_some() {
        16.0 + CHECK_ROW + 20.0
    } else {
        0.0
    };
    let field = if card.field.is_some() {
        16.0 + FIELD_H + FIELD_LINE
    } else {
        0.0
    };
    PADDING + 30.0 + 8.0 + note_lines as f32 * 22.0 + list + field + switch + 10.0 + FOOT_H
}

/// The box to type into, and the line under it.
const FIELD_H: f32 = 40.0;
const FIELD_LINE: f32 = 28.0;

/// The box to type into: dots or text in it, or what it asks for while it's empty, the amber
/// caret after them, and the line under it.
fn paint_field(
    p: &mut Painter,
    field: &Field,
    (x, y, w): (f32, f32, f32),
    (accent, attention): (u32, u32),
) {
    p.fill(x, y, w, FIELD_H, panel::ROW_RADIUS, 0xffffff14);
    p.border(x, y, w, FIELD_H, panel::ROW_RADIUS, 1.0, panel::EDGE);
    let middle = y + FIELD_H / 2.0;
    let inset = 14.0;
    let room = w - 2.0 * inset - 6.0;
    let typed = Style {
        tracking: 0.1,
        ..Style::new(Face::Body, 16.0, panel::BRIGHT)
    };
    let end = if field.shown.is_empty() {
        let label = Style::new(Face::Body, 15.0, panel::PLACEHOLDER);
        p.text(&field.label, x + inset, middle, &label);
        x + inset
    } else {
        // The end of a long entry stays in view, which is where the typing is.
        let mut shown = field.shown.as_str();
        while text::width(shown, &typed) > room {
            let mut chars = shown.chars();
            chars.next();
            shown = chars.as_str();
        }
        p.text(shown, x + inset, middle, &typed);
        x + inset + text::width(shown, &typed)
    };
    p.fill(end + 2.0, middle - 10.0, 2.0, 20.0, 0.0, accent);
    if let Some((line, warning)) = &field.line {
        let style = Style::new(Face::Body, 13.0, if *warning { attention } else { HINT });
        let line = text::ellipsize(line, &style, w);
        p.text(&line, x, y + FIELD_H + FIELD_LINE / 2.0 + 2.0, &style);
    }
}

pub fn paint(
    card: &Card,
    (accent, attention): (u32, u32),
    scale: f64,
) -> (Option<Painted>, Vec<Hit>) {
    let title = Style {
        tracking: 0.02,
        ..Style::new(Face::Display, 27.0, INK)
    };
    let note = Style::new(Face::Body, 15.0, HINT);
    let name = Style::new(Face::BodyBold, 15.0, INK);
    let where_ = Style::new(Face::Body, 14.0, panel::TERTIARY);
    let inner = WIDTH - 2.0 * PADDING;
    let lines = text::wrap(&card.note, &note, inner);
    let height = measure(card, lines.len());
    // Room around the card for its shadow, and more below it, where the shadow falls.
    let logical = Size::<i32, Logical>::from((
        ((WIDTH + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
        ((height + 2.0 * MARGIN + BELOW) * DESIGN_PX).ceil() as i32,
    ));
    let mut hits = Vec::new();
    let painted = Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        let (fx, fy) = (MARGIN, MARGIN);
        panel::glass(p, fx, fy, WIDTH, height);
        let x0 = fx + PADDING;
        let mut y = fy + PADDING;
        // A bar of the accent down the title, as the toast has: this is the one card that ends
        // things.
        p.fill(fx + 1.0, fy + PADDING, 3.0, 30.0, 1.5, accent);
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
                        panel::selected_row(p, rx, y + 1.0, rw, ROW_H - 2.0, ring);
                    } else if lit {
                        p.fill(
                            rx,
                            y + 1.0,
                            rw,
                            ROW_H - 2.0,
                            panel::ROW_RADIUS,
                            panel::QUIET,
                        );
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
        if let Some(field) = &card.field {
            y += 16.0;
            paint_field(p, field, (x0, y, inner), (accent, attention));
            y += FIELD_H + FIELD_LINE;
        }
        if let Some(on) = card.remember {
            y += 16.0;
            hits.push(paint_switch(
                p,
                (on, accent),
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
        p.fill(fx, foot_y, WIDTH, 1.0, 0.0, panel::DIVIDER);
        let button_y = foot_y + (FOOT_H - BUTTON_H) / 2.0;
        let mut right = fx + WIDTH - PADDING;
        for button in card.buttons.iter().rev() {
            let w = button_width(button);
            right -= w;
            hits.push(paint_button(
                p,
                button,
                (right, button_y, w),
                card.hover == Some(button.which),
                accent,
            ));
            right -= BUTTON_GAP;
        }
    });
    (painted, hits)
}

/// The switch that says whether to write the layout down, with what it means under it. The whole
/// row is clickable, not just the box, because an 18-pixel box is too small a target.
fn paint_switch(
    p: &mut Painter,
    (on, accent): (bool, u32),
    x: f32,
    y: f32,
    inner: f32,
    lit: bool,
) -> Hit {
    let label = Style::new(Face::Body, 15.0, INK);
    let key = Style::new(Face::Mono, 12.0, if lit { INK } else { HINT });
    let under = Style::new(Face::Body, 12.0, HINT);
    let box_y = y + (CHECK_ROW - CHECK) / 2.0;
    if on {
        p.fill(x, box_y, CHECK, CHECK, 5.0, accent);
        let tick = panel::ink_on(accent);
        p.icon(icons::TICK, x + 2.0, box_y + 2.0, CHECK - 4.0, Some(tick));
    } else {
        panel::quiet_button(p, x, box_y, CHECK, CHECK, 5.0, lit);
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

/// A card on screen: the button under the pointer and, from its last paint, where it sits, what
/// can be clicked on it, and the paint itself, kept until what it shows (`K`), the screen or the
/// scale changes.
pub struct OnScreen<K = Card> {
    pub hovered: Option<Button>,
    /// Where the card's corner settles, which is what a click is measured against: a button
    /// that moved under the pointer as the card rose into place would be worse than one that
    /// waits for it.
    pub at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<Shown<K>>,
    painted: Option<Painted>,
}

/// What a card was last painted as: what it showed, the screen, the scale, and its accent and
/// secondary colour.
type Shown<K> = (K, Size<i32, Logical>, f64, (u32, u32));

impl<K> Default for OnScreen<K> {
    fn default() -> Self {
        Self {
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }
}

impl<K: PartialEq> OnScreen<K> {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget(&mut self) {
        self.shown = None;
    }

    /// Nothing on screen, and nothing kept.
    pub fn clear(&mut self) {
        self.shown = None;
        self.painted = None;
        self.hits.clear();
    }

    /// What is under (`x`, `y`), in logical pixels from the screen's corner.
    pub fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let px = DESIGN_PX as f64;
        let (lx, ly) = (((x - self.at.x) / px) as f32, ((y - self.at.y) / px) as f32);
        self.hits
            .iter()
            .find(|hit| hit.contains(lx, ly))
            .map(|hit| hit.which)
    }

    /// The pointer is over `over`. Returns whether that changed, so the caller only redraws when
    /// the card has something new to show.
    pub fn hover_over(&mut self, over: Option<Button>) -> bool {
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// The pointer moved to (`x`, `y`). Returns whether what it is over changed.
    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        self.hover_over(self.hit(x, y))
    }

    /// Paints `shows` with `paint`, in the frame's accent and secondary colour, unless it, the
    /// screen, the scale and the colours are what was last painted. Returns the paint's size.
    pub fn paint(
        &mut self,
        shows: K,
        frame: crate::panel::Frame,
        paint: impl FnOnce(&K, (u32, u32), f64) -> (Option<Painted>, Vec<Hit>),
    ) -> Option<Size<i32, Logical>> {
        let inks = (frame.accent, frame.attention);
        let fresh = self
            .shown
            .as_ref()
            .is_some_and(|(shown, size, scale, painted)| {
                *shown == shows && *size == frame.size && *scale == frame.scale && *painted == inks
            });
        if !fresh {
            (self.painted, self.hits) = paint(&shows, inks, frame.scale);
            self.shown = Some((shows, frame.size, frame.scale, inks));
        }
        self.painted.as_ref().map(|painted| painted.logical)
    }

    /// The paint with its corner settling at `at`, and `rise` design pixels above it at `alpha`
    /// while it comes in.
    pub fn place<R>(
        &mut self,
        renderer: &mut R,
        at: Point<f64, Logical>,
        (alpha, rise): (f32, f64),
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.at = at;
        let shown = at + Point::from((0.0, rise * DESIGN_PX as f64));
        self.painted.as_ref()?.element(renderer, shown, alpha)
    }
}

impl OnScreen<Card> {
    /// `card` in the middle of the screen in `frame`, coming in as `opening` says (its opacity,
    /// and how far above its place it is, in design pixels).
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        card: Card,
        frame: crate::panel::Frame,
        opening: (f32, f64),
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let size = self.paint(card, frame, paint)?;
        self.place(renderer, centred(frame.size, size), opening)
    }
}

/// Where something `size` big sits in the middle of a `screen`, never above its top.
pub fn centred(screen: Size<i32, Logical>, size: Size<i32, Logical>) -> Point<f64, Logical> {
    Point::from((
        ((screen.w - size.w) / 2) as f64,
        ((screen.h - size.h) / 2).max(0) as f64,
    ))
}
