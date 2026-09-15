//! Quick settings, on Super+A or a click on the bar's tray: a glass panel. The toggles and sliders
//! changed most often, a power menu, and the way into the Settings app.
//!
//! While it's open it takes every key: arrows or Tab move, Enter or Space presses, ←/→ move a
//! slider, Esc closes. A mint ring shows where the keyboard is. The Wi-Fi and Bluetooth tiles
//! carry a chevron that opens the system's own settings page for them.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    Slipstream, bar,
    exit::Intent,
    icons,
    keys::App,
    launch,
    paint::Painted,
    panel::{self, DARK, GAP, INK, MOCKUP_PX, PADDING},
    status::{self, Reading},
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels.
const WIDTH: f32 = 480.0;
const HEAD_H: f32 = 42.0;
const MENU_H: f32 = 40.0;
const TILE_H: f32 = 60.0;
const TILE_GAP: f32 = 10.0;
const SLIDER_H: f32 = 30.0;
/// The footer: its dividing line, 12 above the text, and the text's line.
const FOOT_H: f32 = 31.0;
/// The chevron's zone at the right of a tile that has one.
const CHEVRON_W: f32 = 36.0;

/// Something in quick settings the keyboard can move to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Control {
    /// The battery line in the header: Settings' Power page, with the charging details.
    Battery,
    /// Locks the screen.
    Lock,
    Power,
    Suspend,
    Restart,
    ShutDown,
    LogOut,
    #[default]
    WiFi,
    /// The chevron on the Wi-Fi tile: the system's network settings.
    WiFiPage,
    Bluetooth,
    /// The chevron on the Bluetooth tile: the system's Bluetooth settings.
    BluetoothPage,
    DoNotDisturb,
    NightLight,
    ReducedMotion,
    PowerMode,
    /// The volume slider; pressing it mutes.
    Volume,
    Brightness,
    AllSettings,
}

const TILES: [Control; 6] = [
    Control::WiFi,
    Control::Bluetooth,
    Control::DoNotDisturb,
    Control::NightLight,
    Control::ReducedMotion,
    Control::PowerMode,
];

const POWER_MENU: [Control; 4] = [
    Control::Suspend,
    Control::Restart,
    Control::ShutDown,
    Control::LogOut,
];

impl Control {
    /// The chevron a tile carries, if it has one.
    fn page(self) -> Option<Control> {
        match self {
            Self::WiFi => Some(Self::WiFiPage),
            Self::Bluetooth => Some(Self::BluetoothPage),
            _ => None,
        }
    }

    fn is_page(self) -> bool {
        matches!(self, Self::WiFiPage | Self::BluetoothPage)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Battery => "Battery details",
            Self::Lock => "Lock",
            Self::Power => "Power",
            Self::Suspend => "Sleep",
            Self::Restart => "Restart",
            Self::ShutDown => "Shut down",
            Self::LogOut => "Log out",
            Self::WiFi => "Wi-Fi",
            Self::WiFiPage => "Network settings",
            Self::Bluetooth => "Bluetooth",
            Self::BluetoothPage => "Bluetooth settings",
            Self::DoNotDisturb => "Do not disturb",
            Self::NightLight => "Night light",
            Self::ReducedMotion => "Reduced motion",
            Self::PowerMode => "Power mode",
            Self::Volume => "Volume",
            Self::Brightness => "Brightness",
            Self::AllSettings => "All settings",
        }
    }
}

/// What a key or click asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Nothing,
    Close,
    /// Turn a tile's setting over, or do what a button does.
    Press(Control),
    /// Set the volume, or the brightness, in percent.
    Volume(u8),
    Brightness(u8),
}

/// Everything quick settings shows that it doesn't own.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub status: Reading,
    pub do_not_disturb: bool,
    pub night_light: bool,
    pub reduced_motion: bool,
    pub user: String,
    /// The keyboard ring's colour, the same as the focused window's.
    pub ring: u32,
}

impl Default for Facts {
    fn default() -> Self {
        Self {
            status: Reading::default(),
            do_not_disturb: false,
            night_light: false,
            reduced_motion: false,
            user: String::new(),
            ring: crate::panel::FOCUS,
        }
    }
}

/// Everything the painted panel depends on.
#[derive(Clone, PartialEq)]
struct Look {
    facts: Facts,
    selected: Control,
    power_menu: bool,
    width: i32,
    scale: f64,
}

/// A slider's track: where a click lands, and the line the level is measured along, in logical
/// pixels.
#[derive(Debug, Clone, Copy)]
struct Track {
    control: Control,
    hit: Rectangle<f64, Logical>,
    left: f64,
    width: f64,
}

