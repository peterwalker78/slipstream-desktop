//! The way out: Log out, Restart and Shut down all come through here.
//!
//! One state machine, one overlay, three destinations. It asks first, then asks every window to
//! close and gives the apps a moment to take it — a moment it spends *out of the way*, taking no
//! keys, so an app's own "save changes?" prompt can be answered. Anything still there when the
//! grace runs out is named, and the answer is to go anyway, to stay, or to wait a little longer.
//!
//! Enter goes on and Esc stays, in every state, as in the explorer and bullet time. The same
//! answers are buttons the pointer can press, each carrying the key that does the same thing:
//! keyboard first, mouse always optional.
//!
//! Wall time throughout: bullet time must never stretch a logout.

use smithay::{
    backend::renderer::{ImportMem, Renderer, element::memory::MemoryRenderBufferRenderElement},
    input::keyboard::Keysym,
    utils::{Logical, Point, Size},
};

use crate::{
    card::{self, Btn, Button, Card, DIM, Hit, Row},
    motion::HYPR,
    paint::Painted,
    panel::{self, AMBER, BELOW, MARGIN, MOCKUP_PX},
    text::{Face, Style},
};

/// After the close request, before anything left over is named.
const GRACE: f64 = 4.0;
/// What `W` adds, as often as it's pressed.
const WAIT: f64 = 10.0;
/// The card opens as the explorer does.
const OPEN: f64 = 0.18;
const REDUCED_FADE: f64 = 0.08;
/// The screen fades to black before the session ends.
const FADE_OUT: f64 = 0.35;

/// Windows named before the list says how many more there are.
const MAX_ROWS: usize = 8;
/// The slim card while the apps are closing, in the toast's place and width.
const SLIM_WIDTH: f32 = 440.0;
const SLIM_TOP: f32 = 58.0;

/// Where the session is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    LogOut,
    Restart,
    ShutDown,
}

impl Intent {
    /// The card's heading.
    pub fn title(self) -> &'static str {
        match self {
            Intent::LogOut => "Log out",
            Intent::Restart => "Restart",
            Intent::ShutDown => "Shut down",
        }
    }

    /// The footer's verb, after the Enter key.
    pub fn verb(self) -> &'static str {
        match self {
            Intent::LogOut => "log out",
            Intent::Restart => "restart",
            Intent::ShutDown => "shut down",
        }
    }
}

/// A window that is open, as the overlay sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Open<W> {
    pub window: W,
    /// The app's name, as messages call it.
    pub app: String,
    pub title: String,
    /// In the code rain rather than tiled.
    pub minimised: bool,
}

/// What the compositor should do about the overlay's last move.
#[derive(Debug, Clone, PartialEq)]
pub enum Act<W> {
    Nothing,
    /// Ask each of these windows to close, as its close button would.
    Close(Vec<W>),
    /// The overlay is finished and nothing happens. Focus is where it was.
    Cancel,
    /// The point of no return has passed: end the session.
    Go(Intent),
    /// The switch on the card was turned, and the setting should follow it.
    Remember(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Modal: naming what's open, waiting for Enter or Esc.
    Asking,
    /// Not modal: the close requests are out, and a save prompt must be able to take keys.
    Closing { until: f64 },
    /// Modal again: something is still here.
    StillHere,
    /// Not modal: another ten seconds, asked for with `W`.
    Waiting { until: f64 },
    /// Fading to black. Nothing can stop it now.
    Going { until: f64 },
    /// The session is ending; the screen stays black.
    Done,
}

/// The slim card at the top, while the apps are closing.
#[derive(Debug, Clone, PartialEq)]
struct Slim {
    title: String,
    note: String,
    /// How much of the bar is filled, in whole pixels, so it repaints only when it moves.
    filled: i32,
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Card(Card),
    Slim(Slim),
    /// Nothing to draw: the fade is a solid colour over the whole screen.
    Blank,
}

pub struct Exit<W> {
    pub intent: Intent,
    phase: Phase,
    /// Every window that was open when the overlay started, in the order they're named.
    asked: Vec<Open<W>>,
    /// Those of them still open, refreshed every pass.
    left: Vec<Open<W>>,
    /// Apps that have opened another window since the close request: a save prompt, most likely.
    prompting: Vec<String>,
    /// When the phase on screen began, on wall time.
    since: f64,
    pub reduced_motion: bool,
    /// Whether to write the layout down on the way out: the setting, until the switch is turned.
    remember: bool,
    /// What the pointer is over.
    hovered: Option<Button>,
    /// Where the card's corner sits on screen, and what can be clicked on it, from the last paint.
    at: Point<f64, Logical>,
    hits: Vec<Hit>,
    shown: Option<(Shape, Size<i32, Logical>, f64)>,
    painted: Option<Painted>,
}

impl<W: Clone + PartialEq> Exit<W> {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn new(
        intent: Intent,
        open: Vec<Open<W>>,
        now: f64,
        reduced_motion: bool,
        remember: bool,
    ) -> Self {
        Self {
            intent,
            phase: Phase::Asking,
            left: open.clone(),
            asked: open,
            prompting: Vec::new(),
            since: now,
            reduced_motion,
            remember,
            hovered: None,
            at: Point::from((0.0, 0.0)),
            hits: Vec::new(),
            shown: None,
            painted: None,
        }
    }

