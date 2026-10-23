//! The system bar along the top: the apps button, the workspaces, a mode's label and the focused
//! window's title on the left; the time and date in the middle, with the plugin chip after them,
//! centred as one; the tray and the notifications bell on the right, and before them the slot of a
//! window docked to the bar.
//!
//! It's laid out in design pixels, painted into a pixmap at the screen's own resolution so text
//! stays sharp at fractional scales, and repainted only when what it shows changes.

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Buffer, Logical, Rectangle, Size},
};

use crate::{
    icons,
    paint::{self, Painter},
    panel::{self, ink_on},
    plugins::card,
    status::Reading,
    text::{self, Face, Style},
};

/// Height in logical pixels: 40 design pixels. Windows tile below it.
pub const HEIGHT: i32 = 32;
/// The same height in design pixels, which every size below is given in.
const TALL: f32 = 40.0;

/// Everything the bar shows. It's repainted when this changes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Content {
    /// The workspace highlighted (0-based): the one on screen, or in bullet time the one being
    /// looked at.
    pub active: usize,
    /// In bullet time, the workspace it started on while the view is elsewhere: ringed, so the way
    /// back stays in sight.
    pub home: Option<usize>,
    /// One per workspace: whether anything is on it.
    pub occupied: Vec<bool>,
    /// One per workspace: whether another screen is showing it. Those are underlined in grey, so
    /// each bar says what the other screens have.
    pub elsewhere: Vec<bool>,
    /// The keyboard is on this bar's screen. Its workspace is highlighted fully only then; on the
    /// other screens it's a fainter fill, and the title is dimmed.
    pub keyboard_here: bool,
    /// One per workspace: its name, or its number.
    pub labels: Vec<String>,
    pub title: String,
    /// A mode's label, such as `BULLET TIME`, and the colour of its chip, 0xRRGGBBAA.
    pub mode: Option<(&'static str, u32)>,
    /// The focused window's ring colour, 0xRRGGBBFF: the workspace in front and Awake's pill.
    pub ring: u32,
    /// Bullet time's colour, the secondary: the ring around the workspace it started on, and
    /// what wants attention.
    pub bullet: u32,

    pub status: Shown,
    /// Do not disturb is on, so the bell shows its sign instead.
    pub do_not_disturb: bool,
    /// Notifications since the notification centre was last opened, on the bell's badge.
    pub unread: usize,
    /// Something on the desktop is being captured: the red dot shows, and a click on it stops it.
    pub sharing: bool,
    /// Caps Lock is on: a chip beside the tray says so.
    pub caps_lock: bool,
    /// Awake is on: a chip beside the tray says so, and a click on it turns it off.
    pub awake: bool,
    /// The keyboard layout in use, such as `GB`, when there are others to switch to. A click on
    /// it switches to the next.
    pub layout: Option<String>,
    /// The plugin chip, when there are plugins.
    pub plugins: Option<crate::plugins::Chip>,
    /// The plugin panel is open from this bar.
    pub plugins_open: bool,
    /// Where the frame of a pane hanging from this bar meets it, when one does: the frame's
    /// left and right edges, in logical pixels from the screen's left. The bar has no lower edge
    /// between them, where it runs on down into the frame, and keeps room for the pane's slot.
    pub dock: Option<(i32, i32)>,
}

/// The part of the status reading the bar draws. The bar is repainted when its content changes,
/// so the volume, brightness, device names and the battery's time left, which only quick settings
/// shows, are left out: a change to one of them has nothing to redraw here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    pub time: String,
    pub date: String,
    pub battery: Option<(u8, bool)>,
    pub online: bool,
    pub bluetooth: bool,
    pub muted: bool,
}

impl Shown {
    pub fn of(reading: &Reading) -> Self {
        Self {
            time: reading.time.clone(),
            date: reading.date.clone(),
            battery: reading.battery,
            online: reading.online,
            bluetooth: reading.bluetooth,
            muted: reading.muted,
        }
    }
}

/// Where each button is, in logical pixels from the screen's top-left corner.
type Targets = Vec<(Target, Rectangle<f64, Logical>)>;

/// Something on the bar that can be clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Apps,
    Workspace(usize),
    Clock,
    Tray,
    Bell,
    /// The red dot while something is shared.
    Sharing,
    Awake,
    /// The keyboard layout's code beside the tray.
    Layout,
    /// Beside the workspaces: bullet time, as Windows' Task View button.
    Overview,
    /// The docked window's slot.
    Dock,
    /// The plugin chip beside the clock.
    Plugins,
}

/// The overview button's width, and the gap before it, in the bar's units.
const OVERVIEW_W: f32 = 30.0;
const OVERVIEW_GAP: f32 = 2.0;

/// Whether every workspace can show its name: `label_widths` are the names' text widths, and
/// the chips, their gaps and the overview button beside them have to fit a third of the bar.
fn names_fit(label_widths: &[f32], wide: f32) -> bool {
    let chips: f32 = label_widths
        .iter()
        .map(|width| (width + 16.0).max(30.0) + 6.0)
        .sum();
    chips + OVERVIEW_GAP + OVERVIEW_W <= wide / 3.0
}

