//! What Slipstream's panels share: the glass card quick settings and the notification centre drop
//! at the top right, just below the bar, sliding down into place as they open; and the `Panel`
//! trait every panel answers to, the explorer, the plugin panel, the shortcut sheet and clipboard
//! history too, so that closing them, or asking whether one is open, goes through one list.

use crate::motion::REDUCED_FADE;
use crate::{
    Slipstream,
    centre::Centre,
    explorer::Explorer,
    history::History,
    keys::Action,
    motion::HYPR,
    paint::{Painted, Painter},
    plugin_panel::PluginPanel,
    quick::QuickSettings,
    sheet::Sheet,
};
use smithay::backend::renderer::{
    ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement,
};
use smithay::utils::{Logical, Point};

/// What a card is drawn for: the screen's logical size and scale, the time on whichever clock
/// the card animates by, and two of the colours from the settings: the accent, for what's on, and
/// the secondary colour, for what wants attention.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub size: smithay::utils::Size<i32, smithay::utils::Logical>,
    pub scale: f64,
    pub time: f64,
    pub accent: u32,
    pub attention: u32,
    /// Where the bar sits, which the cards dropping from under it hang from.
    pub bar: crate::bar::Place,
}

/// Where a card dropping from under the bar at `bar` starts, in design pixels from the screen's
/// top: `TOP` under the bar along the edge, and as much lower as a floating strip's foot is.
pub fn top(bar: crate::bar::Place) -> f32 {
    TOP + bar.drop() as f32 / DESIGN_PX
}

/// Where a card is painted, in design pixels: its corner (`x`, `y`) inside the painted area, its
/// width, and that corner's place on the screen (`left`, `top`).
#[derive(Debug, Clone, Copy)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub left: f32,
    pub top: f32,
}