    /// Whether the layout is to be written down on the way out.
    pub fn remember(&self) -> bool {
        self.remember
    }

    /// Whether the overlay takes every key. It doesn't while the apps are closing: an app's own
    /// save prompt needs Tab, Enter and Esc more than the overlay does.
    pub fn modal(&self) -> bool {
        matches!(self.phase, Phase::Asking | Phase::StillHere)
    }

    /// Whether the screen is fading out, or already black.
    pub fn going(&self) -> bool {
        matches!(self.phase, Phase::Going { .. } | Phase::Done)
    }

    /// How black the screen is, 0 to 1.
    pub fn blackout(&self, now: f64) -> f32 {
        match self.phase {
            Phase::Going { until } => {
                let fade = self.fade_out();
                if fade <= 0.0 {
                    1.0
                } else {
                    (1.0 - (until - now) / fade).clamp(0.0, 1.0) as f32
                }
            }
            Phase::Done => 1.0,
            _ => 0.0,
        }
    }

    fn fade_out(&self) -> f64 {
        if self.reduced_motion { 0.0 } else { FADE_OUT }
    }

    /// A pass of the event loop: what's still open, and whether a timer has run out.
    pub fn tick(&mut self, now: f64, open: &[Open<W>]) -> Act<W> {
        self.left = self
            .asked
            .iter()
            .filter(|asked| open.iter().any(|o| o.window == asked.window))
            .cloned()
            .collect();
        // A window that has appeared since is the app asking a question, not something new to
        // close. It's named against the app that put it up.
        self.prompting = self
            .left
            .iter()
            .map(|open| open.app.clone())
            .filter(|app| {
                open.iter().any(|o| {
                    o.app == *app && !self.asked.iter().any(|asked| asked.window == o.window)
                })
            })
            .collect();
        match self.phase {
            Phase::Asking | Phase::Done => Act::Nothing,
            Phase::Closing { until } | Phase::Waiting { until } => {
                if self.left.is_empty() {
                    self.start_going(now);
                } else if now >= until {
                    self.enter(Phase::StillHere, now);
                }
                Act::Nothing
            }
            // The last one closed while its card was up.
            Phase::StillHere if self.left.is_empty() => {
                self.start_going(now);
                Act::Nothing
            }
            Phase::StillHere => Act::Nothing,
            Phase::Going { until } => {
                if now >= until {
                    self.enter(Phase::Done, now);
                    return Act::Go(self.intent);
                }
                Act::Nothing
            }
        }
    }

    /// A key while the overlay is modal. Enter goes on, Esc stays, whichever card is up.
    pub fn key(&mut self, sym: Keysym, now: f64) -> Act<W> {
        let which = match sym {
            // Enter only, never Space: this card ends the session, and a space typed as it comes
            // up must not answer it.
            Keysym::Return | Keysym::KP_Enter => Button::Go,
            Keysym::Escape => Button::Stay,
            Keysym::w | Keysym::W => Button::Wait,
            Keysym::r | Keysym::R => Button::Remember,
            _ => return Act::Nothing,
        };
        self.press(which, now)
    }

