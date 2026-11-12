//! Apps keeping the screen awake: whatever holds off the fade to the wallpaper, the lock and the
//! screens going off, other than Awake itself. The bar says so with Awake's own eye on a pill
//! named for the app, and the notification centre shows a tile under the calendar with the eye,
//! the apps' icons and what it means.

use resvg::tiny_skia::Pixmap;

use crate::{
    icons,
    paint::Painter,
    panel,
    text::{self, Face, Style},
};

/// One app keeping the screen awake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub name: String,
    /// The app ID or X11 class its icon is found by.
    pub id: Option<String>,
}

impl Holder {
    /// A hold from something that gave no name.
    pub fn unnamed() -> Self {
        Self {
            name: "An app".to_string(),
            id: None,
        }
    }
}

/// The most icons the centre's tile shows side by side.
const ICONS: usize = 3;
/// An icon's side on the tile, the same as a background app's.
pub const ICON: f32 = crate::chips::ICON;
const ICON_RADIUS: f32 = 5.0;
/// How far each icon tucks under the one before it.
const OVERLAP: f32 = 4.0;
const PAD: f32 = 14.0;
const BADGE: f32 = 30.0;
const EYE: f32 = 16.0;
/// The tile's height.
pub const HEIGHT: f32 = 58.0;
/// The faint fill and the edge of the eye's badge, as alphas over the ring's colour: the same as
/// the bar's pills.
const BADGE_FILL: u32 = 0x24;
const BADGE_EDGE: u32 = 0x70;
/// The tile's own colour where it lies on the panel's glass, for the cut-out round each icon
/// that tucks under another: its quiet fill over the glass, made solid.
const CUT_OUT: u32 = over(panel::QUIET, panel::GLASS);

/// `top` (0xRRGGBBAA) laid over `under`, as one solid colour.
const fn over(top: u32, under: u32) -> u32 {
    const fn channel(top: u32, under: u32, shift: u32) -> u32 {
        let (alpha, t, u) = (top & 0xff, (top >> shift) & 0xff, (under >> shift) & 0xff);
        ((t * alpha + u * (255 - alpha) + 127) / 255) << shift
    }
    channel(top, under, 24) | channel(top, under, 16) | channel(top, under, 8) | 0xff
}
/// The longest an app's name runs on the bar's pill before it is cut short.
const PILL_NAME: usize = 16;

/// Who is keeping the screen awake, and the rest of the sentence: "Godot is keeping the screen
/// awake", "Godot and Firefox are …", "Godot and 2 more are …".
pub fn sentence(holders: &[Holder]) -> (String, &'static str) {
    let who = match holders {
        [] => String::new(),
        [one] => one.name.clone(),
        [one, two] => format!("{} and {}", one.name, two.name),
        [one, rest @ ..] => format!("{} and {} more", one.name, rest.len()),
    };
    let rest = if holders.len() > 1 {
        " are keeping the screen awake"
    } else {
        " is keeping the screen awake"
    };
    (who, rest)
}

/// What the hold means for the person at the screen.
pub fn note(holders: usize) -> &'static str {
    if holders > 1 {
        "The screen won't fade or lock until they stop."
    } else {
        "The screen won't fade or lock until it stops."
    }
}

/// The word on the bar's pill: the first app's name in capitals, and how many more.
pub fn pill_word(holders: &[Holder]) -> String {
    let Some(first) = holders.first() else {
        return String::new();
    };
    // Cut in capitals, which can be longer than the name: ß is SS.
    let upper = first.name.to_uppercase();
    let mut name: String = upper.chars().take(PILL_NAME).collect();
    if upper.chars().count() > PILL_NAME {
        name = format!("{}…", name.trim_end());
    }
    match holders.len() {
        1 => name,
        n => format!("{name} +{}", n - 1),
    }
}

