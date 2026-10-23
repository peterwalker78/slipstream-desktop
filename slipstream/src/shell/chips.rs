//! The notification centre's Background Apps: a chip for each app running with no window up,
//! whether its windows have all closed or it only ever kept an icon in the tray. Each is a quiet
//! rounded tile with the app's icon over its name, as many to a row as fit.
//!
//! A chip that needs attention (a tray item whose status says so) carries an amber dot, and an
//! amber ring breathes round it; with reduced motion the ring holds still.

use resvg::tiny_skia::Pixmap;

use crate::{
    paint::Painter,
    panel,
    text::{self, Face, Style},
};

// Sizes in design pixels.
/// The icon's side: chip icons are loaded at this size.
pub const ICON: f32 = 22.0;
const ICON_RADIUS: f32 = 5.0;
const PAD_X: f32 = 14.0;
const PAD_TOP: f32 = 14.0;
const PAD_BOTTOM: f32 = 12.0;
const NAME_GAP: f32 = 5.0;
const NAME_H: f32 = 13.0;
const NAME_PX: f32 = 11.0;
/// The widest a name runs before it is cut short.
const NAME_MAX: f32 = 112.0;
const GAP: f32 = 8.0;
pub const HEIGHT: f32 = PAD_TOP + ICON + NAME_GAP + NAME_H + PAD_BOTTOM;
/// The most rows of chips; any more apps than that are left out.
const ROWS: usize = 3;
/// One breath of the attention ring, in seconds.
const BREATH: f64 = 1.8;

/// One app in the row.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub name: String,
    pub icon: Option<Pixmap>,
    /// The windowless Wayland client's app ID, when it is one.
    pub app_id: Option<String>,
    /// The tray item's key, when it has one.
    pub item: Option<String>,
    /// Its tray item says something wants attention.
    pub attention: bool,
    /// A left click opens its menu rather than activating it.
    pub item_is_menu: bool,
    /// It has a menu for Slipstream to draw.
    pub menu: bool,
}

impl Chip {
    pub fn id(&self) -> ChipId {
        ChipId {
            app_id: self.app_id.clone(),
            item: self.item.clone(),
        }
    }
}

/// What tells a chip apart from the others while the row changes around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipId {
    pub app_id: Option<String>,
    pub item: Option<String>,
}

/// What a press on a chip asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// A left click, Enter or Space: the app comes forward.
    Primary,
    /// A middle click: the tray item's secondary action.
    Secondary,
    /// A right click, the Menu key or Shift+F10: the tray item's menu.
    Menu { keyboard: bool },
}

/// What a press on a chip does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Draw the tray item's menu.
    Menu {
        item: String,
        keyboard: bool,
    },
    /// The tray item has no menu to draw: the app is asked to show one of its own.
    AppMenu(String),
    /// Ask the tray item's app to come forward itself.
    Activate(String),
    /// The tray item's second action.
    Secondary(String),
    /// Bring a windowless app's window back, or start it from its desktop entry.
    Launch(String),
    Nothing,
}

/// What `press` on `chip` does. An app with a tray item is always asked through it, so one that
/// only hid its window is never started a second time; its desktop entry is for an app that
/// has nothing else to be asked through.
pub fn act(chip: &Chip, press: Press) -> Act {
    let menu = |item: String, keyboard| {
        if chip.menu {
            Act::Menu { item, keyboard }
        } else {
            Act::AppMenu(item)
        }
    };
    match (press, chip.item.clone(), chip.app_id.clone()) {
        (Press::Menu { keyboard }, Some(item), _) => menu(item, keyboard),
        (Press::Primary, Some(item), _) if chip.item_is_menu => menu(item, false),
        (Press::Primary, Some(item), _) => Act::Activate(item),
        (Press::Primary, None, Some(app_id)) => Act::Launch(app_id),
        (Press::Secondary, Some(item), _) => Act::Secondary(item),
        _ => Act::Nothing,
    }
}

