//! The system bar along the top: the apps button, workspaces 1–5, a mode's label and the focused
//! window's title on the left; the time and date in the middle; the tray and the notifications bell
//! on the right.
//!
//! It's laid out in the mockup's pixels, painted into a pixmap at the screen's own resolution so
//! text stays sharp at 1.25×, and repainted only when what it shows changes.

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
    status::Reading,
    text::{self, Face, Style},
};

/// Height in logical pixels: the mockup's 40 px at the laptop's 1.25×. Windows tile below it.
pub const HEIGHT: i32 = 32;
/// The same height in the mockup's pixels, which every size below is given in.
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
    /// One per workspace: its name, or its number.
    pub labels: Vec<String>,
    pub title: String,
    /// A mode's label, such as `BULLET TIME`, and the colour of its chip, 0xRRGGBBAA.
    pub mode: Option<(&'static str, u32)>,
    pub status: Reading,
    /// Do not disturb is on, so the bell shows its sign instead.
    pub do_not_disturb: bool,
    /// Notifications since the notification centre was last opened, on the bell's badge.
    pub unread: usize,
    /// Something on the desktop is being captured: the red dot shows, and a click on it stops it.
    pub sharing: bool,
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
    /// Beside the workspaces: bullet time, as Windows' Task View button.
    Overview,
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
            let (pixmap, targets) = paint(&key.0, width, scale)?;
            let size = Size::from((pixmap.width() as i32, pixmap.height() as i32));
            self.buffer = Some((paint::buffer(&pixmap), size));
            self.targets = targets;
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

    /// What's at a point, in logical pixels from the screen's top-left corner.
    pub fn target_at(&self, x: f64, y: f64) -> Option<Target> {
        self.targets
            .iter()
            .find(|(_, area)| area.contains((x, y)))
            .map(|(target, _)| *target)
    }
}

const INK: u32 = 0xdfe4ecff;
/// The 1 px ring inside the button of the workspace bullet time started on.
const HOME_RING: u32 = 0x33ccff99;
const AMBER: u32 = 0xffb547ff;
/// The dot that says the screen is being shared. Red, and nothing else on the bar is.
const LIVE: u32 = 0xff5a5aff;

/// Dark ink on a light chip, light ink on a dark one.
fn ink_on(rgba: u32) -> u32 {
    let [r, g, b, _] = rgba.to_be_bytes().map(|c| c as f32 / 255.0);
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.45 {
        0x1a1206ff
    } else {
        0xf2f4f8ff
    }
}