#[derive(Default)]
pub struct Bar {
    /// What the buffer shows, and for what screen width and scale.
    shown: Option<(Content, i32, f64)>,
    buffer: Option<(MemoryRenderBuffer, Size<i32, Buffer>)>,
    targets: Targets,
}

impl Bar {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    /// The bar for a screen `width` logical pixels wide at `scale`.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        content: Content,
        width: i32,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let key = (content, width, scale);
        if self.shown.as_ref() != Some(&key) {
            let painted = paint(&key.0, width, scale)?;
            let size = Size::from((
                painted.pixmap.width() as i32,
                painted.pixmap.height() as i32,
            ));
            self.buffer = Some((paint::buffer(&painted.pixmap), size));
            self.targets = painted.targets;
            self.shown = Some(key);
        }
        let (buffer, size) = self.buffer.as_ref()?;
        // One buffer pixel per screen pixel, however fractional the scale.
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            buffer,
            Some(alpha),
            Some(Rectangle::from_size((size.w as f64, size.h as f64).into())),
            Some((width, HEIGHT).into()),
            Kind::Unspecified,
        )
        .ok()
    }

    /// Where the docked window's slot goes, in logical pixels from the screen's left: its left
    /// edge and its width. None when nothing is docked or the bar has no room.
    pub fn dock_slot(&self) -> Option<(f64, f64)> {
        self.targets
            .iter()
            .find(|(target, _)| *target == Target::Dock)
            .map(|(_, area)| (area.loc.x, area.size.w))
    }

    /// Where a button is, in logical pixels from the screen's top-left corner.
    pub fn area_of(&self, wanted: Target) -> Option<Rectangle<f64, Logical>> {
        self.targets
            .iter()
            .find(|(target, _)| *target == wanted)
            .map(|(_, area)| *area)
    }

    /// What's at a point, in logical pixels from the screen's top-left corner.
    pub fn target_at(&self, x: f64, y: f64) -> Option<Target> {
        self.targets
            .iter()
            .find(|(_, area)| area.contains((x, y)))
            .map(|(target, _)| *target)
    }
}

const INK: u32 = panel::PROSE;
/// The dot that says the screen is being shared. Red, and nothing else on the bar is.
const LIVE: u32 = 0xff5a5aff;
/// The alpha of the faint pills that say a state is on: Caps Lock, Awake, sharing.
const PILL: u32 = 0x24;

/// The bar as painted: the picture, and where its buttons are.
struct Painted {
    pixmap: Pixmap,
    targets: Targets,
}

/// Paints the bar for a screen `width` logical pixels wide, and says where its buttons are.
fn paint(content: &Content, width: i32, scale: f64) -> Option<Painted> {
    let device_w = (width as f64 * scale).round() as u32;
    let device_h = (HEIGHT as f64 * scale).round() as u32;
    let f = device_h as f32 / TALL;
    let mut bar = Strip {
        p: Painter::new(device_w, device_h, f)?,
        wide: device_w as f32 / f,
        // Design pixels to logical ones, for the click targets.
        unit: f as f64 / scale,
        targets: Vec::new(),
    };
    let wide = bar.wide;
    bar.p.fill(0.0, 0.0, wide, TALL, 0.0, panel::CHIP | 0xe6);
    // The line along the foot, broken where a docked pane's frame carries the bar on downwards.
    let span = content.dock.map(|(left, right)| {
        (
            (left as f64 / bar.unit) as f32,
            (right as f64 / bar.unit) as f32,
        )
    });
    match span {
        Some((left, right)) => {
            bar.p
                .fill(0.0, TALL - 1.0, left.max(0.0), 1.0, 0.0, panel::DIVIDER);
            let rest = (wide - right).max(0.0);
            bar.p
                .fill(right, TALL - 1.0, rest, 1.0, 0.0, panel::DIVIDER);
        }
        None => bar.p.fill(0.0, TALL - 1.0, wide, 1.0, 0.0, panel::DIVIDER),
    }
    let x = paint_left(&mut bar, content);
    let right = paint_right(&mut bar, content);
    let (centre_x, block_w) = paint_centre(&mut bar, content);
    paint_title(&mut bar, content, x, centre_x);

    // Leftmost of them all: room for the docked window's slot, in the same place whatever size
    // its pane is. The slot is alive, so it is drawn over the bar every frame (`slot.rs`), not
    // painted here. A bar too narrow to keep it clear of the clock goes without.
    if span.is_some() {
        let x = right - 8.0 - DOCK_SLOT;
        if x >= centre_x + block_w + 8.0 {
            bar.target(Target::Dock, x, DOCK_SLOT);
        }
    }
    Some(Painted {
        pixmap: bar.p.pixmap,
        targets: bar.targets,
    })
}

/// The bar as it is painted, in design pixels `wide`, and where its buttons are.
struct Strip {
    p: Painter,
    wide: f32,
    /// Logical pixels to a design pixel.
    unit: f64,
    targets: Targets,
}

impl Strip {
    /// A button from `x`, `w` wide. Each takes clicks the bar's whole height, up to the screen's
    /// top edge, so a pointer thrown against the edge still lands on what's under it.
    fn target(&mut self, target: Target, x: f32, w: f32) {
        self.targets.push((
            target,
            Rectangle::new(
                (x as f64 * self.unit, 0.0).into(),
                (w as f64 * self.unit, HEIGHT as f64).into(),
            ),
        ));
    }
}

