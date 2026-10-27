//! The plugin panel, from a click on the chip beside the clock or Super+Y: a glass card hanging
//! under the chip with a card for each plugin that's on, and the way to the Settings app's Plugins
//! page.
//!
//! A card shows what its plugin's program last printed (`plugins.rs`): a few lines, named groups of
//! gauges with the time until each starts over, and up to three buttons. A gauge whose allowance
//! says how long it lasts has the part of that time gone by shaded in the focus ring's colour, and
//! whatever has been used beyond it in the second colour: used faster than time goes by. A card
//! with sections has a cog, which slides it across to a switch for each section.
//!
//! While the panel is open it takes every key: Tab or the arrows move between the cogs, buttons and
//! the settings link, Enter or Space presses, Esc slides a card back or closes. A ring shows where
//! the keyboard is.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, icons,
    paint::{Painted, Painter},
    panel::{self, DESIGN_PX, GAP, INK, PADDING},
    plugins::{Shown, card},
    text::{self, Face, Style},
};

// Sizes in design pixels.
const WIDTH: f32 = 480.0;
const HEAD_H: f32 = 30.0;
const FOOT_H: f32 = 31.0;
const CARD_PAD: f32 = 14.0;
const CARD_GAP: f32 = 10.0;
const NAME_H: f32 = 24.0;
const LINE_H: f32 = 21.0;
const SECTION_HEAD_H: f32 = 24.0;
const GAUGE_ROW_H: f32 = 24.0;
const NOTE_H: f32 = 20.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_GAP: f32 = 8.0;
/// The cog, and the way back from a card's switches.
const ROUND: f32 = 24.0;
/// A row of a card's switches, and its key to the gauges' colours.
const CHOICE_H: f32 = 32.0;
const KEY_H: f32 = 20.0;
/// The squares beside the panel's name, as on the chip.
const SQUARE: f32 = 8.0;
/// How long a card takes to slide between its gauges and its switches.
const TURN: f64 = 0.3;
/// How strongly the time gone by is shaded in a gauge's track.
const TIME_ALPHA: u32 = 0x55;

/// Something the keyboard can be on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    /// A plugin's cog, by its id.
    Cog(String),
    /// The way back from a plugin's switches.
    Back(String),
    /// The switch for a plugin's section, by the plugin's id and the section's name.
    Section(String, String),
    /// A button: its plugin's id and its own.
    Button(String, String),
    Settings,
}

/// What a key or click asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Nothing,
    Close,
    /// Button `button` on plugin `plugin`'s card.
    Press {
        plugin: String,
        button: String,
    },
    /// Show plugin `plugin`'s section `section` if it's hidden, or hide it.
    Toggle {
        plugin: String,
        section: String,
    },
    /// The Settings app's Plugins page.
    Settings,
}

/// Everything the panel shows that it doesn't own.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub cards: Vec<Shown>,
    /// How many plugins there are, on or off.
    pub installed: usize,
    /// The keyboard ring's colour, the same as the focused window's.
    pub ring: u32,
    /// The time gone by in a gauge: the focus ring's colour, the primary, which also lights a
    /// plugin that's on. Used beyond it: the secondary colour, which also marks what wants
    /// attention.
    pub time: u32,
    pub ahead: u32,
    /// The way into the Settings app, as a link.
    pub accent: u32,
    pub reduced_motion: bool,
    /// The key that opens and closes the panel.
    pub key: String,
}

/// A card sliding between its gauges and its switches: the plugin, whether it's going to the
/// switches, and when it started, on the animation clock (`None` until the next frame stamps it).
#[derive(Debug, Clone, PartialEq)]
struct Turn {
    plugin: String,
    to_switches: bool,
    since: Option<f64>,
}

/// Everything the painted panel depends on.
#[derive(Clone, PartialEq)]
struct Look {
    facts: Facts,
    selected: Item,
    under: f64,
    /// The card turned, or turning, and how far: 0 its gauges, 1 its switches.
    turned: Option<(String, f32)>,
    width: i32,
    scale: f64,
}

#[derive(Default)]
pub struct PluginPanel {
    open: bool,
    /// When it opened, on the animation clock.
    opened_at: f64,
    selected: Option<Item>,
    /// The middle of the chip it hangs from, in logical pixels from the screen's left.
    under: f64,
    turn: Option<Turn>,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// The painted area's corner and the card, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Item, Rectangle<f64, Logical>)>,
}