impl Place {
    /// A box in the painted area, in logical pixels on the screen.
    pub fn on_screen(
        self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> smithay::utils::Rectangle<f64, smithay::utils::Logical> {
        smithay::utils::Rectangle::new(
            (
                ((self.left + x - self.x) * DESIGN_PX) as f64,
                ((self.top + y - self.y) * DESIGN_PX) as f64,
            )
                .into(),
            ((w * DESIGN_PX) as f64, (h * DESIGN_PX) as f64).into(),
        )
    }
}

/// Logical pixels per design pixel. Every surface is laid out in design pixels, sized for a 1.25×
/// screen: each is 0.8 logical pixels, which there is exactly one physical pixel.
pub const DESIGN_PX: f32 = 0.8;
/// The card's top right corner, in design pixels: 10 in from the screen's edge, and 48 down, which
/// is 8 below the bar.
pub const RIGHT: f32 = 10.0;
pub const TOP: f32 = 48.0;
/// Room around the card for its shadow, and more below it, where the shadow falls.
pub const MARGIN: f32 = 64.0;
pub const BELOW: f32 = 32.0;
pub const PADDING: f32 = 18.0;
/// Between the card's rows.
pub const GAP: f32 = 14.0;

/// Text, from the brightest down: what's typed or chosen, headings and names, prose, then two
/// steps of secondary text above `HINT` and `PLACEHOLDER`.
pub const BRIGHT: u32 = 0xf2f4f8ff;
pub const INK: u32 = 0xe7ebf1ff;
pub const PROSE: u32 = 0xdfe5eeff;
pub const SECONDARY: u32 = 0xaab2c0ff;
pub const TERTIARY: u32 = 0x9aa4b6ff;
/// What wants attention, until the settings are read: the secondary colour's default.
/// `Slipstream::panel_attention` hands the chosen one out.
pub const AMBER: u32 = 0xffb547ff;
/// Something pressing: a gauge nearly used up. Orange rather than red, which the bar keeps for
/// sharing.
pub const HOT: u32 = 0xff7a45ff;
/// Text on amber.
pub const DARK: u32 = 0x1a1206ff;
/// The code rain's mint.
pub const MINT: u32 = 0x3cf0c0ff;
/// A hairline between a panel's parts.
pub const DIVIDER: u32 = 0xffffff12;
/// A quiet button or keycap, the same under the pointer, and its 1 px edge.
pub const QUIET: u32 = 0xffffff12;
pub const QUIET_LIT: u32 = 0xffffff26;
pub const EDGE: u32 = 0xffffff24;
/// A tile that isn't lit: a switcher window, a notification, a quick settings toggle.
pub const TILE: u32 = 0xffffff0a;
/// Corner radii: tiles, and selectable rows and rectangular buttons.
pub const TILE_RADIUS: f32 = 12.0;
pub const ROW_RADIUS: f32 = 9.0;
/// The near-black of dark chips, the bar and wells, without its alpha: each surface picks its
/// own. The same as the code rain's cards, so they sit on the default wallpaper as one.
pub const CHIP: u32 = 0x0b0d1200;
/// A level's empty track on a notice.
pub const TRACK: u32 = 0xffffff1a;
/// How far in from the frame's left and right edges a panel's head row sits.
pub const HEAD_INSET: f32 = 26.0;
/// Where the keyboard is, until the settings are read: the focused window's ring colour at the
/// same four fifths' opacity the window ring is drawn with. `Slipstream::panel_ring` hands the
/// chosen one to each panel.
pub const FOCUS: u32 = 0x42d3ffcc;
/// What's on or chosen inside a panel, until the settings are read. `Slipstream::panel_accent`
/// hands the chosen one to each panel; `AMBER` stays for what needs noticing.
pub const ACCENT: u32 = 0xffb547ff;

/// Opening: 200 ms from 10 px higher and transparent.
const OPEN: f64 = 0.2;
/// Where every panel and card opens from: transparent, 10 design px above its place.
const HIDDEN: (f32, f64) = (0.0, -10.0);
/// Closing: 140 ms, speeding up as it rises 6 px and fades, so it's gone before it reads as
/// moving. Only the paint lingers: input goes to what's underneath the moment it closes.
const CLOSE: f64 = 0.14;
const CLOSE_RISE: f64 = 6.0;

/// The card, `w` × `h` at (`x`, `y`), with its shadow. With nothing blurred behind it, the glass is
/// nearly solid, as the explorer's is.
pub fn glass(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
    p.card(x, y, w, h, &PANEL_CARD);
}

/// Large panels' card: their glass, its edge, and the soft shadow under it.
const PANEL_CARD: crate::paint::CardStyle = crate::paint::CardStyle {
    radius: 18.0,
    shadow: (16.0, 64.0, 0x00000048),
    fill: GLASS,
    edge: GLASS_EDGE,
};

/// Large panels' glass: the explorer, quick settings, the centre, the cards and the sheet.
pub const GLASS: u32 = 0x10131af8;
pub const GLASS_EDGE: u32 = 0xffffff1c;

/// The small surfaces that come and go by themselves: the toast, the slim card, notification
/// pop-ups and the volume display. Solid, so nothing behind shows through.
pub const NOTICE: u32 = 0x141925ff;
pub const NOTICE_EDGE: u32 = 0xffffff18;
pub const NOTICE_RADIUS: f32 = 12.0;
/// How far a notice's shadow reaches past its card: down by its offset and out by half its blur.
/// Soft and faint, as current desktops' popups are: the card's edge does the separating.
pub const NOTICE_SHADOW: (f32, f32, u32) = (10.0, 40.0, 0x00000040);
pub const NOTICE_MARGIN: f32 = 48.0;

/// Text that teaches a key or says something quietly: 6.4:1 on the glass, 5.4:1 in a quiet
/// button.
pub const HINT: u32 = 0x8f98a8ff;
/// What an empty search box says.
pub const PLACEHOLDER: u32 = 0x7d8697ff;

/// A colour from the settings, each channel 0–1, packed as `0xrrggbbff` for the painter.
pub fn opaque(rgb: [f32; 3]) -> u32 {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    u32::from_be_bytes([r, g, b, 0xff])
}

/// Dark ink on a light colour, light ink on a dark one.
pub fn ink_on(rgba: u32) -> u32 {
    let [r, g, b, _] = rgba.to_be_bytes().map(|c| c as f32 / 255.0);
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.45 {
        DARK
    } else {
        BRIGHT
    }
}

/// `rgba` taken `share` of the way to white, opaque: a link in the accent, or a button lit under
/// the pointer.
pub fn lighter(rgba: u32, share: f32) -> u32 {
    let [r, g, b, _] = rgba.to_be_bytes();
    let lift = |c: u8| c + ((255 - c) as f32 * share).round() as u8;
    u32::from_be_bytes([lift(r), lift(g), lift(b), 0xff])
}

/// Quieter ink on a colour: `ink_on`'s, partly letting the colour through.
pub fn faint_ink_on(rgba: u32) -> u32 {
    (ink_on(rgba) & 0xffffff00) | 0xb8
}

/// A selection's fill: the ring colour, faint.
pub fn selection_fill(ring: u32) -> u32 {
    (ring & 0xffffff00) | 0x1c
}

/// The chosen row of a list: its box filled faintly in the ring colour, with a 2 px edge in it.
pub fn selected_row(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, ring: u32) {
    p.fill(x, y, w, h, ROW_RADIUS, selection_fill(ring));
    p.border(x, y, w, h, ROW_RADIUS, 2.0, ring);
}

/// A quiet button's box: faint, lighter under the pointer (`lit`), with a 1 px edge.
pub fn quiet_button(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, radius: f32, lit: bool) {
    p.fill(x, y, w, h, radius, if lit { QUIET_LIT } else { QUIET });
    p.border(x, y, w, h, radius, 1.0, EDGE);
}

/// A section's title in small capitals.
pub fn section_style() -> crate::text::Style {
    crate::text::Style {
        tracking: 0.12,
        ..crate::text::Style::new(crate::text::Face::Body, 12.0, PLACEHOLDER)
    }
}

/// An app with no icon: a `side`-pixel square at (`x`, `y`), coloured by its name, with its
/// initial in capitals `px` pixels tall.
pub fn app_placeholder(
    p: &mut Painter,
    name: &str,
    x: f32,
    y: f32,
    side: f32,
    radius: f32,
    px: f32,
) {
    const COLOURS: [u32; 6] = [MINT, 0x33ccffff, AMBER, 0xff7a93ff, 0xa78bfaff, 0x7fe3ffff];
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    p.fill(
        x,
        y,
        side,
        side,
        radius,
        COLOURS[hash as usize % COLOURS.len()],
    );
    let initial: String = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_default();
    let style = crate::text::Style::new(crate::text::Face::MonoBold, px, 0x10131aff);
    let w = crate::text::width(&initial, &style);
    p.text(&initial, x + (side - w) / 2.0, y + side / 2.0, &style);
}

/// A notice's card, `w` × `h` at (`x`, `y`), with its shadow.
pub fn notice(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
    let (dy, blur, shadow) = NOTICE_SHADOW;
    p.shadow(x, y, w, h, NOTICE_RADIUS, dy, blur, shadow);
    p.fill(x, y, w, h, NOTICE_RADIUS, NOTICE);
    p.border(x, y, w, h, NOTICE_RADIUS, 1.0, NOTICE_EDGE);
}

/// Between a footer's hints.
pub const HINT_GAP: f32 = 20.0;
/// Between a hint's keycaps, and before its words.
const KEY_GAP: f32 = 4.0;

fn hint_style() -> crate::text::Style {
    crate::text::Style::new(crate::text::Face::Body, 13.0, HINT)
}

/// How wide `key_hint` draws `keys` and `label`.
pub fn key_hint_width(keys: &[&str], label: &str) -> f32 {
    let caps: f32 = shown(keys)
        .map(|key| crate::paint::keycap_width(key) + KEY_GAP)
        .sum();
    let label_w = if label.is_empty() {
        0.0
    } else {
        KEY_GAP + crate::text::width(label, &hint_style())
    };
    caps - KEY_GAP + label_w
}

/// The keys a hint draws: a key with no name, for an action nothing is bound to, gets no cap.
fn shown<'a>(keys: &'a [&'a str]) -> impl Iterator<Item = &'a &'a str> {
    keys.iter().filter(|key| !key.is_empty())
}

/// A footer's hint ending at `right`, centred on `centre`: its keycaps, each a way of doing the
/// same thing, then what they do. Returns where it starts.
pub fn key_hint(p: &mut Painter, right: f32, centre: f32, keys: &[&str], label: &str) -> f32 {
    let style = hint_style();
    let start = right - key_hint_width(keys, label);
    let mut x = start;
    for key in shown(keys) {
        x += p.keycap(key, x, centre) + KEY_GAP;
    }
    if !label.is_empty() {
        p.text(label, x, centre, &style);
    }
    start
}

/// The keyboard's ring, just outside a control's box.
pub fn focus_ring(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, radius: f32, colour: u32) {
    p.border(
        x - 4.0,
        y - 4.0,
        w + 8.0,
        h + 8.0,
        radius + 4.0,
        2.0,
        colour,
    );
}

/// A panel `since` seconds after it opened: its opacity, and how far above its place it is, in
/// design pixels.
pub fn opening(since: f64, reduced_motion: bool) -> (f32, f64) {
    if reduced_motion {
        ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
    } else {
        let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
        (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
    }
}

/// One layer of what a panel last drew: its paint, its corner where the panel settles, and its
/// share of the panel's opacity.
pub type Layer = (Painted, Point<f64, Logical>, f32);

/// A panel's way in and out. It opens from hidden, or from wherever a close it interrupts has
/// got to, and closes by fading out the picture it last drew, rising as it goes. Neither holds
/// anything up: the panel stops taking keys and clicks the moment it closes, and this only paints.
pub struct Fade {
    /// The opacity and rise in design pixels (negative is up) the opening starts from.
    from: (f32, f64),
    /// The opacity and rise last drawn.
    drawn: (f32, f64),
    /// Whether motion was reduced when it was last drawn.
    reduced_motion: bool,
    closing: Option<Closing>,
}

/// A panel fading out: the picture it closed on, front first, the opacity and rise it closed
/// from, and when the fade started on the animation clock, which the first frame after it fills
/// in.
struct Closing {
    picture: Vec<Layer>,
    from: (f32, f64),
    started: Option<f64>,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            from: HIDDEN,
            drawn: (0.0, 0.0),
            reduced_motion: false,
            closing: None,
        }
    }
}

