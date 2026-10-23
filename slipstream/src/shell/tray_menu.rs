//! A tray item's menu, drawn by Slipstream: a small card under the item's chip in the
//! notification centre, with each submenu in a card of its own beside the entry that opens it,
//! on whichever side has room.
//!
//! The keys are a menu's: ↑/↓ (or Tab) move, → or Enter opens a submenu and ← or Esc closes it,
//! Enter or Space chooses, an entry's underlined letter chooses it straight away, and Esc on the
//! first card closes the menu. The pointer chooses what it is over and opens submenus as it goes.
//!
//! The entries come from the app and are only drawn: choosing one sends the app the entry's id
//! (`tray::Command::Click`) at once and does nothing else. The chosen row then shows pressed, in
//! the accent colour, with a ring pulsing out from it, for a moment before the menu closes, so
//! the choice is seen to have landed. The centre stays open.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    icons,
    motion::{HYPR, REDUCED_FADE},
    paint::{CardStyle, Painted, Painter},
    panel::{self, DESIGN_PX, INK},
    text::{self, Face, Style},
    tray::{
        Command,
        menu::{Entry, Layout, Toggle},
    },
};

// Sizes in design pixels.
const ROW_H: f32 = 32.0;
const SEPARATOR_H: f32 = 11.0;
/// Between the card's edge and its rows.
const INSET: f32 = 5.0;
/// Between a row's edge and what's in it.
const ROW_PAD: f32 = 11.0;
const MARK: f32 = 15.0;
const ICON: f32 = 16.0;
const CHEVRON: f32 = 14.0;
const MIN_W: f32 = 184.0;
const MAX_W: f32 = 340.0;
/// Between the chip and the card, and between a card and its submenu's.
const GAP: f32 = 6.0;
/// Kept clear at the screen's edges, and below the bar.
const EDGE: f32 = 10.0;
const TOP: f32 = 48.0;
/// Room around a card for its shadow.
const MARGIN: f32 = 28.0;
const ROW_RADIUS: f32 = 7.0;
const LABEL_PX: f32 = 14.0;

/// A card fades in as it slides the last few pixels into place; closing, it fades out where it
/// is. With reduced motion both are only short fades.
const OPEN: f64 = 0.16;
const CLOSE: f64 = 0.11;
const SLIDE: f64 = 6.0;
/// How long a menu the app hasn't sent yet waits before it says it's coming.
const PATIENCE: f64 = 0.15;
/// How long a chosen entry shows pressed before the menu closes, while a ring pulses out from
/// it by `PULSE` design pixels and fades. With reduced motion it is only the pressed row, held a
/// little less long.
const CONFIRM: f64 = 0.2;
const REDUCED_CONFIRM: f64 = 0.15;
const PULSE: f32 = 9.0;

/// How long the centre stays open after an entry is chosen.
pub fn confirm_length(reduced_motion: bool) -> f64 {
    if reduced_motion {
        REDUCED_CONFIRM
    } else {
        CONFIRM
    }
}

/// The small surfaces' look, a little lighter than the centre's glass so it stands off it.
const CARD: CardStyle = CardStyle {
    radius: 12.0,
    shadow: (10.0, 34.0, 0x0000006e),
    fill: panel::NOTICE,
    edge: 0xffffff1e,
};

/// What a key, click or move asks of the centre: calls to send the app, and whether the menu is
/// still open afterwards.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub commands: Vec<Command>,
    pub end: End,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum End {
    #[default]
    Open,
    /// Put away without choosing anything.
    Closed,
    /// An entry was chosen: the menu and the centre both close.
    Chosen,
}

impl Outcome {
    fn send(command: Command) -> Self {
        Self {
            commands: vec![command],
            end: End::Open,
        }
    }

    fn ended(end: End) -> Self {
        Self {
            commands: Vec::new(),
            end,
        }
    }
}

/// One open card: the entry it lists the entries of (0, the root, for the first), the entry
/// chosen in it, and the first row in view.
#[derive(Debug, Clone, PartialEq)]
struct Level {
    parent: i32,
    chosen: Option<i32>,
    scroll: usize,
    opened_at: f64,
    /// The side it opened to: -1 left, 1 right, 0 below the chip.
    side: i8,
}

/// Everything the painted cards depend on.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    revision: u64,
    levels: Vec<(i32, Option<i32>, usize)>,
    waiting: bool,
    failed: bool,
    size: Size<i32, Logical>,
    scale: f64,
    ring: u32,
    accent: u32,
    /// The entry chosen, painted pressed.
    confirmed: Option<i32>,
}

/// A painted card and where its rows are, in logical pixels from the output's corner.
struct Card {
    painted: Painted,
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    rows: Vec<(i32, Rectangle<f64, Logical>)>,
    level: usize,
}

/// A card that closed, fading where it was.
pub struct Ghost {
    painted: Painted,
    origin: Point<f64, Logical>,
    since: f64,
}

impl Ghost {
    /// It, `now`, or `None` once it has faded.
    pub fn element<R>(
        &self,
        renderer: &mut R,
        now: f64,
        reduced_motion: bool,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let length = if reduced_motion { REDUCED_FADE } else { CLOSE };
        let left = 1.0 - ((now - self.since) / length).clamp(0.0, 1.0);
        (left > 0.0).then(|| self.painted.element(renderer, self.origin, left as f32))?
    }

    pub fn faded(&self, now: f64) -> bool {
        now - self.since >= CLOSE.max(REDUCED_FADE)
    }
}

pub struct Menu {
    /// The item whose menu it is: the only one it ever sends anything to.
    item: String,
    layout: Option<Layout>,
    /// Goes up with each layout, so a new one repaints.
    revision: u64,
    failed: bool,
    levels: Vec<Level>,
    /// The chip it hangs from, in logical pixels from the output's corner.
    anchor: Rectangle<f64, Logical>,
    opened_at: f64,
    /// Opened from the keyboard: the first entry is chosen as soon as there is one.
    keyboard: bool,
    shown: Option<Look>,
    cards: Vec<Card>,
    ghosts: Vec<Ghost>,
    /// The entry chosen, and when: the menu takes no more input, and shows its row pressed.
    confirmed: Option<(i32, f64)>,
    /// The ring that pulses out from the chosen row, painted once for its size.
    pulse: Option<(Painted, Size<i32, Logical>, u32)>,
}