    /// An answer, however it was given. The buttons on the card and the keys that match them come
    /// through here together, so the two can never drift apart.
    fn press(&mut self, which: Button, now: f64) -> Act<W> {
        match (self.phase, which) {
            (Phase::Asking, Button::Go) => {
                let windows: Vec<W> = self.asked.iter().map(|open| open.window.clone()).collect();
                self.enter(Phase::Closing { until: now + GRACE }, now);
                tracing::info!(
                    windows = windows.len(),
                    intent = self.intent.verb(),
                    remember = self.remember,
                    "asking every window to close"
                );
                Act::Close(windows)
            }
            (Phase::StillHere, Button::Go) => {
                tracing::info!(
                    windows = self.left.len(),
                    "going anyway; these windows lose whatever they were holding"
                );
                self.start_going(now);
                Act::Nothing
            }
            (Phase::Asking | Phase::StillHere, Button::Stay) => {
                tracing::info!(intent = self.intent.verb(), "cancelled");
                Act::Cancel
            }
            (Phase::StillHere, Button::Wait) => {
                self.enter(Phase::Waiting { until: now + WAIT }, now);
                Act::Nothing
            }
            // Only while there is still a choice to make: once the close requests are out, the
            // layout being written down has already been decided.
            (Phase::Asking, Button::Remember) => {
                self.remember = !self.remember;
                Act::Remember(self.remember)
            }
            _ => Act::Nothing,
        }
    }

    /// The pointer moved, in logical pixels from the output's corner. Returns whether what it is
    /// over changed, so the caller only redraws when the card has something new to show.
    pub fn hover(&mut self, x: f64, y: f64) -> bool {
        let over = self.modal().then(|| self.hit(x, y)).flatten();
        let changed = over != self.hovered;
        self.hovered = over;
        changed
    }

    /// A click on the card. Outside it nothing happens: this is a question, and the answers are
    /// the buttons, so a stray click neither ends the session nor cancels it.
    pub fn click(&mut self, x: f64, y: f64, now: f64) -> Act<W> {
        if !self.modal() {
            return Act::Nothing;
        }
        match self.hit(x, y) {
            Some(which) => self.press(which, now),
            None => Act::Nothing,
        }
    }

    /// What is under (`x`, `y`), in logical pixels from the output's corner.
    fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let px = MOCKUP_PX as f64;
        let (lx, ly) = (((x - self.at.x) / px) as f32, ((y - self.at.y) / px) as f32);
        self.hits
            .iter()
            .find(|hit| hit.contains(lx, ly))
            .map(|hit| hit.which)
    }

    fn start_going(&mut self, now: f64) {
        let until = now + self.fade_out();
        self.enter(Phase::Going { until }, now);
    }

    fn enter(&mut self, phase: Phase, now: f64) {
        self.phase = phase;
        self.since = now;
    }

    /// What the overlay is showing now.
    fn shape(&self, now: f64) -> Shape {
        match self.phase {
            Phase::Asking => Shape::Card(self.ask_card()),
            Phase::StillHere => Shape::Card(self.still_here_card()),
            Phase::Closing { until } | Phase::Waiting { until } => {
                let total = if matches!(self.phase, Phase::Closing { .. }) {
                    GRACE
                } else {
                    WAIT
                };
                let left = self.left.len();
                let bar = SLIM_WIDTH - 2.0 * 22.0;
                let gone = ((total - (until - now)) / total).clamp(0.0, 1.0);
                Shape::Slim(Slim {
                    title: self.intent.title().to_string(),
                    note: match left {
                        1 => format!("Waiting for {}.", self.left[0].app),
                        n => format!("Closing {n} windows…"),
                    },
                    filled: (gone as f32 * bar).round() as i32,
                })
            }
            Phase::Going { .. } | Phase::Done => Shape::Blank,
        }
    }

