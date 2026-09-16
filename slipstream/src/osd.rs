//! The on-screen display for the volume and brightness keys, and for a charger plugged in or
//! pulled out: one card low on the screen with an
//! icon, a filled bar and the number. The media keys change the machine and say nothing otherwise
//! — the bar's tray icon is small, and brightness isn't in it at all — and on a keyboard-first
//! desktop that feedback is the whole answer to "did that key do anything?".
//!
//! It appears on the keypress and fades about 1.2 s after the last one, so a key held down (or
//! pressed over and over) keeps one card on screen rather than stacking them up. The rise and the
//! fade in are played once, when the card appears: a further press of the same key only moves the
//! bar and puts the card's departure off, so holding a volume key slides the level along rather
//! than flashing the card in and out under it. Reduced motion gets the same card without the
//! fades.

use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind as ElementKind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    icons,
    motion::HYPR,
    paint::{self, Painter},
    panel,
    text::{self, Face, Style},
};

// Sizes in the mockup's pixels, as the toast and the bar use them.
const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 72.0;
/// Logical pixels per mockup pixel.
const MOCKUP_PX: f32 = 0.8;
/// How far the card's bottom edge sits above the bottom of the screen.
const BOTTOM: f32 = 96.0;
/// How long the card stays up after the last press.
const SHOWN: f64 = 1.2;
/// Caps Lock and Num Lock stay up longer, since their cards have a note to read as well.
const SHOWN_LOCK_KEY: f64 = 1.7;
/// A charger's card isn't the answer to a key, so it may not be looked at straight away.
const SHOWN_POWER: f64 = 2.5;
/// Charging's colour, as on the bar's battery.
const MINT: u32 = icons::CHARGING_RGBA;

/// What Caps Lock's card says under its title: on the desktop, what it does to the screensaver;
/// at the lock screen, how the password is being typed. Every way it can go has a note, so on
/// and off read alike.
pub fn caps_lock_note(on: bool, locked: bool, fades: bool, locks_by_itself: bool) -> &'static str {
    match (on, locked, fades, locks_by_itself) {
        (true, true, ..) => "Typing in capitals",
        (false, true, ..) => "Typing in lower case",
        (true, false, true, true) => "Screensaver paused · lock still on",
        (true, false, true, false) => "Screensaver paused",
        (false, false, true, _) => "Screensaver back on",
        (true, false, false, _) => "Typing in capitals",
        (false, false, false, _) => "Typing in lower case",
    }
}

/// What Num Lock's card says under its title.
pub fn num_lock_note(on: bool) -> &'static str {
    if on {
        "Number pad types numbers"
    } else {
        "Number pad moves the cursor"
    }
}
const FADE: f64 = 0.18;

/// What the keys changed. Each carries what the card should say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The speakers, and whether they're muted.
    Volume {
        muted: bool,
    },
    /// The microphone. It has no level worth a bar, only on or off.
    Microphone {
        muted: bool,
    },
    Brightness,
    /// A media key, and whether the player is playing now. The track goes in the detail.
    Media {
        playing: bool,
    },
    /// Caps Lock or Num Lock, and whether it's on.
    CapsLock {
        on: bool,
    },
    NumLock {
        on: bool,
    },
    /// A charger was plugged in or pulled out. The level is the battery's charge.
    Power {
        plugged: bool,
    },
}

impl Kind {
    /// How long its card stays up after the last press.
    fn shown(self) -> f64 {
        match self {
            Kind::CapsLock { .. } | Kind::NumLock { .. } => SHOWN_LOCK_KEY,
            Kind::Power { .. } => SHOWN_POWER,
            _ => SHOWN,
        }
    }

    fn icon(self, level: u8) -> String {
        let icon = match self {
            Kind::Volume { muted: false } => icons::VOLUME,
            Kind::Volume { muted: true } => icons::VOLUME_MUTED,
            Kind::Microphone { muted: false } => icons::MIC,
            Kind::Microphone { muted: true } => icons::MIC_MUTED,
            Kind::Brightness => icons::SUN,
            Kind::Media { playing: true } => icons::PLAY,
            Kind::Media { playing: false } => icons::PAUSE,
            Kind::CapsLock { .. } | Kind::NumLock { .. } => icons::KEYBOARD,
            Kind::Power { plugged } => return icons::battery(level, plugged),
        };
        icon.to_string()
    }

    /// Whether the level means anything: a muted microphone has no level, and neither has one
    /// that isn't muted.
    fn has_bar(self) -> bool {
        matches!(
            self,
            Kind::Volume { .. } | Kind::Brightness | Kind::Power { .. }
        )
    }

