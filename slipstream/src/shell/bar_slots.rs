//! The slots the bar's modules sit in, right of the clock: how wide each is and how it moves.
//!
//! A module's text can change width every second (a counter going from 9 to 10), and the clock
//! and date are centred with the modules as one block, so a slot that followed its text exactly
//! would make the clock wobble. Instead each slot keeps the widest its text has been lately, and
//! only narrows once the text has stayed narrower for a few seconds. Digits are tabular, so a
//! number that only changes its digits keeps its width anyway. Any change of width eases, and so
//! the whole block glides to its new centre; with reduced motion it simply moves.
//!
//! Text is only ever drawn inside its slot. A new slot opens, and its text fades in once it fits;
//! a slot that goes fades its text out first, then closes. Text too wide for its slot as it
//! widens waits, hidden, until it fits.
//!
//! A slot that doesn't fit the room the bar has for modules is dropped, the last ones first.

use slipstream_config::bar::{Shown, Tint};

use crate::{
    anim::{Easing, Tween},
    panel::{self, AMBER, HOT},
    text::{self, Face, Style},
};

/// Room either side of a module's text, inside the patch the pointer lights up.
pub const PAD: f32 = 10.0;
/// Between two modules' slots.
pub const GAP: f32 = 2.0;
/// The divider between the date and the first module, and the space after it: the clock's own
/// padding is before it.
pub const DIVIDER: f32 = 1.0;
pub const AFTER_DIVIDER: f32 = 4.0;
/// The widest the modules may be, all together, whatever room the bar has.
pub const CAP: f32 = 480.0;
/// How long a slot's text has to stay narrower before the slot narrows to fit it, in seconds.
pub const HOLD: f64 = 3.0;
/// How long a change of width takes, in seconds.
const EASE: f64 = 0.25;
/// How long text takes to fade in or out, in seconds.
const FADE: f64 = 0.15;

/// The style of a module's text in `tint`, at `alpha`.
pub fn style(tint: Tint, alpha: f32) -> Style {
    let ink = match tint {
        Tint::Plain => panel::INK,
        Tint::Warning => AMBER,
        Tint::Critical => HOT,
    };
    let alpha = ((ink & 0xff) as f32 * alpha.clamp(0.0, 1.0)).round() as u32;
    Style {
        tabular: true,
        ..Style::new(Face::Body, 14.0, (ink & !0xff) | alpha)
    }
}

/// A module's slot as the bar paints it, this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub id: u64,
    /// The text, already cut to fit.
    pub text: String,
    pub tint: Tint,
    /// The slot's width now, padding included, in the bar's units.
    pub width: f32,
    /// How visible its text is: it fades in and out with its slot.
    pub alpha: f32,
    /// The pointer is on it.
    pub hovered: bool,
    /// It didn't fit last time: it isn't drawn, and its slot is closing.
    pub crowded: bool,
}

/// One module's slot from frame to frame.
#[derive(Debug, Clone)]
struct Slot {
    id: u64,
    text: String,
    tint: Tint,
    /// The text's width, and the width kept for it.
    text_w: f32,
    kept: f32,
    /// Since when its text has been narrower than `kept`, and the widest it has been since.
    narrower: Option<(f64, f32)>,
    width: Tween<1>,
    alpha: Tween<1>,
    /// Its module has nothing to show any more: the slot closes, then goes.
    leaving: bool,
    crowded: bool,
}

/// Every module's slot, for one bar.
#[derive(Debug, Default)]
pub struct Slots {
    slots: Vec<Slot>,
}