/// Left: the apps button, the workspaces, the overview's and the mode's chip. Returns where the
/// window's title can start.
fn paint_left(bar: &mut Strip, content: &Content) -> f32 {
    let wide = bar.wide;
    let mut x = 8.0;
    bar.p.icon(icons::LOGO, x + 6.5, 9.5, 21.0, None);
    // The buttons in the corners reach out to the screen's side edges as well.
    bar.target(Target::Apps, 0.0, x + 34.0);
    x += 34.0 + 8.0;
    // Names take room the clock needs. When they would run past a third of the bar, only the
    // workspace in front shows its name and the rest go by their numbers.
    let widths: Vec<f32> = content
        .labels
        .iter()
        .map(|label| text::width(label, &Style::new(Face::Mono, 15.0, 0)))
        .collect();
    let names_fit = names_fit(&widths, wide);
    for index in 0..content.labels.len() {
        let on = index == content.active;
        let label = if names_fit || on {
            content.labels[index].clone()
        } else {
            (index + 1).to_string()
        };
        let color = if on {
            panel::lighter(content.ring, 1.0 / 3.0)
        } else if content.occupied.get(index).copied().unwrap_or(false) {
            panel::PROSE
        } else {
            panel::PLACEHOLDER
        };
        let style = Style::new(Face::Mono, 15.0, color);
        let label_w = text::width(&label, &style);
        let w = (label_w + 16.0).max(30.0);
        let ring = content.ring & 0xffffff00;
        if on && content.keyboard_here {
            bar.p.fill(x, 7.0, w, 26.0, 6.0, ring | 0x26);
            bar.p.inset_bottom(x, 7.0, w, 26.0, 6.0, ring | 0xff);
        } else if on {
            bar.p.fill(x, 7.0, w, 26.0, 6.0, ring | 0x14);
        } else if content.elsewhere.get(index).copied().unwrap_or(false) {
            bar.p
                .inset_bottom(x, 7.0, w, 26.0, 6.0, (panel::HINT & 0xffffff00) | 0xb0);
        }
        if content.home == Some(index) {
            let home = (content.bullet & 0xffffff00) | 0x99;
            bar.p.border(x, 7.0, w, 26.0, 6.0, 1.0, home);
        }
        bar.p.text(&label, x + (w - label_w) / 2.0, 20.0, &style);
        bar.target(Target::Workspace(index), x, w);
        x += w + 6.0;
    }
    x += OVERVIEW_GAP;
    bar.p
        .icon(icons::OVERVIEW, x + 4.0, 9.0, 22.0, Some(panel::TERTIARY));
    bar.target(Target::Overview, x, OVERVIEW_W);
    x += OVERVIEW_W + 8.0;

    if let Some((mode, chip)) = content.mode {
        let style = Style {
            tracking: 0.08,
            ..Style::new(Face::MonoBold, 15.0, ink_on(chip))
        };
        let w = text::width(mode, &style) + 20.0;
        bar.p.fill(x, 7.0, w, 26.0, 6.0, chip);
        bar.p.text(mode, x + 10.0, 20.0, &style);
        x += w + 8.0;
    }

    x
}

fn clock_styles() -> (Style, Style) {
    let clock = Style {
        tabular: true,
        ..Style::new(Face::Body, 16.0, panel::BRIGHT)
    };
    (clock, Style::new(Face::Body, 14.0, panel::TERTIARY))
}

/// The time and date's width, padding included.
fn clock_width(status: &Shown) -> f32 {
    let (clock, date) = clock_styles();
    14.0 + text::width(&status.time, &clock) + 10.0 + text::width(&status.date, &date) + 14.0
}

/// The chip's place on the bar, the same as the workspaces' buttons: 26 tall on the bar's middle.
const CHIP_Y: f32 = 7.0;
const CHIP_H: f32 = 26.0;
/// The gap between the date and the chip, and the chip's padding.
const CHIP_GAP: f32 = 4.0;
const CHIP_PAD: f32 = 10.0;
/// The chip's squares: two by two, one for each of the first four plugins, lit for each one on.
const SQUARE: f32 = 7.0;
const SQUARE_GAP: f32 = 2.0;
/// A square for a plugin that's off, and the chip's own fill and edge at rest.
const SQUARE_OFF: u32 = 0xffffff29;
const CHIP_FILL: u32 = 0xffffff0a;
const CHIP_EDGE: u32 = 0xffffff1c;

/// The headline on the chip: its gauges and its text, and their style, warming to `attention`
/// as a gauge runs out.
fn chip_glance(
    chip: &crate::plugins::Chip,
    attention: u32,
) -> Option<(&crate::plugins::Glance, Style)> {
    let glance = chip.glance.as_ref()?;
    let ink = glance
        .gauges
        .iter()
        .max()
        .map_or(INK, |percent| match percent {
            75.. => card::ink(*percent, attention),
            _ => INK,
        });
    let style = Style {
        tabular: true,
        ..Style::new(Face::Body, 14.0, ink)
    };
    Some((glance, style))
}