/// The keyboard's stops, in order: card by card its cog and buttons, or on the card showing its
/// switches the way back and each switch, then the settings link.
fn items(facts: &Facts, switches: Option<&str>) -> Vec<Item> {
    let mut items = Vec::new();
    for shown in &facts.cards {
        let Some(card) = &shown.card else {
            continue;
        };
        if switches == Some(shown.id.as_str()) {
            items.push(Item::Back(shown.id.clone()));
            items.extend(
                card.sections
                    .iter()
                    .map(|section| Item::Section(shown.id.clone(), section.name.clone())),
            );
            continue;
        }
        if !card.sections.is_empty() {
            items.push(Item::Cog(shown.id.clone()));
        }
        items.extend(
            card.buttons
                .iter()
                .map(|button| Item::Button(shown.id.clone(), button.id.clone())),
        );
    }
    items.push(Item::Settings);
    items
}

impl PluginPanel {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens under a chip whose middle is `under` logical pixels from the screen's left.
    pub fn open(&mut self, now: f64, under: f64) {
        self.open = true;
        self.opened_at = now;
        self.under = under;
        self.selected = None;
        self.turn = None;
        self.shown = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.turn = None;
        self.shown = None;
        self.painted = None;
        self.targets.clear();
    }

    /// The plugin whose card shows, or is sliding to show, its switches.
    fn switches(&self) -> Option<&str> {
        self.turn
            .as_ref()
            .filter(|turn| turn.to_switches)
            .map(|turn| turn.plugin.as_str())
    }

    /// Where the keyboard is: the one chosen while it's still there, else the first stop.
    fn selected(&self, facts: &Facts) -> Item {
        let items = items(facts, self.switches());
        self.selected
            .clone()
            .filter(|item| items.contains(item))
            .unwrap_or_else(|| items[0].clone())
    }

    /// Slides `plugin`'s card to its switches, or back to its gauges.
    fn turn(&mut self, plugin: &str, to_switches: bool) {
        self.turn = Some(Turn {
            plugin: plugin.to_string(),
            to_switches,
            since: None,
        });
    }