impl Drop for Menu {
    /// However the menu goes, the app hears it closed.
    fn drop(&mut self) {
        for level in self.levels.iter().skip(1).rev() {
            crate::tray::send(Command::CloseSubmenu {
                item: self.item.clone(),
                id: level.parent,
            });
        }
        crate::tray::send(Command::CloseMenu {
            item: self.item.clone(),
        });
    }
}

impl Menu {
    /// Opens `item`'s menu under the chip at `anchor`. The app is asked for its entries
    /// (`tray::Command::OpenMenu`) by whoever opens it.
    pub fn new(item: String, anchor: Rectangle<f64, Logical>, keyboard: bool, now: f64) -> Self {
        Self {
            item,
            layout: None,
            revision: 0,
            failed: false,
            levels: vec![Level {
                parent: 0,
                chosen: None,
                scroll: 0,
                opened_at: now,
                side: 0,
            }],
            anchor,
            opened_at: now,
            keyboard,
            shown: None,
            cards: Vec::new(),
            ghosts: Vec::new(),
            confirmed: None,
            pulse: None,
        }
    }

    pub fn item(&self) -> &str {
        &self.item
    }

    /// The app's entries, first or again. Open submenus stay open while their entries are still
    /// there, and the chosen entry stays chosen. The first card opens as the first entries land.
    pub fn set_layout(&mut self, layout: Layout, now: f64) {
        if self.layout.is_none() && !self.failed && now - self.opened_at < PATIENCE {
            self.levels[0].opened_at = now;
        }
        let keep = self
            .levels
            .iter()
            .skip(1)
            .take_while(|level| {
                layout
                    .root
                    .find(level.parent)
                    .is_some_and(|entry| entry.submenu)
            })
            .count();
        self.levels.truncate(keep + 1);
        for level in &mut self.levels {
            let rows = layout
                .root
                .find(level.parent)
                .map(|entry| &entry.children[..])
                .unwrap_or_default();
            if !level
                .chosen
                .is_some_and(|id| rows.iter().any(|row| row.id == id && row.choosable()))
            {
                level.chosen = None;
            }
        }
        self.layout = Some(layout);
        self.failed = false;
        self.revision += 1;
        if self.keyboard {
            let first = self.first_choosable(self.levels.len() - 1);
            if let Some(level) = self.levels.last_mut()
                && level.chosen.is_none()
            {
                level.chosen = first;
            }
        }
    }

    pub fn set_failed(&mut self) {
        self.failed = true;
    }

    /// The entries listed on card `level`.
    fn rows(&self, level: usize) -> &[Entry] {
        let parent = self.levels.get(level).map_or(0, |level| level.parent);
        self.layout
            .as_ref()
            .and_then(|layout| layout.root.find(parent))
            .map_or(&[], |entry| &entry.children[..])
    }

    fn first_choosable(&self, level: usize) -> Option<i32> {
        self.rows(level)
            .iter()
            .find(|row| row.choosable())
            .map(|row| row.id)
    }

    fn entry(&self, id: i32) -> Option<&Entry> {
        self.layout.as_ref()?.root.find(id)
    }

    /// A key, while the menu is open.
    pub fn key(&mut self, sym: Keysym, shift: bool, now: f64) -> Outcome {
        if self.confirmed.is_some() {
            return Outcome::default();
        }
        self.keyboard = true;
        let deepest = self.levels.len() - 1;
        match sym {
            Keysym::Escape | Keysym::Left if deepest > 0 => self.close_level(now),
            Keysym::Escape | Keysym::Menu => Outcome::ended(End::Closed),
            Keysym::F10 if shift => Outcome::ended(End::Closed),
            Keysym::Up | Keysym::ISO_Left_Tab => self.step(-1),
            Keysym::Tab if shift => self.step(-1),
            Keysym::Down | Keysym::Tab => self.step(1),
            Keysym::Home | Keysym::End => {
                let rows = self.rows(deepest);
                let pick = if sym == Keysym::Home {
                    rows.iter().find(|row| row.choosable())
                } else {
                    rows.iter().rev().find(|row| row.choosable())
                };
                let pick = pick.map(|row| row.id);
                if let Some(level) = self.levels.last_mut() {
                    level.chosen = pick.or(level.chosen);
                }
                Outcome::default()
            }
            Keysym::Right => match self.chosen() {
                Some(entry) if entry.submenu => self.open_level(entry.id, now),
                _ => Outcome::default(),
            },
            Keysym::Return | Keysym::KP_Enter | Keysym::space => match self.chosen() {
                Some(entry) => self.press(entry.id, now),
                None => Outcome::default(),
            },
            _ => match sym.key_char() {
                Some(ch) if ch.is_alphanumeric() => self.mnemonic(ch, now),
                _ => Outcome::default(),
            },
        }
    }

    fn chosen(&self) -> Option<Entry> {
        let id = self.levels.last()?.chosen?;
        self.entry(id).cloned()
    }

    /// Moves the deepest card's choice by `by`, round from the end to the start.
    fn step(&mut self, by: i32) -> Outcome {
        let deepest = self.levels.len() - 1;
        let choosable: Vec<i32> = self
            .rows(deepest)
            .iter()
            .filter(|row| row.choosable())
            .map(|row| row.id)
            .collect();
        if choosable.is_empty() {
            return Outcome::default();
        }
        let level = &mut self.levels[deepest];
        let at = level
            .chosen
            .and_then(|id| choosable.iter().position(|row| *row == id));
        let count = choosable.len() as i32;
        let next = match at {
            Some(at) => (at as i32 + by).rem_euclid(count),
            None if by > 0 => 0,
            None => count - 1,
        };
        level.chosen = Some(choosable[next as usize]);
        Outcome::default()
    }

    /// An entry pressed by Enter, a click or its letter: a submenu opens, anything else is chosen.
    fn press(&mut self, id: i32, now: f64) -> Outcome {
        match self.entry(id) {
            Some(entry) if !entry.choosable() => Outcome::default(),
            Some(entry) if entry.submenu => self.open_level(id, now),
            Some(_) => {
                if let Some(level) = self.levels.last_mut() {
                    level.chosen = Some(id);
                }
                self.confirmed = Some((id, now));
                Outcome {
                    commands: vec![Command::Click {
                        item: self.item.clone(),
                        id,
                    }],
                    end: End::Chosen,
                }
            }
            None => Outcome::default(),
        }
    }