/// Each chip's box (x, y, w, h), as many as fit in `ROWS` rows `room` wide from (`x`, `y`), and
/// the height they take.
pub fn lay_out(chips: &[Chip], x: f32, y: f32, room: f32) -> (Vec<[f32; 4]>, f32) {
    let style = name_style();
    let mut boxes = Vec::new();
    let (mut cx, mut row) = (x, 0);
    for chip in chips {
        let name_w = text::width(&chip.name, &style).min(NAME_MAX);
        let w = (ICON.max(name_w) + 2.0 * PAD_X).min(room);
        if cx > x && cx + w > x + room + 0.5 {
            row += 1;
            cx = x;
        }
        if row >= ROWS {
            break;
        }
        boxes.push([cx, y + row as f32 * (HEIGHT + GAP), w, HEIGHT]);
        cx += w + GAP;
    }
    let rows = boxes.last().map_or(0, |_| row.min(ROWS - 1) + 1);
    let height = rows as f32 * HEIGHT + rows.saturating_sub(1) as f32 * GAP;
    (boxes, height)
}

fn name_style() -> Style {
    Style::new(Face::Body, NAME_PX, panel::HINT)
}

/// How the row is drawn: which chip the keyboard is on, which one's menu is open, and the one
/// launching with how far its pulse has got (0 to 1).
pub struct Paint<'a> {
    pub selected: Option<usize>,
    pub menu: Option<usize>,
    pub launching: Option<(usize, f32)>,
    pub ring: u32,
    /// The dot on a chip that wants attention: the secondary colour.
    pub attention: u32,
    pub reduced_motion: bool,
    pub boxes: &'a [[f32; 4]],
}

/// The chips in their boxes.
pub fn paint(p: &mut Painter, chips: &[Chip], how: &Paint) {
    let style = name_style();
    for (i, (chip, &[x, y, w, h])) in chips.iter().zip(how.boxes).enumerate() {
        match how.launching {
            // The chip pressed brightens and glows in the focus colour as it launches.
            Some((at, t)) if at == i => {
                let pulse = (t * std::f32::consts::PI).sin();
                let fill = 0x12 + (pulse * (0x26 - 0x12) as f32) as u32;
                p.fill(x, y, w, h, panel::ROW_RADIUS, 0xffffff00 | fill);
                p.border(x, y, w, h, panel::ROW_RADIUS, 1.0, panel::EDGE);
                if !how.reduced_motion {
                    let ring = (how.ring & 0xffffff00) | (pulse * 0xcc as f32) as u32;
                    p.border(
                        x - 3.0,
                        y - 3.0,
                        w + 6.0,
                        h + 6.0,
                        panel::ROW_RADIUS + 3.0,
                        2.0,
                        ring,
                    );
                }
            }
            _ => {
                let lit = how.menu == Some(i);
                panel::quiet_button(p, x, y, w, h, panel::ROW_RADIUS, lit);
            }
        }
        if how.selected == Some(i) {
            panel::focus_ring(p, x, y, w, h, panel::ROW_RADIUS, how.ring);
        }
        let icon_x = x + (w - ICON) / 2.0;
        let icon_y = y + PAD_TOP;
        match &chip.icon {
            Some(icon) => p.image(icon, icon_x, icon_y),
            None => panel::app_placeholder(p, &chip.name, icon_x, icon_y, ICON, ICON_RADIUS, 12.0),
        }
        if chip.attention {
            // A dot on the icon's corner, ringed in the chip's own fill so it reads on any icon.
            let (dx, dy) = (icon_x + ICON - 3.0, icon_y - 2.0);
            p.fill(dx - 1.5, dy - 1.5, 8.0, 8.0, 4.0, 0x1b1f29ff);
            p.fill(dx, dy, 5.0, 5.0, 2.5, how.attention);
        }
        let name = text::ellipsize(&chip.name, &style, w - 2.0 * PAD_X + 1.0);
        let name_w = text::width(&name, &style);
        let name_y = icon_y + ICON + NAME_GAP + NAME_H / 2.0;
        p.text(&name, x + (w - name_w) / 2.0, name_y, &style);
    }
}