impl Fade {
    /// Opens again: from where it is drawn if it was still fading out, otherwise from hidden.
    pub fn open(&mut self) {
        self.from = match self.closing.take() {
            Some(_) => self.drawn,
            None => HIDDEN,
        };
    }

    /// Its opacity and rise while it opens, given `opening`, the panel's own way in from hidden
    /// at this moment: the same way, carried on from where it starts.
    pub fn opening(&mut self, (alpha, rise): (f32, f64), reduced_motion: bool) -> (f32, f64) {
        let (from_alpha, from_rise) = self.from;
        self.drawn = (
            alpha + from_alpha * (1.0 - alpha),
            rise / HIDDEN.1 * from_rise,
        );
        self.reduced_motion = reduced_motion;
        self.drawn
    }

    /// Closes, keeping `picture`, what it last drew, to fade out; `None` when nothing is drawn.
    /// A panel drawn some other way hands an empty picture and draws it from `closing`. Closing
    /// again while it fades changes nothing. Returns whether it fades.
    pub fn close(&mut self, picture: Option<Vec<Layer>>) -> bool {
        if self.closing.is_some() {
            return true;
        }
        match picture {
            Some(picture) if self.drawn.0 > 0.0 => {
                self.closing = Some(Closing {
                    picture,
                    from: self.drawn,
                    started: None,
                });
                true
            }
            _ => {
                self.drawn = (0.0, 0.0);
                false
            }
        }
    }