    /// A key while it's open.
    pub fn key(&mut self, sym: Keysym, shift: bool, facts: &Facts) -> Request {
        let items = items(facts, self.switches());
        let at = items
            .iter()
            .position(|item| *item == self.selected(facts))
            .unwrap_or(0);
        let step = |back: bool| {
            if back {
                (at + items.len() - 1) % items.len()
            } else {
                (at + 1) % items.len()
            }
        };
        match sym {
            Keysym::Escape => match self.switches().map(str::to_string) {
                Some(plugin) => return self.press(&Item::Back(plugin)),
                None => return Request::Close,
            },
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                self.selected = Some(items[step(shift || sym == Keysym::ISO_Left_Tab)].clone());
            }
            Keysym::Up | Keysym::Left => self.selected = Some(items[step(true)].clone()),
            Keysym::Down | Keysym::Right => self.selected = Some(items[step(false)].clone()),
            Keysym::Home => self.selected = items.first().cloned(),
            Keysym::End => self.selected = items.last().cloned(),
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                let item = items[at].clone();
                return self.press(&item);
            }
            _ => {}
        }
        Request::Nothing
    }

    fn press(&mut self, item: &Item) -> Request {
        self.selected = Some(item.clone());
        match item {
            Item::Cog(plugin) => {
                self.turn(plugin, true);
                self.selected = Some(Item::Back(plugin.clone()));
                Request::Nothing
            }
            Item::Back(plugin) => {
                self.turn(plugin, false);
                self.selected = Some(Item::Cog(plugin.clone()));
                Request::Nothing
            }
            Item::Section(plugin, section) => Request::Toggle {
                plugin: plugin.clone(),
                section: section.clone(),
            },
            Item::Button(plugin, button) => Request::Press {
                plugin: plugin.clone(),
                button: button.clone(),
            },
            Item::Settings => Request::Settings,
        }
    }

    /// A click, in logical pixels from the output's corner. Outside the card it closes.
    pub fn click(&mut self, x: f64, y: f64) -> Request {
        let target = self
            .targets
            .iter()
            .find(|(_, area)| area.contains((x, y)))
            .map(|(item, _)| item.clone());
        match target {
            Some(item) => self.press(&item),
            None if self.frame.contains((x, y)) => Request::Nothing,
            None => Request::Close,
        }
    }

    /// The panel for a screen `width` logical pixels wide at `scale`, repainted when what it
    /// shows changes.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        frame: panel::Frame,
        facts: Facts,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if !self.open {
            return None;
        }
        let reduced_motion = facts.reduced_motion;
        let now = frame.time;
        let turned = self.turn.as_mut().and_then(|turn| {
            let since = *turn.since.get_or_insert(now);
            let slid = if reduced_motion {
                1.0
            } else {
                crate::anim::Easing::InOutCubic.at((now - since) / TURN) as f32
            };
            let at = if turn.to_switches { slid } else { 1.0 - slid };
            // Gone back all the way is the only end: a slide out starts at 0 too.
            (turn.to_switches || slid < 1.0).then(|| (turn.plugin.clone(), at))
        });
        if turned.is_none() {
            self.turn = None;
        }
        let look = Look {
            selected: self.selected(&facts),
            under: self.under,
            turned,
            facts,
            width: frame.size.w,
            scale: frame.scale,
        };
        if self.shown.as_ref() != Some(&look) {
            self.repaint(&look)?;
            self.shown = Some(look);
        }
        let (alpha, rise) = panel::opening(now - self.opened_at, reduced_motion);
        let at = Point::from((self.origin.x, self.origin.y + rise * DESIGN_PX as f64));
        self.painted.as_ref()?.element(renderer, at, alpha)
    }

    fn repaint(&mut self, look: &Look) -> Option<()> {
        let facts = &look.facts;
        let inner = WIDTH - 2.0 * PADDING;
        let turned = |shown: &Shown| {
            look.turned
                .as_ref()
                .filter(|(plugin, _)| *plugin == shown.id)
                .map_or(0.0, |(_, at)| *at)
        };
        let heights: Vec<f32> = facts
            .cards
            .iter()
            .map(|shown| {
                let at = turned(shown);
                front_height(shown) + (back_height(shown) - front_height(shown)) * at
            })
            .collect();
        let cards_h = if heights.is_empty() {
            LINE_H * 2.0
        } else {
            heights.iter().sum::<f32>() + CARD_GAP * (heights.len() - 1) as f32
        };
        let height = PADDING + HEAD_H + GAP + cards_h + GAP + FOOT_H + PADDING;
        // Centred under the chip, and kept on the screen.
        let screen_w = look.width as f32 / DESIGN_PX;
        let left = (look.under as f32 / DESIGN_PX - WIDTH / 2.0)
            .min(screen_w - panel::RIGHT - WIDTH)
            .max(panel::RIGHT);
        let place = panel::Place {
            x: panel::MARGIN,
            y: panel::MARGIN,
            width: WIDTH,
            left,
            top: panel::TOP,
        };
        let logical = Size::<i32, Logical>::from((
            ((WIDTH + 2.0 * panel::MARGIN) * DESIGN_PX).ceil() as i32,
            ((height + 2.0 * panel::MARGIN + panel::BELOW) * DESIGN_PX).ceil() as i32,
        ));
        let f = look.scale as f32 * DESIGN_PX;
        let mut shapes: Vec<(Item, [f32; 5])> = Vec::new();
        let painted = Painted::new(logical, look.scale, |p| {
            p.f *= DESIGN_PX;
            panel::glass(p, place.x, place.y, WIDTH, height);
            let x0 = place.x + PADDING;
            let mut y = place.y + PADDING;
            paint_head(p, facts, x0, y);
            y += HEAD_H + GAP;
            if facts.cards.is_empty() {
                let quiet = Style::new(Face::Body, 14.0, panel::HINT);
                let said = if facts.installed == 0 {
                    "No plugins yet"
                } else {
                    "No plugins are on"
                };
                p.text(said, x0, y + LINE_H, &quiet);
            }
            for (shown, h) in facts.cards.iter().zip(&heights) {
                p.fill(x0, y, inner, *h, panel::TILE_RADIUS, panel::TILE);
                p.border(x0, y, inner, *h, panel::TILE_RADIUS, 1.0, panel::DIVIDER);
                match turned(shown) {
                    0.0 => paint_front(p, facts, shown, x0, y, inner, &mut shapes),
                    1.0 => paint_back(p, facts, shown, x0, y, inner, &mut shapes),
                    at => {
                        // Mid-slide, both sides are drawn on a strip the card's size, so
                        // neither runs past its edges; nothing on it can be pressed.
                        if let Some(mut strip) = Painter::new(
                            (inner * f).ceil() as u32,
                            (h * f).ceil().max(1.0) as u32,
                            f,
                        ) {
                            let mut none = Vec::new();
                            paint_front(
                                &mut strip,
                                facts,
                                shown,
                                -inner * at,
                                0.0,
                                inner,
                                &mut none,
                            );
                            paint_back(
                                &mut strip,
                                facts,
                                shown,
                                inner * (1.0 - at),
                                0.0,
                                inner,
                                &mut none,
                            );
                            p.image(&strip.pixmap, x0, y);
                        }
                    }
                }
                y += h + CARD_GAP;
            }
            if !facts.cards.is_empty() {
                y -= CARD_GAP;
            }
            y += GAP;
            p.fill(x0, y, inner, 1.0, 0.0, panel::DIVIDER);
            let centre = y + FOOT_H - 9.0;
            let link = Style::new(Face::Body, 14.0, panel::lighter(facts.accent, 0.36));
            let link_w = p.text("Plugin settings", x0, centre, &link);
            shapes.push((
                Item::Settings,
                [x0 - 6.0, centre - 13.0, link_w + 12.0, 26.0, 6.0],
            ));
            let close = panel::key_hint(p, x0 + inner, centre, &[&facts.key, "Esc"], "close");
            panel::key_hint(p, close - panel::HINT_GAP, centre, &["Tab"], "next");
            for (item, [x, y, w, h, radius]) in &shapes {
                if *item == look.selected {
                    panel::focus_ring(p, *x, *y, *w, *h, *radius, facts.ring);
                }
            }
        })?;
        self.origin = place.on_screen(0.0, 0.0, 0.0, 0.0).loc;
        self.frame = place.on_screen(place.x, place.y, WIDTH, height);
        self.targets = shapes
            .into_iter()
            .map(|(item, [x, y, w, h, _])| (item, place.on_screen(x, y, w, h)))
            .collect();
        self.painted = Some(painted);
        Some(())
    }
}

