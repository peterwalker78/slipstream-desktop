//! The password card: a program asking for permission to change the system, on the desktop's
//! own glass.
//!
//! It says what the program wants to do, in polkit's words, who is being asked for (a choice when
//! more than one person could answer), and has a box for the password. The rest of the desktop is
//! shaded behind it, the bar included, which a window drawing a lookalike can't do. Every key goes
//! to the card while it is up, and no app has the keyboard.

use crate::motion::REDUCED_FADE;
use smithay::{
    backend::renderer::{
        Color32F, ImportMem, Renderer,
        element::{
            Kind,
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
        },
    },
    input::keyboard::{Keysym, ModifiersState},
    utils::{Logical, Point, Size},
};
use zeroize::Zeroizing;

use super::{Identity, Request, helper::Verdict};
use crate::{
    card::{self, Btn, Button, Card, Field, Hit, Row},
    lock::{self, Password},
    motion::HYPR,
    paint::Painted,
    panel::DESIGN_PX,
};

const OPEN: f64 = 0.18;

/// How long the card ignores keys and clicks after it appears. It can come up while something
/// else is being typed, and the end of that, Enter included, must not become a password attempt.
const DEAF: f64 = 0.5;

/// The shade over the rest of the desktop.
const SHADE: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.45);

/// The most dots shown, however long the password.
const DOTS_SHOWN: usize = 32;

/// A key, as the card reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Typing, erasing, clearing (Ctrl+U) or Enter.
    Field(lock::Key),
    Cancel,
    Next,
    Previous,
}

/// The card's meaning of a key: `latin` is the key's Latin keysym, `sym` what the layout makes of
/// it, as for the lock's password.
pub fn key_for(latin: Keysym, sym: Keysym, mods: &ModifiersState) -> Key {
    match latin {
        Keysym::Escape => Key::Cancel,
        Keysym::ISO_Left_Tab | Keysym::Up => Key::Previous,
        Keysym::Tab if mods.shift => Key::Previous,
        Keysym::Tab | Keysym::Down => Key::Next,
        _ => Key::Field(lock::key_for(latin, sym, mods)),
    }
}

/// What a key or a click came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// Send the password, if the check is ready for it.
    Submit,
    Cancel,
    /// Someone else was chosen to answer: the check starts again for them.
    Chose,
}

/// The line under the box.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    None,
    /// What the system's login checks have to say: "Place your finger on the reader".
    Info(String),
    Error(String),
    Wrong,
    Empty,
    Failed,
    Checking,
}

pub struct Prompt {
    pub request: Request,
    chosen: usize,
    password: Password,
    /// What the check is waiting for, if it is: whether the answer shows as it's typed, and what
    /// it asks for.
    asked: Option<(bool, String)>,
    /// Enter was pressed before the check asked: the password goes as soon as it does.
    submitted: bool,
    line: Line,
    /// Which check is current, so a stopped one's last words are recognised and dropped.
    pub attempt: u64,
    deaf_until: f64,
    since: f64,
    pub reduced_motion: bool,
    hovered: Option<Button>,
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Card, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
    shade: SolidColorBuffer,
}

impl Prompt {
    /// The card for `request`, with `me` chosen to answer if polkit will take this user's own
    /// password, and the first it offers otherwise.
    pub fn new(request: Request, me: u32, now: f64, reduced_motion: bool) -> Self {
        let chosen = request
            .identities
            .iter()
            .position(|identity| identity.uid == me)
            .unwrap_or(0);
        Self {
            request,
            chosen,
            password: Password::default(),
            asked: None,
            submitted: false,
            line: Line::None,
            attempt: 0,
            deaf_until: now + DEAF,
            since: now,
            reduced_motion,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
            shade: SolidColorBuffer::new((0, 0), SHADE),
        }
    }

    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    /// Who is answering.
    pub fn identity(&self) -> &Identity {
        &self.request.identities[self.chosen]
    }