/// The chip's width in the bar's units: its squares, and its headline when it has one.
fn chip_width(chip: &crate::plugins::Chip) -> f32 {
    let squares = 2.0 * SQUARE + SQUARE_GAP;
    // The colour makes no difference to the width.
    let glance = chip_glance(chip, INK).map_or(0.0, |(glance, style)| {
        let gauges = if glance.gauges.is_empty() {
            0.0
        } else {
            GAUGE_W + 6.0
        };
        8.0 + 1.0 + 8.0 + gauges + text::width(&glance.text, &style)
    });
    CHIP_PAD + squares + glance + CHIP_PAD
}

/// Centre: the time and date, sharing a baseline, then the plugin chip, all centred together.
/// Returns where the block starts and how wide it is.
fn paint_centre(bar: &mut Strip, content: &Content) -> (f32, f32) {
    let (wide, status) = (bar.wide, &content.status);
    let (clock, date) = clock_styles();
    let time_w = text::width(&status.time, &clock);
    let centre_w = clock_width(status);
    let chip_w = content.plugins.as_ref().map_or(0.0, chip_width);
    let block_w = if chip_w > 0.0 {
        centre_w + CHIP_GAP + chip_w
    } else {
        centre_w
    };
    let centre_x = ((wide - block_w) / 2.0).round();
    let baseline = text::baseline(&clock, 20.0);
    bar.p
        .text_on(&status.time, centre_x + 14.0, baseline, &clock);
    bar.p.text_on(
        &status.date,
        centre_x + 14.0 + time_w + 10.0,
        baseline,
        &date,
    );
    bar.target(Target::Clock, centre_x, centre_w);
    if let Some(chip) = &content.plugins {
        paint_chip(bar, chip, content, centre_x + centre_w + CHIP_GAP, chip_w);
    }
    (centre_x, block_w)
}

/// The plugin chip from `x`, `w` wide, on the bar's middle: a square for each plugin, lit for each
/// one on, then the chosen plugin's headline. In the secondary colour while a plugin wants
/// looking at, the primary while its panel is open.
fn paint_chip(bar: &mut Strip, chip: &crate::plugins::Chip, content: &Content, x: f32, w: f32) {
    let (ring, attention) = (content.ring & 0xffffff00, content.bullet & 0xffffff00);
    let (fill, edge) = if content.plugins_open {
        (ring | 0x1f, ring | 0xff)
    } else if chip.attention {
        (attention | 0x1a, attention | 0xbf)
    } else {
        (CHIP_FILL, CHIP_EDGE)
    };
    bar.p.fill(x, CHIP_Y, w, CHIP_H, 6.0, fill);
    bar.p.border(x, CHIP_Y, w, CHIP_H, 6.0, 1.0, edge);
    let squares = 2.0 * SQUARE + SQUARE_GAP;
    let top = CHIP_Y + (CHIP_H - squares) / 2.0;
    let shown = chip.installed.min(4);
    let lit = chip.on.min(shown);
    for index in 0..4 {
        let (column, row) = ((index % 2) as f32, (index / 2) as f32);
        let colour = if index >= shown {
            (SQUARE_OFF & 0xffffff00) | 0x10
        } else if index < lit {
            // The last lit square carries the secondary colour, so the chip says which way to
            // look.
            if chip.attention && index + 1 == lit {
                content.bullet
            } else {
                content.ring
            }
        } else {
            SQUARE_OFF
        };
        bar.p.fill(
            x + CHIP_PAD + column * (SQUARE + SQUARE_GAP),
            top + row * (SQUARE + SQUARE_GAP),
            SQUARE,
            SQUARE,
            2.0,
            colour,
        );
    }
    if let Some((glance, style)) = chip_glance(chip, content.bullet) {
        let mut at = x + CHIP_PAD + squares + 8.0;
        bar.p.fill(at, 13.0, 1.0, 14.0, 0.0, panel::EDGE);
        at += 1.0 + 8.0;
        if !glance.gauges.is_empty() {
            let mut gauge = |y: f32, h: f32, percent: u8| {
                bar.p.fill(at, y, GAUGE_W, h, h / 2.0, 0xffffff26);
                let filled = GAUGE_W * percent as f32 / 100.0;
                if filled > 0.0 {
                    bar.p.fill(
                        at,
                        y,
                        filled.max(h),
                        h,
                        h / 2.0,
                        card::ink(percent, content.bullet),
                    );
                }
            };
            gauge(14.0, 6.0, glance.gauges[0]);
            if let Some(second) = glance.gauges.get(1) {
                gauge(23.0, 3.0, *second);
            }
            at += GAUGE_W + 6.0;
        }
        bar.p.text(&glance.text, at, 20.0, &style);
    }
    bar.target(Target::Plugins, x, w);
}

/// The title of this screen's window fills what's left of the left column, from `x` to the
/// clock, dimmed where the keyboard isn't.
fn paint_title(bar: &mut Strip, content: &Content, x: f32, centre_x: f32) {
    let title_ink = if content.keyboard_here {
        panel::TERTIARY
    } else {
        0x626b7cff
    };
    let title = Style::new(Face::Body, 14.0, title_ink);
    let room = centre_x - 16.0 - x;
    if room > 24.0 {
        bar.p.text(
            &text::ellipsize(&content.title, &title, room),
            x,
            20.0,
            &title,
        );
    }
}