/// Paints the bar for a screen `width` logical pixels wide, and says where its buttons are.
fn paint(content: &Content, width: i32, scale: f64) -> Option<(Pixmap, Targets)> {
    let device_w = (width as f64 * scale).round() as u32;
    let device_h = (HEIGHT as f64 * scale).round() as u32;
    let f = device_h as f32 / TALL;
    let mut p = Painter::new(device_w, device_h, f)?;
    let wide = device_w as f32 / f;
    // Mockup pixels to logical ones, for the click targets.
    let unit = f as f64 / scale;
    let mut targets = Vec::new();
    // Each button takes clicks the bar's whole height, up to the screen's top edge, so a pointer
    // thrown against the edge still lands on what's under it.
    let mut target = |target, x: f32, w: f32| {
        targets.push((
            target,
            Rectangle::new(
                (x as f64 * unit, 0.0).into(),
                (w as f64 * unit, HEIGHT as f64).into(),
            ),
        ));
    };

    p.fill(0.0, 0.0, wide, TALL, 0.0, 0x0a0c11e6);
    p.fill(0.0, TALL - 1.0, wide, 1.0, 0.0, 0xffffff10);

    // Left: the apps button, then the workspaces.
    let mut x = 8.0;
    p.icon(icons::LOGO, x + 6.5, 9.5, 21.0, None);
    // The buttons in the corners reach out to the screen's side edges as well.
    target(Target::Apps, 0.0, x + 34.0);
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
            0x7fe3ffff
        } else if content.occupied.get(index).copied().unwrap_or(false) {
            0xcdd6f4ff
        } else {
            0x7f8aa3ff
        };
        let style = Style::new(Face::Mono, 15.0, color);
        let label_w = text::width(&label, &style);
        let w = (label_w + 16.0).max(30.0);
        if on {
            p.fill(x, 7.0, w, 26.0, 6.0, 0x33ccff26);
            p.inset_bottom(x, 7.0, w, 26.0, 6.0, 0x33ccffff);
        }
        if content.home == Some(index) {
            p.border(x, 7.0, w, 26.0, 6.0, 1.0, HOME_RING);
        }
        p.text(&label, x + (w - label_w) / 2.0, 20.0, &style);
        target(Target::Workspace(index), x, w);
        x += w + 6.0;
    }
    x += OVERVIEW_GAP;
    p.icon(icons::OVERVIEW, x + 4.0, 9.0, 22.0, Some(0x9aa4b6ff));
    target(Target::Overview, x, OVERVIEW_W);
    x += OVERVIEW_W + 8.0;

    if let Some((mode, chip)) = content.mode {
        let style = Style {
            tracking: 0.08,
            ..Style::new(Face::MonoBold, 15.0, ink_on(chip))
        };
        let w = text::width(mode, &style) + 20.0;
        p.fill(x, 7.0, w, 26.0, 5.0, chip);
        p.text(mode, x + 10.0, 20.0, &style);
        x += w + 8.0;
    }

    // Centre: the time and date, sharing a baseline.
    let status = &content.status;
    let clock = Style {
        tabular: true,
        ..Style::new(Face::Body, 16.0, 0xeef1f6ff)
    };
    let date = Style::new(Face::Body, 14.0, 0x9aa4b6ff);
    let time_w = text::width(&status.time, &clock);
    let centre_w = 14.0 + time_w + 10.0 + text::width(&status.date, &date) + 14.0;
    let centre_x = ((wide - centre_w) / 2.0).round();
    let baseline = text::baseline(&clock, 20.0);
    p.text_on(&status.time, centre_x + 14.0, baseline, &clock);
    p.text_on(
        &status.date,
        centre_x + 14.0 + time_w + 10.0,
        baseline,
        &date,
    );
    target(Target::Clock, centre_x, centre_w);

    // The focused window's title fills what's left of the left column.
    let title = Style::new(Face::Body, 14.0, 0x98a2b4ff);
    let room = centre_x - 16.0 - x;
    if room > 24.0 {
        p.text(
            &text::ellipsize(&content.title, &title, room),
            x,
            20.0,
            &title,
        );
    }

    // Right, from the edge inwards: the bell, then the tray.
    let mut right = wide - 8.0 - 36.0;
    let bell = if content.do_not_disturb {
        icons::DND
    } else {
        icons::BELL
    };
    p.icon(bell, right + 9.0, 11.0, 18.0, Some(INK));
    target(Target::Bell, right, wide - right);
    // The unread badge, over the bell's top right corner.
    if content.unread > 0 {
        let count = if content.unread > 99 {
            "99+".to_string()
        } else {
            content.unread.to_string()
        };
        let style = Style::new(Face::MonoBold, 10.0, 0x1a1206ff);
        let count_w = text::width(&count, &style);
        let badge_w = (count_w + 8.0).max(16.0);
        let badge_x = right + 36.0 - 2.0 - badge_w;
        p.fill(badge_x, 6.0, badge_w, 16.0, 8.0, AMBER);
        p.text(&count, badge_x + (badge_w - count_w) / 2.0, 14.0, &style);
    }

    let battery = status.battery.map(|(percent, charging)| {
        let low = percent <= 15 && !charging;
        let ink = if low { AMBER } else { INK };
        (
            icons::battery(percent, charging),
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
    target(Target::Tray, right, tray_w);
    let mut item_x = right + 11.0;
    for (icon, ink) in &tray {
        p.icon(icon, item_x, 11.0, 18.0, Some(*ink));
        item_x += 18.0 + 11.0;
    }
    if let Some((_, percent, style)) = &battery {
        p.text(percent, item_x, 20.0, style);
    }

    // Left of the tray while anything is shared: a red dot and the word, on a faint red pill,
    // which stops every share when clicked.
    if content.sharing {
        let style = Style {
            tracking: 0.06,
            ..Style::new(Face::MonoBold, 12.0, LIVE)
        };
        let word = "SHARING";
        let w = 12.0 + 8.0 + 8.0 + text::width(word, &style) + 12.0;
        right -= 8.0 + w;
        p.fill(right, 8.0, w, 24.0, 12.0, 0xff5a5a26);
        p.border(right, 8.0, w, 24.0, 12.0, 1.0, 0xff5a5a80);
        p.fill(right + 12.0, 16.0, 8.0, 8.0, 4.0, LIVE);
        p.text(word, right + 12.0 + 8.0 + 8.0, 20.0, &style);
        target(Target::Sharing, right, w);
    }

    Some((p.pixmap, targets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        Content {
            active: 1,
            home: None,
            occupied: vec![true, true, false, false, false],
            labels: (1..=5).map(|n| n.to_string()).collect(),
            title: "Firefox".into(),
            mode: Some(("BULLET TIME", AMBER)),
            status: Reading {
                time: "17:24".into(),
                date: "Fri 11 Sep".into(),
                battery: Some((93, false)),
                bluetooth: true,
                ..Reading::default()
            },
            do_not_disturb: false,
            unread: 0,
            sharing: false,
        }
    }

    #[test]
    fn the_overview_button_fits_the_name_budget() {
        let (_, targets) = paint(&content(), 1536, 1.25).unwrap();
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
    fn the_red_dot_shows_only_while_sharing_and_can_be_clicked() {
        let (_, quiet) = paint(&content(), 1536, 1.25).unwrap();
        assert!(!quiet.iter().any(|(target, _)| *target == Target::Sharing));
        let sharing = Content {
            sharing: true,
            ..content()
        };
        let (_, targets) = paint(&sharing, 1536, 1.25).unwrap();
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
    fn names_show_while_they_fit_and_only_the_one_in_front_when_they_dont() {
        let wide_names = |labels: Vec<String>| {
            let content = Content {
                occupied: vec![false; labels.len()],
                labels,
                ..content()
            };
            let (_, targets) = paint(&content, 1536, 1.25).unwrap();
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
        let (pixmap, targets) = paint(&looking, 1536, scale).unwrap();
        let button = |index| {
            targets
                .iter()
                .find(|(target, _)| *target == Target::Workspace(index))
                .map(|(_, area)| *area)
                .unwrap()
        };
        let cyan = |[r, g, b]: [u8; 3]| b > 120 && g > 90 && r < 100;
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
        // The ring down the left edge of home's button, and not of any other.
        let edge = |pixmap: &Pixmap, index| {
            let area = button(index);
            pixel(pixmap, area.loc.x + 0.4, (top + bottom) / 2.0, scale)
        };
        assert!(cyan(edge(&pixmap, 0)));
        assert!(!cyan(edge(&pixmap, 2)));
        let (unringed, _) = paint(
            &Content {
                home: None,
                ..looking
            },
            1536,
            scale,
        )
        .unwrap();
        assert!(!cyan(edge(&unringed, 0)));
    }

    #[test]
    fn the_bar_is_painted_one_pixel_per_screen_pixel() {
        let (pixmap, _) = paint(&content(), 1536, 1.25).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (1920, 40));
        let (pixmap, _) = paint(&content(), 1280, 2.0).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (2560, 64));
    }

    #[test]
    fn clicks_find_the_workspaces_clock_and_bell() {
        let (_, targets) = paint(&content(), 1536, 1.25).unwrap();
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