/// The rings round the chips that need attention, in `attention`, painted apart from the panel so
/// they can breathe without repainting it. Returns whether there were any.
pub fn paint_glow(p: &mut Painter, chips: &[Chip], boxes: &[[f32; 4]], attention: u32) -> bool {
    let mut any = false;
    for (chip, &[x, y, w, h]) in chips.iter().zip(boxes) {
        if chip.attention {
            p.border(
                x - 3.0,
                y - 3.0,
                w + 6.0,
                h + 6.0,
                panel::ROW_RADIUS + 3.0,
                1.5,
                attention,
            );
            p.border(
                x - 1.0,
                y - 1.0,
                w + 2.0,
                h + 2.0,
                panel::ROW_RADIUS + 1.0,
                1.0,
                (attention & 0xffffff00) | 0x40,
            );
            any = true;
        }
    }
    any
}

/// How strongly the attention ring shows `now`: slowly in and out, or steady with reduced motion.
pub fn breath(now: f64, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 0.75;
    }
    let phase = (now / BREATH).fract() * std::f64::consts::TAU;
    (0.3 + 0.7 * (0.5 - 0.5 * phase.cos())) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(name: &str) -> Chip {
        Chip {
            name: name.into(),
            icon: None,
            app_id: None,
            item: None,
            attention: false,
            item_is_menu: false,
            menu: false,
        }
    }

    #[test]
    fn chips_wrap_onto_rows_and_stop_at_the_last_row() {
        let chips: Vec<Chip> = (0..40).map(|_| chip("Chat")).collect();
        let (boxes, height) = lay_out(&chips, 0.0, 0.0, 464.0);
        assert!(boxes.len() < chips.len(), "no more than fit in the rows");
        let rows = boxes.iter().map(|b| b[1]).fold(Vec::new(), |mut rows, y| {
            if !rows.contains(&y) {
                rows.push(y);
            }
            rows
        });
        assert_eq!(rows.len(), ROWS);
        assert_eq!(height, ROWS as f32 * HEIGHT + (ROWS - 1) as f32 * GAP);
        assert!(boxes.iter().all(|b| b[0] + b[2] <= 464.5));
        assert_eq!(lay_out(&[], 0.0, 0.0, 464.0), (Vec::new(), 0.0));
    }

    #[test]
    fn a_long_name_doesnt_make_a_wide_chip() {
        let (boxes, _) = lay_out(
            &[chip(&"Synchronisation Client ".repeat(4))],
            0.0,
            0.0,
            464.0,
        );
        assert!(boxes[0][2] <= NAME_MAX + 2.0 * PAD_X);
    }

    #[test]
    fn a_hidden_app_with_a_tray_icon_is_asked_to_show_itself_not_started_again() {
        let hidden = Chip {
            app_id: Some("org.example.Chat".into()),
            item: Some(":1.7/StatusNotifierItem".into()),
            menu: true,
            ..chip("Chat")
        };
        assert_eq!(
            act(&hidden, Press::Primary),
            Act::Activate(":1.7/StatusNotifierItem".into())
        );
        let closed = Chip {
            app_id: Some("org.example.Editor".into()),
            ..chip("Editor")
        };
        assert_eq!(
            act(&closed, Press::Primary),
            Act::Launch("org.example.Editor".into())
        );
    }

    #[test]
    fn a_right_click_shows_the_items_menu_or_asks_the_app_for_its_own() {
        let item = Some(":1.7/StatusNotifierItem".to_string());
        let with_menu = Chip {
            item: item.clone(),
            menu: true,
            ..chip("Sync")
        };
        let right = Press::Menu { keyboard: false };
        assert_eq!(
            act(&with_menu, right),
            Act::Menu {
                item: ":1.7/StatusNotifierItem".into(),
                keyboard: false
            }
        );
        let without = Chip {
            item: item.clone(),
            ..chip("Sync")
        };
        assert_eq!(
            act(&without, right),
            Act::AppMenu(":1.7/StatusNotifierItem".into())
        );
        let menu_only = Chip {
            item_is_menu: true,
            ..with_menu
        };
        assert!(matches!(act(&menu_only, Press::Primary), Act::Menu { .. }));
        assert_eq!(act(&chip("Editor"), right), Act::Nothing);
        assert_eq!(
            act(&without, Press::Secondary),
            Act::Secondary(":1.7/StatusNotifierItem".into())
        );
    }

    #[test]
    fn the_attention_ring_breathes_unless_motion_is_reduced() {
        assert!(breath(0.0, false) < breath(BREATH / 2.0, false));
        assert_eq!(breath(0.0, true), breath(BREATH / 2.0, true));
    }
}