    fn label(self, level: u8) -> String {
        match self {
            Kind::Microphone { muted: true } => "Microphone off".into(),
            Kind::Microphone { muted: false } => "Microphone on".into(),
            Kind::Volume { muted: true } => format!("Muted · {level}%"),
            Kind::Media { playing: true } => "Playing".into(),
            Kind::Media { playing: false } => "Paused".into(),
            Kind::CapsLock { on } => format!("Caps Lock {}", if on { "on" } else { "off" }),
            Kind::NumLock { on } => format!("Num Lock {}", if on { "on" } else { "off" }),
            _ => format!("{level}%"),
        }
    }
}

struct Painted {
    /// What was painted, so the same card isn't painted again every frame.
    key: (Kind, u8, Option<String>, u32, u64),
    buffer: MemoryRenderBuffer,
    logical: Size<i32, Logical>,
    device: (i32, i32),
}

/// The card on screen. The two times are separate so that pressing the key again holds the card
/// where it is: `since` drives the entrance, `at` decides when it goes.
#[derive(Clone, Copy)]
struct Showing {
    kind: Kind,
    level: u8,
    /// When the card appeared, on the animation clock.
    since: f64,
    /// The last press, which the card's time on screen is measured from.
    at: f64,
}

#[derive(Default)]
pub struct Osd {
    showing: Option<Showing>,
    /// A line under the label: the track a media key moved to.
    detail: Option<String>,
    painted: Option<Painted>,
    pub reduced_motion: bool,
}

impl Osd {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            reduced_motion,
            ..Self::default()
        }
    }

    /// A key changed something. `level` is a percentage, ignored by the kinds that have no bar.
    pub fn show(&mut self, kind: Kind, level: u8, now: f64) {
        self.showing = Some(Showing {
            kind,
            level: level.min(100),
            since: self.entrance(kind, now),
            at: now,
        });
        self.detail = None;
    }

    /// As `show`, with a line of detail under the label.
    pub fn show_with(&mut self, kind: Kind, detail: String, now: f64) {
        self.showing = Some(Showing {
            kind,
            level: 0,
            since: self.entrance(kind, now),
            at: now,
        });
        self.detail = Some(detail).filter(|detail| !detail.is_empty());
    }

    /// When the entrance started for a card about to show `kind`: the moment a card of the same
    /// sort already on screen appeared, else now. Muting, or a media key going from playing to
    /// paused, is the same sort of card and keeps it, since only its contents change.
    fn entrance(&self, kind: Kind, now: f64) -> f64 {
        match self.showing {
            Some(showing)
                if std::mem::discriminant(&showing.kind) == std::mem::discriminant(&kind)
                    && (0.0..showing.kind.shown()).contains(&(now - showing.at)) =>
            {
                showing.since
            }
            _ => now,
        }
    }

    /// Takes the card off, for when quick settings opens and shows the same thing better.
    pub fn hide(&mut self) {
        self.showing = None;
    }

    /// The card for a screen `size` logical pixels across, while one is showing. `ink` is the
    /// focus ring's colour, which the icon is drawn in; the level is amber, as a value is on
    /// quick settings' sliders.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        ink: u32,
        now: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Showing {
            kind,
            level,
            since,
            at,
        } = *self.showing.as_ref()?;
        // Time since the last press decides when the card goes; time since it appeared drives the
        // rise and the fade in, so neither restarts under a held key.
        let age = now - at;
        let life = now - since;
        let shown = kind.shown();
        if !(0.0..shown).contains(&age) {
            self.showing = None;
            self.painted = None;
            return None;
        }
        // The scale is part of the key, so a card painted for one screen isn't shown on another.
        let key = (kind, level, self.detail.clone(), ink, scale.to_bits());
        if self.painted.as_ref().is_none_or(|old| old.key != key) {
            self.painted = paint(kind, level, self.detail.as_deref(), ink, scale);
        }
        let painted = self.painted.as_ref()?;
        let (fade, rise) = if self.reduced_motion {
            (0.0, 0.0)
        } else {
            (FADE, 10.0 * (1.0 - HYPR.at((life / FADE).min(1.0))))
        };
        let alpha = if fade <= 0.0 {
            1.0
        } else {
            (life / fade).min((shown - age) / fade).clamp(0.0, 1.0)
        };
        // The painted area holds the shadow too; the card itself keeps its place.
        let margin = (panel::NOTICE_MARGIN * MOCKUP_PX) as f64;
        let location = Point::<f64, Logical>::from((
            ((size.w - painted.logical.w) / 2) as f64,
            (size.h - painted.logical.h) as f64 + margin
                - (BOTTOM as f64 - rise) * MOCKUP_PX as f64,
        ))
        .to_physical(scale)
        .to_i32_round::<i32>()
        .to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
            &painted.buffer,
            Some(alpha as f32),
            Some(Rectangle::from_size(
                (painted.device.0 as f64, painted.device.1 as f64).into(),
            )),
            Some(painted.logical),
            ElementKind::Unspecified,
        )
        .ok()
    }
}