/// The panel's name, with the chip's squares before it and how many plugins are on after it.
fn paint_head(p: &mut Painter, facts: &Facts, x0: f32, y: f32) {
    let centre = y + HEAD_H / 2.0;
    let on = facts.cards.len();
    let shown = facts.installed.min(4);
    for index in 0..4 {
        let (column, row) = ((index % 2) as f32, (index / 2) as f32);
        let colour = if index < on.min(shown) {
            facts.time
        } else if index < shown {
            0xffffff29
        } else {
            0xffffff10
        };
        p.fill(
            x0 + column * (SQUARE + 2.0),
            centre - SQUARE - 1.0 + row * (SQUARE + 2.0),
            SQUARE,
            SQUARE,
            2.0,
            colour,
        );
    }
    let name = Style::new(Face::BodyBold, 17.0, INK);
    let name_w = p.text("Plugins", x0 + 2.0 * SQUARE + 2.0 + 12.0, centre, &name);
    if facts.installed > 0 {
        let quiet = Style::new(Face::Body, 13.0, panel::HINT);
        p.text(
            &format!("{on} on"),
            x0 + 2.0 * SQUARE + 2.0 + 12.0 + name_w + 10.0,
            centre,
            &quiet,
        );
    }
}

/// How many of a card's sections are hidden.
fn hidden_count(shown: &Shown) -> usize {
    shown.card.as_ref().map_or(0, |card| {
        card.sections
            .iter()
            .filter(|section| shown.hidden.contains(&section.name))
            .count()
    })
}

/// A card's height showing its gauges: its name, then what its program printed, or why it printed
/// nothing.
fn front_height(shown: &Shown) -> f32 {
    let body = match shown.visible() {
        Some(card) => {
            let sections: f32 = card
                .sections
                .iter()
                .map(|section| {
                    SECTION_HEAD_H
                        + section.gauges.len() as f32 * GAUGE_ROW_H
                        + if section.note.is_some() { NOTE_H } else { 0.0 }
                })
                .sum();
            let buttons = if card.buttons.is_empty() {
                0.0
            } else {
                BUTTON_GAP + BUTTON_H
            };
            let hidden = if hidden_count(shown) > 0 { NOTE_H } else { 0.0 };
            card.lines.len() as f32 * LINE_H + sections + buttons + hidden
        }
        None => LINE_H,
    };
    let problem = if shown.card.is_some() && shown.problem.is_some() {
        NOTE_H
    } else {
        0.0
    };
    CARD_PAD + NAME_H + body + problem + CARD_PAD
}

/// Whether any of a card's gauges, shown or hidden, knows how far through its window it is.
fn has_time(shown: &Shown) -> bool {
    shown.card.iter().any(|card| {
        card.sections
            .iter()
            .flat_map(|section| &section.gauges)
            .any(|gauge| gauge.through.is_some())
    })
}

/// A card's height showing its switches: the way back, a row for each section, and the key to the
/// gauges' colours when they show time.
fn back_height(shown: &Shown) -> f32 {
    let rows = shown.card.as_ref().map_or(0, |card| card.sections.len());
    let key = if has_time(shown) {
        6.0 + 2.0 * KEY_H
    } else {
        0.0
    };
    CARD_PAD + NAME_H + 6.0 + rows as f32 * CHOICE_H + key + CARD_PAD
}

/// A round quiet button at (`x`, `y`) with `icon` in it.
fn round_button(p: &mut Painter, icon: &str, x: f32, y: f32) {
    panel::quiet_button(p, x, y, ROUND, ROUND, ROUND / 2.0, false);
    p.icon(icon, x + 3.0, y + 3.0, 18.0, Some(panel::SECONDARY));
}