    /// The first card. It says what is about to happen and nothing else: naming every open window
    /// here was a list nobody read, and the windows that matter are the ones that don't close,
    /// which the "still here" card names.
    fn ask_card(&self) -> Card {
        let count = self.asked.len();
        let note = match count {
            0 => "Nothing is open.".to_string(),
            1 => "One window is open. Its app is asked to close it first.".to_string(),
            n => format!("{n} windows are open. Every app is asked to close first."),
        };
        Card {
            title: self.intent.title().to_string(),
            note,
            rows: Vec::new(),
            more: None,
            remember: Some(self.remember),
            buttons: vec![
                Btn {
                    which: Button::Stay,
                    label: "Stay".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: self.intent.title().to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.hovered,
            selected: None,
        }
    }

    fn still_here_card(&self) -> Card {
        let count = self.left.len();
        let asking = self.prompting.len();
        let note = if asking > 0 {
            "Something is asking to save. Esc goes back to it; anything unsaved is lost if you \
             go on."
                .to_string()
        } else if count == 1 {
            "One window is still here. It was asked to close and hasn't.".to_string()
        } else {
            format!("{count} windows are still here. They were asked to close and haven't.")
        };
        Card {
            title: self.intent.title().to_string(),
            note,
            rows: self
                .left
                .iter()
                .take(MAX_ROWS)
                .map(|open| {
                    let mut row = row(open);
                    if self.prompting.contains(&open.app) {
                        row.note = Some(("asking to save".to_string(), AMBER));
                    }
                    row
                })
                .collect(),
            more: (count > MAX_ROWS).then(|| format!("and {} more", count - MAX_ROWS)),
            remember: None,
            buttons: vec![
                Btn {
                    which: Button::Wait,
                    label: "Wait 10s".to_string(),
                    key: "W".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Stay,
                    label: "Stay".to_string(),
                    key: "ESC".to_string(),
                    primary: false,
                },
                Btn {
                    which: Button::Go,
                    label: "Go anyway".to_string(),
                    key: "⏎".to_string(),
                    primary: true,
                },
            ],
            hover: self.hovered,
            selected: None,
        }
    }

    /// The overlay for a screen `screen` logical pixels big, while one is showing.
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
        let shape = self.shape(now);
        if shape == Shape::Blank {
            self.shown = None;
            self.painted = None;
            return None;
        }
        let fresh = self
            .shown
            .as_ref()
            .is_some_and(|(shown, size, at)| *shown == shape && *size == screen && *at == scale);
        if !fresh {
            self.hits.clear();
            self.painted = match &shape {
                Shape::Card(card) => {
                    let (painted, hits) = card::paint(card, scale);
                    self.hits = hits;
                    painted
                }
                Shape::Slim(slim) => paint_slim(slim, scale),
                Shape::Blank => None,
            };
            self.shown = Some((shape, screen, scale));
        }
        let painted = self.painted.as_ref()?;
        // Where it settles, which is what a click is measured against: the opening rise is a
        // tenth of a second and a button that moved under the pointer would be worse than one
        // that waits for it.
        self.at = self.origin(screen, painted.logical);
        let (alpha, rise) = self.opening(now - self.since);
        let at = self.at + Point::from((0.0, rise * MOCKUP_PX as f64));
        painted.element(renderer, at, alpha)
    }

    /// A card `since` seconds after it appeared: its opacity, and how far above its place it is,
    /// in mockup pixels. The explorer's opening, so the two feel like one desktop.
    fn opening(&self, since: f64) -> (f32, f64) {
        if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0) as f32, 0.0)
        } else {
            let eased = HYPR.at((since / OPEN).clamp(0.0, 1.0));
            (eased.clamp(0.0, 1.0) as f32, -10.0 * (1.0 - eased))
        }
    }

    /// Where the painted surface goes: the ask card is centred, the slim one sits where a toast
    /// would.
    fn origin(
        &self,
        screen: Size<i32, Logical>,
        painted: Size<i32, Logical>,
    ) -> Point<f64, Logical> {
        let x = ((screen.w - painted.w) / 2) as f64;
        match self.phase {
            Phase::Closing { .. } | Phase::Waiting { .. } => {
                Point::from((x, ((SLIM_TOP - MARGIN) * MOCKUP_PX) as f64))
            }
            _ => Point::from((x, ((screen.h - painted.h) / 2).max(0) as f64)),
        }
    }
}

fn row<W>(open: &Open<W>) -> Row {
    Row {
        name: open.app.clone(),
        title: open.title.clone(),
        note: open.minimised.then(|| ("minimised".to_string(), DIM)),
    }
}