fn paint(kind: Kind, level: u8, detail: Option<&str>, ink: u32, scale: f64) -> Option<Painted> {
    const TRACK: u32 = 0xffffff1f;
    const DIM: u32 = 0x8a94a6ff;
    // Room for the shadow all round.
    let m = panel::NOTICE_MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((WIDTH + 2.0 * m) * MOCKUP_PX).ceil() as i32,
        ((HEIGHT + 2.0 * m) * MOCKUP_PX).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * scale).round() as i32,
        (logical.h as f64 * scale).round() as i32,
    );
    let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * MOCKUP_PX)?;
    // Solid, as every notice is: it's read at a glance, often over a film.
    panel::notice(&mut p, m, m, WIDTH, HEIGHT);
    let (dx, dy) = (m, m);
    // A muted thing is drawn in grey, so the card reads before the icon does.
    let muted = matches!(
        kind,
        Kind::Volume { muted: true } | Kind::Microphone { muted: true }
    );
    let ink = if muted { DIM } else { ink };
    p.icon(&kind.icon(level), dx + 22.0, dy + 24.0, 24.0, Some(ink));
    let label = Style {
        tabular: true,
        ..Style::new(Face::Mono, 15.0, 0xdfe5eeff)
    };
    if kind.has_bar() {
        // The bar runs from the icon to the number, which keeps its own room whatever it says.
        const BAR_X: f32 = 60.0;
        const BAR_W: f32 = 236.0;
        const BAR_H: f32 = 6.0;
        let (bar_x, y) = (dx + BAR_X, dy + HEIGHT / 2.0 - BAR_H / 2.0);
        p.fill(bar_x, y, BAR_W, BAR_H, BAR_H / 2.0, TRACK);
        // Below about a percent there's nothing to draw, and a stub would read as more than none.
        let filled = BAR_W * level.min(100) as f32 / 100.0;
        if filled >= BAR_H {
            let bar = match kind {
                _ if muted => DIM,
                Kind::Power { plugged: true } => MINT,
                _ => panel::AMBER,
            };
            p.fill(bar_x, y, filled, BAR_H, BAR_H / 2.0, bar);
        }
        let number = format!("{level}%");
        let width = text::width(&number, &label);
        p.text(
            &number,
            dx + WIDTH - 22.0 - width,
            dy + HEIGHT / 2.0,
            &label,
        );
        let tag = |colour: u32| Style {
            tracking: 0.12,
            ..Style::new(Face::BodyBold, 10.0, colour)
        };
        if let Kind::Power { plugged } = kind {
            // What happened, under the bar, as muted is said.
            let (word, colour) = if plugged {
                ("CHARGING", MINT)
            } else {
                ("ON BATTERY", DIM)
            };
            p.text(word, bar_x, dy + HEIGHT / 2.0 + 16.0, &tag(colour));
        }
        if muted {
            // Muted keeps the level visible — it's what comes back — and says so in the icon's
            // place, under the bar, rather than taking the number's room.
            p.text("MUTED", bar_x, dy + HEIGHT / 2.0 + 16.0, &tag(DIM));
        }
    } else if let Some(detail) = detail {
        // A track: what the player is doing, and what it's playing under it.
        let state = Style {
            tracking: 0.12,
            ..Style::new(Face::BodyBold, 10.0, DIM)
        };
        let title = Style::new(Face::Body, 15.0, 0xdfe5eeff);
        p.text(
            &kind.label(level).to_uppercase(),
            dx + 60.0,
            dy + HEIGHT / 2.0 - 11.0,
            &state,
        );
        let room = WIDTH - 60.0 - 22.0;
        p.text(
            &text::ellipsize(detail, &title, room),
            dx + 60.0,
            dy + HEIGHT / 2.0 + 8.0,
            &title,
        );
    } else {
        p.text(&kind.label(level), dx + 60.0, dy + HEIGHT / 2.0, &label);
    }
    Some(Painted {
        key: (kind, level, detail.map(String::from), ink, scale.to_bits()),
        buffer: paint::buffer(&p.pixmap),
        logical,
        device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RING: u32 = 0x42d3ffff;

    #[test]
    fn the_card_is_the_same_size_whatever_it_says() {
        let quiet = paint(Kind::Volume { muted: false }, 0, None, RING, 1.25).unwrap();
        let loud = paint(Kind::Volume { muted: false }, 100, None, RING, 1.25).unwrap();
        let mic = paint(Kind::Microphone { muted: true }, 0, None, RING, 1.25).unwrap();
        assert_eq!(quiet.logical, loud.logical);
        assert_eq!(quiet.logical, mic.logical);
    }

    #[test]
    fn a_level_over_a_hundred_is_still_a_hundred() {
        let mut osd = Osd::new(false);
        osd.show(Kind::Brightness, 180, 0.0);
        assert_eq!(osd.showing.unwrap().level, 100);
    }

    #[test]
    fn the_card_goes_when_its_time_is_up() {
        let mut osd = Osd::new(false);
        osd.show(Kind::Brightness, 40, 10.0);
        assert!(osd.showing.is_some());
        // A second press while it's up holds it there rather than starting a second card.
        osd.show(Kind::Brightness, 45, 11.0);
        assert_eq!(osd.showing.unwrap().at, 11.0);
    }

    #[test]
    fn a_second_press_moves_the_bar_without_playing_the_entrance_again() {
        let mut osd = Osd::new(false);
        osd.show(Kind::Volume { muted: false }, 40, 10.0);
        // Muting, and turning it up again, are the same card with different contents.
        osd.show(Kind::Volume { muted: true }, 40, 10.3);
        osd.show(Kind::Volume { muted: false }, 45, 10.6);
        let showing = osd.showing.unwrap();
        assert_eq!(showing.since, 10.0, "the entrance started once");
        assert_eq!(showing.at, 10.6);
        assert_eq!(showing.level, 45);
        // A card that has had its time, or a different key, starts afresh.
        let gone = 10.6 + SHOWN + 0.1;
        osd.show(Kind::Volume { muted: false }, 50, gone);
        assert_eq!(osd.showing.unwrap().since, gone);
        osd.show(Kind::Brightness, 50, gone + 0.1);
        assert_eq!(osd.showing.unwrap().since, gone + 0.1);
    }

    #[test]
    fn a_held_key_keeps_the_card_solid() {
        let mut osd = Osd::new(false);
        osd.show(Kind::Volume { muted: false }, 40, 0.0);
        // Once it's in, every further press leaves it fully opaque and at rest: the alpha and the
        // rise both run from `since`, so they don't start over.
        for step in 1..8 {
            let now = FADE + 0.05 * step as f64;
            osd.show(Kind::Volume { muted: false }, 40 + step as u8, now);
            let showing = osd.showing.unwrap();
            let (life, age) = (now - showing.since, now - showing.at);
            assert_eq!(age, 0.0);
            assert!(
                (life / FADE).min((SHOWN - age) / FADE).clamp(0.0, 1.0) == 1.0,
                "faded out at {now}"
            );
            let rise = 10.0 * (1.0 - HYPR.at((life / FADE).min(1.0)));
            assert!(rise.abs() < 1e-4, "rose again at {now}");
        }
    }

    #[test]
    fn the_lock_keys_stay_up_the_same_time_either_way() {
        for on in [true, false] {
            assert_eq!(Kind::CapsLock { on }.shown(), SHOWN_LOCK_KEY);
            assert_eq!(Kind::NumLock { on }.shown(), SHOWN_LOCK_KEY);
        }
        assert_eq!(Kind::Brightness.shown(), SHOWN);
    }

    #[test]
    fn every_lock_key_note_fits_the_card() {
        let title = Style::new(Face::Body, 15.0, 0xdfe5eeff);
        let bools = [true, false];
        for on in bools {
            for locked in bools {
                for fades in bools {
                    for locking in bools {
                        let note = caps_lock_note(on, locked, fades, locking);
                        assert_eq!(text::ellipsize(note, &title, WIDTH - 60.0 - 22.0), note);
                    }
                }
            }
            let note = num_lock_note(on);
            assert_eq!(text::ellipsize(note, &title, WIDTH - 60.0 - 22.0), note);
        }
    }

    #[test]
    fn caps_lock_going_off_says_so_as_going_on_does() {
        assert_eq!(
            caps_lock_note(true, false, true, false),
            "Screensaver paused"
        );
        assert_eq!(
            caps_lock_note(false, false, true, false),
            "Screensaver back on"
        );
        assert_eq!(caps_lock_note(true, true, true, true), "Typing in capitals");
    }

    #[test]
    fn only_the_microphone_goes_without_a_bar() {
        assert!(Kind::Volume { muted: true }.has_bar());
        assert!(Kind::Brightness.has_bar());
        assert!(!Kind::Microphone { muted: false }.has_bar());
        assert_eq!(Kind::Microphone { muted: true }.label(0), "Microphone off");
    }
}
