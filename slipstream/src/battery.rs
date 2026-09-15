//! Running low on battery, in three steps as the charge falls while unplugged; and a card low on
//! the screen whenever a charger is plugged in or pulled out.
//!
//! - **20%:** the living wallpaper slows to half speed, stepping half as often, and a toast says
//!   that's to save power. It comes back to speed on the charger.
//! - **10%:** a toast says the battery is low and how long is left.
//! - **5%:** a card counts down a minute to sleep, so an unsaved document survives in memory rather
//!   than dying with the battery. Enter sleeps now, Esc holds it off until 2% lower; plugging in
//!   takes it away. While locked there's nobody to ask, so the laptop sleeps straight away.
//!
//! The card takes every key, as the other cards do, but ignores them for its first moment, so a
//! key already on its way when it appears can't answer it.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

use crate::{
    Slipstream,
    card::{self, Btn, Button, Card, Hit},
    paint::Painted,
    panel::{self, MOCKUP_PX},
};

/// Where each step starts, in percent.
pub const SAVING: u8 = 20;
pub const LOW: u8 = 10;
pub const CRITICAL: u8 = 5;
/// How long the card counts down before the laptop sleeps.
const COUNTDOWN: f64 = 60.0;
/// Keys are ignored this long after the card appears.
const DEAF: f64 = 1.5;
/// Holding the card off keeps it away until the charge falls this much further.
const HOLD_OFF: u8 = 2;
/// The wallpaper's speed while saving power.
pub const SAVING_PACE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Fine,
    Saving,
    Low,
    Critical,
}

/// The stage for a battery reading: percent and whether it's charging. No battery, or charging,
/// is always fine.
pub fn stage(reading: Option<(u8, bool)>) -> Stage {
    match reading {
        Some((percent, false)) if percent <= CRITICAL => Stage::Critical,
        Some((percent, false)) if percent <= LOW => Stage::Low,
        Some((percent, false)) if percent <= SAVING => Stage::Saving,
        _ => Stage::Fine,
    }
}

/// What a key or a click on the card did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    SleepNow,
    NotYet,
}

#[derive(Default)]
pub struct Battery {
    last: Option<Stage>,
    /// Whether a charger was plugged in at the last reading.
    plugged: Option<bool>,
    /// The card is held off until the charge is at or below this.
    held_off_until: Option<u8>,
    /// A reading to use instead of the machine's, for the `battery:` debug step.
    pub pinned: Option<(u8, bool)>,
    /// Sleep has been asked for at this charge, so it isn't asked again every pass.
    slept_at: Option<u8>,
    pub card: Option<LowCard>,
}

/// The countdown card.
pub struct LowCard {
    since: f64,
    percent: u8,
    left: Option<String>,
    pub reduced_motion: bool,
    hovered: Option<Button>,
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Card, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
}

impl LowCard {
    fn new(percent: u8, left: Option<String>, now: f64, reduced_motion: bool) -> Self {
        Self {
            since: now,
            percent,
            left,
            reduced_motion,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }

    /// Whole seconds until it sleeps.
    fn seconds_left(&self, now: f64) -> u64 {
        (COUNTDOWN - (now - self.since)).ceil().max(0.0) as u64
    }

    fn due(&self, now: f64) -> bool {
        now - self.since >= COUNTDOWN
    }

    pub fn key(&self, sym: Keysym, now: f64) -> Act {
        if now - self.since < DEAF {
            return Act::Nothing;
        }
        match sym {
            Keysym::Return | Keysym::KP_Enter => Act::SleepNow,
            Keysym::Escape => Act::NotYet,
            _ => Act::Nothing,
        }
    }

    pub fn hover(&mut self, x: f64, y: f64) {
        self.hovered = self.hit(x, y);
    }

    pub fn click(&self, x: f64, y: f64) -> Act {
        match self.hit(x, y) {
            Some(Button::Go) => Act::SleepNow,
            Some(Button::Stay) => Act::NotYet,
            _ => Act::Nothing,
        }
    }

    fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let px = MOCKUP_PX as f64;
        let (lx, ly) = (((x - self.at.x) / px) as f32, ((y - self.at.y) / px) as f32);
        self.hits
            .iter()
            .find(|hit| hit.contains(lx, ly))
            .map(|hit| hit.which)
    }

    fn card(&self, now: f64) -> Card {
        let seconds = self.seconds_left(now);
        let left = self
            .left
            .as_ref()
            .map_or(String::new(), |left| format!(" ({left})"));
        Card {
            title: format!("Battery at {}%", self.percent),
            note: format!(
                "Sleeping in {seconds} s{left}, so what you have open is kept safe in memory. \
                 Plug in to carry on."
            ),
            rows: Vec::new(),
            more: None,
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Not yet".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: "Sleep now".to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.hovered,
            selected: None,
        }
    }

    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let card = self.card(now);
        let fresh = self
            .shown
            .as_ref()
            .is_some_and(|(shown, size, at)| *shown == card && *size == screen && *at == scale);
        if !fresh {
            let (painted, hits) = card::paint(&card, scale);
            self.hits = hits;
            self.painted = painted;
            self.shown = Some((card, screen, scale));
        }
        let painted = self.painted.as_ref()?;
        self.at = Point::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        let (alpha, rise) = panel::opening(now - self.since, self.reduced_motion);
        let at = self.at + Point::from((0.0, rise * MOCKUP_PX as f64));
        painted.element(renderer, at, alpha)
    }
}