    /// Whether it is still fading out after closing.
    pub fn is_closing(&self) -> bool {
        self.closing.is_some()
    }

    /// Its opacity and rise at `now` while it fades out. `None` once it has gone, and the picture
    /// is let go.
    pub fn closing(&mut self, now: f64) -> Option<(f32, f64)> {
        let closing = self.closing.as_mut()?;
        let started = *closing.started.get_or_insert(now);
        match closing_look(closing.from, now - started, self.reduced_motion) {
            Some(look) => {
                self.drawn = look;
                Some(look)
            }
            None => {
                self.closing = None;
                self.drawn = (0.0, 0.0);
                None
            }
        }
    }

    /// Its picture at `now` while it fades out, front first.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        now: f64,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some((alpha, rise)) = self.closing(now) else {
            return Vec::new();
        };
        let Some(closing) = self.closing.as_ref() else {
            return Vec::new();
        };
        let lift = Point::from((0.0, rise * DESIGN_PX as f64));
        closing
            .picture
            .iter()
            .filter_map(|(painted, at, share)| painted.element(renderer, *at + lift, alpha * share))
            .collect()
    }
}

/// A closing's opacity and rise, `since` seconds in, from what was drawn when it closed: rises and
/// fades on an accelerating cubic, or with reduced motion only fades, no longer than
/// `REDUCED_FADE`. `None` once it has gone.
fn closing_look(from: (f32, f64), since: f64, reduced_motion: bool) -> Option<(f32, f64)> {
    let length = if reduced_motion { REDUCED_FADE } else { CLOSE };
    let t = since / length;
    if t >= 1.0 {
        return None;
    }
    let t = t.max(0.0);
    if reduced_motion {
        return Some((from.0 * (1.0 - t) as f32, from.1));
    }
    let eased = t.powi(3);
    Some((
        from.0 * (1.0 - eased) as f32,
        from.1 + (-CLOSE_RISE - from.1) * eased,
    ))
}

