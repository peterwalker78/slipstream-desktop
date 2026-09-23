//! The shortcut sheet (Super+/): every key, in four groups.
//!
//! It is generated rather than written out: the bindings' rows come from the bindings themselves
//! and each action's own description, bullet time's from the table its key handling reads, so a
//! key added or changed shows up here without anyone remembering to list it.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, bullet,
    keys::{self, Binding, Group, Mods},
    motion::HYPR,
    paint::{self, Painted, Painter},
    panel::{self, MOCKUP_PX},
    text::{self, Face, Style},
};

// The card's measurements, in mockup pixels: the explorer's frame.
const WIDTH: f32 = 1040.0;
const TOP: f32 = 130.0;
const MARGIN: f32 = 64.0;
const PADDING: f32 = 28.0;
const HEAD_H: f32 = 64.0;
const COLUMN_GAP: f32 = 40.0;
const TITLE_H: f32 = 34.0;
const GROUP_GAP: f32 = 14.0;
/// A row's pitch, and the least it's squeezed to on a short screen.
const ROW: f32 = 30.0;
const ROW_MIN: f32 = 22.0;
const KEY_GAP: f32 = 6.0;
const OPEN: f64 = 0.18;
const REDUCED_FADE: f64 = 0.08;
/// The tour's card, which is narrower than the key list: it is one idea at a time.
const TOUR_WIDTH: f32 = 560.0;
const TOUR_STAGE_H: f32 = 260.0;
const TOUR_WORDS_H: f32 = 120.0;
/// The ring round the tile the keyboard is on, in the stage. The sheet has no ring colour of its
/// own to follow, and the tour is teaching what the ring means rather than matching a setting.
const TOUR_RING: u32 = 0x42d3ffff;
/// A tile on the stage, and its edge. Light enough to read as a window against the stage behind
/// it; the alpha byte is the tile's own, so one on its way into the code rain fades out.
const TOUR_TILE: u32 = 0x2b364600;
const TOUR_TILE_EDGE: u32 = 0x4a586c00;

/// One line of the sheet: its keycaps, what they do, and the bindings it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub keys: Vec<String>,
    pub does: String,
    pub bindings: Vec<(Mods, Keysym)>,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub group: Group,
    pub rows: Vec<Row>,
}

/// Keys that read as one set when they share modifiers: the arrows, the brackets, PgUp and PgDn,
/// and the digits. Their order within the set.
fn set_rank(key: Keysym) -> Option<u32> {
    let rank = match key {
        Keysym::Left => 0,
        Keysym::Right => 1,
        Keysym::Up => 2,
        Keysym::Down => 3,
        Keysym::bracketleft | Keysym::braceleft => 4,
        Keysym::bracketright | Keysym::braceright => 5,
        Keysym::Prior => 6,
        Keysym::Next => 7,
        _ => 10 + keys::DIGITS.iter().position(|digit| *digit == key)? as u32,
    };
    Some(rank)
}