/// One plugin's card showing its gauges, at (`x`, `y`) and `w` wide: its name and cog, its lines,
/// its gauges and its buttons. A card whose program has stopped answering is dimmed, and says why.
fn paint_front(
    p: &mut Painter,
    facts: &Facts,
    shown: &Shown,
    x: f32,
    y: f32,
    w: f32,
    shapes: &mut Vec<(Item, [f32; 5])>,
) {
    let x0 = x + CARD_PAD;
    let inner = w - 2.0 * CARD_PAD;
    let mut y = y + CARD_PAD;
    let stale = shown.problem.is_some();
    let shade = |rgba: u32| if stale { card::dim(rgba) } else { rgba };
    let has_sections = shown
        .card
        .as_ref()
        .is_some_and(|card| !card.sections.is_empty());
    let name = Style::new(Face::BodyBold, 15.0, INK);
    let name_room = if has_sections {
        inner - ROUND - 8.0
    } else {
        inner
    };
    p.text(
        &text::ellipsize(&shown.name, &name, name_room),
        x0,
        y + NAME_H / 2.0 - 2.0,
        &name,
    );
    if has_sections {
        let cog_x = x0 + inner - ROUND;
        let cog_y = y + (NAME_H - ROUND) / 2.0 - 2.0;
        round_button(p, icons::COG, cog_x, cog_y);
        shapes.push((
            Item::Cog(shown.id.clone()),
            [cog_x, cog_y, ROUND, ROUND, ROUND / 2.0],
        ));
    }
    y += NAME_H;
    let Some(card) = shown.visible() else {
        let note = Style::new(Face::Body, 13.0, quiet(facts.ahead));
        let said = match &shown.problem {
            _ if shown.held && shown.problem.is_none() => {
                "Changed since it was turned on: turn it on again to run it".into()
            }
            Some(problem) => problem.clone(),
            None => "Starting…".into(),
        };
        p.text(
            &text::ellipsize(&said, &note, inner),
            x0,
            y + LINE_H / 2.0,
            &note,
        );
        return;
    };
    let line = Style::new(Face::Body, 14.0, shade(panel::SECONDARY));
    for text in &card.lines {
        p.text(
            &text::ellipsize(text, &line, inner),
            x0,
            y + LINE_H / 2.0,
            &line,
        );
        y += LINE_H;
    }
    for section in &card.sections {
        let shade = |rgba: u32| {
            if stale || section.stale {
                card::dim(rgba)
            } else {
                rgba
            }
        };
        let centre = y + SECTION_HEAD_H / 2.0;
        let name = Style::new(Face::BodyBold, 13.5, shade(INK));
        let name_w = p.text(
            &text::ellipsize(&section.name, &name, inner / 2.0),
            x0,
            centre,
            &name,
        );
        if let Some(detail) = &section.detail {
            let quiet = Style::new(Face::Body, 12.5, shade(panel::HINT));
            p.text(
                &text::ellipsize(detail, &quiet, inner / 2.0 - 10.0),
                x0 + name_w + 10.0,
                centre,
                &quiet,
            );
        }
        y += SECTION_HEAD_H;
        for gauge in &section.gauges {
            let centre = y + GAUGE_ROW_H / 2.0;
            let label = Style::new(Face::Body, 13.0, shade(panel::SECONDARY));
            p.text(
                &text::ellipsize(&gauge.label, &label, 96.0),
                x0,
                centre,
                &label,
            );
            // A gauge that only shows time, as a timer's does, has no number.
            let percent = if gauge.measured {
                format!("{}%", gauge.percent)
            } else {
                String::new()
            };
            let number = Style {
                tabular: true,
                ..Style::new(
                    Face::Mono,
                    13.0,
                    shade(meter_text(gauge.percent, facts.ahead)),
                )
            };
            p.text(
                &percent,
                x0 + inner - text::width(&percent, &number),
                centre,
                &number,
            );
            let resets_right = x0 + inner - 46.0;
            let track_x = x0 + 104.0;
            let track_w = resets_right - 96.0 - track_x;
            paint_gauge(
                p,
                facts,
                gauge.percent,
                gauge.through,
                [track_x, centre - 3.0, track_w],
                &shade,
            );
            if let Some(left) = &gauge.resets_in {
                let small = Style::new(Face::Body, 12.0, shade(panel::HINT));
                let left = text::ellipsize(left, &small, 88.0);
                p.text(
                    &left,
                    resets_right - text::width(&left, &small),
                    centre,
                    &small,
                );
            }
            y += GAUGE_ROW_H;
        }
        if let Some(note) = &section.note {
            let style = Style::new(Face::Body, 12.5, quiet(facts.ahead));
            p.text(
                &text::ellipsize(note, &style, inner),
                x0,
                y + NOTE_H / 2.0 - 2.0,
                &style,
            );
            y += NOTE_H;
        }
    }
    let hidden = hidden_count(shown);
    if hidden > 0 {
        let quiet = Style::new(Face::Body, 12.5, panel::HINT);
        p.text(&format!("{hidden} hidden"), x0, y + NOTE_H / 2.0, &quiet);
        y += NOTE_H;
    }
    if !card.buttons.is_empty() {
        y += BUTTON_GAP;
        let label = Style::new(Face::Body, 13.5, INK);
        let mut at = x0;
        for button in &card.buttons {
            let bw = text::width(&button.label, &label) + 28.0;
            panel::quiet_button(p, at, y, bw, BUTTON_H, panel::ROW_RADIUS, false);
            p.text(&button.label, at + 14.0, y + BUTTON_H / 2.0, &label);
            shapes.push((
                Item::Button(shown.id.clone(), button.id.clone()),
                [at, y, bw, BUTTON_H, panel::ROW_RADIUS],
            ));
            at += bw + BUTTON_GAP;
        }
        y += BUTTON_H;
    }
    if let Some(problem) = &shown.problem {
        let note = Style::new(Face::Body, 12.5, quiet(facts.ahead));
        p.text(
            &text::ellipsize(&format!("Not up to date. {problem}"), &note, inner),
            x0,
            y + NOTE_H / 2.0,
            &note,
        );
    }
}