pub struct QuickSettings {
    open: bool,
    /// When it opened, on the animation clock.
    opened_at: f64,
    selected: Control,
    power_menu: bool,
    shown: Option<Look>,
    painted: Option<Painted>,
    /// The painted area's corner and the card, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Control, Rectangle<f64, Logical>)>,
    tracks: Vec<Track>,
    /// A slider held under the pointer, and the level it was last set to, so moving along it
    /// only changes the machine when the level does.
    dragging: Option<(Control, u8)>,
    /// Opens with a short fade instead of rising (`Slipstream::set_reduced_motion`).
    pub reduced_motion: bool,
}

impl Default for QuickSettings {
    fn default() -> Self {
        Self {
            open: false,
            opened_at: 0.0,
            selected: Control::default(),
            power_menu: false,
            shown: None,
            painted: None,
            origin: Point::default(),
            frame: Rectangle::from_size((0.0, 0.0).into()),
            targets: Vec::new(),
            tracks: Vec::new(),
            dragging: None,
            reduced_motion: false,
        }
    }
}

impl QuickSettings {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, now: f64) {
        self.open = true;
        self.opened_at = now;
        self.selected = Control::default();
        self.power_menu = false;
        self.shown = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.power_menu = false;
        self.shown = None;
        self.painted = None;
        self.targets.clear();
        self.tracks.clear();
        self.dragging = None;
    }

    /// The pointer moved with a slider held: its level follows the pointer along the track, even
    /// past either end.
    pub fn drag(&mut self, x: f64) -> Request {
        let Some((control, last)) = self.dragging else {
            return Request::Nothing;
        };
        let Some(track) = self.tracks.iter().find(|track| track.control == control) else {
            return Request::Nothing;
        };
        let percent = level_at(x, track.left, track.width);
        if percent == last {
            return Request::Nothing;
        }
        self.dragging = Some((control, percent));
        slider_request(control, percent)
    }

    /// The button went up: whatever slider was held is let go.
    pub fn release(&mut self) {
        self.dragging = None;
    }

    /// The controls in keyboard order, row by row.
    fn rows(&self, facts: &Facts) -> Vec<Vec<Control>> {
        let mut rows = vec![vec![Control::Battery, Control::Lock, Control::Power]];
        if self.power_menu {
            rows.push(POWER_MENU.to_vec());
        }
        // Each chevron straight after its tile, in the same row.
        rows.extend(TILES.chunks(2).map(|pair| {
            pair.iter()
                .flat_map(|tile| std::iter::once(*tile).chain(tile.page()))
                .collect()
        }));
        rows.push(vec![Control::Volume]);
        if facts.status.brightness.is_some() {
            rows.push(vec![Control::Brightness]);
        }
        rows.push(vec![Control::AllSettings]);
        rows
    }

    /// A key while it's open.
    pub fn key(&mut self, sym: Keysym, shift: bool, facts: &Facts) -> Request {
        let rows = self.rows(facts);
        let (row, column) = rows
            .iter()
            .enumerate()
            .find_map(|(r, controls)| {
                controls
                    .iter()
                    .position(|control| *control == self.selected)
                    .map(|c| (r, c))
            })
            .unwrap_or((0, 0));
        match sym {
            Keysym::Escape if self.power_menu => {
                self.power_menu = false;
                self.selected = Control::Power;
            }
            Keysym::Escape => return Request::Close,
            Keysym::Up | Keysym::Down => {
                let to = if sym == Keysym::Up {
                    row.checked_sub(1)
                } else {
                    Some(row + 1).filter(|&to| to < rows.len())
                };
                if let Some(to) = to {
                    // By what's drawn: a chevron is in its tile's column.
                    let columns = |controls: &[Control]| {
                        controls.iter().filter(|control| !control.is_page()).count()
                    };
                    let wanted = columns(&rows[row][..=column]).saturating_sub(1);
                    let wanted = wanted.min(columns(&rows[to]).saturating_sub(1));
                    self.selected = rows[to]
                        .iter()
                        .enumerate()
                        .find(|(at, _)| columns(&rows[to][..=*at]).saturating_sub(1) == wanted)
                        .map_or(rows[to][0], |(_, control)| *control);
                }
            }
            Keysym::Left | Keysym::Right => {
                let step = if sym == Keysym::Left { -5 } else { 5 };
                match self.selected {
                    Control::Volume => return Request::Volume(nudge(facts.status.volume, step)),
                    Control::Brightness => {
                        return facts.status.brightness.map_or(Request::Nothing, |level| {
                            Request::Brightness(nudge(level, step).max(1))
                        });
                    }
                    _ => {
                        let to = if step < 0 {
                            column.checked_sub(1)
                        } else {
                            Some(column + 1).filter(|&to| to < rows[row].len())
                        };
                        if let Some(to) = to {
                            self.selected = rows[row][to];
                        }
                    }
                }
            }
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let order = rows.concat();
                let at = order
                    .iter()
                    .position(|control| *control == self.selected)
                    .unwrap_or(0);
                let next = if shift || sym == Keysym::ISO_Left_Tab {
                    at + order.len() - 1
                } else {
                    at + 1
                };
                self.selected = order[next % order.len()];
            }
            Keysym::Home | Keysym::End => {
                let order = rows.concat();
                let to = if sym == Keysym::Home {
                    order.first()
                } else {
                    order.last()
                };
                if let Some(control) = to {
                    self.selected = *control;
                }
            }
            // Space presses as Enter does: quick settings is only ever opened on purpose.
            Keysym::Return | Keysym::KP_Enter | Keysym::space => return self.press(self.selected),
            _ => {}
        }
        Request::Nothing
    }

    fn press(&mut self, control: Control) -> Request {
        self.selected = control;
        match control {
            Control::Power => {
                self.power_menu = !self.power_menu;
                if self.power_menu {
                    self.selected = Control::Suspend;
                }
                Request::Nothing
            }
            Control::Brightness => Request::Nothing,
            other => Request::Press(other),
        }
    }

    /// A click, in logical pixels from the output's corner. Outside the card it closes. A press on a
    /// slider also holds it, so dragging moves it.
    pub fn click(&mut self, x: f64, y: f64) -> Request {
        if let Some(track) = self.tracks.iter().find(|track| track.hit.contains((x, y))) {
            let percent = level_at(x, track.left, track.width);
            let control = track.control;
            self.selected = control;
            self.dragging = Some((control, percent));
            return slider_request(control, percent);
        }
        let target = self
            .targets
            .iter()
            .find(|(_, area)| area.contains((x, y)))
            .map(|(control, _)| *control);
        match target {
            Some(control) => self.press(control),
            None if self.frame.contains((x, y)) => Request::Nothing,
            None => Request::Close,
        }
    }

    /// The panel for a screen `width` logical pixels wide at `scale`, repainted when what it
    /// shows changes.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
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
            facts,
            selected: self.selected,
            power_menu: self.power_menu,
            width,
            scale,
        };
        if self.shown.as_ref() != Some(&look) {
            self.repaint(&look)?;
            self.shown = Some(look);
        }
        let (alpha, rise) = panel::opening(now - self.opened_at, reduced_motion);
        let at = Point::from((self.origin.x, self.origin.y + rise * MOCKUP_PX as f64));
        self.painted.as_ref()?.element(renderer, at, alpha)
    }

    fn repaint(&mut self, look: &Look) -> Option<()> {
        let facts = &look.facts;
        let status = &facts.status;
        let mut sliders = vec![(
            Control::Volume,
            if status.muted {
                icons::VOLUME_MUTED
            } else {
                icons::VOLUME
            },
            status.volume,
        )];
        if let Some(level) = status.brightness {
            sliders.push((Control::Brightness, icons::SUN, level));
        }
        let menu_h = if look.power_menu { MENU_H + GAP } else { 0.0 };
        let height = PADDING
            + HEAD_H
            + GAP
            + menu_h
            + 3.0 * TILE_H
            + 2.0 * TILE_GAP
            + GAP
            + sliders.len() as f32 * (SLIDER_H + GAP)
            + FOOT_H
            + PADDING;
        let (fx, fy) = (panel::MARGIN, panel::MARGIN);
        // The card's left edge on screen, in mockup pixels.
        let left = look.width as f32 / MOCKUP_PX - panel::RIGHT - WIDTH;
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
        let mut targets = Vec::new();
        let mut tracks = Vec::new();

        let painted = Painted::new(logical, look.scale, |p| {
            p.f *= MOCKUP_PX;
            panel::glass(p, fx, fy, WIDTH, height);
            let x0 = fx + PADDING;
            let inner = WIDTH - 2.0 * PADDING;
            let mut y = fy + PADDING;
            // Each control's box (x, y, w, h and corner radius), for the keyboard's ring.
            let mut shapes: Vec<(Control, [f32; 5])> = Vec::new();

            // The battery, its icon showing the charger, and quietly beneath it who is logged in;
            // then the lock and power buttons. The battery and name together open the Power page.
            let power_x = x0 + inner - 40.0;
            let lock_x = power_x - 8.0 - 40.0;
            let room = lock_x - 12.0 - x0;
            let battery = Style::new(Face::Body, 16.0, INK);
            let mut text_x = x0;
            if let Some((percent, plugged)) = status.battery {
                p.icon(
                    &icons::battery(percent, plugged),
                    x0,
                    y + 3.0,
                    18.0,
                    Some(INK),
                );
                text_x += 26.0;
            }
            let battery_w = p.text(
                &text::ellipsize(&battery_line(status), &battery, room - (text_x - x0)),
                text_x,
                y + 12.0,
                &battery,
            );
            let small = Style::new(Face::Body, 13.0, 0x8f98a8ff);
            let name_w = p.text(
                &text::ellipsize(&facts.user, &small, room - (text_x - x0)),
                text_x,
                y + 31.0,
                &small,
            );
            let header_w = text_x - x0 + battery_w.max(name_w);
            shapes.push((
                Control::Battery,
                [x0 - 6.0, y + 1.0, header_w + 12.0, 40.0, 8.0],
            ));
            let fill = if look.power_menu {
                0xffffff24
            } else {
                0xffffff10
            };
            p.fill(lock_x, y + 1.0, 40.0, 40.0, 20.0, 0xffffff10);
            p.icon(icons::LOCK, lock_x + 11.0, y + 12.0, 18.0, Some(INK));
            shapes.push((Control::Lock, [lock_x, y + 1.0, 40.0, 40.0, 20.0]));
            p.fill(power_x, y + 1.0, 40.0, 40.0, 20.0, fill);
            p.icon(icons::POWER, power_x + 11.0, y + 12.0, 18.0, Some(INK));
            shapes.push((Control::Power, [power_x, y + 1.0, 40.0, 40.0, 20.0]));
            y += HEAD_H + GAP;

            if look.power_menu {
                let label = Style::new(Face::Body, 14.0, INK);
                let w = (inner - 3.0 * 8.0) / 4.0;
                for (i, control) in POWER_MENU.into_iter().enumerate() {
                    let x = x0 + i as f32 * (w + 8.0);
                    p.fill(x, y, w, MENU_H, 10.0, 0xffffff10);
                    let label_w = text::width(control.label(), &label);
                    p.text(
                        control.label(),
                        x + (w - label_w) / 2.0,
                        y + MENU_H / 2.0,
                        &label,
                    );
                    shapes.push((control, [x, y, w, MENU_H, 10.0]));
                }
                y += MENU_H + GAP;
            }

            // The tiles, two by three: amber when on.
            let tile_w = (inner - TILE_GAP) / 2.0;
            for (i, control) in TILES.into_iter().enumerate() {
                let x = x0 + (i % 2) as f32 * (tile_w + TILE_GAP);
                let top = y + (i / 2) as f32 * (TILE_H + TILE_GAP);
                let (on, detail) = tile(control, facts);
                let fill = if on { panel::AMBER } else { 0xffffff0d };
                p.fill(x, top, tile_w, TILE_H, 13.0, fill);
                let ink = if on { DARK } else { INK };
                let icon = match control {
                    Control::WiFi if !status.wifi => icons::WIFI_OFF,
                    Control::WiFi => icons::WIFI,
                    Control::Bluetooth => icons::BLUETOOTH,
                    Control::DoNotDisturb => icons::DND,
                    Control::NightLight => icons::MOON,
                    Control::ReducedMotion => icons::RUN,
                    _ => icons::GAUGE,
                };
                p.icon(icon, x + 14.0, top + (TILE_H - 22.0) / 2.0, 22.0, Some(ink));
                let page = control.page();
                let room = tile_w - 48.0 - 10.0 - if page.is_some() { CHEVRON_W } else { 0.0 };
                let title = Style::new(Face::BodyBold, 15.0, ink);
                p.text(
                    &text::ellipsize(control.label(), &title, room),
                    x + 48.0,
                    top + 22.0,
                    &title,
                );
                let detail_ink = if on { 0x5a3f10ff } else { 0x9aa3b2ff };
                let detail_style = Style::new(Face::Body, 12.5, detail_ink);
                p.text(
                    &text::ellipsize(&detail, &detail_style, room),
                    x + 48.0,
                    top + 40.0,
                    &detail_style,
                );
                // The chevron's zone, listed before its tile so a click there finds it first.
                if let Some(page) = page {
                    let zone = x + tile_w - CHEVRON_W;
                    p.fill(zone, top + 14.0, 1.0, TILE_H - 28.0, 0.0, 0xffffff14);
                    p.icon(
                        icons::CHEVRON,
                        zone + (CHEVRON_W - 16.0) / 2.0,
                        top + (TILE_H - 16.0) / 2.0,
                        16.0,
                        Some(detail_ink),
                    );
                    shapes.push((page, [zone, top, CHEVRON_W, TILE_H, 13.0]));
                }
                shapes.push((control, [x, top, tile_w, TILE_H, 13.0]));
            }
            y += 3.0 * TILE_H + 2.0 * TILE_GAP + GAP;

            // The sliders: an icon, the track with its knob, and the level.
            for &(control, icon, level) in &sliders {
                let centre = y + SLIDER_H / 2.0;
                p.icon(icon, x0 + 6.0, centre - 9.0, 18.0, Some(INK));
                let track_x = x0 + 38.0;
                let track_w = x0 + inner - 6.0 - 46.0 - 14.0 - track_x;
                let filled = track_w * level.min(100) as f32 / 100.0;
                p.fill(track_x, centre - 3.0, track_w, 6.0, 3.0, 0x3a3f4bff);
                let muted = control == Control::Volume && status.muted;
                let bar_ink = if muted { 0x8e97a8ff } else { panel::AMBER };
                p.fill(track_x, centre - 3.0, filled, 6.0, 3.0, bar_ink);
                let knob = track_x + filled;
                p.shadow(
                    knob - 8.0,
                    centre - 8.0,
                    16.0,
                    16.0,
                    8.0,
                    1.0,
                    3.0,
                    0x00000088,
                );
                p.fill(knob - 8.0, centre - 8.0, 16.0, 16.0, 8.0, 0xffffffff);
                let value = if muted {
                    "Muted".to_string()
                } else {
                    format!("{level}%")
                };
                let value_style = Style::new(Face::Mono, 13.0, 0xaab2c0ff);
                let value_w = text::width(&value, &value_style);
                p.text(&value, x0 + inner - 6.0 - value_w, centre, &value_style);
                targets.push((control, on_screen(x0, y, 36.0, SLIDER_H)));
                let line = on_screen(track_x, y, track_w, SLIDER_H);
                tracks.push(Track {
                    control,
                    hit: on_screen(track_x - 10.0, y, track_w + 20.0, SLIDER_H),
                    left: line.loc.x,
                    width: line.size.w,
                });
                shapes.push((control, [x0, y, inner, SLIDER_H, 8.0]));
                y += SLIDER_H + GAP;
            }

            // The footer: the way into the Settings app, and the keys.
            p.fill(x0, y, inner, 1.0, 0.0, 0xffffff12);
            let centre = y + FOOT_H - 9.0;
            let link = Style::new(Face::Body, 14.0, 0xffd08aff);
            let link_w = p.text("All settings", x0, centre, &link);
            shapes.push((
                Control::AllSettings,
                [x0 - 6.0, centre - 13.0, link_w + 12.0, 26.0, 6.0],
            ));
            panel::key_hint(p, x0 + inner, centre, &["Super+A", "Esc"], "");

            for (control, [x, y, w, h, radius]) in &shapes {
                if *control == look.selected {
                    panel::focus_ring(p, *x, *y, *w, *h, *radius, look.facts.ring);
                }
                if !matches!(control, Control::Volume | Control::Brightness) {
                    targets.push((*control, on_screen(*x, *y, *w, *h)));
                }
            }
        })?;

        self.origin = on_screen(0.0, 0.0, 0.0, 0.0).loc;
        self.frame = on_screen(fx, fy, WIDTH, height);
        self.targets = targets;
        self.tracks = tracks;
        self.painted = Some(painted);
        Some(())
    }
}