/// Right, from the edge inwards: the bell, the tray, the keyboard layout, and the pills that say a
/// state is on. Returns where they end on the left.
fn paint_right(bar: &mut Strip, content: &Content) -> f32 {
    let (wide, status) = (bar.wide, &content.status);
    let mut right = wide - 8.0 - 36.0;
    let bell = if content.do_not_disturb {
        icons::DND
    } else {
        icons::BELL
    };
    bar.p.icon(bell, right + 9.0, 11.0, 18.0, Some(INK));
    bar.target(Target::Bell, right, wide - right);
    // The unread badge, over the bell's top right corner.
    if content.unread > 0 {
        let count = if content.unread > 99 {
            "99+".to_string()
        } else {
            content.unread.to_string()
        };
        let style = Style::new(Face::MonoBold, 10.0, ink_on(content.bullet));
        let count_w = text::width(&count, &style);
        let badge_w = (count_w + 8.0).max(16.0);
        let badge_x = right + 36.0 - 2.0 - badge_w;
        bar.p.fill(badge_x, 6.0, badge_w, 16.0, 8.0, content.bullet);
        bar.p
            .text(&count, badge_x + (badge_w - count_w) / 2.0, 14.0, &style);
    }

    let battery = status.battery.map(|(percent, charging)| {
        let ink = match (percent, charging) {
            (_, true) => content.ring,
            (..=15, false) => content.bullet,
            _ => INK,
        };
        (
            icons::battery(percent, charging.then_some(content.ring)),
            format!("{percent}%"),
            Style::new(Face::Body, 14.0, ink),
        )
    });
    let mut tray: Vec<(String, u32)> = vec![(
        if status.online {
            icons::WIFI
        } else {
            icons::WIFI_OFF
        }
        .to_string(),
        INK,
    )];
    if status.bluetooth {
        tray.push((icons::BLUETOOTH.to_string(), INK));
    }
    let volume = if status.muted {
        icons::VOLUME_MUTED
    } else {
        icons::VOLUME
    };
    tray.push((volume.to_string(), INK));
    let mut percent_w = 0.0;
    if let Some((icon, percent, style)) = &battery {
        tray.push((icon.clone(), u32::from_be_bytes(style.color)));
        percent_w = text::width(percent, style) + 11.0;
    }
    let tray_w = 22.0 + tray.len() as f32 * 18.0 + (tray.len() - 1) as f32 * 11.0 + percent_w;
    right -= 8.0 + tray_w;
    bar.target(Target::Tray, right, tray_w);
    let mut item_x = right + 11.0;
    for (icon, ink) in &tray {
        bar.p.icon(icon, item_x, 11.0, 18.0, Some(*ink));
        item_x += 18.0 + 11.0;
    }
    if let Some((_, percent, style)) = &battery {
        bar.p.text(percent, item_x, 20.0, style);
    }

    // Right beside the tray, while there are layouts to switch between: the one in use, as
    // Windows shows it beside its tray.
    if let Some(layout) = &content.layout {
        let style = Style {
            tracking: 0.06,
            ..Style::new(Face::MonoBold, 12.0, INK)
        };
        let w = 8.0 + text::width(layout, &style) + 8.0;
        right -= 4.0 + w;
        bar.p.text(layout, right + 8.0, 20.0, &style);
        bar.target(Target::Layout, right, w);
    }

    // Left of the tray while Caps Lock is on: the key's arrow and the word, on a faint amber pill,
    // the way the red one says something is shared.
    if content.caps_lock {
        right = pill(
            bar,
            right,
            "CAPS",
            content.bullet,
            Mark::Icon(icons::CAPS_LOCK),
        )
        .0;
    }
    // While Awake is on: an open eye and the word, on a faint pill in the ring's colour, which
    // turns it off when clicked.
    if content.awake {
        let awake = content.ring | 0xff;
        let (x, w) = pill(bar, right, "AWAKE", awake, Mark::Icon(icons::EYE));
        bar.target(Target::Awake, x, w);
        right = x;
    }
    // While anything is shared: a red dot and the word, on a faint red pill, which stops every
    // share when clicked.
    if content.sharing {
        let (x, w) = pill(bar, right, "SHARING", LIVE, Mark::Dot);
        bar.target(Target::Sharing, x, w);
        right = x;
    }
    right
}

/// What goes before a pill's word: an icon, or a dot.
enum Mark {
    Icon(&'static str),
    Dot,
}

/// A faint pill in `colour` saying a state is on, ending 8 short of `right`. Returns where it
/// starts and how wide it is.
fn pill(bar: &mut Strip, right: f32, word: &str, colour: u32, mark: Mark) -> (f32, f32) {
    let style = Style {
        tracking: 0.06,
        ..Style::new(Face::MonoBold, 12.0, colour)
    };
    let (lead, mark_w, gap, edge) = match mark {
        Mark::Icon(_) => (10.0, 14.0, 6.0, 0x70),
        Mark::Dot => (12.0, 8.0, 8.0, 0x80),
    };
    let w = lead + mark_w + gap + text::width(word, &style) + 12.0;
    let x = right - (8.0 + w);
    let p = &mut bar.p;
    p.fill(x, 8.0, w, 24.0, 12.0, (colour & 0xffffff00) | PILL);
    p.border(x, 8.0, w, 24.0, 12.0, 1.0, (colour & 0xffffff00) | edge);
    match mark {
        Mark::Icon(icon) => p.icon(icon, x + lead, 13.0, 14.0, Some(colour)),
        Mark::Dot => p.fill(x + lead, 16.0, 8.0, 8.0, 4.0, colour),
    }
    p.text(word, x + lead + mark_w + gap, 20.0, &style);
    (x, w)
}

/// How wide the docked window's slot is, in the bar's units.
const DOCK_SLOT: f32 = 132.0;

/// A gauge's width on the chip, in the bar's units.
const GAUGE_W: f32 = 18.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_plain(content: &Content, width: i32, scale: f64) -> Option<(Pixmap, Targets)> {
        paint(content, width, scale).map(|painted| (painted.pixmap, painted.targets))
    }