    pub fn key(&mut self, key: Key, now: f64) -> Act {
        if now < self.deaf_until {
            return Act::Nothing;
        }
        match key {
            Key::Cancel => Act::Cancel,
            Key::Next => self.choose_by(1),
            Key::Previous => self.choose_by(-1),
            Key::Field(lock::Key::Type(ch)) => {
                if self.password.push(ch) && matches!(self.line, Line::Wrong | Line::Empty) {
                    self.line = Line::None;
                }
                Act::Nothing
            }
            Key::Field(lock::Key::Erase) => {
                self.password.erase();
                Act::Nothing
            }
            Key::Field(lock::Key::Clear) => {
                self.password.clear();
                Act::Nothing
            }
            Key::Field(lock::Key::Submit) => self.submit(),
            Key::Field(lock::Key::Nothing) => Act::Nothing,
        }
    }

    fn submit(&mut self) -> Act {
        if self.line == Line::Checking {
            return Act::Nothing;
        }
        if self.password.is_empty() {
            self.line = Line::Empty;
            return Act::Nothing;
        }
        Act::Submit
    }

    fn choose_by(&mut self, by: isize) -> Act {
        let count = self.request.identities.len() as isize;
        self.choose((self.chosen as isize + by).rem_euclid(count.max(1)) as usize)
    }

    fn choose(&mut self, index: usize) -> Act {
        if index == self.chosen || index >= self.request.identities.len() {
            return Act::Nothing;
        }
        self.chosen = index;
        self.restart(Line::None);
        Act::Chose
    }

    /// A new check is starting: nothing typed for the last one carries over.
    fn restart(&mut self, line: Line) {
        self.password.clear();
        self.asked = None;
        self.submitted = false;
        self.line = line;
    }

    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        let over = self.hit(x, y);
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// A click on Allow sends the password, on Cancel cancels, and on a person chooses them.
    pub fn click(&mut self, x: f64, y: f64, now: f64) -> Act {
        if now < self.deaf_until {
            return Act::Nothing;
        }
        match self.hit(x, y) {
            Some(Button::Go) => self.submit(),
            Some(Button::Stay) => Act::Cancel,
            Some(Button::Row(index)) => self.choose(index),
            _ => Act::Nothing,
        }
    }

    fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let px = DESIGN_PX as f64;
        let (lx, ly) = (((x - self.at.x) / px) as f32, ((y - self.at.y) / px) as f32);
        self.hits
            .iter()
            .find(|hit| hit.contains(lx, ly))
            .map(|hit| hit.which)
    }

    /// The password, for the check, once it has asked; the box is wiped as it goes. Before the
    /// check has asked, it is kept and goes when it does.
    pub fn answer(&mut self) -> Option<Zeroizing<Vec<u8>>> {
        self.line = Line::Checking;
        if self.asked.is_none() {
            self.submitted = true;
            return None;
        }
        self.asked = None;
        self.submitted = false;
        let answer = Zeroizing::new(self.password.bytes().to_vec());
        self.password.clear();
        Some(answer)
    }

    /// The check asks for something. A password already sent with Enter goes now; anything else
    /// waits for Enter.
    pub fn asks(&mut self, echo: bool, prompt: String) -> Option<Zeroizing<Vec<u8>>> {
        self.asked = Some((echo, prompt));
        if self.submitted && !echo {
            return self.answer();
        }
        if self.line == Line::Checking {
            self.line = Line::None;
        }
        None
    }

    /// Something to show from the check, an error or not.
    pub fn told(&mut self, text: String, error: bool) {
        self.line = if error {
            Line::Error(text)
        } else {
            Line::Info(text)
        };
    }

    /// A check ended without letting anyone in: the box empties and a new one starts.
    pub fn ended(&mut self, verdict: Verdict) {
        self.restart(match verdict {
            Verdict::Refused => Line::Wrong,
            _ => Line::Failed,
        });
    }

    fn field(&self) -> Field {
        let (echo, label) = match &self.asked {
            Some((echo, prompt)) => (*echo, label(prompt)),
            None => (false, "Password".to_string()),
        };
        let shown = if echo {
            String::from_utf8_lossy(self.password.bytes()).into_owned()
        } else {
            "•".repeat(self.password.chars().min(DOTS_SHOWN))
        };
        let line = match &self.line {
            Line::None => None,
            Line::Info(text) => Some((text.clone(), false)),
            Line::Error(text) => Some((text.clone(), true)),
            Line::Wrong => Some(("That password didn't work. Try again.".to_string(), true)),
            Line::Empty => Some(("Type the password first".to_string(), true)),
            Line::Failed => Some(("The password couldn't be checked".to_string(), true)),
            Line::Checking => Some(("Checking…".to_string(), false)),
        };
        Field { label, shown, line }
    }

    fn card(&self, ring: u32) -> Card {
        let several = self.request.identities.len() > 1;
        let rows = self
            .request
            .identities
            .iter()
            .map(|identity| Row {
                name: identity.shown_name().to_string(),
                title: if identity.full_name.is_empty() {
                    String::new()
                } else {
                    identity.name.clone()
                },
                note: None,
            })
            .collect();
        Card {
            title: "Password needed".to_string(),
            note: if several {
                format!(
                    "{} Choose who is typing their password.",
                    self.request.message
                )
            } else {
                self.request.message.clone()
            },
            rows,
            more: None,
            field: Some(self.field()),
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Cancel".to_string(),
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
            hover: self.hovered,
            selected: several.then_some((self.chosen, ring)),
        }
    }

    /// The card, and the shade behind it over the whole screen.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        screen: Size<i32, Logical>,
        scale: f64,
        now: f64,
        ring: u32,
    ) -> (
        Option<MemoryRenderBufferRenderElement<R>>,
        SolidColorRenderElement,
    )
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let card = self.card(ring);
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
        let since = now - self.since;
        let (alpha, rise) = if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
        } else {
            let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
            (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
        };
        self.shade.update(screen, SHADE);
        let shade = SolidColorRenderElement::from_buffer(
            &self.shade,
            (0, 0),
            scale,
            alpha,
            Kind::Unspecified,
        );
        let Some(painted) = self.painted.as_ref() else {
            return (None, shade);
        };
        self.at = Point::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((screen.h - painted.logical.h) / 2).max(0) as f64,
        ));
        let at = self.at + Point::from((0.0, rise * DESIGN_PX as f64));
        (painted.element(renderer, at, alpha), shade)
    }
}

