//! The notification centre, on Super+N or a click on the bar's clock or bell: a glass panel. This
//! month's calendar, then the notifications, newest first, with Do not disturb and Clear all.
//!
//! While it's open it takes every key that isn't a binding: ↑/↓ or Tab move, ←/→ change the month
//! (Home goes back to this one), Enter or Space opens a notification or flips Do not disturb,
//! Delete dismisses one, Esc closes.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, bar,
    calendar::{self, Date},
    notices::{self, Notices},
    paint::Painted,
    panel::{self, AMBER, DARK, GAP, INK, MOCKUP_PX, PADDING},
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
const WIDTH: f32 = 500.0;
/// The panel reaches down to 10 from the bottom of the screen.
const BOTTOM: f32 = 10.0;
const CAL_PAD_X: f32 = 16.0;
const CAL_PAD_Y: f32 = 14.0;
/// The month's name and a line's margin under it.
const CAL_HEAD: f32 = 28.0;
const DOW_H: f32 = 26.0;
const CELL_H: f32 = 29.0;
const CELL_GAP: f32 = 3.0;
const HEAD_H: f32 = 24.0;
const FOOT_H: f32 = 29.0;
const CARD_GAP: f32 = 10.0;
const CARD_LINES: usize = 4;

/// Where the keyboard can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    DoNotDisturb,
    Notice(u32),
    ClearAll,
}

/// What a key or click asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Nothing,
    Close,
    ToggleDoNotDisturb,
    Open(u32),
    Dismiss(u32),
    ClearAll,
    /// One of a notification's action buttons.
    Invoke(u32, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    DoNotDisturb,
    Open(u32),
    Dismiss(u32),
    ClearAll,
    Action(u32, usize),
}

/// Everything the centre shows that it doesn't own, besides the notifications.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub today: Option<Date>,
    pub do_not_disturb: bool,
    pub reduced_motion: bool,
    /// The keyboard ring's colour, the same as the focused window's.
    pub ring: u32,
}

/// Everything the painted panel depends on.
#[derive(Clone, PartialEq)]
struct Look {
    today: Option<Date>,
    month: i32,
    do_not_disturb: bool,
    ring: u32,
    revision: u64,
    selected: Item,
    action: Option<usize>,
    scroll: usize,
    size: Size<i32, Logical>,
    scale: f64,
}

pub struct Centre {
    open: bool,
    /// When it opened, on the animation clock.
    opened_at: f64,
    /// Months from this one.
    month: i32,
    selected: Item,
    /// Which of the chosen notification's action buttons the keyboard is on, if any.
    action: Option<usize>,
    /// How many action buttons each notification drawn has.
    buttons: std::collections::HashMap<u32, usize>,
    /// The first notification in view.
    scroll: usize,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// The painted area's corner and the card, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Target, Rectangle<f64, Logical>)>,
    /// Opens with a short fade instead of rising (`Slipstream::set_reduced_motion`).
    pub reduced_motion: bool,
}

impl Default for Centre {
    fn default() -> Self {
        Self {
            open: false,
            opened_at: 0.0,
            month: 0,
            selected: Item::DoNotDisturb,
            action: None,
            buttons: std::collections::HashMap::new(),
            scroll: 0,
            shown: None,
            painted: None,
            origin: Point::default(),
            frame: Rectangle::from_size((0.0, 0.0).into()),
            targets: Vec::new(),
            reduced_motion: false,
        }
    }
}

/// The keyboard's stops, in order, for notifications `ids`.
fn items(ids: &[u32]) -> Vec<Item> {
    let mut items = vec![Item::DoNotDisturb];
    items.extend(ids.iter().map(|id| Item::Notice(*id)));
    if !ids.is_empty() {
        items.push(Item::ClearAll);
    }
    items
}

impl Centre {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens on this month, with the newest notification chosen.
    pub fn open(&mut self, now: f64, ids: &[u32]) {
        self.open = true;
        self.opened_at = now;
        self.month = 0;
        self.scroll = 0;
        self.selected = ids
            .first()
            .map_or(Item::DoNotDisturb, |id| Item::Notice(*id));
        self.shown = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = None;
        self.painted = None;
        self.targets.clear();
    }

