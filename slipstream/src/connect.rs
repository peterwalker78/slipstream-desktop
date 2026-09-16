//! The card the first time a screen is plugged in: what should it show?
//!
//! A screen Slipstream has never seen gets a workspace of its own, so plugging a monitor in never
//! takes a workspace off the screen already in use. That is the right answer for a second screen
//! you want to drag things on to, and the wrong one if you plugged the monitor in to carry on
//! what you were already doing, so the card offers both and remembers the answer against the
//! screen (`known.rs`). It appears once per monitor, ever — not once per dock.
//!
//! It takes every key while it's up, as the other cards do, but ignores them for its first moment:
//! a key already on its way when a screen lights up mustn't answer it.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

use crate::{
    card::{self, Btn, Button, Card, Hit},
    paint::Painted,
    panel::{self, MOCKUP_PX},
};

/// Keys are ignored this long after the card appears.
const DEAF: f64 = 1.5;

/// What a key or a click on the card came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Enter: keep the workspace of its own it already has.
    Own,
    /// Esc: show the workspace it would have taken before, and give the one of its own back.
    Share,
}

/// The card, while it's waiting for an answer.
pub struct Connect {
    /// What the screen is known by, which is what the answer is remembered against.
    pub monitor: String,
    /// The connector it came up on, for the title.
    pub connector: String,
    /// The workspace of its own it has been given.
    pub own: usize,
    /// What it is called on the bar.
    own_label: String,
    /// The workspace it would have taken under the old rule, and that name, when there is one
    /// free. Without one the card only confirms what it did.
    pub share: Option<usize>,
    share_label: String,
    since: f64,
    pub reduced_motion: bool,
    hovered: Option<Button>,
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Card, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
}

impl Connect {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        monitor: String,
        connector: String,
        own: usize,
        own_label: String,
        share: Option<(usize, String)>,
        now: f64,
        reduced_motion: bool,
    ) -> Self {
        let (share, share_label) = match share {
            Some((index, label)) => (Some(index), label),
            None => (None, String::new()),
        };
        Self {
            monitor,
            connector,
            own,
            own_label,
            share,
            share_label,
            since: now,
            reduced_motion,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }

    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn key(&self, sym: Keysym, now: f64) -> Act {
        if now - self.since < DEAF {
            return Act::Nothing;
        }
        match sym {
            // Enter only, never Space: the card appears by itself when a lead is pushed in, and a
            // space typed as it does must not answer it.
            Keysym::Return | Keysym::KP_Enter => Act::Own,
            Keysym::Escape if self.share.is_some() => Act::Share,
            // With nothing free to share there is only one answer, and Esc takes it.
            Keysym::Escape => Act::Own,
            _ => Act::Nothing,
        }
    }

    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        let over = self.hit(x, y);
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// A click on the card. Beside it nothing happens: this is a question, and the answers are the
    /// buttons.
    pub fn click(&self, x: f64, y: f64) -> Act {
        match self.hit(x, y) {
            Some(Button::Go) => Act::Own,
            Some(Button::Stay) if self.share.is_some() => Act::Share,
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

    fn card(&self) -> Card {
        let title = if self.connector.is_empty() {
            "Screen connected".to_string()
        } else {
            format!("{} connected", self.connector)
        };
        let note = match self.share.is_some() {
            // Said plainly, because the difference only shows up later: one answer leaves the
            // numbered workspaces alone, the other spends one of them on this screen.
            true => format!(
                "{} — showing {}, a workspace of its own, so your numbered workspaces stay on the \
                 screen you're using. Or it can show {} instead, which then belongs to this \
                 screen.",
                self.monitor, self.own_label, self.share_label
            ),
            false => format!(
                "{} — showing {}, a workspace of its own. Super+P moves the keyboard to it, and \
                 Super+Shift+P sends a window there.",
                self.monitor, self.own_label
            ),
        };
        let mut buttons = Vec::new();
        if self.share.is_some() {
            buttons.push(Btn {
                which: Button::Stay,
                // Capitalised where it's a name, and "Show workspace 2" where it's a number.
                label: format!("Show {}", self.share_label),
                key: "ESC".to_string(),
                primary: false,
            });
        }
        buttons.push(Btn {
            which: Button::Go,
            label: "A workspace of its own".to_string(),
            key: "⏎".to_string(),
            primary: true,
        });
        Card {
            title,
            note,
            rows: Vec::new(),
            more: None,
            remember: None,
            buttons,
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
        self.at = Point::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        let (alpha, rise) = panel::opening(now - self.since, self.reduced_motion);
        let at = self.at + Point::from((0.0, rise * MOCKUP_PX as f64));
        painted.element(renderer, at, alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(share: Option<(usize, String)>) -> Connect {
        Connect::new(
            "Dell U2720Q".into(),
            "DP-1".into(),
            5,
            "Workspace 6".into(),
            share,
            0.0,
            true,
        )
    }

    #[test]
    fn a_key_on_its_way_when_the_screen_lights_up_does_not_answer_it() {
        let card = card(Some((1, "Workspace 2".into())));
        assert_eq!(card.key(Keysym::Return, 0.5), Act::Nothing);
        assert_eq!(card.key(Keysym::Return, DEAF + 0.1), Act::Own);
    }

    #[test]
    fn enter_keeps_the_workspace_of_its_own_and_esc_shares_one() {
        let card = card(Some((1, "Workspace 2".into())));
        let now = DEAF + 0.1;
        assert_eq!(card.key(Keysym::Return, now), Act::Own);
        assert_eq!(card.key(Keysym::Escape, now), Act::Share);
        assert_eq!(card.key(Keysym::a, now), Act::Nothing);
    }

    #[test]
    fn with_no_workspace_free_to_share_both_keys_keep_its_own() {
        let card = card(None);
        let now = DEAF + 0.1;
        assert_eq!(card.key(Keysym::Return, now), Act::Own);
        assert_eq!(
            card.key(Keysym::Escape, now),
            Act::Own,
            "nothing else to do"
        );
    }

    #[test]
    fn the_card_offers_one_answer_when_there_is_nothing_to_share() {
        assert_eq!(card(None).card().buttons.len(), 1);
        assert_eq!(
            card(Some((1, "Workspace 2".into()))).card().buttons.len(),
            2
        );
    }
}