    /// The entry whose underlined letter is `ch`: chosen at once when it is the only one, else
    /// the choice moves along those that have it. With no letter underlined, the first letters
    /// of the labels do the same.
    fn mnemonic(&mut self, ch: char, now: f64) -> Outcome {
        let ch = ch.to_lowercase().next().unwrap_or(ch);
        let deepest = self.levels.len() - 1;
        let rows = self.rows(deepest);
        let marked: Vec<i32> = rows
            .iter()
            .filter(|row| row.choosable() && row.label.key() == Some(ch))
            .map(|row| row.id)
            .collect();
        if let [only] = marked[..] {
            return self.press(only, now);
        }
        let candidates = if marked.is_empty() {
            rows.iter()
                .filter(|row| {
                    row.choosable()
                        && row
                            .label
                            .text
                            .chars()
                            .next()
                            .and_then(|first| first.to_lowercase().next())
                            == Some(ch)
                })
                .map(|row| row.id)
                .collect()
        } else {
            marked
        };
        if candidates.is_empty() {
            return Outcome::default();
        }
        let level = &mut self.levels[deepest];
        let at = level
            .chosen
            .and_then(|id| candidates.iter().position(|row| *row == id));
        level.chosen = Some(candidates[at.map_or(0, |at| (at + 1) % candidates.len())]);
        Outcome::default()
    }

    /// Opens entry `id`'s submenu beside the deepest card.
    fn open_level(&mut self, id: i32, now: f64) -> Outcome {
        if self.levels.len() > crate::tray::menu::MAX_DEPTH {
            return Outcome::default();
        }
        if let Some(level) = self.levels.last_mut() {
            level.chosen = Some(id);
        }
        self.levels.push(Level {
            parent: id,
            chosen: None,
            scroll: 0,
            opened_at: now,
            side: 1,
        });
        if self.keyboard {
            let first = self.first_choosable(self.levels.len() - 1);
            if let Some(level) = self.levels.last_mut() {
                level.chosen = first;
            }
        }
        Outcome::send(Command::OpenSubmenu {
            item: self.item.clone(),
            id,
        })
    }

    /// Closes the deepest card, which fades where it was.
    fn close_level(&mut self, now: f64) -> Outcome {
        let deepest = self.levels.len() - 1;
        let Some(level) = (deepest > 0).then(|| self.levels.pop()).flatten() else {
            return Outcome::default();
        };
        if let Some(at) = self.cards.iter().position(|card| card.level == deepest) {
            let card = self.cards.remove(at);
            self.ghosts.push(Ghost {
                painted: card.painted,
                origin: card.origin,
                since: now,
            });
        }
        Outcome::send(Command::CloseSubmenu {
            item: self.item.clone(),
            id: level.parent,
        })
    }

    /// The pointer moved to (`x`, `y`): the entry under it is chosen, and a submenu opens as it
    /// passes over the entry for it. Cards deeper than the one it is over close.
    pub fn hover(&mut self, x: f64, y: f64, now: f64) -> Outcome {
        if self.confirmed.is_some() {
            return Outcome::default();
        }
        let Some((level, id)) = self.row_at(x, y) else {
            return Outcome::default();
        };
        let Some(entry) = self.entry(id).cloned() else {
            return Outcome::default();
        };
        if !entry.choosable() {
            return Outcome::default();
        }
        self.keyboard = false;
        let mut outcome = Outcome::default();
        let opens_here = self
            .levels
            .get(level + 1)
            .is_some_and(|next| next.parent == id);
        while self.levels.len() > level + 1 && !(opens_here && self.levels.len() == level + 2) {
            outcome.commands.extend(self.close_level(now).commands);
        }
        self.levels[level].chosen = Some(id);
        if entry.submenu && !opens_here {
            outcome.commands.extend(self.open_level(id, now).commands);
        }
        outcome
    }

    /// A click at (`x`, `y`). `None` when it is outside every card, which puts the menu away.
    pub fn click(&mut self, x: f64, y: f64, now: f64) -> Option<Outcome> {
        if self.confirmed.is_some() {
            return Some(Outcome::default());
        }
        if let Some((level, id)) = self.row_at(x, y) {
            if self
                .levels
                .get(level + 1)
                .is_some_and(|next| next.parent == id)
            {
                // Its submenu is already open.
                return Some(Outcome::default());
            }
            let mut outcome = self.hover(x, y, now);
            if !self.entry(id).is_some_and(|entry| entry.submenu) {
                let pressed = self.press(id, now);
                outcome.commands.extend(pressed.commands);
                outcome.end = pressed.end;
            }
            return Some(outcome);
        }
        let open = self.levels.len();
        let inside = self
            .cards
            .iter()
            .any(|card| card.level < open && card.frame.contains((x, y)));
        inside.then(Outcome::default)
    }

    fn row_at(&self, x: f64, y: f64) -> Option<(usize, i32)> {
        // Deeper cards are drawn over shallower ones. A card whose submenu the app has just taken
        // away is still painted until the next frame, but is no longer there to choose from.
        let open = self.levels.len();
        self.cards
            .iter()
            .rev()
            .filter(|card| card.level < open)
            .find_map(|card| {
                card.rows
                    .iter()
                    .find(|(_, area)| area.contains((x, y)))
                    .map(|(id, _)| (card.level, *id))
            })
    }

    /// The cards that have faded out since, kept by the centre after the menu itself has gone.
    pub fn into_ghosts(mut self, now: f64) -> Vec<Ghost> {
        let mut ghosts = std::mem::take(&mut self.ghosts);
        ghosts.extend(
            std::mem::take(&mut self.cards)
                .into_iter()
                .map(|card| Ghost {
                    painted: card.painted,
                    origin: card.origin,
                    since: now,
                }),
        );
        ghosts
    }