/// The keycaps for a row's bindings: one per modifier combination when its keys form a set
/// (`Super+← → ↑ ↓`, `Super+1–9`), otherwise one per key.
fn keycaps(bindings: &[(Mods, Keysym)]) -> Vec<String> {
    let mut by_mods: Vec<(Mods, Vec<Keysym>)> = Vec::new();
    for (mods, key) in bindings {
        match by_mods.iter_mut().find(|(held, _)| held == mods) {
            Some((_, keys)) => keys.push(*key),
            None => by_mods.push((*mods, vec![*key])),
        }
    }
    let mut caps: Vec<String> = Vec::new();
    for (mods, mut group) in by_mods {
        let mut names: Vec<String> = Vec::new();
        if group.len() > 1 && group.iter().all(|key| set_rank(*key).is_some()) {
            group.sort_by_key(|key| set_rank(*key));
            for key in &group {
                let name = keys::key_name(*key);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            let digits: Vec<u32> = names.iter().filter_map(|name| name.parse().ok()).collect();
            let run = digits.len() == names.len()
                && digits.len() > 2
                && digits.windows(2).all(|pair| pair[1] == pair[0] + 1);
            let joined = if run {
                format!("{}–{}", digits[0], digits[digits.len() - 1])
            } else {
                names.join(" ")
            };
            caps.push(keys::mods_prefix(mods) + &joined);
        } else {
            for key in group {
                let cap = keys::label(mods, key);
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
        }
    }
    caps
}

/// The sheet's sections for `bindings`, in `Group::ALL`'s order: each group's bindings merged by
/// what they do, bullet time's keys after its own binding, and the panels' shared keys last.
pub fn sections(bindings: &[Binding]) -> Vec<Section> {
    let mut sections: Vec<Section> = Group::ALL
        .iter()
        .map(|group| Section {
            group: *group,
            rows: Vec::new(),
        })
        .collect();
    for binding in bindings {
        let group = binding.action.group();
        let does = binding.action.describe();
        let Some(section) = sections.iter_mut().find(|section| section.group == group) else {
            continue;
        };
        match section.rows.iter_mut().find(|row| row.does == does) {
            Some(row) => row.bindings.push((binding.mods, binding.key)),
            None => section.rows.push(Row {
                keys: Vec::new(),
                does: does.to_string(),
                bindings: vec![(binding.mods, binding.key)],
            }),
        }
    }
    for section in &mut sections {
        for row in &mut section.rows {
            row.keys = keycaps(&row.bindings);
        }
        let fixed = |keys: &[&str], does: &str| Row {
            keys: keys.iter().map(|key| key.to_string()).collect(),
            does: does.to_string(),
            bindings: Vec::new(),
        };
        match section.group {
            // A tapped Super is no binding — nothing is held with it — so the sheet says it
            // here, at the top, where the key that opens most things belongs.
            Group::AppsAndWindows => section.rows.insert(0, fixed(&["Super"], "apps")),
            Group::BulletTime => section
                .rows
                .extend(bullet::KEYS.iter().map(|row| fixed(&[row.keys], row.does))),
            // Every panel and card answers these the same way.
            Group::PanelsAndSystem => section.rows.extend([
                fixed(&["Esc"], "in a panel: back out one level"),
                fixed(&["Tab", "← → ↑ ↓"], "in a panel: move"),
                fixed(&["Enter"], "in a panel: press"),
                fixed(&["Home End", "PgUp PgDn"], "in a panel: jump"),
                fixed(&["Caps Lock"], "hold: Awake, the screen stays on"),
            ]),
            _ => {}
        }
    }
    sections
}

/// The rows of `sections` that match `query` by what they do or by their keys, and the sections
/// left with any.
fn filtered(sections: Vec<Section>, query: &str) -> Vec<Section> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return sections;
    }
    sections
        .into_iter()
        .map(|mut section| {
            section.rows.retain(|row| {
                row.does.to_lowercase().contains(&query)
                    || row
                        .keys
                        .iter()
                        .any(|key| key.to_lowercase().contains(&query))
            });
            section
        })
        .filter(|section| !section.rows.is_empty())
        .collect()
}

pub enum Outcome {
    Nothing,
    Close,
}

/// Everything the painted sheet depends on.
#[derive(Clone, PartialEq)]
struct Look {
    query: String,
    /// The tour's lesson and how far through its loop, in thirtieths of a second: the sheet is
    /// repainted only when this changes, so a moving stage costs one paint a frame and a still
    /// list costs none.
    tour: Option<(usize, u64)>,
    screen: Size<i32, Logical>,
    scale: f64,
}

#[derive(Default)]
pub struct Sheet {
    open: bool,
    opened_at: f64,
    /// Which lesson of the tour is showing, and when it started, or `None` for the key list.
    tour: Option<(usize, f64)>,
    query: String,
    pub reduced_motion: bool,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// The card, in logical pixels on the screen.
    frame: Rectangle<f64, Logical>,
}