/// A gauge's track at `[x, y, w]`, 6 tall: the time gone by in its window shaded in the time
/// colour, `percent` used filled over it, and any used beyond the time gone by in the second
/// colour.
fn paint_gauge(
    p: &mut Painter,
    facts: &Facts,
    percent: u8,
    through: Option<f32>,
    [x, y, w]: [f32; 3],
    shade: &dyn Fn(u32) -> u32,
) {
    const H: f32 = 6.0;
    p.fill(x, y, w, H, H / 2.0, shade(panel::TRACK));
    if let Some(through) = through.filter(|through| *through > 0.0) {
        let time = (facts.time & 0xffffff00) | TIME_ALPHA;
        p.fill(x, y, (w * through).max(H), H, H / 2.0, shade(time));
    }
    let used = w * percent as f32 / 100.0;
    if used > 0.0 {
        p.fill(
            x,
            y,
            used.max(H),
            H,
            H / 2.0,
            shade(card::ink(percent, facts.ahead)),
        );
    }
    if let Some(through) = through {
        let gone = w * through;
        if used > gone + 1.0 {
            p.fill(gone + x, y, used - gone, H, H / 2.0, shade(facts.ahead));
        }
    }
}

/// One plugin's card showing its switches, at (`x`, `y`) and `w` wide: the way back, a switch for
/// each section the plugin reports, and, when its gauges show time, what their two colours mean.
fn paint_back(
    p: &mut Painter,
    facts: &Facts,
    shown: &Shown,
    x: f32,
    y: f32,
    w: f32,
    shapes: &mut Vec<(Item, [f32; 5])>,
) {
    let x0 = x + CARD_PAD;
    let inner = w - 2.0 * CARD_PAD;
    let mut y = y + CARD_PAD;
    let back_y = y + (NAME_H - ROUND) / 2.0 - 2.0;
    round_button(p, icons::BACK, x0, back_y);
    shapes.push((
        Item::Back(shown.id.clone()),
        [x0, back_y, ROUND, ROUND, ROUND / 2.0],
    ));
    let name = Style::new(Face::BodyBold, 15.0, INK);
    p.text(
        &text::ellipsize(
            &format!("{} shows", shown.name),
            &name,
            inner - ROUND - 10.0,
        ),
        x0 + ROUND + 10.0,
        y + NAME_H / 2.0 - 2.0,
        &name,
    );
    y += NAME_H + 6.0;
    let label = Style::new(Face::Body, 14.0, INK);
    for section in shown.card.iter().flat_map(|card| &card.sections) {
        let on = !shown.hidden.contains(&section.name);
        let centre = y + CHOICE_H / 2.0;
        p.text(
            &text::ellipsize(&section.name, &label, inner - 60.0),
            x0,
            centre,
            &label,
        );
        let (sw, sh) = (34.0, 20.0);
        let sx = x0 + inner - sw;
        p.fill(
            sx,
            centre - sh / 2.0,
            sw,
            sh,
            sh / 2.0,
            if on { facts.accent } else { panel::QUIET_LIT },
        );
        let knob = if on { sx + sw - sh + 3.0 } else { sx + 3.0 };
        p.fill(
            knob,
            centre - sh / 2.0 + 3.0,
            sh - 6.0,
            sh - 6.0,
            (sh - 6.0) / 2.0,
            if on {
                panel::ink_on(facts.accent)
            } else {
                panel::SECONDARY
            },
        );
        shapes.push((
            Item::Section(shown.id.clone(), section.name.clone()),
            [
                x0 - 6.0,
                y + 2.0,
                inner + 12.0,
                CHOICE_H - 4.0,
                panel::ROW_RADIUS,
            ],
        ));
        y += CHOICE_H;
    }
    if has_time(shown) {
        y += 6.0;
        let quiet = Style::new(Face::Body, 12.5, panel::HINT);
        let swatch = |p: &mut Painter, y: f32, colour: u32, said: &str| {
            p.fill(x0, y + KEY_H / 2.0 - 3.0, 18.0, 6.0, 3.0, colour);
            p.text(said, x0 + 28.0, y + KEY_H / 2.0, &quiet);
        };
        swatch(
            p,
            y,
            (facts.time & 0xffffff00) | TIME_ALPHA,
            "Shaded: how much of the period has gone by",
        );
        swatch(
            p,
            y + KEY_H,
            facts.ahead,
            "Used faster than time has gone by",
        );
    }
}