/// `level` moved by `step` percentage points, within 0–100.
fn nudge(level: u8, step: i32) -> u8 {
    (level as i32 + step).clamp(0, 100) as u8
}

/// Whether a tile is on, and the line under its name.
fn tile(control: Control, facts: &Facts) -> (bool, String) {
    let status = &facts.status;
    let on_off = |on: bool, detail: &str| {
        (
            on,
            if on {
                detail.to_string()
            } else {
                "Off".to_string()
            },
        )
    };
    match control {
        Control::WiFi => (
            status.wifi,
            match (status.wifi, &status.network) {
                (false, _) => "Off".to_string(),
                (true, Some(network)) => network.clone(),
                (true, None) => "Not connected".to_string(),
            },
        ),
        Control::Bluetooth => on_off(
            status.bluetooth,
            status.bluetooth_device.as_deref().unwrap_or("On"),
        ),
        Control::DoNotDisturb => on_off(facts.do_not_disturb, "Pop-ups hidden"),
        Control::NightLight => on_off(facts.night_light, "Warmer colours"),
        Control::ReducedMotion => on_off(facts.reduced_motion, "On"),
        Control::PowerMode => match status.power_profile.as_deref() {
            Some("power-saver") => (true, "Saver".to_string()),
            Some("performance") => (true, "Performance".to_string()),
            Some(_) => (false, "Balanced".to_string()),
            None => (false, "Unavailable".to_string()),
        },
        _ => (false, String::new()),
    }
}