impl Sheet {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, now: f64) {
        self.open = true;
        self.opened_at = now;
        self.tour = None;
        self.query.clear();
        self.shown = None;
    }

    /// Starts the tour, from the sheet or straight from the wallpaper's prompt.
    pub fn start_tour(&mut self, now: f64) {
        self.open = true;
        self.opened_at = now;
        self.tour = Some((0, now));
        self.query.clear();
        self.shown = None;
    }

    pub fn touring(&self) -> bool {
        self.tour.is_some()
    }

    pub fn close(&mut self) {
        self.open = false;
        self.tour = None;
        self.shown = None;
        self.painted = None;
    }

    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    /// A key while it's open. On the tour, Enter and the arrows step through the lessons and Esc
    /// goes back to the list. Otherwise typing filters the rows; Esc clears the filter, then
    /// closes.
    pub fn key(&mut self, sym: Keysym, ch: Option<char>, now: f64) -> Outcome {
        if let Some((step, _)) = self.tour {
            match sym {
                Keysym::Escape => self.tour = None,
                Keysym::Return | Keysym::KP_Enter | Keysym::Right | Keysym::Down => {
                    match step + 1 < crate::tour::LESSONS.len() {
                        // Past the last lesson it goes back to the list it came from.
                        false => self.tour = None,
                        true => self.tour = Some((step + 1, now)),
                    }
                }
                Keysym::Left | Keysym::Up => {
                    self.tour = Some((step.saturating_sub(1), now));
                }
                _ => return Outcome::Nothing,
            }
            self.shown = None;
            return Outcome::Nothing;
        }
        // The first row of the list starts it.
        if matches!(sym, Keysym::Return | Keysym::KP_Enter) {
            self.start_tour(now);
            return Outcome::Nothing;
        }
        match sym {
            Keysym::Escape if !self.query.is_empty() => self.query.clear(),
            Keysym::Escape => return Outcome::Close,
            Keysym::BackSpace => {
                self.query.pop();
            }
            _ => {
                if let Some(ch) = ch.filter(|ch| !ch.is_control()) {
                    self.query.push(ch);
                }
            }
        }
        Outcome::Nothing
    }

    /// Whether a point in logical pixels on the screen is on the card.
    pub fn contains(&self, pos: Point<f64, Logical>) -> bool {
        self.frame.contains(pos)
    }

    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        bindings: &[Binding],
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if !self.open {
            return None;
        }
        // On the tour, the stage moves, so the card is repainted as the loop turns — thirtieths
        // of a second, which is what a tween needs and less than a frame costs.
        let tour = self.tour.map(|(step, since)| {
            let t = if self.reduced_motion {
                // Reduced motion holds each lesson at the end of its move: the arrangement it
                // teaches, without the moving.
                1.0
            } else {
                ((now - since) / crate::tour::LOOP).rem_euclid(1.0)
            };
            (step, (t * 30.0) as u64)
        });
        let look = Look {
            query: self.query.clone(),
            tour,
            screen,
            scale,
        };
        if self.shown.as_ref() != Some(&look) {
            self.painted = match tour {
                Some((step, frame)) => paint_tour(&look, step, frame as f32 / 30.0),
                None => {
                    let sections = filtered(sections(bindings), &look.query);
                    paint(&look, &sections)
                }
            };
            self.shown = Some(look);
        }
        let painted = self.painted.as_ref()?;
        let at = Point::<f64, Logical>::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((TOP - MARGIN) * MOCKUP_PX) as f64,
        ));
        self.frame = Rectangle::new(
            (
                at.x + (MARGIN * MOCKUP_PX) as f64,
                at.y + (MARGIN * MOCKUP_PX) as f64,
            )
                .into(),
            (
                painted.logical.w as f64 - (2.0 * MARGIN * MOCKUP_PX) as f64,
                painted.logical.h as f64 - (2.0 * MARGIN * MOCKUP_PX) as f64,
            )
                .into(),
        );
        let since = now - self.opened_at;
        let (alpha, rise) = if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
        } else {
            let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
            (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
        };
        painted.element(
            renderer,
            at + Point::from((0.0, rise * MOCKUP_PX as f64)),
            alpha,
        )
    }
}

/// How tall a column of sections is at a row pitch of `pitch`.
fn column_height(sections: &[&Section], pitch: f32) -> f32 {
    sections
        .iter()
        .map(|section| TITLE_H + section.rows.len() as f32 * pitch)
        .sum::<f32>()
        + sections.len().saturating_sub(1) as f32 * GROUP_GAP
}