impl Slots {
    /// The slots for `shown` at `now` (wall seconds), given the `room` the bar had for modules
    /// when it was last painted, with the module `hovered` lit.
    pub fn place(
        &mut self,
        shown: &[(u64, Shown)],
        hovered: Option<u64>,
        room: f32,
        now: f64,
        reduced: bool,
    ) -> Vec<Placed> {
        let (ease, fade) = if reduced { (0.0, 0.0) } else { (EASE, FADE) };
        let widest_text = (room.min(CAP) - DIVIDER - AFTER_DIVIDER - footprint(2.0 * PAD)).max(0.0);
        // Those that have gone close where they were; the rest follow the modules' order.
        let mut slots: Vec<Slot> = Vec::with_capacity(shown.len());
        let mut old = std::mem::take(&mut self.slots).into_iter().peekable();
        for (id, module) in shown {
            while let Some(gone) = old.next_if(|slot| !shown.iter().any(|(id, _)| *id == slot.id)) {
                slots.push(gone);
            }
            let mut slot = match old.next_if(|slot| slot.id == *id) {
                Some(slot) => slot,
                None => match take(&mut old, *id) {
                    Some(slot) => slot,
                    None => Slot {
                        id: *id,
                        text: String::new(),
                        tint: module.tint,
                        text_w: 0.0,
                        kept: 0.0,
                        narrower: None,
                        width: Tween::at_rest([0.0]),
                        alpha: Tween::at_rest([0.0]),
                        leaving: false,
                        crowded: false,
                    },
                },
            };
            let style = style(module.tint, 1.0);
            slot.text = text::ellipsize(&module.text, &style, widest_text);
            slot.tint = module.tint;
            slot.leaving = false;
            slot.text_w = text::width(&slot.text, &style);
            slot.keep(slot.text_w, now);
            slots.push(slot);
        }
        slots.extend(old);

        // Room for each in turn, until one doesn't fit: it and every one after it are dropped.
        let mut used = DIVIDER + AFTER_DIVIDER;
        let mut fits = true;
        for slot in &mut slots {
            let wanted = if slot.leaving {
                0.0
            } else {
                slot.kept + 2.0 * PAD
            };
            if wanted > 0.0 {
                fits = fits && used + footprint(wanted) <= room.min(CAP);
                used += footprint(wanted);
            }
            slot.crowded = !fits && !slot.leaving;
            if slot.leaving {
                // Its fade and its closing were set going when it left.
                continue;
            }
            let width = if slot.crowded { 0.0 } else { wanted };
            slot.width
                .retarget([width as f64], now, ease, Easing::OutCubic);
            // The text shows only while it fits its slot as the slot is now.
            let fits_now = slot.text_w <= slot.width.value(now)[0] as f32 + 0.5;
            if slot.crowded || !fits_now {
                slot.alpha = Tween::at_rest([0.0]);
            } else {
                slot.alpha.retarget([1.0], now, fade, Easing::OutCubic);
            }
        }
        slots.retain(|slot| !(slot.leaving && slot.width.done(now)));
        // Anything gone this time has nothing to show: its text fades, then its slot closes, and
        // once it has closed it is taken out.
        for slot in &mut slots {
            if !shown.iter().any(|(id, _)| *id == slot.id) && !slot.leaving {
                slot.leaving = true;
                let visible = slot.alpha.value(now)[0] > 0.01;
                slot.alpha.retarget([0.0], now, fade, Easing::OutCubic);
                slot.width = Tween {
                    from: slot.width.value(now),
                    to: [0.0],
                    start: now + if visible { fade } else { 0.0 },
                    duration: ease,
                    easing: Easing::OutCubic,
                };
            }
        }
        self.slots = slots;
        self.slots
            .iter()
            .map(|slot| Placed {
                id: slot.id,
                text: slot.text.clone(),
                tint: slot.tint,
                width: slot.width.value(now)[0] as f32,
                alpha: slot.alpha.value(now)[0] as f32,
                hovered: hovered == Some(slot.id) && !slot.leaving,
                crowded: slot.crowded,
            })
            .collect()
    }
}

/// The slot with `id` further along `old`, taken out of it: a module moved in the settings.
fn take(old: &mut std::iter::Peekable<std::vec::IntoIter<Slot>>, id: u64) -> Option<Slot> {
    let rest: Vec<Slot> = old.collect();
    let (found, rest): (Vec<Slot>, Vec<Slot>) = rest.into_iter().partition(|slot| slot.id == id);
    *old = rest.into_iter().peekable();
    found.into_iter().next()
}

impl Slot {
    /// The text is now `width` wide: the slot widens at once, and narrows only once the text has
    /// stayed narrower for `HOLD` seconds, to the widest it has been in that time.
    fn keep(&mut self, width: f32, now: f64) {
        if width >= self.kept {
            self.kept = width;
            self.narrower = None;
            return;
        }
        let (since, widest) = match self.narrower {
            Some((since, widest)) => (since, widest.max(width)),
            None => (now, width),
        };
        if now - since >= HOLD {
            self.kept = widest;
            self.narrower = None;
        } else {
            self.narrower = Some((since, widest));
        }
    }
}

