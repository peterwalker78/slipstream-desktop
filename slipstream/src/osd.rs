//! The on-screen display for the volume and brightness keys: one card low on the screen with an
//! icon, a filled bar and the number. The media keys change the machine and say nothing otherwise
//! — the bar's tray icon is small, and brightness isn't in it at all — and on a keyboard-first
//! desktop that feedback is the whole answer to "did that key do anything?".
//!
//! It appears on the keypress and fades about 1.2 s after the last one, so a key held down (or
//! pressed over and over) keeps one card on screen rather than stacking them up. Reduced motion
//! gets the same card without the fades.

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
/// Caps Lock going on stays up longer, since its card has a note to read as well.
const SHOWN_CAPS_ON: f64 = 1.7;

/// The note on Caps Lock's card when it goes on, depending on whether the screen locks by itself.
pub const CAPS_ON_NOTE: &str = "Screensaver paused";
pub const CAPS_ON_NOTE_LOCKING: &str = "Screensaver paused · lock still on";
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
}

impl Kind {
    /// How long its card stays up after the last press.
    fn shown(self) -> f64 {
        match self {
            Kind::CapsLock { on: true } => SHOWN_CAPS_ON,
            _ => SHOWN,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Kind::Volume { muted: false } => icons::VOLUME,
            Kind::Volume { muted: true } => icons::VOLUME_MUTED,
            Kind::Microphone { muted: false } => icons::MIC,
            Kind::Microphone { muted: true } => icons::MIC_MUTED,
            Kind::Brightness => icons::SUN,
            Kind::Media { playing: true } => icons::PLAY,
            Kind::Media { playing: false } => icons::PAUSE,
            Kind::CapsLock { .. } | Kind::NumLock { .. } => icons::KEYBOARD,
        }
    }

    /// Whether the level means anything: a muted microphone has no level, and neither has one
    /// that isn't muted.
    fn has_bar(self) -> bool {
        matches!(self, Kind::Volume { .. } | Kind::Brightness)
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

#[derive(Default)]
pub struct Osd {
    /// What's showing, at what level, and when the last key was pressed on the animation clock.
    showing: Option<(Kind, u8, f64)>,
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
        self.showing = Some((kind, level.min(100), now));
        self.detail = None;
    }

    /// As `show`, with a line of detail under the label.
    pub fn show_with(&mut self, kind: Kind, detail: String, now: f64) {
        self.showing = Some((kind, 0, now));
        self.detail = Some(detail).filter(|detail| !detail.is_empty());
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
        let (kind, level, at) = *self.showing.as_ref()?;
        let age = now - at;
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
            (FADE, 10.0 * (1.0 - HYPR.at((age / FADE).min(1.0))))
        };
        let alpha = if fade <= 0.0 {
            1.0
        } else {
            (age / fade).min((shown - age) / fade).clamp(0.0, 1.0)
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
    p.icon(kind.icon(), dx + 22.0, dy + 24.0, 24.0, Some(ink));
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
            let bar = if muted { DIM } else { panel::AMBER };
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
        if muted {
            // Muted keeps the level visible — it's what comes back — and says so in the icon's
            // place, under the bar, rather than taking the number's room.
            p.text(
                "MUTED",
                bar_x,
                dy + HEIGHT / 2.0 + 16.0,
                &Style {
                    tracking: 0.12,
                    ..Style::new(Face::BodyBold, 10.0, DIM)
                },
            );
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
        assert_eq!(osd.showing.unwrap().1, 100);
    }

    #[test]
    fn the_card_goes_when_its_time_is_up() {
        let mut osd = Osd::new(false);
        osd.show(Kind::Brightness, 40, 10.0);
        assert!(osd.showing.is_some());
        // A second press while it's up holds it there rather than starting a second card.
        osd.show(Kind::Brightness, 45, 11.0);
        assert_eq!(osd.showing.unwrap().2, 11.0);
    }

    #[test]
    fn caps_lock_going_on_stays_up_half_a_second_longer() {
        assert_eq!(Kind::CapsLock { on: true }.shown(), SHOWN + 0.5);
        assert_eq!(Kind::CapsLock { on: false }.shown(), SHOWN);
    }

    #[test]
    fn caps_locks_notes_fit_the_card() {
        let title = Style::new(Face::Body, 15.0, 0xdfe5eeff);
        for note in [CAPS_ON_NOTE, CAPS_ON_NOTE_LOCKING] {
            assert_eq!(text::ellipsize(note, &title, WIDTH - 60.0 - 22.0), note);
        }
    }

    #[test]
    fn only_the_microphone_goes_without_a_bar() {
        assert!(Kind::Volume { muted: true }.has_bar());
        assert!(Kind::Brightness.has_bar());
        assert!(!Kind::Microphone { muted: false }.has_bar());
        assert_eq!(Kind::Microphone { muted: true }.label(0), "Microphone off");
    }
}