/// What the box asks for, from PAM's prompt: "Password: " becomes "Password".
fn label(prompt: &str) -> String {
    let label = prompt.trim().trim_end_matches(':').trim();
    if label.is_empty() {
        "Password".to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(uid: u32, name: &str, full_name: &str) -> Identity {
        Identity {
            uid,
            name: name.into(),
            full_name: full_name.into(),
        }
    }

    fn prompt(identities: Vec<Identity>) -> Prompt {
        let request = Request::stand_in(
            "Authentication is required to install software.".into(),
            identities,
        );
        Prompt::new(request, 1000, 0.0, false)
    }

    fn typed(prompt: &mut Prompt, text: &str) {
        for ch in text.chars() {
            prompt.key(Key::Field(lock::Key::Type(ch)), DEAF);
        }
    }

    #[test]
    fn the_password_is_never_shown_only_counted() {
        let mut prompt = prompt(vec![identity(1000, "alex", "Alex Doe")]);
        prompt.asks(false, "Password: ".into());
        typed(&mut prompt, "s3cret");
        let field = prompt.card(0).field.unwrap();
        assert_eq!(field.shown, "••••••");
        assert_eq!(field.label, "Password");
        assert!(!format!("{:?}", prompt.card(0)).contains("s3cret"));
    }

    #[test]
    fn keys_typed_as_it_appears_are_not_taken() {
        let mut prompt = prompt(vec![identity(1000, "alex", "")]);
        assert_eq!(
            prompt.key(Key::Field(lock::Key::Type('x')), 0.1),
            Act::Nothing
        );
        assert_eq!(prompt.key(Key::Field(lock::Key::Submit), 0.1), Act::Nothing);
        assert_eq!(prompt.key(Key::Cancel, 0.1), Act::Nothing);
        assert!(prompt.password.is_empty());
        assert_eq!(prompt.key(Key::Cancel, DEAF), Act::Cancel);
    }

    #[test]
    fn enter_sends_once_the_check_asks_and_wipes_the_box() {
        let mut prompt = prompt(vec![identity(1000, "alex", "")]);
        typed(&mut prompt, "pw");
        // Enter before the check is ready: kept until it asks.
        assert_eq!(prompt.key(Key::Field(lock::Key::Submit), DEAF), Act::Submit);
        assert!(prompt.answer().is_none());
        let sent = prompt
            .asks(false, "Password:".into())
            .expect("sent as soon as asked");
        assert_eq!(&sent[..], b"pw");
        assert!(prompt.password.is_empty(), "wiped once sent");
        // Enter again while it's checking does nothing.
        typed(&mut prompt, "more");
        assert_eq!(
            prompt.key(Key::Field(lock::Key::Submit), DEAF),
            Act::Nothing
        );
    }

    #[test]
    fn a_wrong_password_says_so_and_starts_again() {
        let mut prompt = prompt(vec![identity(1000, "alex", "")]);
        prompt.asks(false, "Password:".into());
        typed(&mut prompt, "nope");
        prompt.key(Key::Field(lock::Key::Submit), DEAF);
        assert!(prompt.answer().is_some());
        prompt.ended(Verdict::Refused);
        assert_eq!(prompt.line, Line::Wrong);
        assert!(prompt.asked.is_none(), "waits for the new check to ask");
        typed(&mut prompt, "a");
        assert_eq!(prompt.line, Line::None, "typing again clears the warning");
        assert_eq!(prompt.key(Key::Field(lock::Key::Clear), DEAF), Act::Nothing);
        assert_eq!(
            prompt.key(Key::Field(lock::Key::Submit), DEAF),
            Act::Nothing
        );
        assert_eq!(prompt.line, Line::Empty, "an empty box isn't sent");
    }

    #[test]
    fn this_user_is_asked_for_first_and_others_can_be_chosen() {
        let mut prompt = prompt(vec![
            identity(0, "root", ""),
            identity(1000, "alex", "Alex Doe"),
            identity(1001, "sam", ""),
        ]);
        assert_eq!(prompt.identity().name, "alex");
        let card = prompt.card(0);
        assert_eq!(card.rows[1].name, "Alex Doe");
        assert_eq!(card.rows[1].title, "alex");
        assert_eq!(card.selected, Some((1, 0)));
        prompt.asks(false, "Password:".into());
        typed(&mut prompt, "half");
        assert_eq!(prompt.key(Key::Next, DEAF), Act::Chose);
        assert_eq!(prompt.identity().name, "sam");
        assert!(
            prompt.password.is_empty(),
            "nothing typed for one goes to another"
        );
        assert_eq!(prompt.key(Key::Next, DEAF), Act::Chose);
        assert_eq!(prompt.identity().name, "root", "and it wraps");
    }

    #[test]
    fn one_person_is_named_but_not_a_choice() {
        let mut prompt = prompt(vec![identity(0, "root", "")]);
        assert_eq!(prompt.card(0).selected, None);
        assert_eq!(prompt.card(0).rows[0].name, "root");
        assert_eq!(prompt.key(Key::Next, DEAF), Act::Nothing);
    }

    #[test]
    fn a_shown_prompt_shows_what_is_typed() {
        let mut prompt = prompt(vec![identity(1000, "alex", "")]);
        prompt.asks(true, "Verification code: ".into());
        typed(&mut prompt, "123");
        let field = prompt.card(0).field.unwrap();
        assert_eq!(
            (field.label.as_str(), field.shown.as_str()),
            ("Verification code", "123")
        );
        // An answer typed ahead for a hidden prompt never goes to a shown one by itself.
        prompt.restart(Line::None);
        typed(&mut prompt, "pw");
        prompt.key(Key::Field(lock::Key::Submit), DEAF);
        assert!(prompt.answer().is_none());
        assert!(prompt.asks(true, "Name:".into()).is_none());
    }
}