    fn content() -> Content {
        Content {
            active: 1,
            home: None,
            occupied: vec![true, true, false, false, false],
            elsewhere: vec![false; 5],
            keyboard_here: true,
            labels: (1..=5).map(|n| n.to_string()).collect(),
            title: "Firefox".into(),
            mode: Some(("BULLET TIME", panel::AMBER)),
            ring: 0x42d3ffff,
            bullet: panel::AMBER,
            status: Shown {
                time: "17:24".into(),
                date: "Fri 11 Sep".into(),
                battery: Some((93, false)),
                bluetooth: true,
                ..Shown::default()
            },
            do_not_disturb: false,
            unread: 0,
            sharing: false,
            caps_lock: false,
            awake: false,
            layout: None,
            plugins: None,
            plugins_open: false,
            dock: None,
        }
    }

    #[test]
    fn the_overview_button_fits_the_name_budget() {
        let (_, targets) = paint_plain(&content(), 1536, 1.25).unwrap();
        let area = |wanted: Target| {
            targets
                .iter()
                .find(|(target, _)| *target == wanted)
                .map(|(_, area)| *area)
                .unwrap()
        };
        let (last, overview) = (area(Target::Workspace(4)), area(Target::Overview));
        assert!(
            overview.loc.x >= last.loc.x + last.size.w,
            "after the workspaces"
        );
        assert!(overview.loc.x + overview.size.w < area(Target::Clock).loc.x);
        // Names that fit only without the button go by their numbers.
        let wide = 1200.0;
        let chips_only = (wide / 3.0 - 8.0) / 5.0 - 6.0 - 16.0;
        assert!(!names_fit(&[chips_only; 5], wide));
        assert!(names_fit(&[chips_only - 10.0; 5], wide));
    }

    #[test]
    fn the_awake_chip_shows_only_while_awake_and_sits_beside_caps_lock() {
        let (_, quiet) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert!(!quiet.iter().any(|(target, _)| *target == Target::Awake));
        let both = Content {
            awake: true,
            caps_lock: true,
            sharing: true,
            ..content()
        };
        let (_, targets) = paint_plain(&both, 1536, 1.25).unwrap();
        let area = |wanted: Target| {
            targets
                .iter()
                .find(|(target, _)| *target == wanted)
                .map(|(_, area)| *area)
                .unwrap()
        };
        let (awake, dot, tray) = (
            area(Target::Awake),
            area(Target::Sharing),
            area(Target::Tray),
        );
        assert!(awake.loc.x + awake.size.w <= tray.loc.x);
        assert!(
            dot.loc.x + dot.size.w <= awake.loc.x,
            "the chips don't overlap"
        );
    }

    #[test]
    fn the_layout_shows_beside_the_tray_only_when_there_are_others() {
        let (_, quiet) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert!(!quiet.iter().any(|(target, _)| *target == Target::Layout));
        let two = Content {
            layout: Some("GB".into()),
            awake: true,
            ..content()
        };
        let (_, targets) = paint_plain(&two, 1536, 1.25).unwrap();
        let area = |wanted: Target| {
            targets
                .iter()
                .find(|(target, _)| *target == wanted)
                .map(|(_, area)| *area)
                .unwrap()
        };
        let (layout, awake, tray) = (
            area(Target::Layout),
            area(Target::Awake),
            area(Target::Tray),
        );
        assert!(layout.loc.x + layout.size.w <= tray.loc.x);
        assert!(
            awake.loc.x + awake.size.w <= layout.loc.x,
            "{awake:?} {layout:?}"
        );
    }

    #[test]
    fn the_red_dot_shows_only_while_sharing_and_can_be_clicked() {
        let (_, quiet) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert!(!quiet.iter().any(|(target, _)| *target == Target::Sharing));
        let sharing = Content {
            sharing: true,
            ..content()
        };
        let (_, targets) = paint_plain(&sharing, 1536, 1.25).unwrap();
        let (_, dot) = targets
            .iter()
            .find(|(target, _)| *target == Target::Sharing)
            .expect("a red dot to click");
        let (_, tray) = targets
            .iter()
            .find(|(target, _)| *target == Target::Tray)
            .unwrap();
        assert!(
            dot.loc.x + dot.size.w <= tray.loc.x,
            "left of the tray, not over it"
        );
    }