    pub fn key(&mut self, sym: Keysym, shift: bool, ids: &[u32]) -> Request {
        let request = self.key_inner(sym, shift, ids);
        // Moving to another stop leaves the last one's buttons.
        if !matches!(sym, Keysym::Left | Keysym::Right) {
            self.action = None;
        }
        request
    }

    fn key_inner(&mut self, sym: Keysym, shift: bool, ids: &[u32]) -> Request {
        let items = items(ids);
        let at = items
            .iter()
            .position(|item| *item == self.selected)
            .unwrap_or(0);
        match sym {
            Keysym::Escape => return Request::Close,
            Keysym::Up => self.selected = items[at.saturating_sub(1)],
            Keysym::Down => self.selected = items[(at + 1).min(items.len() - 1)],
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let next = if shift || sym == Keysym::ISO_Left_Tab {
                    at + items.len() - 1
                } else {
                    at + 1
                };
                self.selected = items[next % items.len()];
            }
            // The month moves by page, so ←/→ never flip the calendar while a card is chosen.
            Keysym::Prior => self.month -= 1,
            Keysym::Next => self.month += 1,
            // Home comes back to this month and the first card; End goes to the last card.
            Keysym::Home | Keysym::End => {
                let cards: Vec<Item> = items
                    .iter()
                    .copied()
                    .filter(|item| matches!(item, Item::Notice(_)))
                    .collect();
                let card = if sym == Keysym::Home {
                    self.month = 0;
                    cards.first()
                } else {
                    cards.last()
                };
                if let Some(card) = card {
                    self.selected = *card;
                }
            }
            // ← and → walk the chosen notification's action buttons, and back to the card.
            Keysym::Left | Keysym::Right => {
                if let Item::Notice(id) = self.selected {
                    let count = self.buttons.get(&id).copied().unwrap_or(0);
                    self.action = match (self.action, sym == Keysym::Right) {
                        (None, true) if count > 0 => Some(0),
                        (Some(at), true) => Some((at + 1).min(count.saturating_sub(1))),
                        (Some(0), false) => None,
                        (Some(at), false) => Some(at - 1),
                        (keep, _) => keep,
                    };
                }
            }
            // Space presses as Enter does: the centre is only ever opened on purpose.
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if let (Item::Notice(id), Some(action)) = (self.selected, self.action) {
                    return Request::Invoke(id, action);
                }
                return match self.selected {
                    Item::DoNotDisturb => Request::ToggleDoNotDisturb,
                    Item::Notice(id) => Request::Open(id),
                    Item::ClearAll => Request::ClearAll,
                };
            }
            Keysym::Delete | Keysym::BackSpace => {
                if let Item::Notice(id) = self.selected {
                    return Request::Dismiss(id);
                }
            }
            _ => {}
        }
        Request::Nothing
    }

    /// A click, in logical pixels from the output's corner. Outside the card it closes.
    pub fn click(&mut self, x: f64, y: f64) -> Request {
        let target = self
            .targets
            .iter()
            .find(|(_, area)| area.contains((x, y)))
            .map(|(target, _)| *target);
        match target {
            Some(Target::DoNotDisturb) => {
                self.selected = Item::DoNotDisturb;
                Request::ToggleDoNotDisturb
            }
            Some(Target::Open(id)) => Request::Open(id),
            Some(Target::Dismiss(id)) => Request::Dismiss(id),
            Some(Target::ClearAll) => Request::ClearAll,
            Some(Target::Action(id, index)) => Request::Invoke(id, index),
            None if self.frame.contains((x, y)) => Request::Nothing,
            None => Request::Close,
        }
    }

    /// Notification `id` is going: the keyboard moves on to the one after it, else the one
    /// before, else to Do not disturb.
    pub fn forget(&mut self, id: u32, ids: &[u32]) {
        if self.selected != Item::Notice(id) {
            return;
        }
        let at = ids.iter().position(|other| *other == id);
        let next = at.and_then(|at| {
            ids.get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|before| ids.get(before)))
        });
        self.selected = next.map_or(Item::DoNotDisturb, |next| Item::Notice(*next));
    }

    /// The panel for a screen `size` logical pixels big at `scale`, repainted when what it shows
    /// changes.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        now: f64,
        facts: Facts,
        notices: &Notices,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if !self.open {
            return None;
        }
        // An app may have taken the chosen notification back.
        let ids = notices.ids();
        let gone = match self.selected {
            Item::Notice(id) => !ids.contains(&id),
            Item::ClearAll => ids.is_empty(),
            Item::DoNotDisturb => false,
        };
        if gone {
            self.selected = Item::DoNotDisturb;
        }
        let look = Look {
            today: facts.today,
            month: self.month,
            do_not_disturb: facts.do_not_disturb,
            ring: facts.ring,
            revision: notices.revision(),
            selected: self.selected,
            action: self.action,
            scroll: self.scroll,
            size,
            scale,
        };
        if self.shown.as_ref() != Some(&look) {
            self.repaint(&look, notices)?;
            self.shown = Some(Look {
                scroll: self.scroll,
                ..look
            });
        }
        let (alpha, rise) = panel::opening(now - self.opened_at, facts.reduced_motion);
        let at = Point::from((self.origin.x, self.origin.y + rise * MOCKUP_PX as f64));
        self.painted.as_ref()?.element(renderer, at, alpha)
    }

    fn repaint(&mut self, look: &Look, notices: &Notices) -> Option<()> {
        let screen_w = look.size.w as f32 / MOCKUP_PX;
        let screen_h = look.size.h as f32 / MOCKUP_PX;
        let height = (screen_h - panel::TOP - BOTTOM).max(420.0);
        let (fx, fy) = (panel::MARGIN, panel::MARGIN);
        // The card's left edge on screen, in mockup pixels.
        let left = screen_w - panel::RIGHT - WIDTH;
        let on_screen = |x: f32, y: f32, w: f32, h: f32| {
            Rectangle::<f64, Logical>::new(
                (
                    ((left + x - fx) * MOCKUP_PX) as f64,
                    ((panel::TOP + y - fy) * MOCKUP_PX) as f64,
                )
                    .into(),
                ((w * MOCKUP_PX) as f64, (h * MOCKUP_PX) as f64).into(),
            )
        };
        let logical = Size::<i32, Logical>::from((
            ((WIDTH + 2.0 * panel::MARGIN) * MOCKUP_PX).ceil() as i32,
            ((height + 2.0 * panel::MARGIN + panel::BELOW) * MOCKUP_PX).ceil() as i32,
        ));
        let listed: Vec<&notices::Notice> = notices.listed().collect();
        let mut targets = Vec::new();
        let mut buttons = std::collections::HashMap::new();
        let mut button_ring: Option<[f32; 4]> = None;
        let mut scroll = look.scroll;

        let painted = Painted::new(logical, look.scale, |p| {
            p.f *= MOCKUP_PX;
            panel::glass(p, fx, fy, WIDTH, height);
            let x0 = fx + PADDING;
            let inner = WIDTH - 2.0 * PADDING;
            let mut y = fy + PADDING;
            // Each stop's box (x, y, w, h and corner radius), for the keyboard's ring.
            let mut shapes: Vec<(Item, [f32; 5])> = Vec::new();

            // The calendar: the month shown, with today marked when it's this month.
            let shown = look
                .today
                .map(|today| (today, calendar::shift(today.year, today.month, look.month)));
            let weeks = shown.map_or(5, |(_, (year, month))| {
                calendar::grid(year, month).len() / 7
            });
            let card_h = 2.0 * CAL_PAD_Y + CAL_HEAD + DOW_H + weeks as f32 * (CELL_H + CELL_GAP);
            p.fill(x0, y, inner, card_h, 13.0, 0xffffff08);
            if let Some((today, (year, month))) = shown {
                let head_centre = y + CAL_PAD_Y + 10.0;
                let title = Style::new(Face::BodyBold, 16.0, INK);
                let month_name = calendar::MONTHS[month as usize - 1];
                p.text(
                    &format!("{month_name} {year}"),
                    x0 + CAL_PAD_X,
                    head_centre,
                    &title,
                );
                let weekday = calendar::WEEKDAYS[calendar::weekday(today) as usize];
                let note = if look.month == 0 {
                    format!("Today is {weekday} {}", today.day)
                } else {
                    let this_month = calendar::MONTHS[today.month as usize - 1];
                    format!("Today is {weekday} {} {this_month}", today.day)
                };
                let note_style = Style::new(Face::Body, 14.0, 0x8f98a8ff);
                let note_w = text::width(&note, &note_style);
                p.text(
                    &note,
                    x0 + inner - CAL_PAD_X - note_w,
                    head_centre,
                    &note_style,
                );

                let grid_x = x0 + CAL_PAD_X;
                let grid_y = y + CAL_PAD_Y + CAL_HEAD;
                let cell_w = (inner - 2.0 * CAL_PAD_X - 6.0 * CELL_GAP) / 7.0;
                let initial = Style::new(Face::Body, 12.0, 0x7d8697ff);
                for (column, letter) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
                    let cell_x = grid_x + column as f32 * (cell_w + CELL_GAP);
                    let w = text::width(letter, &initial);
                    p.text(
                        letter,
                        cell_x + (cell_w - w) / 2.0,
                        grid_y + DOW_H / 2.0,
                        &initial,
                    );
                }
                for (i, cell) in calendar::grid(year, month).iter().enumerate() {
                    let cell_x = grid_x + (i % 7) as f32 * (cell_w + CELL_GAP);
                    let cell_y = grid_y + DOW_H + CELL_GAP + (i / 7) as f32 * (CELL_H + CELL_GAP);
                    let is_today = look.month == 0 && cell.in_month && cell.day == today.day;
                    let style = if is_today {
                        p.fill(cell_x, cell_y, cell_w, CELL_H, 8.0, AMBER);
                        Style::new(Face::BodyBold, 14.0, DARK)
                    } else if cell.in_month {
                        Style::new(Face::Body, 14.0, 0xd5dae3ff)
                    } else {
                        Style::new(Face::Body, 14.0, 0x6f7888ff)
                    };
                    let style = Style {
                        tabular: true,
                        ..style
                    };
                    let label = cell.day.to_string();
                    let w = text::width(&label, &style);
                    p.text(
                        &label,
                        cell_x + (cell_w - w) / 2.0,
                        cell_y + CELL_H / 2.0,
                        &style,
                    );
                }
            }
            y += card_h + GAP;

            // "Notifications", and the Do not disturb switch.
            let heading = Style::new(Face::BodyBold, 16.0, INK);
            p.text("Notifications", x0, y + HEAD_H / 2.0, &heading);
            let switch_x = x0 + inner - 42.0;
            let (track, knob, knob_x) = if look.do_not_disturb {
                (AMBER, DARK, switch_x + 21.0)
            } else {
                (0x3a3f4bff, 0xe9ebf0ff, switch_x + 3.0)
            };
            p.fill(switch_x, y, 42.0, 24.0, 12.0, track);
            p.fill(knob_x, y + 3.0, 18.0, 18.0, 9.0, knob);
            let label = Style::new(Face::Body, 14.0, 0x9aa3b2ff);
            let label_x = switch_x - 10.0 - text::width("Do not disturb", &label);
            p.text("Do not disturb", label_x, y + HEAD_H / 2.0, &label);
            let dnd_x = label_x - 8.0;
            shapes.push((
                Item::DoNotDisturb,
                [dnd_x, y - 4.0, x0 + inner + 4.0 - dnd_x, HEAD_H + 8.0, 10.0],
            ));
            y += HEAD_H + GAP;

            // The footer, along the bottom: the keys, and Clear all.
            let foot_y = fy + height - PADDING - FOOT_H;
            let foot_centre = foot_y + FOOT_H / 2.0;
            let keys = Style::new(Face::Mono, 12.0, panel::HINT);
            let month = Style::new(Face::Body, 13.0, panel::HINT);
            let mut x = x0;
            for key in ["PgUp", "PgDn"] {
                x += p.keycap(key, x, foot_centre) + 4.0;
            }
            x += 4.0 + p.text("month", x + 4.0, foot_centre, &month) + 16.0;
            p.keycap("Esc", x, foot_centre);
            let clear_ink = if listed.is_empty() { panel::HINT } else { INK };
            let clear = Style::new(Face::Body, 14.0, clear_ink);
            let clear_w = text::width("Clear all", &clear) + 28.0;
            let clear_x = x0 + inner - clear_w;
            p.fill(clear_x, foot_y, clear_w, FOOT_H, 8.0, 0xffffff10);
            p.text("Clear all", clear_x + 14.0, foot_centre, &clear);
            if !listed.is_empty() {
                shapes.push((Item::ClearAll, [clear_x, foot_y, clear_w, FOOT_H, 8.0]));
            }

            // The notifications, newest first, from the first one scrolled to.
            let bottom = foot_y - GAP;
            if listed.is_empty() {
                scroll = 0;
                let empty = Style::new(Face::Body, 15.0, 0x7d8697ff);
                let message = "You’re all caught up.";
                let w = text::width(message, &empty);
                p.text(message, x0 + (inner - w) / 2.0, y + 49.0, &empty);
            } else {
                let heights: Vec<f32> = listed
                    .iter()
                    .map(|notice| notices::card_height(notice, inner, CARD_LINES))
                    .collect();
                // Keep the chosen notification in view.
                scroll = scroll.min(listed.len() - 1);
                let chosen = listed
                    .iter()
                    .position(|notice| Item::Notice(notice.id) == look.selected);
                if let Some(at) = chosen {
                    scroll = scroll.min(at);
                    let span = |from: usize| {
                        heights[from..=at].iter().sum::<f32>() + (at - from) as f32 * CARD_GAP
                    };
                    while scroll < at && span(scroll) > bottom - y {
                        scroll += 1;
                    }
                }
                let mut top = y;
                let mut drawn = 0;
                for (notice, h) in listed.iter().zip(&heights).skip(scroll) {
                    if top + h > bottom {
                        break;
                    }
                    p.fill(x0, top, inner, *h, 13.0, 0xffffff0d);
                    let hits = notices::paint_card(p, notice, x0, top, inner, CARD_LINES, true);
                    if let Some([x, y, w, h]) = hits.cross {
                        targets.push((Target::Dismiss(notice.id), on_screen(x, y, w, h)));
                    }
                    buttons.insert(notice.id, hits.buttons.len());
                    for (index, [x, y, w, h]) in hits.buttons.iter().copied().enumerate() {
                        targets.push((Target::Action(notice.id, index), on_screen(x, y, w, h)));
                        if look.selected == Item::Notice(notice.id) && look.action == Some(index) {
                            button_ring = Some([x, y, w, h]);
                        }
                    }
                    targets.push((Target::Open(notice.id), on_screen(x0, top, inner, *h)));
                    shapes.push((Item::Notice(notice.id), [x0, top, inner, *h, 13.0]));
                    top += h + CARD_GAP;
                    drawn += 1;
                }
                let hidden = listed.len() - drawn;
                if hidden > 0 {
                    let more = format!("{hidden} MORE");
                    let w = text::width(&more, &keys);
                    p.text(&more, x0 + (inner - w) / 2.0, foot_centre, &keys);
                }
            }

            if let Some([x, y, w, h]) = button_ring {
                panel::focus_ring(p, x, y, w, h, 7.0, look.ring);
            }
            for (item, [x, y, w, h, radius]) in &shapes {
                if *item == look.selected && look.action.is_none() {
                    panel::focus_ring(p, *x, *y, *w, *h, *radius, look.ring);
                }
                let target = match item {
                    Item::DoNotDisturb => Target::DoNotDisturb,
                    Item::ClearAll => Target::ClearAll,
                    Item::Notice(_) => continue,
                };
                targets.push((target, on_screen(*x, *y, *w, *h)));
            }
        })?;

        self.scroll = scroll;
        self.buttons = buttons;
        self.origin = on_screen(0.0, 0.0, 0.0, 0.0).loc;
        self.frame = on_screen(fx, fy, WIDTH, height);
        self.targets = targets;
        self.painted = Some(painted);
        Some(())
    }
}