/// The tour's card: the stage with the tiles moving on it, what the lesson says, and its keys.
fn paint_tour(look: &Look, step: usize, t: f32) -> Option<Painted> {
    let lesson = crate::tour::LESSONS.get(step)?;
    let screen_w = look.screen.w as f32 / MOCKUP_PX;
    let width = TOUR_WIDTH.min(screen_w - 40.0).max(420.0);
    let height = HEAD_H + TOUR_STAGE_H + TOUR_WORDS_H + PADDING;
    let logical = Size::<i32, Logical>::from((
        ((width + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * look.scale).round().max(1.0) as i32,
        (logical.h as f64 * look.scale).round().max(1.0) as i32,
    );
    let f = look.scale as f32 * MOCKUP_PX;
    let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;
    let (fx, fy) = (MARGIN, MARGIN);
    panel::glass(&mut p, fx, fy, width, height);

    // The head: which lesson this is, and the way out.
    let centre = fy + HEAD_H / 2.0;
    let x = fx + PADDING;
    p.text(
        lesson.title,
        x,
        centre,
        &Style::new(Face::Body, 22.0, panel::INK),
    );
    let mut right_x = fx + width - PADDING;
    right_x -= paint::keycap_width("Esc");
    p.keycap("Esc", right_x, centre);
    p.fill(fx, fy + HEAD_H, width, 1.0, 0.0, 0xffffff12);

    // The stage: a little screen with the tiles doing what the lesson says.
    let stage = crate::tour::stage(step, t);
    let sx = fx + PADDING;
    let sy = fy + HEAD_H + 20.0;
    let (sw, sh) = (width - 2.0 * PADDING, TOUR_STAGE_H - 40.0);
    p.fill(sx, sy, sw, sh, 10.0, 0x0a0c11ff);
    p.border(sx, sy, sw, sh, 10.0, 1.0, 0xffffff14);
    // The code rain's strip takes width from the tiles as it fills, as the real one does.
    let rain_w = stage.rain * sw * 0.18;
    if rain_w > 0.5 {
        p.fill(
            sx + sw - rain_w,
            sy,
            rain_w,
            sh,
            6.0,
            (0x3cf0c000) | (0x40_u32 * stage.rain.min(1.0) as u32).min(0x40),
        );
    }
    let area_w = sw - rain_w;
    for (index, tile) in stage.tiles.iter().enumerate() {
        if tile.alpha <= 0.01 {
            continue;
        }
        let (tx, ty) = (sx + tile.x * area_w, sy + tile.y * sh);
        let (tw, th) = (tile.w * area_w, tile.h * sh);
        let opacity = ((tile.alpha * 255.0) as u32).min(255);
        let (w, h) = ((tw - 12.0).max(0.0), (th - 12.0).max(0.0));
        p.fill(tx + 6.0, ty + 6.0, w, h, 7.0, TOUR_TILE | opacity);
        p.border(tx + 6.0, ty + 6.0, w, h, 7.0, 1.0, TOUR_TILE_EDGE | opacity);
        if stage.focus == Some(index) {
            p.border(tx + 6.0, ty + 6.0, w, h, 7.0, 2.0, TOUR_RING);
        }
    }

    // What it says, its keys, and where you are.
    let words = Style::new(Face::Body, 15.0, 0xa8b1c2ff);
    let mut y = sy + sh + 22.0;
    for line in text::wrap(lesson.says, &words, width - 2.0 * PADDING) {
        p.text(&line, sx, y, &words);
        y += 21.0;
    }
    y += 8.0;
    let mut key_x = sx;
    for key in lesson.keys {
        key_x += p.keycap(key, key_x, y + 4.0) + KEY_GAP;
    }
    // Dots for the lessons, this one lit, and what moves on.
    let dots = crate::tour::LESSONS.len();
    let mut dot_x = fx + width - PADDING - (dots as f32 * 12.0 - 5.0);
    for index in 0..dots {
        let lit = index == step;
        let size = if lit { 7.0 } else { 5.0 };
        let shade = if lit { panel::AMBER } else { 0xffffff2e };
        p.fill(dot_x, y + 4.0 - size / 2.0, size, size, size / 2.0, shade);
        dot_x += 12.0;
    }
    let next = if step + 1 < dots { "next" } else { "the keys" };
    let hint = Style::new(Face::Body, 13.0, 0x7d8697ff);
    let hint_w = text::width(next, &hint);
    p.text(next, fx + width - PADDING - hint_w, y + 24.0, &hint);
    let cap_w = paint::keycap_width("⏎");
    p.keycap(
        "⏎",
        fx + width - PADDING - hint_w - KEY_GAP - cap_w,
        y + 24.0,
    );
    Some(Painted {
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
        scale: look.scale,
    })
}

fn paint(look: &Look, sections: &[Section]) -> Option<Painted> {
    let screen_w = look.screen.w as f32 / MOCKUP_PX;
    let screen_h = look.screen.h as f32 / MOCKUP_PX;
    let width = WIDTH.min(screen_w - 40.0).max(480.0);
    // Left: apps, windows and workspaces; right: bullet time, the panels and the system.
    let left: Vec<&Section> = sections
        .iter()
        .filter(|section| {
            matches!(
                section.group,
                Group::AppsAndWindows | Group::WorkspacesAndArranging
            )
        })
        .collect();
    let right: Vec<&Section> = sections
        .iter()
        .filter(|section| !left.contains(section))
        .collect();
    // Rows close up on a screen too short for them at full pitch.
    let room = screen_h - TOP - 24.0 - HEAD_H - 16.0 - PADDING;
    let pitch = [&left, &right]
        .iter()
        .filter(|column| !column.is_empty())
        .map(|column| {
            let rows: usize = column.iter().map(|section| section.rows.len()).sum();
            let fixed = column_height(column, 0.0);
            ((room - fixed) / rows.max(1) as f32).clamp(ROW_MIN, ROW)
        })
        .fold(ROW, f32::min);
    let body_h = column_height(&left, pitch)
        .max(column_height(&right, pitch))
        .max(40.0);
    let height = HEAD_H + 16.0 + body_h + PADDING;
    let logical = Size::<i32, Logical>::from((
        ((width + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * look.scale).round().max(1.0) as i32,
        (logical.h as f64 * look.scale).round().max(1.0) as i32,
    );
    let f = look.scale as f32 * MOCKUP_PX;
    let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;
    let (fx, fy) = (MARGIN, MARGIN);
    panel::glass(&mut p, fx, fy, width, height);

    // The head: the filter, and the keys that close it.
    let centre = fy + HEAD_H / 2.0;
    let x = fx + PADDING;
    if look.query.is_empty() {
        p.text(
            "Keyboard shortcuts",
            x,
            centre,
            &Style::new(Face::Body, 22.0, panel::INK),
        );
        let title_w = text::width(
            "Keyboard shortcuts",
            &Style::new(Face::Body, 22.0, panel::INK),
        );
        p.text(
            "type to filter",
            x + title_w + 16.0,
            centre + 1.0,
            &Style::new(Face::Body, 15.0, 0x7d8697ff),
        );
    } else {
        let typed = p.text(
            &look.query,
            x,
            centre,
            &Style::new(Face::Body, 22.0, 0xf2f4f8ff),
        );
        p.fill(x + typed + 2.0, centre - 11.0, 2.5, 22.0, 0.0, panel::AMBER);
    }
    let mut right_x = fx + width - PADDING;
    for key in ["Esc", "Super+/"] {
        right_x -= paint::keycap_width(key);
        p.keycap(key, right_x, centre);
        right_x -= KEY_GAP;
    }
    // The way into the tour, to the left of the keys that close the sheet, on the key already
    // under a finger. Laid out from where those finished, so the two can never run together.
    if look.query.is_empty() {
        let says = "take the tour";
        let hint = Style::new(Face::Body, 14.0, 0x8f98a8ff);
        right_x -= 14.0 + text::width(says, &hint);
        p.text(says, right_x, centre + 1.0, &hint);
        right_x -= KEY_GAP + paint::keycap_width("⏎");
        p.keycap("⏎", right_x, centre);
    }
    p.fill(fx, fy + HEAD_H, width, 1.0, 0.0, 0xffffff12);

    let column_w = (width - 2.0 * PADDING - COLUMN_GAP) / 2.0;
    let title = Style {
        tracking: 0.08,
        ..Style::new(Face::Mono, 12.0, 0x8f98a8ff)
    };
    let does = Style::new(Face::Body, 15.0, 0xcdd6f4ff);
    let top = fy + HEAD_H + 16.0;
    if sections.is_empty() {
        p.text(
            &format!("No keys match “{}”.", look.query.trim()),
            fx + PADDING,
            top + 20.0,
            &Style::new(Face::Body, 16.0, 0x8b93a3ff),
        );
    }
    for (column, list) in [left, right].iter().enumerate() {
        let x0 = fx + PADDING + column as f32 * (column_w + COLUMN_GAP);
        // The keycaps take as much of the column as the widest row's need, and no more than half.
        let keys_w = list
            .iter()
            .flat_map(|section| &section.rows)
            .map(|row| {
                row.keys
                    .iter()
                    .map(|key| paint::keycap_width(key) + KEY_GAP)
                    .sum::<f32>()
            })
            .fold(0.0, f32::max)
            .min(column_w / 2.0);
        let mut y = top;
        for section in list {
            p.text(
                &section.group.title().to_uppercase(),
                x0,
                y + TITLE_H / 2.0,
                &title,
            );
            y += TITLE_H;
            for row in &section.rows {
                let centre = y + pitch / 2.0;
                let mut key_x = x0;
                for key in &row.keys {
                    key_x += p.keycap(key, key_x, centre) + KEY_GAP;
                }
                let does_x = x0 + keys_w + 10.0;
                let room = x0 + column_w - does_x;
                p.text(
                    &text::ellipsize(&row.does, &does, room),
                    does_x,
                    centre,
                    &does,
                );
                y += pitch;
            }
            y += GROUP_GAP;
        }
    }
    Some(Painted {
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
        scale: look.scale,
    })
}

impl Slipstream {
    /// Super+/: the shortcut sheet opens, or closes.
    pub fn toggle_sheet(&mut self) {
        if self.sheet.is_open() {
            self.sheet.close();
            return;
        }
        self.close_panels();
        let now = self.clock.tick();
        self.sheet.reduced_motion = self.clock.reduced_motion;
        self.sheet.open(now);
    }

    pub fn sheet_key(&mut self, sym: Keysym, ch: Option<char>) {
        let now = self.clock.tick();
        if let Outcome::Close = self.sheet.key(sym, ch, now) {
            self.sheet.close();
        }
    }

    /// A click while the sheet is open, in logical pixels from the output's corner: outside the
    /// card it closes, and on a bar button that button then does its own thing.
    pub fn sheet_click(&mut self, x: f64, y: f64) {
        if self.sheet.contains(Point::from((x, y))) {
            return;
        }
        self.sheet.close();
        let target = (!self.fullscreen_on_screen())
            .then(|| self.focused_bar_target(x, y))
            .flatten();
        if let Some(target) = target {
            self.bar_clicked(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_appears_once() {
        let bindings = keys::checked(keys::defaults());
        let sections = sections(&bindings);
        for binding in &bindings {
            let rows = sections
                .iter()
                .flat_map(|section| &section.rows)
                .filter(|row| row.bindings.contains(&(binding.mods, binding.key)))
                .count();
            assert_eq!(rows, 1, "{binding:?}");
        }
        let groups: Vec<Group> = sections.iter().map(|section| section.group).collect();
        assert_eq!(groups, Group::ALL);
        let row = |does: &str| {
            sections
                .iter()
                .flat_map(|section| &section.rows)
                .find(|row| row.does == does)
                .unwrap_or_else(|| panic!("no row for {does}"))
                .keys
                .clone()
        };
        assert_eq!(row("move focus"), ["Super+← → ↑ ↓"]);
        assert_eq!(row("go to a workspace"), ["Super+1–9"]);
        assert_eq!(row("narrower, wider"), ["Super+[ ]"]);
        assert_eq!(row("shorter, taller"), ["Super+Shift+[ ]"]);
        assert_eq!(row("switch window"), ["Alt+Tab", "Alt+Shift+Tab"]);
        assert_eq!(row("apps"), ["Super"]);
        assert_eq!(row("turn the layout on its side"), ["Super+R"]);
        assert_eq!(row("maximise"), ["Super+F"]);
        assert_eq!(row("every key"), ["Super+/"]);
        assert_eq!(row("jump to a label"), ["J K L…"]);
    }

    #[test]
    fn typing_filters_the_rows() {
        let bindings = keys::checked(keys::defaults());
        let found = filtered(sections(&bindings), "minim");
        let rows: Vec<&str> = found
            .iter()
            .flat_map(|section| &section.rows)
            .map(|row| row.does.as_str())
            .collect();
        assert_eq!(
            rows,
            [
                "minimise to the code rain",
                "bring back the last minimised",
                "minimise"
            ]
        );
        assert!(filtered(sections(&bindings), "nothing like this").is_empty());
        let mut sheet = Sheet::default();
        sheet.open(0.0);
        sheet.key(Keysym::k, Some('k'), 0.0);
        assert_eq!(sheet.query, "k");
        assert!(matches!(
            sheet.key(Keysym::Escape, None, 0.0),
            Outcome::Nothing
        ));
        assert!(matches!(
            sheet.key(Keysym::Escape, None, 0.0),
            Outcome::Close
        ));
    }
}