/// The header's main line, e.g. `78% · 3 h 10 min left`, beside the battery's icon.
fn battery_line(status: &Reading) -> String {
    match (status.battery, &status.battery_time) {
        (Some((percent, _)), Some(time)) => format!("{percent}% · {time}"),
        (Some((percent, true)), None) => format!("{percent}% · plugged in"),
        (Some((percent, false)), None) => format!("{percent}%"),
        (None, _) => "On mains power".to_string(),
    }
}

/// The user's full name from the password database, else their login name.
pub fn user_name() -> String {
    let login = std::env::var("USER").unwrap_or_default();
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| full_name(&passwd, &login))
        .unwrap_or(login)
}

/// The name in `login`'s comment field in `/etc/passwd`, before any comma.
fn full_name(passwd: &str, login: &str) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != login {
            return None;
        }
        let name = fields.nth(3)?.split(',').next()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

impl Slipstream {
    /// Super+A, or the bar's tray: opens quick settings, or closes them.
    pub fn toggle_quick_settings(&mut self) {
        if self.quick.is_open() {
            self.quick.close();
        } else {
            self.close_panels();
            let now = self.clock.tick();
            self.quick.open(now);
            // Its sliders show the volume and the brightness, so the keys' display would be a
            // second answer to the same question.
            self.osd.hide();
            // The network's name and Bluetooth are otherwise read every 10 s.
            status::refresh();
        }
    }

