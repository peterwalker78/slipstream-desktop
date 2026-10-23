//! The plugin panel, from a click on the chip beside the clock: a glass card hanging under the
//! chip with a card for each plugin that's on, and the way to the Settings app's Plugins page.
//!
//! A card shows what its plugin's program last printed (`plugins.rs`): a few lines, named groups of
//! gauges with the time until each starts over, and up to three buttons. While the panel is open it
//! takes every key: Tab or the arrows move between the buttons and the settings link, Enter or
//! Space presses, Esc closes. A ring shows where the keyboard is.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, meter,
    paint::{Painted, Painter},
    panel::{self, DESIGN_PX, GAP, INK, PADDING},
    plugins::Shown,
    text::{self, Face, Style},
};

// Sizes in design pixels.
const WIDTH: f32 = 480.0;
const HEAD_H: f32 = 30.0;
const FOOT_H: f32 = 31.0;
const CARD_PAD: f32 = 14.0;
const CARD_GAP: f32 = 10.0;
const NAME_H: f32 = 22.0;
const LINE_H: f32 = 21.0;
const SECTION_HEAD_H: f32 = 24.0;
const GAUGE_ROW_H: f32 = 24.0;
const NOTE_H: f32 = 20.0;
const BUTTON_H: f32 = 30.0;
const BUTTON_GAP: f32 = 8.0;
/// The squares beside the panel's name, as on the chip.
const SQUARE: f32 = 8.0;

/// Something the keyboard can be on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
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
    Press { plugin: String, button: String },
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
    pub reduced_motion: bool,
}

/// Everything the painted panel depends on.
#[derive(Clone, PartialEq)]
struct Look {
    facts: Facts,
    selected: Item,
    under: f64,
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
    shown: Option<Look>,
    painted: Option<Painted>,
    /// The painted area's corner and the card, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Item, Rectangle<f64, Logical>)>,
}

