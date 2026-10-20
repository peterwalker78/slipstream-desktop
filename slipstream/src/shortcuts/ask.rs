//! The card an app's shortcuts wait behind: "Let Talk use these keys?"
//!
//! One row for each shortcut the app asked for, with the keys it would like. The arrow keys choose
//! a row, and pressing other keys gives the chosen one those keys instead, so picking your own
//! takes nothing new learnt; Backspace leaves it out. Enter allows, Esc doesn't, as on every card.
//!
//! It appears by itself, often while someone is typing, so it ignores keys for its first moment.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
};

use crate::{
    card::{self, Btn, Button, Card},
    exit,
    keys::Mods,
    panel::{self},
};

use super::trigger::Trigger;

/// Keys are ignored this long after the card appears.
const DEAF: f64 = 1.5;
/// Rows shown at once; the arrow keys scroll the rest.
const VISIBLE: usize = 7;

/// What a key or a click came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    Allow,
    Deny,
    /// Keys pressed for the chosen row.
    Pick(Trigger),
}

/// One shortcut on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub description: String,
    pub trigger: Option<Trigger>,
}

pub struct Ask {
    pub app: String,
    pub rows: Vec<Row>,
    selected: usize,
    top: usize,
    /// Why the keys last pressed can't be used, until other keys are.
    refusal: Option<String>,
    since: f64,
    pub reduced_motion: bool,
    on_screen: card::OnScreen,
}