impl Slipstream {
    /// Super+N, or the bar's clock or bell: opens the notification centre, or closes it.
    pub fn toggle_notification_centre(&mut self) {
        if self.centre.is_open() {
            self.centre.close();
            return;
        }
        self.close_panels();
        let now = self.clock.tick();
        let ids = self.notices.ids();
        self.centre.open(now, &ids);
        self.notices.mark_read();
        // The pop-ups would sit on top of it; their notifications are all in it.
        let gone = self.notices.hide_popups();
        crate::notify::closed(gone, crate::notify::Reason::Expired);
    }

    pub fn centre_facts(&self) -> Facts {
        Facts {
            today: self.status.lock().unwrap().today,
            do_not_disturb: self.settings.notifications.do_not_disturb,
            reduced_motion: self.centre.reduced_motion,
            ring: self.panel_ring(),
        }
    }

    pub fn centre_key(&mut self, key: Keysym, shift: bool) {
        let ids = self.notices.ids();
        let request = self.centre.key(key, shift, &ids);
        self.centre_request(request);
    }

    /// A click while the centre is open, in logical pixels from the output's corner. Outside it
    /// closes; on another of the bar's buttons, that button then does its own thing.
    pub fn centre_click(&mut self, x: f64, y: f64) {
        match self.centre.click(x, y) {
            Request::Close => {
                self.centre.close();
                let target = (!self.fullscreen_on_screen())
                    .then(|| self.focused_bar_target(x, y))
                    .flatten()
                    .filter(|target| !matches!(target, bar::Target::Clock | bar::Target::Bell));
                if let Some(target) = target {
                    self.bar_clicked(target);
                }
            }
            request => self.centre_request(request),
        }
    }