/// The centre's tile, `w` wide at (`x`, `y`): Awake's eye on a badge in the ring's colour, the
/// apps' icons overlapping beside it, and what they're doing over what it means.
pub fn paint(
    p: &mut Painter,
    x: f32,
    y: f32,
    w: f32,
    holders: &[Holder],
    icons: &[Option<Pixmap>],
    ring: u32,
) {
    panel::quiet_button(p, x, y, w, HEIGHT, panel::ROW_RADIUS, false);
    let ring = ring | 0xff;
    let badge_y = y + (HEIGHT - BADGE) / 2.0;
    let mut cx = x + PAD;
    p.fill(
        cx,
        badge_y,
        BADGE,
        BADGE,
        BADGE / 2.0,
        (ring & 0xffffff00) | BADGE_FILL,
    );
    p.border(
        cx,
        badge_y,
        BADGE,
        BADGE,
        BADGE / 2.0,
        1.0,
        (ring & 0xffffff00) | BADGE_EDGE,
    );
    let eye_at = (BADGE - EYE) / 2.0;
    p.icon(icons::EYE, cx + eye_at, badge_y + eye_at, EYE, Some(ring));
    cx += BADGE + 10.0;

    // Each icon sits on a cut-out in the panel's own glass, so the one it tucks under reads as
    // behind it.
    let icon_y = y + (HEIGHT - ICON) / 2.0;
    for (i, holder) in holders.iter().take(ICONS).enumerate() {
        if i > 0 {
            cx -= OVERLAP;
            p.fill(
                cx - 2.0,
                icon_y - 2.0,
                ICON + 4.0,
                ICON + 4.0,
                ICON_RADIUS + 2.0,
                CUT_OUT,
            );
        }
        match icons.get(i).and_then(Option::as_ref) {
            Some(icon) => p.image(icon, cx, icon_y),
            None => panel::app_placeholder(p, &holder.name, cx, icon_y, ICON, ICON_RADIUS, 12.0),
        }
        cx += ICON;
    }
    cx += 12.0;

    let room = x + w - PAD - cx;
    let (who, rest) = sentence(holders);
    let bold = Style::new(Face::BodyBold, 14.0, panel::INK);
    let plain = Style::new(Face::Body, 14.0, panel::SECONDARY);
    let rest_w = text::width(rest, &plain);
    let who = text::ellipsize(&who, &bold, (room - rest_w).max(room * 0.4));
    let line = y + HEIGHT / 2.0 - 9.0;
    let after = cx + p.text(&who, cx, line, &bold);
    let rest = text::ellipsize(rest, &plain, x + w - PAD - after);
    p.text(&rest, after, line, &plain);
    let quiet = Style::new(Face::Body, 12.0, panel::TERTIARY);
    let note = text::ellipsize(note(holders.len()), &quiet, room);
    p.text(&note, cx, y + HEIGHT / 2.0 + 10.0, &quiet);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holders(names: &[&str]) -> Vec<Holder> {
        names
            .iter()
            .map(|name| Holder {
                name: name.to_string(),
                id: None,
            })
            .collect()
    }

    #[test]
    fn the_sentence_names_who_is_keeping_the_screen_awake() {
        assert_eq!(
            sentence(&holders(&["Player"])),
            ("Player".into(), " is keeping the screen awake")
        );
        assert_eq!(
            sentence(&holders(&["Player", "Game"])),
            ("Player and Game".into(), " are keeping the screen awake")
        );
        assert_eq!(
            sentence(&holders(&["Player", "Game", "Editor"])).0,
            "Player and 2 more"
        );
        assert_eq!(note(1), "The screen won't fade or lock until it stops.");
        assert_eq!(note(2), "The screen won't fade or lock until they stop.");
    }

    #[test]
    fn the_cut_out_is_the_tiles_fill_on_the_glass() {
        assert_eq!(CUT_OUT, 0x21242aff);
        assert_eq!(over(0xffffffff, panel::GLASS), 0xffffffff);
        assert_eq!(over(0xffffff00, 0x102030ff), 0x102030ff);
    }

    #[test]
    fn the_pill_names_the_first_app_and_counts_the_rest() {
        assert_eq!(pill_word(&holders(&["Player"])), "PLAYER");
        assert_eq!(pill_word(&holders(&["Player", "Game"])), "PLAYER +1");
        assert_eq!(
            pill_word(&holders(&["A Very Long Application Name"])),
            "A VERY LONG APPL…"
        );
        assert_eq!(pill_word(&[]), "");
        // Sixteen letters, which in capitals run to seventeen, are cut as seventeen would be.
        assert_eq!(
            pill_word(&holders(&["Großstadtplanerx"])),
            "GROSSSTADTPLANER…"
        );
        assert_eq!(pill_word(&holders(&["Straße"])), "STRASSE");
    }
}