    #[test]
    fn a_docked_window_has_a_slot_over_its_pane_and_the_bar_runs_on_into_the_frame() {
        let (plain, quiet) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert!(!quiet.iter().any(|(target, _)| *target == Target::Dock));
        let docked = |span| Content {
            dock: Some(span),
            ..content()
        };
        let area = |targets: &Targets, wanted: Target| {
            targets
                .iter()
                .find(|(target, _)| *target == wanted)
                .map(|(_, area)| *area)
                .unwrap()
        };
        // The slot stands just left of the tray, and stays there whatever size the pane is.
        let (pixmap, targets) = paint_plain(&docked((1000, 1536)), 1536, 1.25).unwrap();
        let (slot, tray) = (area(&targets, Target::Dock), area(&targets, Target::Tray));
        assert!(slot.loc.x + slot.size.w <= tray.loc.x, "{slot:?} {tray:?}");
        assert!(
            tray.loc.x - (slot.loc.x + slot.size.w) < 12.0,
            "{slot:?} {tray:?}"
        );
        for span in [(760, 1536), (1300, 1536), (1500, 1536)] {
            let (_, targets) = paint_plain(&docked(span), 1536, 1.25).unwrap();
            assert_eq!(area(&targets, Target::Dock), slot, "{span:?}");
        }
        // The line along the bar's foot stops where the frame takes over.
        let foot = |pixmap: &Pixmap, x: u32| pixmap.pixel(x, pixmap.height() - 1).unwrap();
        assert_ne!(foot(&pixmap, 400), foot(&pixmap, 1400));
        assert_eq!(foot(&pixmap, 400), foot(&plain, 400));
        assert_ne!(foot(&pixmap, 1400), foot(&plain, 1400));
    }

    #[test]
    fn names_show_while_they_fit_and_only_the_one_in_front_when_they_dont() {
        let wide_names = |labels: Vec<String>| {
            let content = Content {
                occupied: vec![false; labels.len()],
                labels,
                ..content()
            };
            let (_, targets) = paint_plain(&content, 1536, 1.25).unwrap();
            targets
                .iter()
                .filter(|(target, _)| matches!(target, Target::Workspace(_)))
                .map(|(_, area)| area.size.w)
                .collect::<Vec<_>>()
        };
        let short = wide_names(vec!["Mail".into(), "Web".into(), "3".into()]);
        assert!(short[0] > short[2], "a name is wider than a number");
        let long: Vec<String> = (0..12)
            .map(|n| format!("A long workspace name {n}"))
            .collect();
        let widths = wide_names(long);
        assert_eq!(widths.len(), 12);
        assert!(
            widths[1] > widths[0],
            "only the one in front keeps its name"
        );
    }

    /// The pixel at a point in logical pixels, as red, green and blue.
    fn pixel(pixmap: &Pixmap, x: f64, y: f64, scale: f64) -> [u8; 3] {
        let at = pixmap
            .pixel((x * scale) as u32, (y * scale) as u32)
            .unwrap()
            .demultiply();
        [at.red(), at.green(), at.blue()]
    }

    #[test]
    fn the_highlight_and_the_home_ring_mark_their_workspaces() {
        let scale = 1.25;
        let looking = Content {
            active: 3,
            home: Some(0),
            ..content()
        };
        let (pixmap, targets) = paint_plain(&looking, 1536, scale).unwrap();
        let button = |index| {
            targets
                .iter()
                .find(|(target, _)| *target == Target::Workspace(index))
                .map(|(_, area)| *area)
                .unwrap()
        };
        let cyan = |[r, g, b]: [u8; 3]| b > 120 && g > 90 && r < 100;
        let amber = |[r, g, b]: [u8; 3]| r > 120 && g > 80 && b < 100;
        // Clicks take the bar's whole height, but the buttons are drawn 7 to 33 of its 40 units
        // down: 5.6 to 26.4 logical pixels.
        let (top, bottom) = (5.6, 26.4);
        // The highlight's band along the bottom, on the workspace being looked at only.
        let band = |index| {
            let area = button(index);
            pixel(&pixmap, area.loc.x + area.size.w / 2.0, bottom - 0.8, scale)
        };
        assert!(cyan(band(3)));
        assert!(!cyan(band(1)));
        // The ring down the left edge of home's button, in bullet time's colour, and not of any
        // other.
        let edge = |pixmap: &Pixmap, index| {
            let area = button(index);
            pixel(pixmap, area.loc.x + 0.4, (top + bottom) / 2.0, scale)
        };
        assert!(amber(edge(&pixmap, 0)));
        assert!(!amber(edge(&pixmap, 2)));
        let (unringed, _) = paint_plain(
            &Content {
                home: None,
                ..looking.clone()
            },
            1536,
            scale,
        )
        .unwrap();
        assert!(!amber(edge(&unringed, 0)));

        // On a screen without the keyboard its own workspace has no band, and one another screen
        // shows is underlined in grey.
        let (other, _) = paint_plain(
            &Content {
                keyboard_here: false,
                elsewhere: vec![false, true, false, false, false],
                ..looking
            },
            1536,
            scale,
        )
        .unwrap();
        let band = |pixmap: &Pixmap, index| {
            let area = button(index);
            pixel(pixmap, area.loc.x + area.size.w / 2.0, bottom - 0.8, scale)
        };
        assert!(!cyan(band(&other, 3)));
        let [r, g, b] = band(&other, 1);
        assert!(
            r > 80 && g > 80 && b > 80 && !cyan([r, g, b]),
            "grey: {r} {g} {b}"
        );
        assert!(
            band(&pixmap, 1).iter().all(|channel| *channel < 80),
            "no underline while it's on no other screen"
        );
    }