impl Slipstream {
    /// Once a pass of the event loop: the battery's reading decides the wallpaper's pace, the
    /// warnings, and the card.
    pub fn tick_battery(&mut self) {
        let (reading, left) = {
            let status = self.status.lock().unwrap();
            // Only a time that counts down: "until full" belongs to a charger, whatever the
            // pinned reading says.
            let left = status
                .battery_time
                .clone()
                .filter(|time| time.ends_with("left"));
            (status.battery, left)
        };
        let reading = self.battery.pinned.or(reading);
        let stage = stage(reading);
        let percent = reading.map_or(100, |(percent, _)| percent);
        let now = self.wall();

        let pace = if stage >= Stage::Saving {
            SAVING_PACE
        } else {
            1.0
        };
        for saver in self.savers.values_mut() {
            saver.pace = pace;
        }

        // Said on the change, not when a session starts on the charger.
        if let Some((percent, plugged)) = reading
            && self
                .battery
                .plugged
                .replace(plugged)
                .is_some_and(|was| was != plugged)
        {
            tracing::info!(percent, plugged, "charger plugged in or pulled out");
            self.show_osd(crate::osd::Kind::Power { plugged }, percent);
        }

        let before = self.battery.last.replace(stage);
        // Told once on the way down, and not at all when a session starts already low: the bar's
        // amber percentage says that.
        if let Some(before) = before
            && stage > before
        {
            match stage {
                Stage::Saving => self.show_toast(
                    &format!("Battery at {percent}%"),
                    "The wallpaper slows down to save power.",
                ),
                Stage::Low => {
                    let left = left.clone().unwrap_or_else(|| "plug in soon".to_string());
                    self.show_toast(&format!("Battery low · {percent}%"), &left);
                }
                _ => {}
            }
        }

        if stage < Stage::Critical {
            self.battery.card = None;
            self.battery.slept_at = None;
            if stage == Stage::Fine {
                self.battery.held_off_until = None;
            }
            return;
        }
        let held_off = self
            .battery
            .held_off_until
            .is_some_and(|until| percent > until);
        if self.lock.is_some() {
            // Nobody to ask: sleep, once for this charge.
            self.battery.card = None;
            if !held_off && self.battery.slept_at != Some(percent) {
                self.battery.slept_at = Some(percent);
                tracing::info!(percent, "battery critical while locked: sleeping");
                self.request_power(crate::power::Power::Sleep);
            }
            return;
        }
        if held_off {
            return;
        }
        match self.battery.card.as_ref() {
            None if self.battery.slept_at != Some(percent) => {
                tracing::info!(percent, "battery critical: counting down to sleep");
                self.close_panels();
                self.battery.card =
                    Some(LowCard::new(percent, left, now, self.clock.reduced_motion));
            }
            Some(card) if card.due(now) => {
                self.battery.card = None;
                self.battery.slept_at = Some(percent);
                tracing::info!(percent, "battery critical: sleeping");
                self.request_power(crate::power::Power::Sleep);
            }
            _ => {}
        }
    }

    pub fn battery_key(&mut self, sym: Keysym) {
        let now = self.wall();
        let act = self
            .battery
            .card
            .as_ref()
            .map_or(Act::Nothing, |card| card.key(sym, now));
        self.act_on_battery(act);
    }

    pub fn battery_click(&mut self, x: f64, y: f64) {
        let act = self
            .battery
            .card
            .as_ref()
            .map_or(Act::Nothing, |card| card.click(x, y));
        self.act_on_battery(act);
    }

    pub fn battery_hover(&mut self, pos: Point<f64, Logical>) {
        let Some(screen) = self.focused_screen_geometry() else {
            return;
        };
        let pos = pos - screen.loc.to_f64();
        if let Some(card) = self.battery.card.as_mut() {
            card.hover(pos.x, pos.y);
        }
    }

    fn act_on_battery(&mut self, act: Act) {
        let Some(card) = self.battery.card.as_ref() else {
            return;
        };
        let percent = card.percent;
        match act {
            Act::Nothing => {}
            Act::SleepNow => {
                self.battery.card = None;
                self.battery.slept_at = Some(percent);
                tracing::info!(percent, "sleeping now, from the battery card");
                self.request_power(crate::power::Power::Sleep);
            }
            Act::NotYet => {
                self.battery.card = None;
                self.battery.held_off_until = Some(percent.saturating_sub(HOLD_OFF));
                tracing::info!(percent, "the battery card was held off");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_follow_the_charge_while_unplugged() {
        assert_eq!(stage(None), Stage::Fine);
        assert_eq!(stage(Some((50, false))), Stage::Fine);
        assert_eq!(stage(Some((20, false))), Stage::Saving);
        assert_eq!(stage(Some((10, false))), Stage::Low);
        assert_eq!(stage(Some((5, false))), Stage::Critical);
        assert_eq!(
            stage(Some((3, true))),
            Stage::Fine,
            "charging is always fine"
        );
    }

    #[test]
    fn a_key_already_on_its_way_cannot_answer_the_card() {
        let card = LowCard::new(5, None, 100.0, false);
        assert_eq!(card.key(Keysym::Return, 100.2), Act::Nothing);
        assert_eq!(card.key(Keysym::Return, 102.0), Act::SleepNow);
        assert_eq!(card.key(Keysym::Escape, 102.0), Act::NotYet);
        assert_eq!(card.key(Keysym::space, 102.0), Act::Nothing);
    }

    #[test]
    fn the_countdown_runs_a_minute() {
        let card = LowCard::new(5, None, 10.0, false);
        assert_eq!(card.seconds_left(10.0), 60);
        assert_eq!(card.seconds_left(40.5), 30);
        assert!(!card.due(69.0));
        assert!(card.due(70.0));
    }
}