/// A panel that drops over the desktop: the explorer, quick settings, the plugin panel, the
/// notification centre, the shortcut sheet and clipboard history. One is open at a time.
pub trait Panel {
    fn is_open(&self) -> bool;
    fn close(&mut self);
    /// Paints its text again next frame: a face for characters it lacked has landed.
    fn forget_painted_text(&mut self);
    /// The binding that opens and closes it, which therefore doesn't close it on the way.
    fn binding(&self) -> Action;
    /// Whether volume, brightness and media keys leave it open, showing the change there or
    /// keeping a half-typed query.
    fn stays_for_media(&self) -> bool;
}

macro_rules! panel {
    ($panel:ty, $binding:expr, stays_for_media: $stays:expr) => {
        impl Panel for $panel {
            fn is_open(&self) -> bool {
                <$panel>::is_open(self)
            }
            fn close(&mut self) {
                <$panel>::close(self)
            }
            fn forget_painted_text(&mut self) {
                <$panel>::forget_painted_text(self)
            }
            fn binding(&self) -> Action {
                $binding
            }
            fn stays_for_media(&self) -> bool {
                $stays
            }
        }
    };
}

panel!(Explorer, Action::Explorer, stays_for_media: true);
panel!(QuickSettings, Action::QuickSettings, stays_for_media: true);
panel!(PluginPanel, Action::Plugins, stays_for_media: true);
panel!(Centre, Action::NotificationCentre, stays_for_media: false);
panel!(Sheet, Action::ShortcutSheet, stays_for_media: true);
panel!(History, Action::ClipboardHistory, stays_for_media: true);

impl Slipstream {
    /// Every panel, open or not.
    pub fn panels(&self) -> [&dyn Panel; 6] {
        [
            &self.explorer,
            &self.quick,
            &self.plugin_panel,
            &self.centre,
            &self.sheet,
            &self.history,
        ]
    }

    pub fn panels_mut(&mut self) -> [&mut dyn Panel; 6] {
        [
            &mut self.explorer,
            &mut self.quick,
            &mut self.plugin_panel,
            &mut self.centre,
            &mut self.sheet,
            &mut self.history,
        ]
    }

    pub fn panel_open(&self) -> bool {
        self.panels().iter().any(|panel| panel.is_open())
    }