/// The keyboard's stops, in order: every button, card by card, then the settings link.
fn items(facts: &Facts) -> Vec<Item> {
    facts
        .cards
        .iter()
        .flat_map(|shown| {
            shown
                .card
                .iter()
                .flat_map(|card| &card.buttons)
                .map(|button| Item::Button(shown.id.clone(), button.id.clone()))
        })
        .chain(std::iter::once(Item::Settings))
        .collect()
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
        self.shown = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = None;
        self.painted = None;
        self.targets.clear();
    }

    /// Where the keyboard is: the one chosen while it's still there, else the first stop.
    fn selected(&self, facts: &Facts) -> Item {
        let items = items(facts);
        self.selected
            .clone()
            .filter(|item| items.contains(item))
            .unwrap_or_else(|| items[0].clone())
    }

    /// A key while it's open.
    pub fn key(&mut self, sym: Keysym, shift: bool, facts: &Facts) -> Request {
        let items = items(facts);
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
            Keysym::Escape => return Request::Close,
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                self.selected = Some(items[step(shift || sym == Keysym::ISO_Left_Tab)].clone());
            }
            Keysym::Up | Keysym::Left => self.selected = Some(items[step(true)].clone()),
            Keysym::Down | Keysym::Right => self.selected = Some(items[step(false)].clone()),
            Keysym::Home => self.selected = items.first().cloned(),
            Keysym::End => self.selected = items.last().cloned(),
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                return Self::press(&items[at]);
            }
            _ => {}
        }
        Request::Nothing
    }

    fn press(item: &Item) -> Request {
        match item {
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
            Some(item) => {
                self.selected = Some(item.clone());
                Self::press(&item)
            }
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
        let look = Look {
            selected: self.selected(&facts),
            under: self.under,
            facts,
            width: frame.size.w,
            scale: frame.scale,
        };
        if self.shown.as_ref() != Some(&look) {
            self.repaint(&look)?;
            self.shown = Some(look);
        }
        let (alpha, rise) = panel::opening(frame.time - self.opened_at, reduced_motion);
        let at = Point::from((self.origin.x, self.origin.y + rise * DESIGN_PX as f64));
        self.painted.as_ref()?.element(renderer, at, alpha)
    }

    fn repaint(&mut self, look: &Look) -> Option<()> {
        let facts = &look.facts;
        let inner = WIDTH - 2.0 * PADDING;
        let heights: Vec<f32> = facts.cards.iter().map(card_height).collect();
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
                paint_card(p, shown, x0, y, inner, *h, &mut shapes);
                y += h + CARD_GAP;
            }
            if !facts.cards.is_empty() {
                y -= CARD_GAP;
            }
            y += GAP;
            p.fill(x0, y, inner, 1.0, 0.0, panel::DIVIDER);
            let centre = y + FOOT_H - 9.0;
            let link = Style::new(Face::Body, 14.0, 0xffd08aff);
            let link_w = p.text("Plugin settings", x0, centre, &link);
            shapes.push((
                Item::Settings,
                [x0 - 6.0, centre - 13.0, link_w + 12.0, 26.0, 6.0],
            ));
            let close = panel::key_hint(p, x0 + inner, centre, &["Esc"], "close");
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
            panel::MINT
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

/// A card's height: its name, then what its program printed, or why it printed nothing.
fn card_height(shown: &Shown) -> f32 {
    let body = match &shown.card {
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
            card.lines.len() as f32 * LINE_H + sections + buttons
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

/// One plugin's card, `h` tall at (`x`, `y`): its name, its lines, its gauges and its buttons. A
/// card whose program has stopped answering is dimmed, and says why.
fn paint_card(
    p: &mut Painter,
    shown: &Shown,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    shapes: &mut Vec<(Item, [f32; 5])>,
) {
    p.fill(x, y, w, h, panel::TILE_RADIUS, panel::TILE);
    p.border(x, y, w, h, panel::TILE_RADIUS, 1.0, panel::DIVIDER);
    let x0 = x + CARD_PAD;
    let inner = w - 2.0 * CARD_PAD;
    let mut y = y + CARD_PAD;
    let stale = shown.problem.is_some();
    let shade = |rgba: u32| if stale { meter::dim(rgba) } else { rgba };
    let name = Style::new(Face::BodyBold, 15.0, INK);
    p.text(
        &text::ellipsize(&shown.name, &name, inner),
        x0,
        y + NAME_H / 2.0 - 2.0,
        &name,
    );
    y += NAME_H;
    let Some(card) = &shown.card else {
        let note = Style::new(Face::Body, 13.0, 0xd9b27aff);
        let said = match &shown.problem {
            _ if shown.held => "Changed since it was turned on: turn it on again to run it".into(),
            Some(problem) => format!("Not answering: {problem}"),
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
                meter::dim(rgba)
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
            let percent = format!("{}%", gauge.percent);
            let number = Style {
                tabular: true,
                ..Style::new(Face::Mono, 13.0, shade(meter_text(gauge.percent)))
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
            p.fill(track_x, centre - 3.0, track_w, 6.0, 3.0, shade(0x3a3f4bff));
            let filled = track_w * gauge.percent as f32 / 100.0;
            if filled > 0.0 {
                p.fill(
                    track_x,
                    centre - 3.0,
                    filled.max(6.0),
                    6.0,
                    3.0,
                    shade(meter::ink(gauge.percent)),
                );
            }
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
            let style = Style::new(Face::Body, 12.5, 0xd9b27aff);
            p.text(
                &text::ellipsize(note, &style, inner),
                x0,
                y + NOTE_H / 2.0 - 2.0,
                &style,
            );
            y += NOTE_H;
        }
    }
    if !card.buttons.is_empty() {
        y += BUTTON_GAP;
        let label = Style::new(Face::Body, 13.5, INK);
        let mut at = x0;
        for button in &card.buttons {
            let bw = text::width(&button.label, &label) + 28.0;
            panel::quiet_button(p, at, y, bw, BUTTON_H, 8.0, false);
            p.text(&button.label, at + 14.0, y + BUTTON_H / 2.0, &label);
            shapes.push((
                Item::Button(shown.id.clone(), button.id.clone()),
                [at, y, bw, BUTTON_H, 8.0],
            ));
            at += bw + BUTTON_GAP;
        }
        y += BUTTON_H;
    }
    if let Some(problem) = shown.problem.as_ref().filter(|_| shown.card.is_some()) {
        let note = Style::new(Face::Body, 12.5, 0xd9b27aff);
        p.text(
            &text::ellipsize(&format!("Not up to date: {problem}"), &note, inner),
            x0,
            y + NOTE_H / 2.0,
            &note,
        );
    }
}

/// A gauge's number: amber and hotter as its allowance runs out, quiet otherwise.
fn meter_text(percent: u8) -> u32 {
    match percent {
        75.. => meter::ink(percent),
        _ => panel::SECONDARY,
    }
}

impl Slipstream {
    /// A click on the chip: opens the panel under it, or closes it.
    pub fn toggle_plugin_panel(&mut self) {
        if self.plugin_panel.is_open() {
            self.plugin_panel.close();
        } else {
            let under = self.chip_middle();
            self.close_panels();
            let now = self.clock.tick();
            self.plugin_panel.open(now, under);
        }
    }

    pub fn plugin_facts(&self) -> Facts {
        let cards = self.plugins.shown();
        let installed = self
            .plugins
            .chip(&cards)
            .map_or(0, |chip| chip.installed);
        Facts {
            cards,
            installed,
            ring: self.panel_ring(),
            reduced_motion: self.clock.reduced_motion,
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
            Request::Press { plugin, button } => self.plugins.press(&plugin, &button),
            Request::Settings => {
                self.plugin_panel.close();
                self.open_slipstream_settings("plugins");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use slipstream_config::plugins::{Button, Card};

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
                    })
                    .collect(),
                ..Card::default()
            }),
            problem: None,
            held: false,
        }
    }

    fn facts(cards: Vec<Shown>) -> Facts {
        Facts {
            installed: cards.len(),
            cards,
            ring: panel::FOCUS,
            reduced_motion: true,
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

    #[test]
    fn a_card_grows_with_what_it_shows() {
        let bare = card_height(&shown("a", &[]));
        let buttons = card_height(&shown("a", &["go"]));
        assert!(buttons > bare);
        let quiet = Shown {
            card: None,
            ..shown("a", &[])
        };
        assert_eq!(card_height(&quiet), bare + LINE_H);
    }
}