    pub fn quick_facts(&self) -> Facts {
        Facts {
            status: self.status.lock().unwrap().clone(),
            do_not_disturb: self.settings.notifications.do_not_disturb,
            night_light: self.settings.display.night_light,
            reduced_motion: self.quick.reduced_motion,
            user: self.user.clone(),
            ring: self.panel_ring(),
        }
    }

    pub fn quick_key(&mut self, key: Keysym, shift: bool) {
        let facts = self.quick_facts();
        let request = self.quick.key(key, shift, &facts);
        self.quick_request(request);
    }

    /// A click while quick settings is open, in logical pixels from the output's corner. Outside
    /// the panel it closes; on another of the bar's buttons, that button then does its own thing.
    pub fn quick_click(&mut self, x: f64, y: f64) {
        match self.quick.click(x, y) {
            Request::Close => {
                self.quick.close();
                let target = (!self.fullscreen_on_screen())
                    .then(|| self.focused_bar_target(x, y))
                    .flatten();
                if let Some(target) = target.filter(|target| *target != bar::Target::Tray) {
                    self.bar_clicked(target);
                }
            }
            request => self.quick_request(request),
        }
    }

    pub(crate) fn quick_request(&mut self, request: Request) {
        match request {
            Request::Nothing => {}
            Request::Close => self.quick.close(),
            Request::Volume(percent) => {
                let was_muted = {
                    let mut shown = self.status.lock().unwrap();
                    shown.volume = percent;
                    std::mem::replace(&mut shown.muted, false)
                };
                if was_muted {
                    status::change(status::set_mute(false));
                }
                status::change(status::set_volume(percent));
            }
            Request::Brightness(percent) => {
                if let Some(command) = launch::set_brightness(percent) {
                    self.status.lock().unwrap().brightness = Some(percent);
                    status::change(command);
                }
            }
            Request::Press(control) => self.quick_press(control),
        }
    }