    /// Every panel and card fading out after it closed, but the explorer, which draws its own.
    pub fn fades_mut(&mut self) -> impl Iterator<Item = &mut Fade> {
        [
            &mut self.quick.fade,
            &mut self.plugin_panel.fade,
            &mut self.centre.fade,
            &mut self.sheet.fade,
            &mut self.history.fade,
        ]
        .into_iter()
        .chain(self.shortcuts.fading.as_mut())
        .chain(self.fading.iter_mut())
        .filter(|fade| fade.is_closing())
    }

    /// Closes every panel, before another opens or something else takes the screen.
    pub fn close_panels(&mut self) {
        for panel in self.panels_mut() {
            panel.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG's contrast ratio between two colours, 0xRRGGBBAA, the alpha ignored.
    fn contrast(a: u32, b: u32) -> f64 {
        let luminance = |rgba: u32| {
            let [r, g, b, _] = rgba.to_be_bytes().map(|c| {
                let c = c as f64 / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn hint_ink_is_at_least_4_5_to_1() {
        let glass = 0x10131aff;
        // A quiet button: white at 7% over the glass.
        let quiet = 0x21232aff;
        assert!(contrast(HINT, glass) >= 4.5, "{}", contrast(HINT, glass));
        assert!(contrast(HINT, quiet) >= 4.5, "{}", contrast(HINT, quiet));
        assert!(contrast(PLACEHOLDER, glass) >= 4.5);
    }

    fn picture() -> Option<Vec<Layer>> {
        Painted::new((1, 1).into(), 1.0, |_| {})
            .map(|painted| vec![(painted, Point::default(), 1.0)])
    }

    #[test]
    fn a_closed_panel_lets_go_at_once_and_only_its_picture_fades() {
        let mut fade = Fade::default();
        fade.open();
        assert_eq!(fade.opening((1.0, 0.0), false), (1.0, 0.0));
        assert!(fade.close(picture()), "closing waits for nothing");
        assert!(fade.is_closing());
        // From where it was drawn, then partway up and still mostly there, speeding up.
        assert_eq!(fade.closing(5.0), Some((1.0, 0.0)));
        let (alpha, rise) = fade.closing(5.0 + CLOSE / 2.0).unwrap();
        assert!(alpha > 0.8 && rise < 0.0 && rise > -CLOSE_RISE);
        assert_eq!(fade.closing(5.0 + CLOSE + 0.001), None);
        assert!(!fade.is_closing(), "and then the picture is let go");
    }

    #[test]
    fn a_panel_closed_before_it_was_drawn_has_nothing_to_fade() {
        let mut fade = Fade::default();
        fade.open();
        assert!(!fade.close(picture()));
        assert!(!fade.close(None));
    }

    #[test]
    fn reopened_mid_fade_it_comes_back_from_where_it_is_drawn() {
        let mut fade = Fade::default();
        fade.opening(opening(OPEN, false), false);
        fade.close(picture());
        fade.closing(0.0);
        let drawn = fade.closing(CLOSE * 0.8).unwrap();
        fade.open();
        assert!(!fade.is_closing());
        assert_eq!(fade.opening(opening(0.0, false), false), drawn);
        assert!((fade.opening(opening(OPEN, false), false).0 - 1.0).abs() < 1e-3);
    }

    #[test]
    fn with_reduced_motion_closing_only_fades_and_no_longer_than_the_short_fade() {
        let mut fade = Fade::default();
        fade.opening(opening(REDUCED_FADE, true), true);
        fade.close(picture());
        assert_eq!(fade.closing(0.0), Some((1.0, 0.0)));
        assert_eq!(fade.closing(REDUCED_FADE / 2.0), Some((0.5, 0.0)));
        assert_eq!(fade.closing(REDUCED_FADE), None);
    }

    #[test]
    fn a_key_nothing_is_bound_to_gets_no_empty_cap() {
        assert_eq!(
            key_hint_width(&["", "Esc"], "close"),
            key_hint_width(&["Esc"], "close")
        );
    }
}