/// A gauge's number: in `attention` and hotter as its allowance runs out, quiet otherwise.
fn meter_text(percent: u8, attention: u32) -> u32 {
    match percent {
        75.. => card::ink(percent, attention),
        _ => panel::SECONDARY,
    }
}

/// A note that something isn't right: `attention`, a little quieter.
fn quiet(attention: u32) -> u32 {
    (attention & 0xffffff00) | 0xc8
}

impl Slipstream {
    /// A click on the chip, or its key: opens the panel under the chip, or closes it.
    pub fn toggle_plugin_panel(&mut self) {
        if self.plugin_panel.is_open() {
            self.plugin_panel.close();
        } else {
            let under = self.chip_middle();
            self.close_panels();
            let now = self.clock.tick();
            self.plugin_panel.open(now, under);
            // A plugin's folder can change without anything to say so: look again.
            self.plugins.rescan();
        }
    }

    pub fn plugin_facts(&self) -> Facts {
        let cards = self.plugins.shown();
        let installed = self.plugins.chip(&cards).map_or(0, |chip| chip.installed);
        Facts {
            cards,
            installed,
            ring: self.panel_ring(),
            time: panel::opaque(self.ring_rgb),
            ahead: panel::opaque(self.bullet_rgb),
            accent: self.panel_accent(),
            reduced_motion: self.clock.reduced_motion,
            key: self.key_label(crate::keys::Action::Plugins),
        }
    }

    /// Where the chip's middle is on the focused screen's bar, in logical pixels from its left;
    /// the screen's middle if its bar has no chip.
    pub fn chip_middle(&self) -> f64 {
        let name = self.focused_screen_name();
        self.drawn
            .chromes
            .get(&name)
            .and_then(|chrome| chrome.bar.area_of(crate::bar::Target::Plugins))
            .map(|area| area.loc.x + area.size.w / 2.0)
            .or_else(|| {
                self.focused_screen_geometry()
                    .map(|geo| geo.size.w as f64 / 2.0)
            })
            .unwrap_or_default()
    }

    pub fn plugin_key(&mut self, key: Keysym, shift: bool) {
        let facts = self.plugin_facts();
        let request = self.plugin_panel.key(key, shift, &facts);
        self.plugin_request(request);
    }

    pub fn plugin_click(&mut self, x: f64, y: f64) {
        let request = self.plugin_panel.click(x, y);
        self.plugin_request(request);
    }