    fn quick_press(&mut self, control: Control) {
        let reading = self.status.lock().unwrap().clone();
        match control {
            Control::Power | Control::Brightness => {}
            Control::Lock => {
                self.quick.close();
                tracing::info!("lock chosen in quick settings");
                self.lock_now();
            }
            // Sleep leaves everything running, so it goes straight to logind. Restart and Shut
            // down end the session, so they take the way out first, like logging out.
            Control::Suspend => {
                self.quick.close();
                tracing::info!(verb = "suspend", "chosen in quick settings");
                self.request_power(crate::power::Power::Sleep);
            }
            Control::Restart | Control::ShutDown => {
                let intent = if control == Control::Restart {
                    Intent::Restart
                } else {
                    Intent::ShutDown
                };
                tracing::info!(verb = intent.verb(), "chosen in quick settings");
                self.begin_exit(intent);
            }
            Control::LogOut => {
                tracing::info!("log out chosen in quick settings");
                self.begin_exit(Intent::LogOut);
            }
            // Each change shows at once; the reading catches up once it's made.
            Control::WiFi => {
                let on = !reading.wifi;
                self.status.lock().unwrap().wifi = on;
                status::change(status::set_wifi(on));
            }
            Control::Bluetooth => {
                let on = !reading.bluetooth;
                {
                    let mut shown = self.status.lock().unwrap();
                    shown.bluetooth = on;
                    shown.bluetooth_device = None;
                }
                status::change(status::set_bluetooth(on));
            }
            Control::PowerMode => match reading.power_profile.as_deref() {
                Some(current) => {
                    let next = status::next_power_profile(current);
                    self.status.lock().unwrap().power_profile = Some(next.to_string());
                    status::change(status::set_power_profile(next));
                }
                None => self.show_toast(
                    "Power mode",
                    "power-profiles-daemon isn’t running, so there are no power modes to switch \
                     between.",
                ),
            },
            Control::Volume => {
                self.status.lock().unwrap().muted = !reading.muted;
                status::change(status::set_mute(!reading.muted));
            }
            Control::DoNotDisturb => {
                let on = !self.settings.notifications.do_not_disturb;
                self.change_settings(|settings| settings.notifications.do_not_disturb = on);
            }
            Control::NightLight => {
                let on = !self.settings.display.night_light;
                self.change_settings(|settings| settings.display.night_light = on);
            }
            Control::ReducedMotion => {
                let on = !self.settings.motion.reduced;
                self.change_settings(|settings| settings.motion.reduced = on);
            }
            Control::AllSettings => {
                self.quick.close();
                self.launch_app(App::Settings);
            }
            Control::Battery => {
                self.quick.close();
                self.open_slipstream_settings("power");
            }
            Control::WiFiPage | Control::BluetoothPage => {
                self.quick.close();
                let page = if control == Control::WiFiPage {
                    launch::Page::Network
                } else {
                    launch::Page::Bluetooth
                };
                self.open_settings_page(page);
            }
        }
    }

