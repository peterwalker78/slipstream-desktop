//! The offer at login: "Reopen 3 windows from last time?"
//!
//! The way out's glass card, the same 180 ms, the same rule — Enter goes on, Esc stays — and the
//! same buttons, so answering it needs nothing new learnt. It says plainly that apps come back and
//! documents don't, and names anything in the record it can't reopen rather than quietly
//! dropping it.
//!
//! Wall time, like the way out: this is drawn while the desktop is still filling up.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

use crate::{
    card::{self, Btn, Button, Card, DIM, Hit, Row},
    motion::HYPR,
    paint::Painted,
    panel::MOCKUP_PX,
};

/// The card opens as the way out's does, and as the explorer does.
const OPEN: f64 = 0.18;
const REDUCED_FADE: f64 = 0.08;
/// Apps named in the card's prose before it stops listing them.
const MAX_NAMED: usize = 6;

/// What the answer came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Enter: put the layout back.
    Reopen,
    /// Esc: an empty desktop, and the record left alone for next time.
    Clean,
}

/// The card, while it's waiting for an answer.
pub struct Offer {
    windows: usize,
    /// The apps it would reopen, named once each.
    apps: Vec<String>,
    /// Apps in the record with no desktop entry here.
    lost: Vec<String>,
    since: f64,
    pub reduced_motion: bool,
    hovered: Option<Button>,
    /// Where the card's corner sits on screen, and what can be clicked on it, from the last paint.
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Card, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
}

impl Offer {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn new(
        windows: usize,
        apps: Vec<String>,
        lost: Vec<String>,
        now: f64,
        reduced_motion: bool,
    ) -> Self {
        Self {
            windows,
            apps,
            lost,
            since: now,
            reduced_motion,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }

    /// A key while the card is up. It takes every key: nothing else should be answered first.
    pub fn key(&mut self, sym: Keysym) -> Act {
        match sym {
            // Enter only, never Space: the card appears by itself at login, and a space typed as
            // it does must not answer it.
            Keysym::Return | Keysym::KP_Enter => Act::Reopen,
            Keysym::Escape => Act::Clean,
            _ => Act::Nothing,
        }
    }

    /// The pointer moved, in logical pixels from the output's corner. Returns whether what it is
    /// over changed, so the card is only repainted when it has something new to show.
    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        let over = self.hit(x, y);
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// A click on the card. Beside it nothing happens: this is a question, and the answers are the
    /// buttons, so a stray click neither reopens nor throws the record away.
    pub fn click(&mut self, x: f64, y: f64) -> Act {
        match self.hit(x, y) {
            Some(Button::Go) => Act::Reopen,
            Some(Button::Stay) => Act::Clean,
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

    /// The card as it stands.
    fn card(&self) -> Card {
        let named = self.apps.len().min(MAX_NAMED);
        let mut list = self.apps[..named].join(", ");
        if self.apps.len() > named {
            list.push_str(&format!(" and {} more", self.apps.len() - named));
        }
        let windows = self.windows;
        let plural = if windows == 1 { "window" } else { "windows" };
        Card {
            title: format!("Reopen {windows} {plural}?"),
            // Said here rather than discovered afterwards: the gap between "my windows came back"
            // and "my work came back" is where the disappointment lives.
            note: format!(
                "{list} — on the workspaces they were on, in the layout they were in. It reopens \
                 the apps, not what they had open."
            ),
            rows: self
                .lost
                .iter()
                .map(|app| Row {
                    name: app.clone(),
                    title: String::new(),
                    note: Some(("no app found".to_string(), DIM)),
                })
                .collect(),
            more: None,
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Start clean".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: "Reopen".to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.hovered,
            selected: None,
        }
    }

    /// The card on a screen `screen` logical pixels big.
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
        let card = self.card();
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
        // Where it settles, which is what a click is measured against: a button that moved under
        // the pointer would be worse than one that waits for it.
        self.at = Point::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        let (alpha, rise) = self.opening(now - self.since);
        let at = self.at + Point::from((0.0, rise * MOCKUP_PX as f64));
        painted.element(renderer, at, alpha)
    }

    /// Its opacity and how far above its place it is, `since` seconds after it appeared.
    fn opening(&self, since: f64) -> (f32, f64) {
        if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
        } else {
            let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
            (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> Offer {
        Offer::new(
            3,
            vec!["Konsole".into(), "Firefox".into()],
            vec!["some-run-box-command".into()],
            0.0,
            false,
        )
    }

    #[test]
    fn enter_reopens_and_esc_starts_clean() {
        let mut offer = offer();
        assert_eq!(offer.key(Keysym::a), Act::Nothing);
        assert_eq!(offer.key(Keysym::Escape), Act::Clean);
        assert_eq!(offer.key(Keysym::Return), Act::Reopen);
    }

    #[test]
    fn the_card_counts_the_windows_and_names_the_apps() {
        let card = offer().card();
        assert_eq!(card.title, "Reopen 3 windows?");
        assert!(card.note.starts_with("Konsole, Firefox — "));
        // What it can't reopen is named rather than quietly dropped.
        assert_eq!(card.rows.len(), 1);
        assert_eq!(card.rows[0].name, "some-run-box-command");
        assert!(
            card.buttons
                .iter()
                .any(|b| b.which == Button::Go && b.primary)
        );
    }

    #[test]
    fn one_window_is_not_windows() {
        let offer = Offer::new(1, vec!["Konsole".into()], vec![], 0.0, false);
        assert_eq!(offer.card().title, "Reopen 1 window?");
    }

    #[test]
    fn a_long_list_of_apps_stops_naming_them() {
        let apps: Vec<String> = (0..9).map(|n| format!("App {n}")).collect();
        let offer = Offer::new(9, apps, vec![], 0.0, false);
        assert!(
            offer
                .card()
                .note
                .starts_with("App 0, App 1, App 2, App 3, App 4, App 5 and 3 more — ")
        );
    }

    #[test]
    fn a_click_beside_the_card_answers_nothing() {
        let mut offer = offer();
        assert_eq!(offer.click(5.0, 5.0), Act::Nothing);
    }
}