    fn plugin_request(&mut self, request: Request) {
        match request {
            Request::Nothing => {}
            Request::Close => self.plugin_panel.close(),
            Request::Press { plugin, button } => self.plugin_press(&plugin, &button),
            Request::Toggle { plugin, section } => self.change_settings(|settings| {
                settings.plugins.toggle_hidden(&plugin, &section);
            }),
            Request::Settings => {
                self.plugin_panel.close();
                self.open_slipstream_settings("plugins");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use slipstream_config::{
        card::{Gauge, Section},
        plugins::{Button, Card},
    };

    use super::*;

    fn shown(id: &str, buttons: &[&str]) -> Shown {
        Shown {
            id: id.into(),
            name: id.into(),
            card: Some(Card {
                buttons: buttons
                    .iter()
                    .map(|b| Button {
                        id: b.to_string(),
                        label: b.to_string(),
                        ..Button::default()
                    })
                    .collect(),
                ..Card::default()
            }),
            hidden: Vec::new(),
            problem: None,
            held: false,
        }
    }

    fn with_sections(id: &str, names: &[&str], through: Option<f32>) -> Shown {
        let mut shown = shown(id, &[]);
        shown.card.as_mut().unwrap().sections = names
            .iter()
            .map(|name| Section {
                name: name.to_string(),
                detail: None,
                gauges: vec![Gauge {
                    label: "Week".into(),
                    percent: 40,
                    measured: true,
                    resets_in: None,
                    through,
                }],
                note: None,
                stale: false,
            })
            .collect();
        shown
    }

    fn facts(cards: Vec<Shown>) -> Facts {
        Facts {
            installed: cards.len(),
            cards,
            ring: panel::FOCUS,
            time: 0x42d3ffff,
            ahead: panel::AMBER,
            accent: panel::ACCENT,
            reduced_motion: true,
            key: "Super+Y".into(),
        }
    }

    #[test]
    fn the_keys_go_round_every_button_and_the_settings_link() {
        let facts = facts(vec![shown("timer", &["pause", "skip"]), shown("disk", &[])]);
        let mut panel = PluginPanel::default();
        panel.open(0.0, 800.0);
        assert_eq!(
            panel.key(Keysym::Return, false, &facts),
            Request::Press {
                plugin: "timer".into(),
                button: "pause".into()
            }
        );
        panel.key(Keysym::Tab, false, &facts);
        panel.key(Keysym::Tab, false, &facts);
        assert_eq!(panel.key(Keysym::space, false, &facts), Request::Settings);
        panel.key(Keysym::Tab, false, &facts);
        panel.key(Keysym::Tab, true, &facts);
        assert_eq!(panel.key(Keysym::Return, false, &facts), Request::Settings);
        assert_eq!(panel.key(Keysym::Escape, false, &facts), Request::Close);
    }

    #[test]
    fn with_nothing_to_press_the_keyboard_is_on_the_settings_link() {
        let facts = facts(Vec::new());
        let mut panel = PluginPanel::default();
        panel.open(0.0, 800.0);
        assert_eq!(panel.key(Keysym::Down, false, &facts), Request::Nothing);
        assert_eq!(panel.key(Keysym::Return, false, &facts), Request::Settings);
    }

    /// The cog slides a card to a switch for every section, hidden ones included, and Esc slides it
    /// back before it closes anything.
    #[test]
    fn the_cog_leads_to_a_switch_for_every_section_and_esc_comes_back() {
        let mut usage = with_sections("allotments", &["Orchard", "Meadow", "Copse"], None);
        usage.hidden = vec!["Meadow".into(), "Copse".into()];
        let facts = facts(vec![usage]);
        let mut panel = PluginPanel::default();
        panel.open(0.0, 800.0);
        assert_eq!(panel.selected(&facts), Item::Cog("allotments".into()));
        assert_eq!(panel.key(Keysym::Return, false, &facts), Request::Nothing);
        assert_eq!(panel.switches(), Some("allotments"));
        panel.key(Keysym::Down, false, &facts);
        panel.key(Keysym::Down, false, &facts);
        assert_eq!(
            panel.key(Keysym::space, false, &facts),
            Request::Toggle {
                plugin: "allotments".into(),
                section: "Meadow".into()
            },
            "a hidden section has its switch too"
        );
        assert_eq!(panel.key(Keysym::Escape, false, &facts), Request::Nothing);
        assert_eq!(panel.switches(), None);
        assert_eq!(panel.selected(&facts), Item::Cog("allotments".into()));
        assert_eq!(panel.key(Keysym::Escape, false, &facts), Request::Close);
    }

    #[test]
    fn a_card_grows_with_what_it_shows() {
        let bare = front_height(&shown("a", &[]));
        let buttons = front_height(&shown("a", &["go"]));
        assert!(buttons > bare);
        let quiet = Shown {
            card: None,
            ..shown("a", &[])
        };
        assert_eq!(front_height(&quiet), bare + LINE_H);
        let mut usage = with_sections("allotments", &["Orchard", "Meadow"], None);
        let both = front_height(&usage);
        usage.hidden = vec!["Meadow".into()];
        assert!(
            front_height(&usage) < both,
            "a hidden section takes no room"
        );
        assert_eq!(
            back_height(&with_sections("allotments", &["Orchard"], Some(0.5))),
            back_height(&with_sections("allotments", &["Orchard"], None)) + 6.0 + 2.0 * KEY_H,
            "the key to the colours shows only for gauges that show time"
        );
    }

    #[test]
    fn colours_come_from_the_settings_rgb() {
        assert_eq!(panel::opaque([1.0, 0.5, 0.0]), 0xff8000ff);
    }
}