    /// Changes the settings file as the Settings app would, and applies the change straight away
    /// rather than waiting for the file watch.
    pub fn change_settings(&mut self, change: impl FnOnce(&mut slipstream_config::Settings)) {
        if !self.owns_state {
            self.show_toast(
                "Setting not saved",
                "Another Slipstream session owns the settings file, so this one leaves it alone.",
            );
            return;
        }
        let path = slipstream_config::path();
        let saved = slipstream_config::read(&path).and_then(|mut file| {
            change(&mut file);
            slipstream_config::write(&path, &file)
        });
        if let Err(err) = saved {
            tracing::warn!(path = %path.display(), "couldn't change the settings file: {err}");
            self.show_toast(
                "Setting not saved",
                "The settings file couldn’t be read or written, so it was left as it is. The log \
                 says why.",
            );
            return;
        }
        if let Some(settings) = crate::settings::reload() {
            self.apply_settings(settings);
        }
    }
}

/// The level at `x` along a track starting at `left` and `width` long, as a percentage.
fn level_at(x: f64, left: f64, width: f64) -> u8 {
    (((x - left) / width.max(1.0)).clamp(0.0, 1.0) * 100.0).round() as u8
}

/// What setting a slider to `percent` asks for. Brightness never goes fully dark.
fn slider_request(control: Control, percent: u8) -> Request {
    match control {
        Control::Volume => Request::Volume(percent),
        _ => Request::Brightness(percent.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            status: Reading {
                volume: 62,
                brightness: Some(70),
                battery: Some((78, false)),
                battery_time: Some("3 h 10 min left".into()),
                power_profile: Some("balanced".into()),
                ..Reading::default()
            },
            user: "Alex Morgan".into(),
            ..Facts::default()
        }
    }

    #[test]
    fn arrows_move_between_rows_and_escape_backs_out() {
        let facts = facts();
        let mut quick = QuickSettings::default();
        quick.open(0.0);
        assert_eq!(quick.selected, Control::WiFi, "it opens on the first tile");
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(
            quick.selected,
            Control::WiFiPage,
            "→ reaches the tile's chevron"
        );
        quick.key(Keysym::Down, false, &facts);
        assert_eq!(
            quick.selected,
            Control::DoNotDisturb,
            "↓ stays in the column"
        );
        quick.key(Keysym::Up, false, &facts);
        assert_eq!(quick.selected, Control::WiFi, "↑ comes back to the tile");
        quick.key(Keysym::Right, false, &facts);
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(quick.selected, Control::Bluetooth);
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(quick.selected, Control::BluetoothPage);
        assert_eq!(
            quick.key(Keysym::Return, false, &facts),
            Request::Press(Control::BluetoothPage)
        );
        quick.key(Keysym::Down, false, &facts);
        assert_eq!(quick.selected, Control::NightLight);
        quick.key(Keysym::Down, false, &facts);
        quick.key(Keysym::Down, false, &facts);
        assert_eq!(quick.selected, Control::Volume, "a one-control row");
        assert_eq!(
            quick.key(Keysym::Right, false, &facts),
            Request::Volume(67),
            "→ turns the slider up"
        );
        quick.key(Keysym::Up, false, &facts);
        assert_eq!(quick.selected, Control::ReducedMotion);

        for _ in 0..4 {
            quick.key(Keysym::Up, false, &facts);
        }
        assert_eq!(
            quick.selected,
            Control::Battery,
            "the header's battery line"
        );
        assert_eq!(
            quick.key(Keysym::Return, false, &facts),
            Request::Press(Control::Battery)
        );
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(
            quick.key(Keysym::Return, false, &facts),
            Request::Press(Control::Lock)
        );
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(quick.selected, Control::Power);
        assert_eq!(quick.key(Keysym::Return, false, &facts), Request::Nothing);
        assert_eq!(quick.selected, Control::Suspend, "into the power menu");
        quick.key(Keysym::Right, false, &facts);
        assert_eq!(
            quick.key(Keysym::space, false, &facts),
            Request::Press(Control::Restart)
        );
        assert_eq!(quick.key(Keysym::Escape, false, &facts), Request::Nothing);
        assert_eq!(quick.selected, Control::Power, "Esc closes the menu first");
        assert_eq!(quick.key(Keysym::Escape, false, &facts), Request::Close);
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_control() {
        let facts = facts();
        let mut quick = QuickSettings::default();
        quick.open(0.0);
        quick.key(Keysym::End, false, &facts);
        assert_eq!(quick.selected, Control::AllSettings);
        quick.key(Keysym::Home, false, &facts);
        assert_eq!(quick.selected, Control::Battery);
    }

    #[test]
    fn tab_goes_round_every_control() {
        let facts = Facts {
            status: Reading {
                brightness: None,
                ..facts().status
            },
            ..facts()
        };
        let mut quick = QuickSettings::default();
        quick.open(0.0);
        quick.key(Keysym::Tab, true, &facts);
        assert_eq!(quick.selected, Control::Power);
        quick.key(Keysym::Tab, true, &facts);
        assert_eq!(quick.selected, Control::Lock, "lock comes before power");
        quick.key(Keysym::Tab, true, &facts);
        assert_eq!(
            quick.selected,
            Control::Battery,
            "the battery line comes first"
        );
        quick.key(Keysym::Tab, true, &facts);
        assert_eq!(
            quick.selected,
            Control::AllSettings,
            "back past the start wraps round"
        );
        quick.key(Keysym::Tab, true, &facts);
        assert_eq!(
            quick.selected,
            Control::Volume,
            "no brightness slider without a backlight"
        );
    }

    #[test]
    fn clicks_find_tiles_and_slider_levels_and_outside_closes() {
        let mut quick = QuickSettings::default();
        quick.open(0.0);
        let look = Look {
            facts: facts(),
            selected: quick.selected,
            power_menu: false,
            width: 1536,
            scale: 1.25,
        };
        quick.repaint(&look).unwrap();
        let right = quick.frame.loc.x + quick.frame.size.w;
        assert!(
            (right - 1528.0).abs() < 0.5,
            "10 mockup pixels from the edge"
        );

        let wifi = quick
            .targets
            .iter()
            .find(|(control, _)| *control == Control::WiFi)
            .unwrap()
            .1;
        let (x, y) = (
            wifi.loc.x + wifi.size.w / 2.0,
            wifi.loc.y + wifi.size.h / 2.0,
        );
        assert_eq!(quick.click(x, y), Request::Press(Control::WiFi));
        let right_edge = wifi.loc.x + wifi.size.w - 5.0;
        assert_eq!(
            quick.click(right_edge, y),
            Request::Press(Control::WiFiPage),
            "the chevron's zone opens the settings page"
        );

        let volume = *quick
            .tracks
            .iter()
            .find(|track| track.control == Control::Volume)
            .unwrap();
        let y = volume.hit.loc.y + volume.hit.size.h / 2.0;
        assert_eq!(
            quick.click(volume.left + volume.width / 4.0, y),
            Request::Volume(25)
        );
        assert_eq!(quick.click(volume.left - 5.0, y), Request::Volume(0));
        assert_eq!(quick.click(20.0, 500.0), Request::Close);
    }

    #[test]
    fn the_name_comes_from_the_password_database() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      alex:x:1000:1000:Alex Morgan,,,:/home/alex:/bin/bash\n\
                      sam:x:1001:1001::/home/sam:/bin/sh";
        assert_eq!(full_name(passwd, "alex"), Some("Alex Morgan".into()));
        assert_eq!(full_name(passwd, "sam"), None);
        assert_eq!(full_name(passwd, "nobody"), None);
    }

    #[test]
    fn a_held_slider_follows_the_pointer_and_only_asks_when_the_level_changes() {
        assert_eq!(level_at(50.0, 0.0, 200.0), 25);
        assert_eq!(level_at(-40.0, 0.0, 200.0), 0, "past the left end");
        assert_eq!(level_at(400.0, 0.0, 200.0), 100, "past the right end");
        assert!(matches!(
            slider_request(Control::Brightness, 0),
            Request::Brightness(1)
        ));
    }
}