    /// The cards for a screen `size` logical pixels big at `scale`, deepest first, repainted
    /// when what they show changes. `icon` finds an entry's icon by name at a pixel size.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        frame: panel::Frame,
        ring: u32,
        reduced_motion: bool,
        icon: &mut dyn FnMut(&str, u32) -> Option<resvg::tiny_skia::Pixmap>,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let now = frame.time;
        self.ghosts.retain(|ghost| !ghost.faded(now));
        let waiting = self.layout.is_none() && !self.failed;
        let look = Look {
            revision: self.revision,
            levels: self
                .levels
                .iter()
                .map(|level| (level.parent, level.chosen, level.scroll))
                .collect(),
            waiting: waiting && now - self.opened_at >= PATIENCE,
            failed: self.failed,
            size: frame.size,
            scale: frame.scale,
            ring,
            accent: frame.accent,
            confirmed: self.confirmed.map(|(id, _)| id),
        };
        if self.shown.as_ref() != Some(&look) {
            let sides: Vec<i8> = self.levels.iter().map(|level| level.side).collect();
            self.repaint(&look, icon);
            // A submenu placed on the other side than its entry's chevron pointed: once more,
            // with the chevron pointing to it.
            let moved = self.levels.iter().map(|level| level.side).ne(sides);
            self.shown = (!moved).then(|| Look {
                levels: self
                    .levels
                    .iter()
                    .map(|level| (level.parent, level.chosen, level.scroll))
                    .collect(),
                ..look
            });
        }
        let mut elements = Vec::new();
        if let Some((id, since)) = self.confirmed
            && !reduced_motion
        {
            let (time, accent) = (now - since, frame.accent);
            elements.extend(self.pulse_element(renderer, id, (now, time), frame.scale, accent));
        }
        for card in self.cards.iter().rev() {
            let Some(level) = self.levels.get(card.level) else {
                continue;
            };
            let (alpha, _) = opening(now - level.opened_at, reduced_motion);
            let at = card.origin + self.card_shift(card, now, reduced_motion);
            elements.extend(card.painted.element(renderer, at, alpha));
        }
        elements.extend(
            self.ghosts
                .iter()
                .filter_map(|ghost| ghost.element(renderer, now, reduced_motion)),
        );
        elements
    }

    /// How far `card` still is from its place as it opens.
    fn card_shift(&self, card: &Card, now: f64, reduced_motion: bool) -> Point<f64, Logical> {
        let Some(level) = self.levels.get(card.level) else {
            return Point::default();
        };
        let (_, shift) = opening(now - level.opened_at, reduced_motion);
        let (dx, dy) = match level.side {
            0 => (0.0, -shift),
            side => (-shift * f64::from(side), 0.0),
        };
        Point::from((dx * DESIGN_PX as f64, dy * DESIGN_PX as f64))
    }

    /// The ring pulsing out from row `id`, `since` seconds after it was chosen, wherever the
    /// row's card is.
    fn pulse_element<R>(
        &mut self,
        renderer: &mut R,
        id: i32,
        (now, since): (f64, f64),
        scale: f64,
        accent: u32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let t = (since / CONFIRM).clamp(0.0, 1.0);
        if t >= 1.0 {
            return None;
        }
        let open = self.levels.len();
        let (card, area) = self
            .cards
            .iter()
            .filter(|card| card.level < open)
            .find_map(|card| {
                let (_, area) = card.rows.iter().find(|(row, _)| *row == id)?;
                Some((card, *area))
            })?;
        let row = Rectangle::new(area.loc + self.card_shift(card, now, false), area.size);
        let size =
            Size::<i32, Logical>::from((row.size.w.round() as i32, row.size.h.round() as i32));
        if self
            .pulse
            .as_ref()
            .is_none_or(|(painted, painted_size, painted_accent)| {
                painted.scale != scale || *painted_size != size || *painted_accent != accent
            })
        {
            let (w, h) = (size.w as f32 / DESIGN_PX, size.h as f32 / DESIGN_PX);
            let painted = Painted::new(size, scale, |p| {
                p.f *= DESIGN_PX;
                p.border(1.25, 1.25, w - 2.5, h - 2.5, ROW_RADIUS, 2.5, accent);
            })?;
            self.pulse = Some((painted, size, accent));
        }
        let (painted, ..) = self.pulse.as_ref()?;
        let (out, alpha) = pulse(t);
        let out = f64::from(out * DESIGN_PX);
        let at = row.loc - Point::from((out, out));
        let grown = Size::<i32, Logical>::from((
            (row.size.w + 2.0 * out).round() as i32,
            (row.size.h + 2.0 * out).round() as i32,
        ));
        painted.element_sized(renderer, at, grown, alpha)
    }

    fn repaint(
        &mut self,
        look: &Look,
        icon: &mut dyn FnMut(&str, u32) -> Option<resvg::tiny_skia::Pixmap>,
    ) {
        let screen = (
            look.size.w as f32 / DESIGN_PX,
            look.size.h as f32 / DESIGN_PX,
        );
        let anchor = design(self.anchor);
        let mut cards = Vec::new();
        let mut parent_frame: Option<[f32; 4]> = None;
        let mut parent_row: Option<[f32; 4]> = None;
        let shown_levels = if self.layout.is_some() {
            self.levels.len()
        } else {
            1
        };
        for level in 0..shown_levels {
            let rows: Vec<Entry> = self.rows(level).to_vec();
            let notice = if self.layout.is_none() {
                if look.failed {
                    Some("Not responding")
                } else if look.waiting {
                    Some("Waiting for the app…")
                } else {
                    return self.cards.clear();
                }
            } else if rows.is_empty() {
                Some("Nothing here")
            } else {
                None
            };
            let icon_px = (ICON * DESIGN_PX * look.scale as f32).round().max(1.0) as u32;
            let pictures: Vec<Option<resvg::tiny_skia::Pixmap>> = rows
                .iter()
                .map(|row| row.icon.as_deref().and_then(|name| icon(name, icon_px)))
                .collect();
            let sizes = Sizes::of(&rows, notice, pictures.iter().any(Option::is_some));
            let fit = sizes.fit(screen.1 - TOP - EDGE);
            // Keep the chosen entry in view.
            let chosen_at = self.levels[level]
                .chosen
                .and_then(|id| rows.iter().position(|row| row.id == id));
            let scroll = keep_in_view(self.levels[level].scroll, chosen_at, rows.len(), fit);
            self.levels[level].scroll = scroll;
            let height = sizes.height(&rows, scroll, fit);
            let (x, y) = match (parent_frame, parent_row) {
                (Some(frame), Some(row)) => {
                    let (x, y, side) = beside(frame, row, (sizes.width, height), screen);
                    self.levels[level].side = side;
                    (x, y)
                }
                _ => below(anchor, (sizes.width, height), screen),
            };
            let chosen = self.levels[level].chosen;
            let open_child = self.levels.get(level + 1).map(|next| next.parent);
            let child_side = self.levels.get(level + 1).map_or(1, |next| next.side);
            let mut row_boxes = Vec::new();
            let mut opener: Option<[f32; 4]> = None;
            let logical = Size::<i32, Logical>::from((
                ((sizes.width + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
                ((height + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
            ));
            let painted = Painted::new(logical, look.scale, |p| {
                p.f *= DESIGN_PX;
                p.card(MARGIN, MARGIN, sizes.width, height, &CARD);
                let mut ry = MARGIN + INSET;
                if let Some(notice) = notice {
                    let style = Style::new(Face::Body, LABEL_PX, panel::HINT);
                    p.text(notice, MARGIN + ROW_PAD + INSET, ry + ROW_H / 2.0, &style);
                    return;
                }
                let shown_rows = rows.iter().zip(&pictures).skip(scroll).take(fit);
                for (row, picture) in shown_rows {
                    let h = if row.separator { SEPARATOR_H } else { ROW_H };
                    let (rx, rw) = (MARGIN + INSET, sizes.width - 2.0 * INSET);
                    if row.separator {
                        p.fill(
                            rx + ROW_PAD,
                            ry + h / 2.0 - 0.5,
                            rw - 2.0 * ROW_PAD,
                            1.0,
                            0.0,
                            panel::DIVIDER,
                        );
                    } else {
                        let lit = chosen == Some(row.id) || open_child == Some(row.id);
                        let shade = if look.confirmed == Some(row.id) {
                            p.fill(rx, ry, rw, h, ROW_RADIUS, look.accent);
                            Shade::Pressed
                        } else if lit {
                            selected(p, rx, ry, rw, h, look.ring);
                            Shade::Lit
                        } else {
                            Shade::Plain
                        };
                        let leftward = open_child == Some(row.id) && child_side < 0;
                        paint_row(
                            p,
                            row,
                            picture.as_ref(),
                            &sizes,
                            [rx, ry, rw, h],
                            (shade, look.accent),
                            leftward,
                        );
                        row_boxes.push((row.id, [rx, ry, rw, h]));
                        if open_child == Some(row.id) {
                            opener = Some([rx, ry, rw, h]);
                        }
                    }
                    ry += h;
                }
                if scroll > 0 {
                    paint_more(p, MARGIN + sizes.width / 2.0, MARGIN + 3.0, true);
                }
                if scroll + fit < rows.len() {
                    paint_more(p, MARGIN + sizes.width / 2.0, MARGIN + height - 3.0, false);
                }
            });
            let Some(painted) = painted else {
                break;
            };
            let on_screen = |bx: f32, by: f32, bw: f32, bh: f32| {
                Rectangle::<f64, Logical>::new(
                    (
                        ((x + bx - MARGIN) * DESIGN_PX) as f64,
                        ((y + by - MARGIN) * DESIGN_PX) as f64,
                    )
                        .into(),
                    ((bw * DESIGN_PX) as f64, (bh * DESIGN_PX) as f64).into(),
                )
            };
            cards.push(Card {
                origin: Point::from((
                    ((x - MARGIN) * DESIGN_PX) as f64,
                    ((y - MARGIN) * DESIGN_PX) as f64,
                )),
                frame: on_screen(MARGIN, MARGIN, sizes.width, height),
                rows: row_boxes
                    .iter()
                    .map(|(id, [bx, by, bw, bh])| (*id, on_screen(*bx, *by, *bw, *bh)))
                    .collect(),
                painted,
                level,
            });
            parent_frame = Some([x, y, sizes.width, height]);
            parent_row = opener.map(|[_, by, _, bh]| [x, y + by - MARGIN, sizes.width, bh]);
            if parent_row.is_none() {
                break;
            }
        }
        self.cards = cards;
    }
}

/// How wide a card is, and what its rows make room for.
struct Sizes {
    width: f32,
    marks: bool,
    icons: bool,
    chevrons: bool,
}

impl Sizes {
    /// The card for `rows`, or for `notice` alone; `icons` when any row has an icon to show.
    fn of(rows: &[Entry], notice: Option<&str>, icons: bool) -> Self {
        let style = label_style(INK);
        let marks = rows.iter().any(|row| row.toggle != Toggle::None);
        let chevrons = rows.iter().any(|row| row.submenu);
        let label_w = match notice {
            Some(notice) => text::width(notice, &style),
            None => rows
                .iter()
                .map(|row| text::width(&row.label.text, &style))
                .fold(0.0, f32::max),
        };
        // A little room past the longest label, so it never just misses fitting.
        let mut width = 2.0 * (INSET + ROW_PAD) + label_w + 18.0;
        if marks {
            width += MARK + 10.0;
        }
        if icons {
            width += ICON + 10.0;
        }
        if chevrons {
            width += CHEVRON + 14.0;
        }
        Sizes {
            width: width.clamp(MIN_W, MAX_W),
            marks,
            icons,
            chevrons,
        }
    }

    /// How many rows fit in `room`, counting each as a full one.
    fn fit(&self, room: f32) -> usize {
        (((room - 2.0 * INSET) / ROW_H).floor() as usize).max(1)
    }

    fn height(&self, rows: &[Entry], scroll: usize, fit: usize) -> f32 {
        if rows.is_empty() {
            return ROW_H + 2.0 * INSET;
        }
        let shown: f32 = rows
            .iter()
            .skip(scroll.min(rows.len().saturating_sub(fit)))
            .take(fit)
            .map(|row| if row.separator { SEPARATOR_H } else { ROW_H })
            .sum();
        shown + 2.0 * INSET
    }
}

/// The chosen entry: tinted in the keyboard's ring colour, with a faint edge in it. Quieter than
/// a list's chosen row, since a menu's choice moves with the pointer.
fn selected(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, ring: u32) {
    let ring = ring & 0xffffff00;
    p.fill(x, y, w, h, ROW_RADIUS, ring | 0x2c);
    p.border(x, y, w, h, ROW_RADIUS, 1.0, ring | 0x80);
}

/// How a row is drawn: as it is, chosen, or pressed once the choice is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shade {
    Plain,
    Lit,
    Pressed,
}

/// The pulse `t` of the way through: how far out from the row its ring is, in design pixels, and
/// how strongly it shows. It moves fastest at first and fades fastest at the end.
fn pulse(t: f64) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    ((eased as f32) * PULSE, (1.0 - t * t) as f32)
}

fn label_style(ink: u32) -> Style {
    Style::new(Face::Body, LABEL_PX, ink)
}

/// One entry: its checkmark or radio button, icon, label with its letter underlined, and the
/// chevron of a submenu.
fn paint_row(
    p: &mut Painter,
    row: &Entry,
    picture: Option<&resvg::tiny_skia::Pixmap>,
    sizes: &Sizes,
    [x, y, w, h]: [f32; 4],
    (shade, accent): (Shade, u32),
    leftward: bool,
) {
    let centre = y + h / 2.0;
    let ink = match shade {
        _ if !row.enabled => panel::PLACEHOLDER,
        Shade::Pressed => panel::ink_on(accent),
        Shade::Lit => panel::BRIGHT,
        Shade::Plain => INK,
    };
    let mut lx = x + ROW_PAD;
    if sizes.marks {
        paint_mark(
            p,
            row.toggle,
            row.enabled,
            (shade == Shade::Pressed, accent),
            lx,
            centre,
        );
        lx += MARK + 10.0;
    }
    if sizes.icons {
        if let Some(picture) = picture {
            p.image(picture, lx, centre - ICON / 2.0);
        }
        lx += ICON + 10.0;
    }
    let style = label_style(ink);
    let room = x + w - ROW_PAD - lx - if sizes.chevrons { CHEVRON + 14.0 } else { 0.0 };
    let label = text::ellipsize(&row.label.text, &style, room);
    p.text(&label, lx, centre, &style);
    // The letter that chooses it, underlined while it is still in the label shown.
    let shown = label
        .chars()
        .count()
        .saturating_sub(usize::from(label != row.label.text));
    if let Some(at) = row.label.mnemonic.filter(|at| *at < shown) {
        let before: String = label.chars().take(at).collect();
        let letter: String = label.chars().skip(at).take(1).collect();
        let ux = lx + text::width(&before, &style);
        let uw = text::width(&letter, &style);
        let line = (ink & 0xffffff00) | 0xb0;
        p.fill(ux, text::baseline(&style, centre) + 2.0, uw, 1.2, 0.0, line);
    }
    // A submenu's chevron points the way it opened.
    if row.submenu {
        let chevron_ink = if row.enabled {
            panel::SECONDARY
        } else {
            panel::PLACEHOLDER
        };
        let body = if leftward {
            icons::BACK
        } else {
            icons::CHEVRON
        };
        let cx = x + w - ROW_PAD - CHEVRON;
        p.icon(body, cx, centre - CHEVRON / 2.0, CHEVRON, Some(chevron_ink));
    }
}

/// A checkbox or radio button, on in the accent.
/// A checkmark or radio button; on a pressed row, in the row's ink over its accent.
fn paint_mark(
    p: &mut Painter,
    toggle: Toggle,
    enabled: bool,
    (pressed, on): (bool, u32),
    x: f32,
    centre: f32,
) {
    let y = centre - MARK / 2.0;
    let ink = panel::ink_on(on);
    let (edge, accent, tick) = if pressed {
        ((ink & 0xffffff00) | 0x99, ink, on)
    } else if enabled {
        (panel::TERTIARY, on, ink)
    } else {
        (panel::PLACEHOLDER, panel::PLACEHOLDER, panel::DARK)
    };
    match toggle {
        Toggle::None => {}
        Toggle::Check(on) => {
            if on {
                p.fill(x, y, MARK, MARK, 4.0, accent);
                p.icon(icons::TICK, x + 1.5, y + 1.5, MARK - 3.0, Some(tick));
            } else {
                p.border(x + 0.5, y + 0.5, MARK - 1.0, MARK - 1.0, 4.0, 1.5, edge);
            }
        }
        Toggle::Radio(on) => {
            let r = MARK / 2.0;
            if on {
                p.border(x + 0.5, y + 0.5, MARK - 1.0, MARK - 1.0, r, 1.5, accent);
                p.fill(x + r - 3.5, y + r - 3.5, 7.0, 7.0, 3.5, accent);
            } else {
                p.border(x + 0.5, y + 0.5, MARK - 1.0, MARK - 1.0, r, 1.5, edge);
            }
        }
    }
}

/// A small chevron at the top or bottom of a card that has more rows than it shows.
fn paint_more(p: &mut Painter, x: f32, y: f32, up: bool) {
    let body = if up {
        r#"<path d="M7 14l5-5 5 5"/>"#
    } else {
        r#"<path d="M7 10l5 5 5-5"/>"#
    };
    p.icon(body, x - 6.0, y - 6.0, 12.0, Some(panel::HINT));
}

/// A box in logical pixels, in design pixels.
fn design(area: Rectangle<f64, Logical>) -> [f32; 4] {
    let d = DESIGN_PX as f64;
    [
        (area.loc.x / d) as f32,
        (area.loc.y / d) as f32,
        (area.size.w / d) as f32,
        (area.size.h / d) as f32,
    ]
}

/// Where a card `size` big goes under the chip at `anchor`: lined up with its left edge, and
/// above it if there's no room below, never off the screen.
fn below([ax, ay, _, ah]: [f32; 4], (w, h): (f32, f32), (sw, sh): (f32, f32)) -> (f32, f32) {
    let x = ax.min(sw - EDGE - w).max(EDGE);
    let under = ay + ah + GAP;
    let y = if under + h <= sh - EDGE {
        under
    } else if ay - GAP - h >= TOP {
        ay - GAP - h
    } else {
        (sh - EDGE - h).max(TOP)
    };
    (x, y)
}

/// Where a submenu's card goes beside its parent card `frame`, level with the entry `row` that
/// opens it: to the right if it fits, else to the left. Returns the side too.
fn beside(
    [fx, _, fw, _]: [f32; 4],
    [_, ry, _, _]: [f32; 4],
    (w, h): (f32, f32),
    (sw, sh): (f32, f32),
) -> (f32, f32, i8) {
    let right = fx + fw + GAP - 2.0 * INSET;
    let (x, side) = if right + w <= sw - EDGE {
        (right, 1)
    } else {
        ((fx - w - GAP + 2.0 * INSET).max(EDGE), -1)
    };
    let y = (ry - INSET).min(sh - EDGE - h).max(TOP);
    (x, y, side)
}

/// The first row in view, so that `chosen` is, out of `count` with `fit` in view.
fn keep_in_view(scroll: usize, chosen: Option<usize>, count: usize, fit: usize) -> usize {
    let most = count.saturating_sub(fit);
    let scroll = scroll.min(most);
    match chosen {
        Some(at) if at < scroll => at,
        Some(at) if at >= scroll + fit => (at + 1 - fit).min(most),
        _ => scroll,
    }
}

/// A card `since` seconds after it opened: its opacity, and how far short of its place it is,
/// in design pixels.
fn opening(since: f64, reduced_motion: bool) -> (f32, f64) {
    if reduced_motion {
        ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
    } else {
        let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
        (eased.clamp(0.0, 1.0) as f32, SLIDE * (1.0 - eased).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::menu::tests::{labelled, layout, node, separator};
    use zbus::zvariant::Value;

    const ITEM: &str = "org.example.StatusNotifierItem-4242-1/StatusNotifierItem";

    fn anchor() -> Rectangle<f64, Logical> {
        Rectangle::new((1040.0, 300.0).into(), (60.0, 52.0).into())
    }

    /// A menu like a sync app's: a couple of entries, a checkbox, a radio pair in a submenu, a
    /// separator and one that can't be chosen.
    fn sample() -> Layout {
        layout(vec![
            labelled(1, "_Open folder"),
            labelled(2, "rm -rf ~; $(reboot) https://example.org"),
            node(
                3,
                &[
                    ("label", Value::from("_Pause syncing")),
                    ("toggle-type", Value::from("checkmark")),
                    ("toggle-state", Value::from(0i32)),
                ],
                Vec::new(),
            ),
            separator(4),
            node(
                5,
                &[
                    ("label", Value::from("_Speed")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![
                    node(
                        6,
                        &[
                            ("label", Value::from("_Fast")),
                            ("toggle-type", Value::from("radio")),
                            ("toggle-state", Value::from(1i32)),
                        ],
                        Vec::new(),
                    ),
                    node(
                        7,
                        &[
                            ("label", Value::from("_Gentle")),
                            ("toggle-type", Value::from("radio")),
                        ],
                        Vec::new(),
                    ),
                ],
            ),
            node(
                8,
                &[
                    ("label", Value::from("_Quit")),
                    ("enabled", Value::from(false)),
                ],
                Vec::new(),
            ),
        ])
    }

    fn menu() -> Menu {
        let mut menu = Menu::new(ITEM.into(), anchor(), true, 0.0);
        menu.set_layout(sample(), 0.0);
        menu
    }

    #[test]
    fn a_menu_entry_only_ever_sends_its_own_app_its_own_event() {
        let mut menu = menu();
        // The entry whose label reads like a command, chosen from the keyboard.
        menu.key(Keysym::Down, false, 0.0);
        let chosen = menu.key(Keysym::Return, false, 0.0);
        assert_eq!(
            chosen,
            Outcome {
                commands: vec![Command::Click {
                    item: ITEM.into(),
                    id: 2
                }],
                end: End::Chosen,
            },
            "nothing but the item's own entry id, to the item itself"
        );
        // Whatever is pressed, no command names anything but this item.
        let mut menu = self::menu();
        for sym in [
            Keysym::Down,
            Keysym::Right,
            Keysym::Down,
            Keysym::Return,
            Keysym::p,
            Keysym::s,
            Keysym::f,
        ] {
            for command in menu.key(sym, false, 0.0).commands {
                let item = match &command {
                    Command::Click { item, .. }
                    | Command::OpenSubmenu { item, .. }
                    | Command::CloseSubmenu { item, .. }
                    | Command::CloseMenu { item } => item,
                    other => panic!("a menu sent {other:?}"),
                };
                assert_eq!(item, ITEM);
            }
        }
    }

    #[test]
    fn the_keys_move_skip_what_cant_be_chosen_and_wrap() {
        let mut menu = menu();
        assert_eq!(
            menu.levels[0].chosen,
            Some(1),
            "the first, opened from the keyboard"
        );
        menu.key(Keysym::Up, false, 0.0);
        assert_eq!(
            menu.levels[0].chosen,
            Some(5),
            "round to the last that can be chosen"
        );
        menu.key(Keysym::Down, false, 0.0);
        assert_eq!(menu.levels[0].chosen, Some(1));
        menu.key(Keysym::End, false, 0.0);
        assert_eq!(menu.levels[0].chosen, Some(5));
    }

    #[test]
    fn right_opens_a_submenu_and_left_closes_it() {
        let mut menu = menu();
        menu.key(Keysym::End, false, 0.0);
        let opened = menu.key(Keysym::Right, false, 0.0);
        assert_eq!(
            opened.commands,
            [Command::OpenSubmenu {
                item: ITEM.into(),
                id: 5
            }]
        );
        assert_eq!(menu.levels.len(), 2);
        assert_eq!(menu.levels[1].chosen, Some(6));
        let closed = menu.key(Keysym::Left, false, 0.0);
        assert_eq!(
            closed.commands,
            [Command::CloseSubmenu {
                item: ITEM.into(),
                id: 5
            }]
        );
        assert_eq!(menu.key(Keysym::Escape, false, 0.0).end, End::Closed);
    }

    #[test]
    fn an_underlined_letter_chooses_its_entry_and_opens_a_submenu() {
        let mut menu = menu();
        assert_eq!(
            menu.key(Keysym::p, false, 0.0).commands,
            [Command::Click {
                item: ITEM.into(),
                id: 3
            }]
        );
        let mut menu = self::menu();
        menu.key(Keysym::s, false, 0.0);
        assert_eq!(menu.levels.len(), 2, "Speed's submenu");
        assert_eq!(
            menu.key(Keysym::g, false, 0.0).commands,
            [Command::Click {
                item: ITEM.into(),
                id: 7
            }]
        );
        let mut menu = self::menu();
        assert!(
            menu.key(Keysym::q, false, 0.0).commands.is_empty(),
            "Quit can't be chosen"
        );
        assert_eq!(
            menu.key(Keysym::r, false, 0.0).end,
            End::Open,
            "a first letter only moves"
        );
        assert_eq!(menu.levels[0].chosen, Some(2));
    }

    #[test]
    fn a_new_layout_keeps_what_is_open_while_it_is_still_there() {
        let mut menu = menu();
        menu.key(Keysym::s, false, 0.0);
        menu.set_layout(sample(), 0.0);
        assert_eq!(menu.levels.len(), 2);
        assert_eq!(menu.levels[1].chosen, Some(6));
        menu.set_layout(layout(vec![labelled(1, "Only")]), 0.0);
        assert_eq!(menu.levels.len(), 1, "the submenu went");
    }

    fn painted(menu: &mut Menu) {
        let look = Look {
            revision: menu.revision,
            levels: Vec::new(),
            waiting: false,
            failed: false,
            size: (1536, 960).into(),
            scale: 1.25,
            ring: panel::FOCUS,
            accent: panel::ACCENT,
            confirmed: menu.confirmed.map(|(id, _)| id),
        };
        menu.repaint(&look, &mut |_, _| None);
    }

    #[test]
    fn the_pointer_over_a_submenu_the_app_took_away_chooses_nothing() {
        let mut menu = menu();
        menu.key(Keysym::s, false, 0.0);
        painted(&mut menu);
        let (_, row) = menu.cards[1].rows[0];
        let (x, y) = (row.loc.x + 5.0, row.loc.y + 5.0);
        assert_eq!(menu.hover(x, y, 0.0), Outcome::default(), "already chosen");
        menu.set_layout(layout(vec![labelled(1, "Only")]), 0.0);
        assert_eq!(menu.hover(x, y, 0.0), Outcome::default());
        assert_eq!(menu.click(x, y, 0.0), None, "nothing there now");
    }

    #[test]
    fn the_pointer_chooses_what_it_is_over_and_opens_submenus_on_the_way() {
        let mut menu = Menu::new(ITEM.into(), anchor(), false, 0.0);
        menu.set_layout(sample(), 0.0);
        painted(&mut menu);
        assert_eq!(
            menu.levels[0].chosen, None,
            "nothing chosen until the pointer says"
        );
        let row = |menu: &Menu, id: i32| {
            let (_, area) = menu.cards[0]
                .rows
                .iter()
                .find(|(row, _)| *row == id)
                .unwrap();
            (area.loc.x + 5.0, area.loc.y + 5.0)
        };
        let (x, y) = row(&menu, 5);
        assert_eq!(
            menu.hover(x, y, 0.0).commands,
            [Command::OpenSubmenu {
                item: ITEM.into(),
                id: 5
            }]
        );
        painted(&mut menu);
        let (x, y) = row(&menu, 3);
        assert_eq!(
            menu.hover(x, y, 0.0).commands,
            [Command::CloseSubmenu {
                item: ITEM.into(),
                id: 5
            }]
        );
        let chosen = menu.click(x, y, 0.0).unwrap();
        assert_eq!(
            chosen.commands,
            [Command::Click {
                item: ITEM.into(),
                id: 3
            }]
        );
        assert_eq!(chosen.end, End::Chosen);
        let (x, y) = row(&menu, 8);
        assert_eq!(
            menu.click(x, y, 0.0),
            Some(Outcome::default()),
            "can't be chosen"
        );
    }

    #[test]
    fn the_card_stays_on_screen_and_a_submenu_opens_where_there_is_room() {
        let screen = (1920.0, 1200.0);
        assert_eq!(
            below([1300.0, 400.0, 70.0, 60.0], (200.0, 300.0), screen),
            (1300.0, 466.0)
        );
        let (x, _) = below([1800.0, 400.0, 70.0, 60.0], (200.0, 300.0), screen);
        assert_eq!(x, 1710.0, "pulled in from the right edge");
        let (_, y) = below([1300.0, 1000.0, 70.0, 60.0], (200.0, 300.0), screen);
        assert_eq!(y, 694.0, "above the chip when there's no room below");
        let parent = [1300.0, 466.0, 200.0, 300.0];
        let row = [1300.0, 500.0, 200.0, 32.0];
        assert_eq!(beside(parent, row, (200.0, 100.0), screen).2, 1);
        let parent = [1700.0, 466.0, 200.0, 300.0];
        let (x, _, side) = beside(parent, row, (200.0, 100.0), screen);
        assert_eq!(side, -1, "to the left at the screen's right edge");
        assert!(x < 1700.0 - 190.0, "clear of the parent but for its edge");
    }

    #[test]
    fn a_long_menu_scrolls_to_keep_the_chosen_entry_in_view() {
        assert_eq!(keep_in_view(0, Some(30), 40, 20), 11);
        assert_eq!(keep_in_view(11, Some(5), 40, 20), 5);
        assert_eq!(keep_in_view(50, None, 40, 20), 20);
        assert_eq!(keep_in_view(0, Some(3), 4, 20), 0);
    }

    #[test]
    fn reduced_motion_only_fades_the_cards() {
        let (alpha, shift) = opening(0.04, true);
        assert!(alpha > 0.0 && alpha < 1.0);
        assert_eq!(shift, 0.0);
        assert_eq!(opening(REDUCED_FADE, true), (1.0, 0.0));
        assert!(opening(0.0, false).1 > 0.0, "slides in otherwise");
    }

    #[test]
    fn a_chosen_entry_is_sent_at_once_then_flashes_and_takes_no_more_input() {
        let mut menu = menu();
        let chosen = menu.key(Keysym::space, false, 1.0);
        assert_eq!(
            chosen.commands,
            vec![Command::Click {
                item: ITEM.into(),
                id: 1
            }],
            "the event goes on the keypress, before any flash"
        );
        assert_eq!(chosen.end, End::Chosen);
        assert_eq!(menu.confirmed, Some((1, 1.0)));
        assert_eq!(menu.key(Keysym::Return, false, 1.05), Outcome::default());
        assert_eq!(menu.key(Keysym::o, false, 1.05), Outcome::default());
        assert_eq!(menu.hover(0.0, 0.0, 1.05), Outcome::default());
        assert_eq!(menu.click(0.0, 0.0, 1.05), Some(Outcome::default()));
    }

    #[test]
    fn a_disabled_entry_or_a_submenus_entry_gets_no_flash() {
        let mut menu = menu();
        assert_eq!(menu.press(8, 0.0), Outcome::default(), "disabled");
        assert_eq!(menu.press(5, 0.0).end, End::Open, "opens its submenu");
        assert_eq!(menu.confirmed, None);
        assert_eq!(menu.press(6, 0.0).end, End::Chosen);
        assert_eq!(menu.confirmed, Some((6, 0.0)));
    }

    #[test]
    fn the_pulse_moves_out_and_fades_and_the_confirmation_is_brief() {
        assert_eq!(pulse(0.0), (0.0, 1.0));
        let (half_out, half_alpha) = pulse(0.5);
        assert!(half_out > PULSE / 2.0, "eases out: {half_out}");
        assert!(
            half_alpha > 0.5 && half_alpha < 1.0,
            "fades late: {half_alpha}"
        );
        assert_eq!(pulse(1.0), (PULSE, 0.0));
        assert!(confirm_length(true) < confirm_length(false));
        assert!((0.15..=0.25).contains(&confirm_length(false)));
    }
}