    fn area(targets: &Targets, wanted: Target) -> Option<Rectangle<f64, Logical>> {
        targets
            .iter()
            .find(|(target, _)| *target == wanted)
            .map(|(_, area)| *area)
    }

    fn chip(on: usize, glance: Option<(&str, Vec<u8>)>) -> crate::plugins::Chip {
        crate::plugins::Chip {
            installed: 3,
            on,
            glance: glance.map(|(text, gauges)| crate::plugins::Glance {
                text: text.into(),
                gauges,
            }),
            attention: false,
        }
    }

    #[test]
    fn the_chip_follows_the_date_and_the_block_stays_centred() {
        let (_, plain) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert_eq!(area(&plain, Target::Plugins), None, "no plugins, no chip");
        let with = Content {
            plugins: Some(chip(2, Some(("81%", vec![62, 81])))),
            ..content()
        };
        let (_, targets) = paint_plain(&with, 1536, 1.25).unwrap();
        let (clock, chip) = (
            area(&targets, Target::Clock).unwrap(),
            area(&targets, Target::Plugins).unwrap(),
        );
        assert!(chip.loc.x > clock.loc.x + clock.size.w, "after the date");
        let left = clock.loc.x;
        let right = chip.loc.x + chip.size.w;
        assert!(((left + right) / 2.0 - 768.0).abs() < 1.0, "{left} {right}");
        assert!(clock.loc.x < area(&plain, Target::Clock).unwrap().loc.x);
    }

    /// The chip sits on the bar's middle, as the workspaces' buttons do: as much bar shows above
    /// its edge as below it.
    #[test]
    fn the_chip_is_centred_on_the_bar_from_top_to_bottom() {
        for scale in [1.0, 1.25, 2.0] {
            let open = Content {
                plugins: Some(chip(1, Some(("3", Vec::new())))),
                plugins_open: true,
                ..content()
            };
            let (pixmap, targets) = paint_plain(&open, 1536, scale).unwrap();
            let chip = area(&targets, Target::Plugins).unwrap();
            let x = ((chip.loc.x + chip.size.w / 2.0) * scale) as u32;
            let edge: Vec<u32> = (0..pixmap.height())
                .filter(|y| {
                    let pixel = pixmap.pixel(x, *y).unwrap().demultiply();
                    // The cyan edge, however softly a fractional scale draws it.
                    pixel.blue() >= 100 && u16::from(pixel.blue()) > 3 * u16::from(pixel.red())
                })
                .collect();
            let (top, bottom) = (edge[0], *edge.last().unwrap());
            let above = top;
            let below = pixmap.height() - 1 - bottom;
            assert!(
                above.abs_diff(below) <= 1 + (scale as u32),
                "scale {scale}: {above} above, {below} below"
            );
        }
    }

    #[test]
    fn the_chip_is_as_wide_as_its_headline_needs() {
        let squares = chip_width(&chip(1, None));
        let text = chip_width(&chip(1, Some(("18:42", Vec::new()))));
        let gauges = chip_width(&chip(1, Some(("18:42", vec![40]))));
        assert!(squares < text && text < gauges);
    }

    #[test]
    fn the_bar_is_painted_one_pixel_per_screen_pixel() {
        let (pixmap, _) = paint_plain(&content(), 1536, 1.25).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (1920, 40));
        let (pixmap, _) = paint_plain(&content(), 1280, 2.0).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (2560, 64));
    }

    #[test]
    fn clicks_find_the_workspaces_clock_and_bell() {
        let (_, targets) = paint_plain(&content(), 1536, 1.25).unwrap();
        let workspaces: Vec<_> = targets
            .iter()
            .filter(|(target, _)| matches!(target, Target::Workspace(_)))
            .map(|(_, area)| *area)
            .collect();
        assert_eq!(workspaces.len(), 5);
        assert!(
            workspaces
                .windows(2)
                .all(|pair| pair[0].loc.x + pair[0].size.w <= pair[1].loc.x),
            "left to right, without overlapping"
        );

        let bar = Bar {
            targets,
            ..Bar::default()
        };
        let third = workspaces[2];
        assert_eq!(
            bar.target_at(third.loc.x + 2.0, third.loc.y + 2.0),
            Some(Target::Workspace(2))
        );
        assert_eq!(bar.target_at(768.0, 16.0), Some(Target::Clock));
        assert_eq!(bar.target_at(1525.0, 16.0), Some(Target::Bell));
        assert_eq!(bar.target_at(768.0, 60.0), None, "below the bar");
        assert_eq!(
            bar.target_at(third.loc.x + 2.0, 0.0),
            Some(Target::Workspace(2)),
            "the top edge of the screen"
        );
        assert_eq!(bar.target_at(768.0, 0.0), Some(Target::Clock));
        assert_eq!(
            bar.target_at(0.0, 0.0),
            Some(Target::Apps),
            "the top left corner"
        );
        assert_eq!(
            bar.target_at(1535.9, 0.0),
            Some(Target::Bell),
            "the top right corner"
        );
    }
}