    fn centre_request(&mut self, request: Request) {
        match request {
            Request::Nothing => {}
            Request::Close => self.centre.close(),
            Request::ToggleDoNotDisturb => {
                let on = !self.settings.notifications.do_not_disturb;
                self.change_settings(|settings| settings.notifications.do_not_disturb = on);
            }
            Request::Open(id) => {
                self.centre.close();
                self.open_notice(id);
            }
            Request::Dismiss(id) => {
                let ids = self.notices.ids();
                self.centre.forget(id, &ids);
                self.dismiss_notice(id);
            }
            Request::ClearAll => {
                self.centre.forget_all();
                self.clear_notices();
            }
            Request::Invoke(id, index) => {
                let ids = self.notices.ids();
                self.centre.forget(id, &ids);
                self.invoke_notice_action(id, index);
            }
        }
    }
}

impl Centre {
    /// Every notification is going: the keyboard goes back to Do not disturb.
    fn forget_all(&mut self) {
        self.selected = Item::DoNotDisturb;
        self.scroll = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notices::Notice;

    fn notice(id: u32, body: &str) -> Notice {
        Notice {
            id,
            app: "Mail".into(),
            app_id: None,
            icon: None,
            summary: format!("Message {id}"),
            body: body.into(),
            time: "08:47".into(),
            default_action: true,
            actions: Vec::new(),
            resident: false,
            transient: false,
            version: 0,
            digest: false,
        }
    }

    fn look(centre: &Centre, notices: &Notices) -> Look {
        Look {
            today: Some(Date {
                year: 2026,
                month: 9,
                day: 11,
            }),
            month: centre.month,
            do_not_disturb: false,
            ring: panel::FOCUS,
            revision: notices.revision(),
            selected: centre.selected,
            action: centre.action,
            scroll: centre.scroll,
            size: (1536, 960).into(),
            scale: 1.25,
        }
    }

    #[test]
    fn keys_move_through_the_stops_and_change_the_month() {
        let mut centre = Centre::default();
        centre.open(0.0, &[7, 5]);
        assert_eq!(centre.selected, Item::Notice(7), "the newest first");
        assert_eq!(centre.key(Keysym::Return, false, &[7, 5]), Request::Open(7));
        centre.key(Keysym::Down, false, &[7, 5]);
        assert_eq!(
            centre.key(Keysym::Delete, false, &[7, 5]),
            Request::Dismiss(5)
        );
        centre.key(Keysym::Down, false, &[7, 5]);
        assert_eq!(centre.key(Keysym::space, false, &[7, 5]), Request::ClearAll);
        centre.key(Keysym::Tab, false, &[7, 5]);
        assert_eq!(
            centre.key(Keysym::Return, false, &[7, 5]),
            Request::ToggleDoNotDisturb,
            "Tab wraps round to the switch"
        );
        centre.key(Keysym::Prior, false, &[7, 5]);
        centre.key(Keysym::Prior, false, &[7, 5]);
        assert_eq!(centre.month, -2);
        centre.key(Keysym::Left, false, &[7, 5]);
        centre.key(Keysym::Right, false, &[7, 5]);
        assert_eq!(centre.month, -2, "←/→ leave the month alone");
        centre.key(Keysym::End, false, &[7, 5]);
        assert_eq!(centre.selected, Item::Notice(5), "the last card");
        centre.key(Keysym::Home, false, &[7, 5]);
        assert_eq!(centre.month, 0);
        assert_eq!(centre.selected, Item::Notice(7), "the first card");
        assert_eq!(centre.key(Keysym::Escape, false, &[7, 5]), Request::Close);
    }

    #[test]
    fn a_dismissed_notification_hands_the_keyboard_on() {
        let mut centre = Centre::default();
        centre.open(0.0, &[3, 2, 1]);
        centre.selected = Item::Notice(2);
        centre.forget(2, &[3, 2, 1]);
        assert_eq!(centre.selected, Item::Notice(1), "the one after");
        centre.forget(1, &[3, 1]);
        assert_eq!(centre.selected, Item::Notice(3), "else the one before");
        centre.forget(3, &[3]);
        assert_eq!(centre.selected, Item::DoNotDisturb);
    }

    #[test]
    fn clicks_open_or_dismiss_and_outside_closes() {
        let mut notices = Notices::default();
        notices.add(notice(1, "First"), 0.0, None, true);
        notices.add(notice(2, "Second"), 0.0, None, true);
        let mut centre = Centre::default();
        centre.open(0.0, &notices.ids());
        let look = look(&centre, &notices);
        centre.repaint(&look, &notices).unwrap();
        let right = centre.frame.loc.x + centre.frame.size.w;
        assert!(
            (right - 1528.0).abs() < 0.5,
            "10 mockup pixels from the edge"
        );
        let targets = centre.targets.clone();
        let area = |wanted: Target| {
            targets
                .iter()
                .find(|(target, _)| *target == wanted)
                .unwrap()
                .1
        };
        let middle = |area: Rectangle<f64, Logical>| {
            (
                area.loc.x + area.size.w / 2.0,
                area.loc.y + area.size.h / 2.0,
            )
        };
        let (x, y) = middle(area(Target::Dismiss(1)));
        assert_eq!(centre.click(x, y), Request::Dismiss(1), "the cross first");
        let open = area(Target::Open(1));
        assert_eq!(
            centre.click(open.loc.x + 10.0, open.loc.y + open.size.h - 4.0),
            Request::Open(1)
        );
        let (x, y) = middle(area(Target::ClearAll));
        assert_eq!(centre.click(x, y), Request::ClearAll);
        assert_eq!(centre.click(100.0, 500.0), Request::Close);
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_chosen_one_in_view() {
        let mut notices = Notices::default();
        let body = "A notification with enough to say that it wraps onto several lines. ".repeat(3);
        for id in 1..=20 {
            notices.add(notice(id, &body), 0.0, None, true);
        }
        let mut centre = Centre::default();
        centre.open(0.0, &notices.ids());
        for _ in 0..19 {
            centre.key(Keysym::Down, false, &notices.ids());
        }
        assert_eq!(centre.selected, Item::Notice(1), "the oldest");
        let look = look(&centre, &notices);
        centre.repaint(&look, &notices).unwrap();
        assert!(centre.scroll > 0);
        assert!(
            centre
                .targets
                .iter()
                .any(|(target, _)| *target == Target::Open(1)),
            "drawn, so it can be seen and clicked"
        );
    }

    #[test]
    fn left_and_right_walk_a_notifications_buttons_and_enter_presses_one() {
        let mut centre = Centre::default();
        centre.selected = Item::Notice(7);
        centre.buttons.insert(7, 2);
        let ids = [7];
        centre.key(Keysym::Right, false, &ids);
        centre.key(Keysym::Right, false, &ids);
        centre.key(Keysym::Right, false, &ids);
        assert_eq!(centre.action, Some(1), "stops at the last button");
        assert_eq!(
            centre.key(Keysym::Return, false, &ids),
            Request::Invoke(7, 1)
        );
        centre.key(Keysym::Right, false, &ids);
        centre.key(Keysym::Left, false, &ids);
        assert_eq!(centre.action, None, "back on the card");
        assert_eq!(centre.key(Keysym::Return, false, &ids), Request::Open(7));
    }
}