fn paint_slim(slim: &Slim, scale: f64) -> Option<Painted> {
    let heading = Style {
        tracking: 0.1,
        ..Style::new(Face::BodyBold, 13.0, AMBER)
    };
    let prose = Style::new(Face::Body, 16.0, 0xdfe5eeff);
    let pad = 22.0;
    let height = 16.0 + 20.0 + 4.0 + 24.0 + 10.0 + 4.0 + 16.0;
    let logical = Size::<i32, Logical>::from((
        ((SLIM_WIDTH + 2.0 * MARGIN) * MOCKUP_PX).ceil() as i32,
        ((height + 2.0 * MARGIN + BELOW) * MOCKUP_PX).ceil() as i32,
    ));
    Painted::new(logical, scale, |p| {
        p.f *= MOCKUP_PX;
        let (fx, fy) = (MARGIN, MARGIN);
        // The toast's shape: an amber edge, the card over the rest of it.
        let radius = panel::NOTICE_RADIUS;
        let (dy, blur, shadow) = panel::NOTICE_SHADOW;
        p.shadow(fx, fy, SLIM_WIDTH, height, radius, dy, blur, shadow);
        p.fill(fx, fy, SLIM_WIDTH, height, radius, AMBER);
        p.fill(
            fx + 4.0,
            fy,
            SLIM_WIDTH - 4.0,
            height,
            radius - 3.0,
            panel::NOTICE,
        );
        p.border(fx, fy, SLIM_WIDTH, height, radius, 1.0, panel::NOTICE_EDGE);
        p.text(
            &slim.title.to_uppercase(),
            fx + pad,
            fy + 16.0 + 10.0,
            &heading,
        );
        p.text(&slim.note, fx + pad, fy + 16.0 + 24.0 + 12.0, &prose);
        // How much of the grace has gone.
        let bar_y = fy + height - 16.0 - 4.0;
        let bar_w = SLIM_WIDTH - 2.0 * pad;
        p.fill(fx + pad, bar_y, bar_w, 4.0, 2.0, 0xffffff14);
        if slim.filled > 0 {
            p.fill(fx + pad, bar_y, slim.filled as f32, 4.0, 2.0, AMBER);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(window: u32, app: &str) -> Open<u32> {
        Open {
            window,
            app: app.to_string(),
            title: format!("{app} window"),
            minimised: false,
        }
    }

    fn exit() -> Exit<u32> {
        Exit::new(
            Intent::LogOut,
            vec![open(1, "Konsole"), open(2, "Firefox")],
            0.0,
            false,
            false,
        )
    }

    /// The card as it would be drawn, and where its answers are, in logical pixels on a 1536×960
    /// screen: what a click is measured against.
    fn laid_out(exit: &mut Exit<u32>) -> Vec<(Button, f64, f64)> {
        let screen = Size::<i32, Logical>::from((1536, 960));
        let Shape::Card(card) = exit.shape(0.0) else {
            panic!("the card should be up");
        };
        let (painted, hits) = card::paint(&card, 1.25);
        let painted = painted.unwrap();
        exit.hits = hits;
        exit.at = exit.origin(screen, painted.logical);
        let px = MOCKUP_PX as f64;
        exit.hits
            .iter()
            .map(|hit| {
                let (x, y) = hit.centre();
                (
                    hit.which,
                    exit.at.x + x as f64 * px,
                    exit.at.y + y as f64 * px,
                )
            })
            .collect()
    }

    fn centre_of(laid: &[(Button, f64, f64)], which: Button) -> (f64, f64) {
        laid.iter()
            .find(|(w, _, _)| *w == which)
            .map(|(_, x, y)| (*x, *y))
            .unwrap_or_else(|| panic!("{which:?} should be on the card"))
    }

    #[test]
    fn esc_at_the_ask_leaves_everything_alone() {
        let mut exit = exit();
        assert!(exit.modal());
        assert_eq!(exit.key(Keysym::Escape, 0.5), Act::Cancel);
    }

    #[test]
    fn enter_asks_every_window_to_close_and_stops_taking_keys() {
        let mut exit = exit();
        assert_eq!(exit.key(Keysym::Return, 0.5), Act::Close(vec![1, 2]));
        // While the apps are closing, keys belong to their save prompts.
        assert!(!exit.modal());
        assert_eq!(exit.key(Keysym::Escape, 0.6), Act::Nothing);
    }

    #[test]
    fn the_grace_ends_early_when_the_last_window_goes() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        assert_eq!(exit.tick(0.3, &[open(2, "Firefox")]), Act::Nothing);
        assert!(!exit.going());
        // Both gone well inside the four seconds: straight to the fade, then out.
        assert_eq!(exit.tick(0.5, &[]), Act::Nothing);
        assert!(exit.going());
        assert_eq!(exit.tick(0.9, &[]), Act::Go(Intent::LogOut));
    }

    #[test]
    fn what_is_left_after_the_grace_is_named() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        let left = [open(2, "Firefox")];
        exit.tick(1.0, &left);
        assert!(!exit.modal());
        exit.tick(GRACE + 0.1, &left);
        assert!(exit.modal());
        let card = exit.still_here_card();
        assert_eq!(card.rows.len(), 1);
        assert_eq!(card.rows[0].name, "Firefox");
        // Esc here is the useful answer: go and deal with the prompt.
        assert_eq!(exit.key(Keysym::Escape, GRACE + 0.2), Act::Cancel);
    }

    #[test]
    fn a_new_window_from_an_app_still_here_is_its_save_prompt() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        let open = [open(2, "Firefox"), open(3, "Firefox")];
        exit.tick(GRACE + 0.1, &open);
        assert_eq!(exit.prompting, vec!["Firefox".to_string()]);
        let card = exit.still_here_card();
        assert_eq!(
            card.rows[0].note.as_ref().map(|(text, _)| text.as_str()),
            Some("asking to save")
        );
        // The prompt itself isn't something new to close.
        assert_eq!(card.rows.len(), 1);
    }

    #[test]
    fn w_waits_another_ten_seconds_and_can_be_pressed_again() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        let left = [open(2, "Firefox")];
        exit.tick(GRACE + 0.1, &left);
        exit.key(Keysym::w, GRACE + 0.2);
        assert!(!exit.modal());
        exit.tick(GRACE + WAIT, &left);
        assert!(!exit.modal());
        exit.tick(GRACE + WAIT + 0.3, &left);
        assert!(exit.modal());
        exit.key(Keysym::w, GRACE + WAIT + 0.4);
        assert!(!exit.modal());
    }

    #[test]
    fn go_anyway_ends_the_session_with_windows_still_open() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        let left = [open(2, "Firefox")];
        exit.tick(GRACE + 0.1, &left);
        exit.key(Keysym::Return, GRACE + 0.2);
        assert!(exit.going());
        assert_eq!(exit.tick(GRACE + 0.3, &left), Act::Nothing);
        assert_eq!(exit.tick(GRACE + 0.6, &left), Act::Go(Intent::LogOut));
        // Once it's gone it stays gone: the screen is black and nothing fires twice.
        assert_eq!(exit.tick(GRACE + 1.0, &left), Act::Nothing);
        assert_eq!(exit.blackout(GRACE + 1.0), 1.0);
    }

    #[test]
    fn reduced_motion_cuts_straight_to_the_action() {
        let mut exit = Exit::new(Intent::Restart, vec![open(1, "Konsole")], 0.0, true, false);
        exit.key(Keysym::Return, 0.0);
        assert_eq!(exit.tick(0.1, &[]), Act::Nothing);
        assert_eq!(exit.blackout(0.1), 1.0);
        assert_eq!(exit.tick(0.2, &[]), Act::Go(Intent::Restart));
    }

    #[test]
    fn logging_out_with_nothing_open_needs_one_keypress() {
        let mut exit = Exit::new(Intent::ShutDown, Vec::<Open<u32>>::new(), 0.0, false, false);
        assert_eq!(exit.ask_card().note, "Nothing is open.");
        assert_eq!(exit.key(Keysym::Return, 0.0), Act::Close(vec![]));
        assert_eq!(exit.tick(0.01, &[]), Act::Nothing);
        assert!(exit.going());
    }

    #[test]
    fn a_long_list_says_how_many_more_there_are() {
        let mut exit = Exit::new(
            Intent::LogOut,
            (0..11).map(|i| open(i, "Konsole")).collect(),
            0.0,
            false,
            false,
        );
        // The ask card counts them; only the windows that wouldn't close get named.
        assert_eq!(
            exit.ask_card().note,
            "11 windows are open. Every app is asked to close first."
        );
        assert!(exit.ask_card().rows.is_empty());
        exit.key(Keysym::Return, 0.0);
        let left: Vec<Open<u32>> = (0..11).map(|i| open(i, "Konsole")).collect();
        exit.tick(GRACE + 0.1, &left);
        let card = exit.still_here_card();
        assert_eq!(card.rows.len(), MAX_ROWS);
        assert_eq!(card.more.as_deref(), Some("and 3 more"));
    }

    #[test]
    fn the_card_grows_with_its_list_rather_than_clipping_it() {
        let mut short = exit();
        short.key(Keysym::Return, 0.0);
        short.tick(GRACE + 0.1, &[open(1, "Konsole")]);
        let short = card::paint(&short.still_here_card(), 1.25).0.unwrap();
        let mut long = Exit::new(
            Intent::LogOut,
            (0..8).map(|i| open(i, "Konsole")).collect(),
            0.0,
            false,
            false,
        );
        long.key(Keysym::Return, 0.0);
        let all: Vec<Open<u32>> = (0..8).map(|i| open(i, "Konsole")).collect();
        long.tick(GRACE + 0.1, &all);
        let long = card::paint(&long.still_here_card(), 1.25).0.unwrap();
        assert_eq!(short.logical.w, long.logical.w);
        assert!(long.logical.h > short.logical.h);
    }

    #[test]
    fn the_ask_cards_answers_can_be_clicked_as_well_as_typed() {
        let mut exit = exit();
        let laid = laid_out(&mut exit);
        let (x, y) = centre_of(&laid, Button::Go);
        assert_eq!(exit.click(x, y, 0.5), Act::Close(vec![1, 2]));
    }

    #[test]
    fn the_still_here_cards_answers_can_be_clicked_too() {
        let mut exit = exit();
        exit.key(Keysym::Return, 0.0);
        exit.tick(GRACE + 0.1, &[open(2, "Firefox")]);
        let laid = laid_out(&mut exit);
        let (x, y) = centre_of(&laid, Button::Wait);
        assert_eq!(exit.click(x, y, GRACE + 0.2), Act::Nothing);
        assert!(!exit.modal());
        exit.tick(GRACE + WAIT + 0.3, &[open(2, "Firefox")]);
        let laid = laid_out(&mut exit);
        let (x, y) = centre_of(&laid, Button::Stay);
        assert_eq!(exit.click(x, y, GRACE + WAIT + 0.4), Act::Cancel);
    }

    #[test]
    fn a_click_beside_the_card_answers_nothing() {
        let mut exit = exit();
        laid_out(&mut exit);
        assert_eq!(exit.click(5.0, 5.0, 0.5), Act::Nothing);
        assert!(exit.modal());
    }

    #[test]
    fn the_pointer_lights_up_whatever_it_is_over() {
        let mut exit = exit();
        let laid = laid_out(&mut exit);
        let (x, y) = centre_of(&laid, Button::Go);
        assert!(exit.hover(x, y));
        assert_eq!(exit.hovered, Some(Button::Go));
        // Moving within the same button changes nothing, so the card isn't repainted.
        assert!(!exit.hover(x + 1.0, y));
        assert!(exit.hover(5.0, 5.0));
        assert_eq!(exit.hovered, None);
    }

    #[test]
    fn the_switch_is_only_on_the_card_that_still_has_a_choice() {
        let mut exit = exit();
        assert_eq!(exit.ask_card().remember, Some(false));
        // Turned on by its key or by clicking its row, and the setting is told either way.
        assert_eq!(exit.key(Keysym::r, 0.1), Act::Remember(true));
        assert!(exit.remember());
        let laid = laid_out(&mut exit);
        let (x, y) = centre_of(&laid, Button::Remember);
        assert_eq!(exit.click(x, y, 0.2), Act::Remember(false));
        assert!(!exit.remember());
        // Once the close requests are out it is too late to change: the record is already taken.
        exit.key(Keysym::Return, 0.3);
        exit.tick(GRACE + 0.1, &[open(2, "Firefox")]);
        assert_eq!(exit.still_here_card().remember, None);
        assert_eq!(exit.key(Keysym::r, GRACE + 0.2), Act::Nothing);
    }

    #[test]
    fn the_setting_decides_where_the_switch_starts() {
        let exit = Exit::new(Intent::LogOut, vec![open(1, "Konsole")], 0.0, false, true);
        assert_eq!(exit.ask_card().remember, Some(true));
        assert!(exit.remember());
    }
}