/// How much of the block a slot `width` wide takes: itself, and the gap after it, which opens and
/// closes with it.
pub fn footprint(width: f32) -> f32 {
    width + GAP * (width / (2.0 * PAD)).min(1.0)
}

/// The divider and the space after it, before the slots, which come in with the first of them
/// rather than all at once.
pub fn lead(placed: &[Placed]) -> f32 {
    let open: f32 = placed.iter().map(|slot| slot.width).sum();
    (DIVIDER + AFTER_DIVIDER) * (open / (2.0 * PAD)).min(1.0)
}

/// How wide the modules are together, divider included, for the slots `placed`: nothing when
/// none is open at all, so a bar with no modules is exactly the bar it always was.
pub fn width(placed: &[Placed]) -> f32 {
    let slots: f32 = placed.iter().map(|slot| footprint(slot.width)).sum();
    if slots <= 0.01 {
        return 0.0;
    }
    lead(placed) + slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str) -> Shown {
        Shown {
            text: text.into(),
            ..Shown::default()
        }
    }

    fn widths(placed: &[Placed]) -> Vec<f32> {
        placed.iter().map(|slot| slot.width).collect()
    }

    const ROOM: f32 = 400.0;

    #[test]
    fn a_slot_widens_at_once_and_narrows_only_after_the_text_has_stayed_narrow() {
        let mut slots = Slots::default();
        let place = |slots: &mut Slots, text: &str, now: f64| {
            slots.place(&[(1, shown(text))], None, ROOM, now, true)[0].width
        };
        let wide = place(&mut slots, "100", 0.0);
        assert_eq!(place(&mut slots, "99", 1.0), wide, "kept wide");
        assert_eq!(place(&mut slots, "9", 2.0), wide);
        assert_eq!(place(&mut slots, "8", 3.9), wide, "not yet");
        let narrower = place(&mut slots, "7", 4.1);
        assert!(narrower < wide, "after three seconds of narrower text");
        assert_eq!(
            narrower,
            text::width("99", &style(Tint::Plain, 1.0)) + 2.0 * PAD,
            "to the widest it was in that time"
        );
        assert!(place(&mut slots, "12345", 4.2) > wide, "wider at once");
    }

    #[test]
    fn a_number_that_only_changes_its_digits_never_moves_anything() {
        let mut slots = Slots::default();
        let first = slots.place(&[(1, shown("42%"))], None, ROOM, 0.0, false);
        let settled = widths(&slots.place(&[(1, shown("42%"))], None, ROOM, 1.0, false));
        for (n, now) in (10..99).zip((0..).map(|i| 1.0 + i as f64 * 0.1)) {
            let placed = slots.place(&[(1, shown(&format!("{n}%")))], None, ROOM, now, false);
            assert_eq!(widths(&placed), settled, "{n}");
        }
        assert!(first[0].width < settled[0], "it opened, easing");
    }

    #[test]
    fn a_new_slot_opens_and_a_gone_one_closes_with_reduced_motion_jumping() {
        let mut slots = Slots::default();
        let open = slots.place(&[(1, shown("Mail"))], None, ROOM, 0.0, false);
        assert_eq!((open[0].width, open[0].alpha), (0.0, 0.0));
        let half = slots.place(&[(1, shown("Mail"))], None, ROOM, EASE / 2.0, false);
        assert!(half[0].width > 0.0 && half[0].alpha < 1.0);
        let full = slots
            .place(&[(1, shown("Mail"))], None, ROOM, 1.0, false)
            .remove(0);
        assert_eq!(full.alpha, 1.0, "faded in once it fitted");
        let closing = slots.place(&[], None, ROOM, 1.0, false);
        assert_eq!(closing.len(), 1, "still there, closing");
        let fading = slots.place(&[], None, ROOM, 1.0 + FADE / 2.0, false);
        assert_eq!(
            fading[0].width, full.width,
            "the text fades before the slot closes"
        );
        assert!(fading[0].alpha < 1.0 && fading[0].alpha > 0.0);
        let later = slots.place(&[], None, ROOM, 1.0 + FADE + EASE / 2.0, false);
        assert!(later[0].width < full.width && later[0].text == "Mail");
        assert!(slots.place(&[], None, ROOM, 2.0, false).is_empty(), "gone");
        let full = full.width;

        let mut reduced = Slots::default();
        let at_once = reduced.place(&[(1, shown("Mail"))], None, ROOM, 0.0, true);
        assert_eq!((at_once[0].width, at_once[0].alpha), (full, 1.0));
        reduced.place(&[], None, ROOM, 0.0, true);
        assert!(reduced.place(&[], None, ROOM, 0.0, true).is_empty());
    }

    #[test]
    fn slots_that_dont_fit_are_dropped_from_the_end() {
        let mut slots = Slots::default();
        let three: Vec<(u64, Shown)> = (1..=3)
            .map(|id| (id, shown("A module of some width")))
            .collect();
        let one_wide = text::width("A module of some width", &style(Tint::Plain, 1.0)) + 2.0 * PAD;
        let room = DIVIDER + AFTER_DIVIDER + 2.0 * footprint(one_wide) + 1.0;
        let placed = slots.place(&three, None, room, 0.0, true);
        assert_eq!(
            placed.iter().map(|slot| slot.crowded).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(width(&placed) <= room);
        // A bar with more room than the cap still keeps to it.
        let many: Vec<(u64, Shown)> = (1..=10)
            .map(|id| (id, shown("A module of some width")))
            .collect();
        let placed = Slots::default().place(&many, None, 5000.0, 0.0, true);
        assert!(width(&placed) <= CAP);
        // One far too long for the room is cut short to fit it, rather than dropped.
        let long = [(1, shown(&"word ".repeat(30)))];
        let placed = Slots::default().place(&long, None, 200.0, 0.0, true);
        assert!(!placed[0].crowded && placed[0].text.ends_with('…'));
        assert!(width(&placed) <= 200.0);
    }

    #[test]
    fn a_module_s_text_never_spills_out_of_its_slot() {
        let mut slots = Slots::default();
        let script: [&[(u64, &str)]; 5] = [
            &[(1, "9")],
            &[(1, "Downloading the whole thing: 45%")],
            &[(2, "new"), (1, "Downloading the whole thing: 45%")],
            &[(2, "new")],
            &[],
        ];
        let mut now = 0.0;
        for step in script {
            let shown: Vec<(u64, Shown)> =
                step.iter().map(|(id, text)| (*id, shown(text))).collect();
            for _ in 0..60 {
                for placed in slots.place(&shown, None, ROOM, now, false) {
                    let text_w = text::width(&placed.text, &style(placed.tint, 1.0));
                    assert!(
                        placed.alpha <= 0.0 || text_w <= placed.width + 0.5,
                        "{placed:?} at {now}"
                    );
                }
                now += 1.0 / 60.0;
            }
        }
    }

    #[test]
    fn no_slots_take_no_room_at_all() {
        assert_eq!(width(&[]), 0.0);
        let mut slots = Slots::default();
        assert!(slots.place(&[], None, ROOM, 0.0, false).is_empty());
    }

    #[test]
    fn a_class_tints_the_text_and_nothing_else() {
        assert_eq!(style(Tint::Warning, 1.0).color, AMBER.to_be_bytes());
        assert_eq!(style(Tint::Critical, 1.0).color, HOT.to_be_bytes());
        assert_eq!(style(Tint::Plain, 1.0).color, panel::INK.to_be_bytes());
        for tint in [Tint::Plain, Tint::Warning, Tint::Critical] {
            let style = style(tint, 1.0);
            assert_eq!((style.face, style.px), (Face::Body, 14.0));
        }
        assert_eq!(style(Tint::Plain, 0.0).color[3], 0);
    }

    #[test]
    fn the_pointer_lights_the_slot_it_is_on() {
        let mut slots = Slots::default();
        let two = [(1, shown("a")), (2, shown("b"))];
        let placed = slots.place(&two, Some(2), ROOM, 0.0, true);
        assert_eq!(
            placed.iter().map(|slot| slot.hovered).collect::<Vec<_>>(),
            [false, true]
        );
    }
}