impl Ask {
    pub fn new(app: String, rows: Vec<Row>, now: f64, reduced_motion: bool) -> Self {
        Self {
            app,
            rows,
            selected: 0,
            top: 0,
            refusal: None,
            since: now,
            reduced_motion,
            on_screen: Default::default(),
        }
    }

    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.on_screen.forget();
    }

    /// A key while the card is up: `key` is the unshifted symbol, as shortcuts are matched.
    pub fn key(&mut self, key: Keysym, mods: Mods, now: f64) -> Act {
        if now - self.since < DEAF || key.is_modifier_key() {
            return Act::Nothing;
        }
        let plain = mods == Mods::default();
        match key {
            Keysym::Return | Keysym::KP_Enter if plain => Act::Allow,
            Keysym::Escape if plain => Act::Deny,
            Keysym::Up if plain => self.step(-1),
            Keysym::Down | Keysym::Tab if plain => self.step(1),
            Keysym::BackSpace | Keysym::Delete if plain => {
                if let Some(row) = self.rows.get_mut(self.selected) {
                    row.trigger = None;
                }
                self.refusal = None;
                Act::Nothing
            }
            _ => Act::Pick(Trigger::from_press(mods, key)),
        }
    }

    fn step(&mut self, by: isize) -> Act {
        let count = self.rows.len() as isize;
        if count > 0 {
            self.select((self.selected as isize + by).rem_euclid(count) as usize);
        }
        Act::Nothing
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + VISIBLE {
            self.top = self.selected + 1 - VISIBLE;
        }
    }

    /// The chosen row gets these keys. A row that had them already gives them up: two
    /// shortcuts can't share keys.
    pub fn set(&mut self, trigger: Trigger) {
        for row in &mut self.rows {
            if row.trigger == Some(trigger) {
                row.trigger = None;
            }
        }
        if let Some(row) = self.rows.get_mut(self.selected) {
            row.trigger = Some(trigger);
        }
        self.refusal = None;
    }

    /// The keys pressed can't be used, and this says why.
    pub fn refuse(&mut self, why: String) {
        self.refusal = Some(why);
    }

    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        self.on_screen.hover(x, y)
    }

    /// A click on a row chooses it; the buttons answer. Beside the card nothing happens.
    pub fn click(&mut self, x: f64, y: f64, now: f64) -> Act {
        if now - self.since < DEAF {
            return Act::Nothing;
        }
        match self.on_screen.hit(x, y) {
            Some(Button::Go) => Act::Allow,
            Some(Button::Stay) => Act::Deny,
            Some(Button::Row(row)) => {
                self.select(self.top + row);
                Act::Nothing
            }
            _ => Act::Nothing,
        }
    }

    fn card(&self, ring: u32) -> Card {
        let app = &self.app;
        let note = self.refusal.clone().unwrap_or_else(|| {
            format!(
                "They work from any window, and only {app} hears them, as a press and a release: \
                 never the window you're in, and nothing else you type. To change one, choose it \
                 and press the keys you want. Backspace leaves it out."
            )
        });
        let end = (self.top + VISIBLE).min(self.rows.len());
        let hidden = self.rows.len() - (end - self.top);
        Card {
            title: format!("Let {app} use these keys?"),
            note,
            rows: self.rows[self.top..end]
                .iter()
                .map(|row| card::Row {
                    name: if row.description.is_empty() {
                        row.id.clone()
                    } else {
                        row.description.clone()
                    },
                    title: String::new(),
                    note: Some(match row.trigger {
                        Some(trigger) => (trigger.label(), panel::AMBER),
                        None => ("left out".to_string(), panel::HINT),
                    }),
                })
                .collect(),
            more: (hidden > 0).then(|| format!("{hidden} more — arrow keys scroll")),
            field: None,
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Don't allow".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: "Allow".to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.on_screen.hovered,
            selected: Some((self.selected - self.top, ring)),
        }
    }

    /// The card on a screen `screen` logical pixels big.
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        frame: crate::panel::Frame,
        ring: u32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (_screen, _scale, now) = (frame.size, frame.scale, frame.time);
        let card = self.card(ring);
        let (alpha, rise) = exit::opening(now - self.since, self.reduced_motion);
        self.on_screen.element(renderer, card, frame, (alpha, rise))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: Mods = Mods {
        ctrl: true,
        alt: true,
        shift: false,
        logo: false,
    };

    fn ask() -> Ask {
        let row = |id: &str, trigger: Option<&str>| Row {
            id: id.to_string(),
            description: String::new(),
            trigger: trigger.and_then(Trigger::parse),
        };
        Ask::new(
            "Talk".into(),
            vec![row("talk", Some("CTRL+ALT+t")), row("mute", None)],
            0.0,
            false,
        )
    }

    #[test]
    fn keys_typed_as_it_appears_answer_nothing() {
        let mut ask = ask();
        assert_eq!(ask.key(Keysym::Return, Mods::default(), 0.5), Act::Nothing);
        assert_eq!(ask.key(Keysym::Return, Mods::default(), 2.0), Act::Allow);
        assert_eq!(ask.key(Keysym::Escape, Mods::default(), 2.0), Act::Deny);
    }

    #[test]
    fn pressed_keys_go_to_the_chosen_row() {
        let mut ask = ask();
        assert_eq!(ask.key(Keysym::Down, Mods::default(), 2.0), Act::Nothing);
        let Act::Pick(trigger) = ask.key(Keysym::M, CTRL_ALT, 2.0) else {
            panic!("Ctrl+Alt+M wasn't picked");
        };
        assert_eq!(trigger.spec(), "CTRL+ALT+m");
        ask.set(trigger);
        assert_eq!(ask.rows[1].trigger, Some(trigger));
        // Held keys on their own pick nothing.
        assert_eq!(ask.key(Keysym::Control_L, CTRL_ALT, 2.0), Act::Nothing);
    }

    #[test]
    fn two_shortcuts_never_share_keys() {
        let mut ask = ask();
        let talk = ask.rows[0].trigger.unwrap();
        ask.key(Keysym::Down, Mods::default(), 2.0);
        ask.set(talk);
        assert_eq!(ask.rows[0].trigger, None);
        assert_eq!(ask.rows[1].trigger, Some(talk));
    }

    #[test]
    fn backspace_leaves_a_shortcut_out() {
        let mut ask = ask();
        ask.key(Keysym::BackSpace, Mods::default(), 2.0);
        assert_eq!(ask.rows[0].trigger, None);
        assert_eq!(ask.card(0).rows[0].note.as_ref().unwrap().0, "left out");
    }

    #[test]
    fn a_refusal_says_why_until_other_keys_are_pressed() {
        let mut ask = ask();
        ask.refuse("Super+L is one of Slipstream's own shortcuts.".into());
        assert!(ask.card(0).note.starts_with("Super+L"));
        ask.set(Trigger::parse("CTRL+ALT+x").unwrap());
        assert!(ask.card(0).note.starts_with("They work"));
    }
}
