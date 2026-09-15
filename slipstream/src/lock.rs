//! The lock screen. While it is up, no window is drawn, captured or given input: every screen
//! shows the living wallpaper under a dark veil, the focused one the lock card (the clock, the
//! date, who is logged in, and the password pill), and the keyboard types into that pill and
//! nothing else. Only the password checked by `auth.rs`, or logind (in the login session), takes
//! it down.
//!
//! It never goes up without a way to take it down: with no PAM service for Slipstream installed,
//! locking is refused.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    desktop::{PopupKind, PopupManager},
    input::{
        keyboard::{Keysym, ModifiersState},
        pointer::MotionEvent,
    },
    utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::seat::WaylandFocus,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    Slipstream,
    anim::Easing,
    auth::Verdict,
    icons,
    keys::Action,
    motion::HYPR,
    paint::{self, Painter},
    text::{self, Face, Style},
};

/// The longest password the pill takes, in bytes.
pub const PASSWORD_LIMIT: usize = 512;
/// Where a PAM service for Slipstream can be installed. Without one, the lock never goes up.
pub const SERVICE_FILES: [&str; 2] = ["/etc/pam.d/slipstream", "/usr/lib/pam.d/slipstream"];

/// Logical pixels per mockup pixel.
const MOCKUP_PX: f32 = 0.8;
/// Locking fades the veil and the card in.
const FADE_IN: f64 = 0.18;
/// Reduced motion's fades, in and out.
const REDUCED_FADE: f64 = 0.08;
/// A wrong or empty Enter shakes the pill this long, this far each way (mockup pixels).
const SHAKE: f64 = 0.3;
const SHAKE_PX: f64 = 12.0;
/// Unlocking: the card fades and grows over this long, while the desktop comes forward over the
/// other.
const CARD_OUT: f64 = 0.45;
const DESKTOP_IN: f64 = 0.46;
/// CSS `ease-in`.
const EASE_IN: Easing = Easing::Bezier(0.42, 0.0, 1.0, 1.0);
/// The veil over the wallpaper: `#05070b70`.
pub const VEIL: [f32; 4] = [0.02, 0.027, 0.043, 0x70 as f32 / 255.0];
/// The wallpaper's brightness behind the lock.
pub const WALLPAPER_GLOW: f32 = 0.62;

const AMBER: u32 = 0xffb547ff;

/// Whether some PAM service file for Slipstream exists, asking `exists` of each place one can be.
pub fn can_lock(exists: impl Fn(&Path) -> bool) -> bool {
    SERVICE_FILES.iter().any(|file| exists(Path::new(file)))
}

/// Whether the lock is up, for the log's filter (`main.rs`), which reaches it with no
/// `Slipstream` to hand.
static HELD: AtomicBool = AtomicBool::new(false);

pub fn held() -> bool {
    HELD.load(Ordering::Relaxed)
}

/// Whether the `lock` debug step may act. Off unless the nested backend turns it on, so a login
/// session's debug script can never reach the lock.
static STEPS_ALLOWED: AtomicBool = AtomicBool::new(false);

/// Whether a compositor lets debug steps lock: only nested.
pub fn steps_allowed_for(nested: bool) -> bool {
    nested
}

/// Set once, when the backend is chosen.
pub fn allow_steps(allowed: bool) {
    STEPS_ALLOWED.store(allowed, Ordering::Relaxed);
}

pub fn steps_allowed() -> bool {
    STEPS_ALLOWED.load(Ordering::Relaxed)
}

/// Each password check's number, so an answer that arrives after its lock has gone (or after
/// another check began) is recognised and dropped.
static NEXT_CHECK: AtomicU64 = AtomicU64::new(1);

/// The password being typed, in memory that is wiped whenever it is cleared and when dropped. It
/// is allocated once at its full size, so it never moves and leaves no copy behind, and it has no
/// `Debug` or `Display`, so it can't be formatted into a log or a message by mistake.
pub struct Password(Zeroizing<String>);

impl Default for Password {
    fn default() -> Self {
        Self(Zeroizing::new(String::with_capacity(PASSWORD_LIMIT)))
    }
}

impl Password {
    /// Adds a character, unless that would pass `PASSWORD_LIMIT` bytes.
    pub fn push(&mut self, ch: char) -> bool {
        if self.0.len() + ch.len_utf8() > PASSWORD_LIMIT {
            return false;
        }
        self.0.push(ch);
        true
    }

    /// Takes off the last character, wiping its bytes.
    pub fn erase(&mut self) {
        let Some((at, _)) = self.0.char_indices().next_back() else {
            return;
        };
        // SAFETY: the bytes from a character boundary to the end become zeros, which are valid
        // UTF-8, and are then cut off.
        unsafe { self.0.as_mut_vec()[at..].zeroize() };
        self.0.truncate(at);
    }

    /// Wipes it all.
    pub fn clear(&mut self) {
        self.0.zeroize();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many characters, for the dots.
    pub fn chars(&self) -> usize {
        self.0.chars().count()
    }

    pub fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// What a key does to the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Type(char),
    Erase,
    Clear,
    Submit,
    Nothing,
}

/// The pill's meaning of a key: `latin` is the key's Latin keysym (layout-independent, for the
/// control chords), `sym` what the layout makes of it with the modifiers held, which is what
/// gets typed.
pub fn key_for(latin: Keysym, sym: Keysym, mods: &ModifiersState) -> Key {
    let chord = mods.ctrl || mods.alt || mods.logo;
    match latin {
        Keysym::Escape => return Key::Clear,
        Keysym::u | Keysym::BackSpace if mods.ctrl => return Key::Clear,
        Keysym::BackSpace => return Key::Erase,
        Keysym::Return | Keysym::KP_Enter | Keysym::ISO_Enter => return Key::Submit,
        _ => {}
    }
    if chord {
        return Key::Nothing;
    }
    // UTF-32 rather than UTF-8, which would leave each character in a heap string, unwiped.
    char::from_u32(smithay::input::keyboard::xkb::keysym_to_utf32(sym))
        .filter(|ch| *ch != '\0' && !ch.is_control())
        .map_or(Key::Nothing, Key::Type)
}

/// The keys that still act while locked, whatever else is held: the volume, mute, microphone
/// and brightness keys, and switching virtual terminals.
pub fn allowed(key: Keysym) -> Option<Action> {
    let first_vt = Keysym::XF86_Switch_VT_1.raw();
    if (first_vt..=Keysym::XF86_Switch_VT_12.raw()).contains(&key.raw()) {
        return Some(Action::SwitchVt((key.raw() - first_vt + 1) as i32));
    }
    Some(match key {
        Keysym::XF86_AudioRaiseVolume => Action::Volume(5),
        Keysym::XF86_AudioLowerVolume => Action::Volume(-5),
        Keysym::XF86_AudioMute => Action::ToggleMute { microphone: false },
        Keysym::XF86_AudioMicMute => Action::ToggleMute { microphone: true },
        Keysym::XF86_MonBrightnessUp => Action::Brightness(5),
        Keysym::XF86_MonBrightnessDown => Action::Brightness(-5),
        _ => return None,
    })
}

/// What the line under the pill says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hint {
    Ready,
    Empty,
    Checking,
    Refused(Option<String>),
    Failed,
}

/// What a key or an answer means for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// Check the password now, as check number `id`.
    Check(u64),
    Unlock,
}

/// Why the screen is locking, which decides how a refusal is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Super+L, quick settings, or `loginctl lock-session`: said every time.
    Asked,
    /// Idle for its minutes, or about to sleep: said once a session, so a toast doesn't come
    /// back every time the machine sleeps or sits idle.
    ByItself,
}

/// The lock, while it's up.
pub struct Lock<W> {
    password: Password,
    hint: Hint,
    /// The check under way.
    checking: Option<u64>,
    /// The window that had the keyboard, to have it back.
    pub remembered: Option<W>,
    /// When it locked, and when the pill last shook, on wall time.
    since: f64,
    shook: Option<f64>,
    pub reduced_motion: bool,
    card: Card,
    /// The screens that have drawn the lock since it went up, by name.
    pub drawn: Vec<String>,
}

impl<W> Lock<W> {
    pub fn new(remembered: Option<W>, now: f64, reduced_motion: bool) -> Self {
        Self {
            password: Password::default(),
            hint: Hint::Ready,
            checking: None,
            remembered,
            since: now,
            shook: None,
            reduced_motion,
            card: Card::default(),
            drawn: Vec::new(),
        }
    }

    pub fn checking(&self) -> bool {
        self.checking.is_some()
    }

    /// A key for the pill. Nothing is taken while a check is under way.
    pub fn key(&mut self, key: Key, now: f64) -> Outcome {
        if self.checking.is_some() {
            return Outcome::Nothing;
        }
        match key {
            Key::Type(ch) => {
                if self.password.push(ch) {
                    self.hint = Hint::Ready;
                }
            }
            Key::Erase => self.password.erase(),
            Key::Clear => {
                self.password.clear();
                self.hint = Hint::Ready;
            }
            Key::Submit if self.password.is_empty() => {
                self.hint = Hint::Empty;
                self.shook = Some(now);
            }
            Key::Submit => {
                let id = NEXT_CHECK.fetch_add(1, Ordering::Relaxed);
                self.checking = Some(id);
                self.hint = Hint::Checking;
                return Outcome::Check(id);
            }
            Key::Nothing => {}
        }
        Outcome::Nothing
    }

    /// The password, for handing to its check.
    pub fn password(&self) -> &[u8] {
        self.password.bytes()
    }

    /// Wipes the password: after it has gone to its check, and on unlocking.
    pub fn forget_password(&mut self) {
        self.password.clear();
    }

    /// Check `id` answered. An answer for any other check is dropped.
    pub fn checked(&mut self, id: u64, verdict: Verdict, now: f64) -> Outcome {
        if self.checking != Some(id) {
            return Outcome::Nothing;
        }
        self.checking = None;
        self.password.clear();
        match verdict {
            Verdict::Unlock => return Outcome::Unlock,
            Verdict::Refused(message) => {
                self.hint = Hint::Refused(message);
                self.shook = Some(now);
            }
            Verdict::Failed => self.hint = Hint::Failed,
        }
        Outcome::Nothing
    }

    /// What the card shows now: never the password, only how many characters it has.
    fn view(&self, facts: &Facts, now: f64) -> View {
        let (hint, amber) = match &self.hint {
            Hint::Ready => ("Enter to unlock".to_string(), false),
            Hint::Empty => ("Type your password first".to_string(), true),
            Hint::Checking => ("Checking…".to_string(), false),
            Hint::Refused(message) => (
                message
                    .clone()
                    .unwrap_or_else(|| "Wrong password".to_string()),
                true,
            ),
            Hint::Failed => ("Couldn't check the password".to_string(), true),
        };
        let (hint, amber) = match (facts.caps_lock, amber, &self.hint) {
            (true, _, Hint::Ready) => ("Caps Lock is on".to_string(), true),
            (true, true, _) => (format!("{hint} · Caps Lock is on"), true),
            _ => (hint, amber),
        };
        // The caret blinks once a second, and holds still for reduced motion.
        let caret = self.reduced_motion || ((now - self.since) % 1.0) < 0.5;
        View {
            facts: facts.clone(),
            dots: self.password.chars(),
            hint,
            amber,
            caret: caret && self.checking.is_none(),
        }
    }

    /// How far in the veil and the card have faded.
    pub fn shown(&self, now: f64) -> f32 {
        let fade = if self.reduced_motion {
            REDUCED_FADE
        } else {
            FADE_IN
        };
        ((now - self.since) / fade).clamp(0.0, 1.0) as f32
    }

    /// The pill's shake, in logical pixels.
    fn shake(&self, now: f64) -> f64 {
        let Some(at) = self.shook.filter(|_| !self.reduced_motion) else {
            return 0.0;
        };
        shake_at((now - at) / SHAKE) * MOCKUP_PX as f64
    }

    /// The card's elements for a screen `size` big.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        facts: &Facts,
        now: f64,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let view = self.view(facts, now);
        let alpha = self.shown(now);
        let dx = self.shake(now);
        self.card
            .elements(renderer, &view, size, scale, alpha, 1.0, dx)
    }

    /// The card as it was at the moment of unlocking, to fade out.
    pub fn leave(self, facts: &Facts, now: f64) -> Unlocking {
        let mut view = self.view(facts, now);
        view.caret = false;
        Unlocking {
            card: self.card,
            view,
            since: now,
            reduced_motion: self.reduced_motion,
        }
    }
}

/// The shake's offset in mockup pixels, `t` of the way through: out to the left by a quarter,
/// over to the right by three quarters, and back, as the mockup's keyframes go.
fn shake_at(t: f64) -> f64 {
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    if t < 0.25 {
        -SHAKE_PX * t / 0.25
    } else if t < 0.75 {
        -SHAKE_PX + 2.0 * SHAKE_PX * (t - 0.25) / 0.5
    } else {
        SHAKE_PX * (1.0 - (t - 0.75) / 0.25)
    }
}

/// The lock going away: the card fading and growing, and the desktop coming forward.
pub struct Unlocking {
    card: Card,
    view: View,
    since: f64,
    pub reduced_motion: bool,
}

impl Unlocking {
    pub fn done(&self, now: f64) -> bool {
        let length = if self.reduced_motion {
            REDUCED_FADE
        } else {
            CARD_OUT.max(DESKTOP_IN)
        };
        now - self.since >= length
    }

    /// The desktop's scale and opacity now.
    pub fn desktop(&self, now: f64) -> (f64, f32) {
        let t = now - self.since;
        if self.reduced_motion {
            let k = (t / REDUCED_FADE).clamp(0.0, 1.0);
            return (1.0, (0.3 + 0.7 * k) as f32);
        }
        let k = HYPR.at((t / DESKTOP_IN).clamp(0.0, 1.0));
        (0.93 + 0.07 * k, (0.3 + 0.7 * k).clamp(0.0, 1.0) as f32)
    }

    /// The veil's and the card's opacity now, and the card's scale.
    pub fn card_state(&self, now: f64) -> (f32, f64) {
        let t = now - self.since;
        if self.reduced_motion {
            return ((1.0 - t / REDUCED_FADE).clamp(0.0, 1.0) as f32, 1.0);
        }
        let k = EASE_IN.at((t / CARD_OUT).clamp(0.0, 1.0));
        ((1.0 - k) as f32, 1.0 + 0.05 * k)
    }

    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        now: f64,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let (alpha, grow) = self.card_state(now);
        if alpha <= 0.0 {
            return Vec::new();
        }
        self.card
            .elements(renderer, &self.view, size, scale, alpha, grow, 0.0)
    }
}

/// What the card shows besides the pill: read from the status thread and the keyboard each frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    pub time: String,
    /// As the mockup writes it on the lock: `Sunday 13 September`.
    pub date: String,
    pub user: String,
    pub network: Option<String>,
    pub battery: Option<(u8, bool)>,
    pub caps_lock: bool,
}

impl Facts {
    /// The long date for `today`, or nothing before the first reading.
    pub fn long_date(today: Option<crate::calendar::Date>) -> String {
        today.map_or_else(String::new, |date| {
            format!(
                "{} {} {}",
                crate::calendar::WEEKDAYS[crate::calendar::weekday(date) as usize],
                date.day,
                crate::calendar::MONTHS[(date.month as usize).clamp(1, 12) - 1]
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct View {
    facts: Facts,
    dots: usize,
    hint: String,
    amber: bool,
    caret: bool,
}

/// One painted part of the card, with what it was painted from.
struct Piece {
    key: String,
    buffer: MemoryRenderBuffer,
    /// Its size in logical pixels, and in the buffer's pixels.
    logical: Size<i32, Logical>,
    device: (i32, i32),
}

impl Piece {
    /// A piece `w` × `h` mockup pixels, painted at `scale` in mockup pixels.
    fn paint(
        key: String,
        w: f32,
        h: f32,
        scale: f64,
        draw: impl FnOnce(&mut Painter),
    ) -> Option<Self> {
        let logical = Size::<i32, Logical>::from((
            (w * MOCKUP_PX).ceil().max(1.0) as i32,
            (h * MOCKUP_PX).ceil().max(1.0) as i32,
        ));
        let device = (
            (logical.w as f64 * scale).round().max(1.0) as i32,
            (logical.h as f64 * scale).round().max(1.0) as i32,
        );
        let mut p = Painter::new(device.0 as u32, device.1 as u32, scale as f32 * MOCKUP_PX)?;
        draw(&mut p);
        Some(Self {
            key,
            buffer: paint::buffer(&p.pixmap),
            logical,
            device,
        })
    }

    /// The piece centred across `width` with its top at `y` (logical), grown by `grow` about the
    /// screen's centre `centre`, and moved `dx` across.
    #[allow(clippy::too_many_arguments)]
    fn element<R>(
        &self,
        renderer: &mut R,
        width: i32,
        y: f64,
        centre: (f64, f64),
        grow: f64,
        dx: f64,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let x = ((width - self.logical.w) / 2) as f64 + dx;
        let at = Point::<f64, Logical>::from((
            centre.0 + (x - centre.0) * grow,
            centre.1 + (y - centre.1) * grow,
        ));
        let shown = Size::<i32, Logical>::from((
            (self.logical.w as f64 * grow).round() as i32,
            (self.logical.h as f64 * grow).round() as i32,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_physical(scale).to_i32_round::<i32>().to_f64(),
            &self.buffer,
            Some(alpha),
            Some(Rectangle::from_size(
                (self.device.0 as f64, self.device.1 as f64).into(),
            )),
            Some(shown),
            Kind::Unspecified,
        )
        .ok()
    }
}

// The card's parts, in mockup pixels: the clock and date at the top, who is logged in with the
// pill and its hint in the middle, and the status row at the foot, spaced out between them as the
// mockup's column is.
const TOP: f32 = 150.0;
const BOTTOM: f32 = 56.0;
const CLOCK_PX: f32 = 220.0;
const DATE_PX: f32 = 34.0;
const TOP_H: f32 = CLOCK_PX + 6.0 + DATE_PX * 1.2;
/// The name is a quiet label over the pill, nearer to it than the column's gaps.
const NAME_PX: f32 = 18.0;
const USER_H: f32 = NAME_PX * 1.2;
const NAME_GAP: f32 = 8.0;
const PILL_H: f32 = 14.0 * 2.0 + 22.0 * 1.2 + 2.0;
const PILL_MIN: f32 = 380.0;
const HINT_PX: f32 = 16.0;
const HINT_H: f32 = HINT_PX * 1.2;
const STATUS_H: f32 = 20.0;
const GAP: f32 = 14.0;
/// The most dots shown, however long the password.
const DOTS_SHOWN: usize = 32;

#[derive(Default)]
struct Card {
    top: Option<Piece>,
    user: Option<Piece>,
    pill: Option<Piece>,
    hint: Option<Piece>,
    status: Option<Piece>,
}

/// Keeps `slot` painted from `key`, painting again only when the key changes.
fn keep(slot: &mut Option<Piece>, key: String, paint: impl FnOnce(String) -> Option<Piece>) {
    if slot.as_ref().is_none_or(|piece| piece.key != key) {
        *slot = paint(key);
    }
}

impl Card {
    #[allow(clippy::too_many_arguments)]
    fn elements<R>(
        &mut self,
        renderer: &mut R,
        view: &View,
        size: Size<i32, Logical>,
        scale: f64,
        alpha: f32,
        grow: f64,
        dx: f64,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let s = scale.to_bits();
        let facts = &view.facts;
        keep(
            &mut self.top,
            format!("{s}|{}|{}", facts.time, facts.date),
            |key| paint_top(key, &facts.time, &facts.date, scale),
        );
        keep(&mut self.user, format!("{s}|{}", facts.user), |key| {
            paint_user(key, &facts.user, scale)
        });
        let dots = view.dots.min(DOTS_SHOWN);
        keep(
            &mut self.pill,
            format!("{s}|{dots}|{}|{}", view.caret, facts.caps_lock),
            |key| paint_pill(key, dots, view.caret, facts.caps_lock, scale),
        );
        keep(
            &mut self.hint,
            format!("{s}|{}|{}", view.hint, view.amber),
            |key| paint_hint(key, &view.hint, view.amber, scale),
        );
        keep(
            &mut self.status,
            format!("{s}|{:?}|{:?}", facts.network, facts.battery),
            |key| paint_status(key, facts.network.as_deref(), facts.battery, scale),
        );

        let px = MOCKUP_PX as f64;
        let height = size.h as f64 / px;
        let middle_h = (USER_H + NAME_GAP + PILL_H + GAP + HINT_H) as f64;
        let top_end = (TOP + TOP_H) as f64;
        let status_y = height - (BOTTOM + STATUS_H) as f64;
        let gap = ((status_y - top_end - middle_h) / 2.0).max(0.0);
        let user_y = top_end + gap;
        let pill_y = user_y + (USER_H + NAME_GAP) as f64;
        let hint_y = pill_y + (PILL_H + GAP) as f64;
        let centre = (size.w as f64 / 2.0, size.h as f64 / 2.0);
        let place = [
            (&self.top, TOP as f64, 0.0),
            (&self.user, user_y, 0.0),
            (&self.pill, pill_y, dx),
            (&self.hint, hint_y, 0.0),
            (&self.status, status_y, 0.0),
        ];
        place
            .into_iter()
            .filter_map(|(piece, y, dx)| {
                piece
                    .as_ref()?
                    .element(renderer, size.w, y * px, centre, grow, dx, scale, alpha)
            })
            .collect()
    }
}

fn paint_top(key: String, time: &str, date: &str, scale: f64) -> Option<Piece> {
    let clock = Style {
        tracking: 0.01,
        tabular: true,
        ..Style::new(Face::Display, CLOCK_PX, 0xf3f5f9ff)
    };
    let day = Style::new(Face::Body, DATE_PX, 0xd3d9e4ff);
    let (clock_w, day_w) = (text::width(time, &clock), text::width(date, &day));
    let w = clock_w.max(day_w).max(1.0) + 8.0;
    Piece::paint(key, w, TOP_H, scale, |p| {
        p.text(time, (w - clock_w) / 2.0, CLOCK_PX / 2.0, &clock);
        p.text(
            date,
            (w - day_w) / 2.0,
            CLOCK_PX + 6.0 + DATE_PX * 0.6,
            &day,
        );
    })
}

fn paint_user(key: String, user: &str, scale: f64) -> Option<Piece> {
    let name = Style::new(Face::Body, NAME_PX, 0xb3bbc8ff);
    let w = text::width(user, &name).max(1.0) + 8.0;
    Piece::paint(key, w, USER_H, scale, |p| {
        p.text(user, 4.0, USER_H / 2.0, &name);
    })
}

/// The password pill. With Caps Lock on, the key's arrow sits at its right end, in amber, as
/// Windows marks its password box.
fn paint_pill(key: String, dots: usize, caret: bool, caps_lock: bool, scale: f64) -> Option<Piece> {
    const CAPS_ICON: f32 = 22.0;
    const CAPS_INSET: f32 = 20.0;
    let (label, style) = if dots == 0 {
        (
            "Password".to_string(),
            Style::new(Face::Body, 22.0, 0xaab2c0ff),
        )
    } else {
        (
            "•".repeat(dots),
            Style {
                tracking: 0.3,
                ..Style::new(Face::Body, 22.0, 0xf3f5f9ff)
            },
        )
    };
    let label_w = text::width(&label, &style);
    let caret_w = 5.0;
    // Room for the arrow on both sides, so the dots stay in the middle.
    let caps_room = if caps_lock {
        2.0 * (CAPS_ICON + CAPS_INSET)
    } else {
        0.0
    };
    let w = PILL_MIN.max(label_w + caret_w + 48.0 + caps_room);
    Piece::paint(key, w, PILL_H, scale, |p| {
        p.fill(0.0, 0.0, w, PILL_H, 30.0, 0xffffff1f);
        p.border(0.0, 0.0, w, PILL_H, 30.0, 1.0, 0xffffff30);
        if caps_lock {
            let (x, y) = (w - CAPS_INSET - CAPS_ICON, (PILL_H - CAPS_ICON) / 2.0);
            p.icon(icons::CAPS_LOCK, x, y, CAPS_ICON, Some(AMBER));
        }
        let x = (w - label_w - caret_w) / 2.0;
        p.text(&label, x, PILL_H / 2.0, &style);
        if caret {
            // CSS draws the caret after the text, 2 px on, 1.1 em tall.
            let h = 22.0 * 1.1;
            p.fill(x + label_w + 2.0, (PILL_H - h) / 2.0, 3.0, h, 0.0, AMBER);
        }
    })
}

fn paint_hint(key: String, hint: &str, amber: bool, scale: f64) -> Option<Piece> {
    let style = Style::new(Face::Body, HINT_PX, if amber { AMBER } else { 0xb3bbc8ff });
    let hint_w = text::width(hint, &style);
    let w = hint_w.max(1.0) + 8.0;
    Piece::paint(key, w, HINT_H, scale, |p| {
        p.text(hint, 4.0, HINT_H / 2.0, &style);
    })
}

fn paint_status(
    key: String,
    network: Option<&str>,
    battery: Option<(u8, bool)>,
    scale: f64,
) -> Option<Piece> {
    const INK: u32 = 0xcfd6e2ff;
    const ICON: f32 = 18.0;
    let words = Style::new(Face::Body, 16.0, INK);
    let dot = Style::new(Face::Body, 16.0, 0x6f7888ff);
    // Each part is an icon and its words; the parts are joined by a dot with room either side.
    let mut parts: Vec<(String, String, Option<u32>)> = Vec::new();
    parts.push((
        icons::WIFI.to_string(),
        network.unwrap_or("Not connected").to_string(),
        Some(INK),
    ));
    if let Some((percent, charging)) = battery {
        parts.push((
            icons::battery(percent, charging),
            format!("{percent}%"),
            Some(INK),
        ));
    }
    parts.push((icons::LOGO.to_string(), "Slipstream".to_string(), None));
    let part_w = |words_text: &str| ICON + 10.0 + text::width(words_text, &words);
    let joint = 16.0 + text::width("·", &dot) + 16.0;
    let w = parts.iter().map(|(_, t, _)| part_w(t)).sum::<f32>()
        + joint * (parts.len() - 1) as f32
        + 8.0;
    Piece::paint(key, w, STATUS_H, scale, |p| {
        let mut x = 4.0;
        for (i, (icon, label, ink)) in parts.iter().enumerate() {
            if i > 0 {
                x += 10.0;
                p.text("·", x + 6.0, STATUS_H / 2.0, &dot);
                x += joint - 10.0;
            }
            p.icon(icon, x, (STATUS_H - ICON) / 2.0, ICON, *ink);
            x += ICON + 10.0;
            x += p.text(label, x, STATUS_H / 2.0, &words);
        }
    })
}

impl Slipstream {
    /// Locks now, as someone asked. Refused, with a toast, when no PAM service is installed to
    /// unlock with, and while the way out is closing apps. Returns whether the lock is up.
    pub fn lock_now(&mut self) -> bool {
        self.lock_for(Reason::Asked)
    }

    /// Locks now, for `reason`.
    pub fn lock_for(&mut self, reason: Reason) -> bool {
        if self.lock.is_some() {
            return true;
        }
        if !can_lock(|file| file.exists()) {
            tracing::warn!(
                ?reason,
                "not locking: no PAM service for Slipstream is installed"
            );
            if reason == Reason::Asked || !self.lock_refusal_told {
                self.lock_refusal_told = true;
                self.wake_ui();
                self.show_toast(
                    "The lock isn't set up",
                    "Run scripts/install-session once to add it.",
                );
            }
            return false;
        }
        match self.exit.as_ref().map(|exit| exit.modal()) {
            Some(true) => self.cancel_exit(),
            Some(false) => {
                tracing::warn!("not locking while the way out is closing apps");
                return false;
            }
            None => {}
        }
        let remembered = self.focused_window();
        self.close_panels();
        // Alt+Tab's switcher goes without switching: nothing is focused or restored under the lock.
        self.cancel_cycle();
        self.leave_bullet_time_now();
        // A screenshot asked for just before is never taken of the lock, nor flashes over it.
        self.screenshot_requests.clear();
        self.flash = None;
        self.snip = None;
        self.snip_flights.clear();
        if let Some(picker) = self.share.take() {
            tracing::info!("sharing declined: the screen locked");
            picker.answer(false);
        }
        let serial = SERIAL_COUNTER.next_serial();
        let time = smithay::backend::input::InputTime::now();
        let pointer = self.seat.get_pointer().unwrap();
        let keyboard = self.seat.get_keyboard().unwrap();
        // The pointer's grab first: ending a menu's pointer grab gives its keyboard grab's focus
        // back, which is taken away below. Only a grab there is: ending none still sends the
        // pointer's focus a motion.
        if pointer.is_grabbed() {
            pointer.unset_grab(self, serial, time);
        }
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        // Menus close, as a click elsewhere would close them.
        let surfaces: Vec<_> = self
            .all_open_windows()
            .iter()
            .chain(self.space.elements())
            .filter_map(|window| window.wl_surface().map(|surface| surface.into_owned()))
            .collect();
        for surface in surfaces {
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                if let PopupKind::Xdg(popup) = popup {
                    popup.send_popup_done();
                }
            }
        }
        // `HELD` first, so nothing below or after is logged at trace level.
        HELD.store(true, Ordering::Relaxed);
        let now = self.wall();
        self.lock = Some(Lock::new(remembered, now, self.clock.reduced_motion));
        self.unlocking = None;
        // A core dump taken while the password is in memory would hold it.
        // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) };
        self.clear_focus();
        let at = pointer.current_location();
        pointer.motion(
            self,
            None,
            &MotionEvent {
                location: at,
                serial,
                time,
            },
        );
        pointer.frame(self);
        // A cursor a client drew is its pixels; the lock shows the plain one.
        self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        let dh = self.display_handle.clone();
        smithay::wayland::selection::data_device::set_data_device_focus(&dh, &self.seat, None);
        smithay::wayland::selection::primary_selection::set_primary_focus(&dh, &self.seat, None);
        self.logind.locked_hint(true);
        tracing::info!(?reason, "locked");
        true
    }

    /// Takes the lock down: the window that had the keyboard gets it back, if it's still open.
    pub fn unlock(&mut self) {
        let Some(mut lock) = self.lock.take() else {
            return;
        };
        lock.forget_password();
        let remembered = lock.remembered.take();
        let now = self.wall();
        let facts = self.lock_facts();
        self.unlocking = Some(lock.leave(&facts, now));
        HELD.store(false, Ordering::Relaxed);
        // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1) };
        self.logind.locked_hint(false);
        tracing::info!("unlocked");
        self.wake_ui();
        match remembered.filter(|window| window.alive() && self.workspaces.find(window).is_some()) {
            Some(window) => self.focus_window(&window),
            None => self.restore_focus(),
        }
        // The pointer enters whatever it is over.
        let at = self.pointer_location();
        self.pointer_moved_to(at, smithay::backend::input::InputTime::now());
    }

    /// A key for the pill, as the keyboard filter passed it on.
    pub fn lock_key(&mut self, key: Key) {
        let now = self.wall();
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        if let Outcome::Check(id) = lock.key(key, now) {
            let answers = self.lock_answers.clone();
            crate::auth::start_check(lock.password(), move |verdict| {
                let _ = answers.send((id, verdict));
            });
            lock.forget_password();
        }
    }

    /// A password check answered.
    pub fn lock_checked(&mut self, id: u64, verdict: Verdict) {
        let now = self.wall();
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        match &verdict {
            Verdict::Unlock => tracing::info!("password accepted"),
            Verdict::Refused(_) => tracing::info!("password refused"),
            Verdict::Failed => tracing::warn!("couldn't check the password"),
        }
        if lock.checked(id, verdict, now) == Outcome::Unlock {
            self.unlock();
        }
    }

    /// A pass of the event loop: the screen locks by itself once idle for its minutes, and a
    /// sleep waiting on the lock is let go once every screen has drawn it.
    pub fn tick_lock(&mut self) {
        let now = std::time::Instant::now();
        // Caps Lock keeps the wallpaper from fading but never holds off the lock: it's a key
        // anyone can leave on, so it mustn't leave an unattended desktop unlocked.
        let held = self.fullscreen_on_screen();
        if self.lock.is_none() && self.idle.lock_due(now, held) {
            tracing::info!("idle long enough to lock");
            self.idle.restart_lock_wait(now);
            self.lock_for(Reason::ByItself);
        }
        if self.sleep_delay.waiting() {
            let drawn = self.lock.as_ref().is_some_and(|lock| {
                self.screens
                    .iter()
                    .all(|screen| lock.drawn.contains(&screen.output.name()))
            });
            if self.sleep_delay.tick(self.wall(), drawn) {
                tracing::info!(drawn, "the lock is on screen; sleep can go ahead");
            }
        }
    }

    /// A frame of `output` built while locked is on the display now, so a sleep waiting on the lock
    /// can count this screen. Called once the frame has been shown, not when it was built: a
    /// frame that failed to render or queue never counts.
    pub fn lock_frame_shown(&mut self, output: &str) {
        if let Some(lock) = self.lock.as_mut()
            && !lock.drawn.iter().any(|name| name == output)
        {
            lock.drawn.push(output.to_string());
        }
    }

    /// Something logind said about this session.
    pub fn logind_event(&mut self, event: crate::logind::Event) {
        use crate::logind::{Event, Response};
        match crate::logind::respond(&event, self.settings.lock.before_sleep) {
            Response::Lock => {
                self.lock_now();
            }
            Response::Unlock => {
                if self.lock.is_some() {
                    tracing::info!("logind unlocked the session");
                }
                self.unlock();
            }
            Response::LockForSleep => {
                if self.lock_for(Reason::ByItself) {
                    let now = self.wall();
                    self.sleep_delay.wait(now);
                    // The loop may have nothing else to wake it before the grace runs out.
                    let grace = std::time::Duration::from_secs_f64(crate::logind::SLEEP_GRACE);
                    let _ = self.loop_handle.insert_source(
                        smithay::reexports::calloop::timer::Timer::from_duration(grace),
                        |_, _, state| {
                            state.tick_lock();
                            smithay::reexports::calloop::timer::TimeoutAction::Drop
                        },
                    );
                } else {
                    self.sleep_delay.release();
                }
            }
            Response::Release => self.sleep_delay.release(),
            Response::Reinhibit => self.logind.inhibit(),
            Response::Hold => {
                if let Event::Inhibitor(fd) = event {
                    self.sleep_delay.hold(fd);
                }
            }
            Response::Warn => self.show_toast(
                "Sleep won't lock first",
                "logind can't be reached, so loginctl can't lock or unlock this session either.",
            ),
        }
    }

    /// What the card shows, from the latest readings.
    pub fn lock_facts(&self) -> Facts {
        let status = self.status.lock().unwrap();
        Facts {
            time: status.time.clone(),
            date: Facts::long_date(status.today),
            user: self.user.clone(),
            network: status.network.clone(),
            battery: status.battery,
            caps_lock: self
                .seat
                .get_keyboard()
                .is_some_and(|keyboard| keyboard.modifier_state().caps_lock),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(lock: &mut Lock<()>, text: &str) {
        for ch in text.chars() {
            lock.key(Key::Type(ch), 0.0);
        }
    }

    #[test]
    fn typing_appends_and_esc_clears() {
        let mut lock = Lock::<()>::new(None, 0.0, false);
        typed(&mut lock, "pässwörd");
        assert_eq!(lock.password(), "pässwörd".as_bytes());
        lock.key(Key::Erase, 0.0);
        assert_eq!(lock.password(), "pässwör".as_bytes());
        assert_eq!(lock.view(&Facts::default(), 0.0).dots, 7);
        lock.key(Key::Clear, 0.0);
        assert!(lock.password().is_empty());
        lock.key(Key::Erase, 0.0);
        assert!(lock.password().is_empty(), "erasing nothing is fine");
    }

    #[test]
    fn the_password_stops_at_512_bytes() {
        let mut lock = Lock::<()>::new(None, 0.0, false);
        typed(&mut lock, &"a".repeat(600));
        assert_eq!(lock.password().len(), PASSWORD_LIMIT);
        let mut lock = Lock::<()>::new(None, 0.0, false);
        typed(&mut lock, &"a".repeat(PASSWORD_LIMIT - 1));
        lock.key(Key::Type('é'), 0.0);
        assert_eq!(
            lock.password().len(),
            PASSWORD_LIMIT - 1,
            "a character that would cross the limit isn't cut in half"
        );
        // The buffer never grew, so no copy of the password was left behind by a move.
        assert_eq!(lock.password.0.capacity(), PASSWORD_LIMIT);
    }

    #[test]
    fn nothing_is_accepted_while_checking() {
        let mut lock = Lock::<()>::new(None, 0.0, false);
        assert_eq!(lock.key(Key::Submit, 0.0), Outcome::Nothing, "empty");
        typed(&mut lock, "secret");
        let Outcome::Check(id) = lock.key(Key::Submit, 0.0) else {
            panic!("a check should start");
        };
        lock.forget_password();
        for key in [Key::Type('x'), Key::Submit, Key::Clear, Key::Erase] {
            assert_eq!(lock.key(key, 0.0), Outcome::Nothing);
        }
        assert!(lock.password().is_empty());
        assert_eq!(
            lock.checked(id + 1, Verdict::Unlock, 0.0),
            Outcome::Nothing,
            "another check's answer"
        );
        assert!(lock.checking());
        assert_eq!(lock.checked(id, Verdict::Unlock, 0.0), Outcome::Unlock);
    }

    #[test]
    fn a_refusal_clears_the_field() {
        let mut lock = Lock::<()>::new(None, 0.0, false);
        typed(&mut lock, "wrong");
        let Outcome::Check(id) = lock.key(Key::Submit, 1.0) else {
            panic!("a check should start");
        };
        assert_eq!(
            lock.checked(id, Verdict::Refused(None), 2.0),
            Outcome::Nothing
        );
        assert!(lock.password().is_empty() && !lock.checking());
        let view = lock.view(&Facts::default(), 2.0);
        assert_eq!((view.hint.as_str(), view.amber), ("Wrong password", true));
        assert!(lock.shake(2.0 + 0.3 * 0.25) < 0.0, "the pill shakes");
        typed(&mut lock, "right");
        assert_eq!(lock.view(&Facts::default(), 3.0).hint, "Enter to unlock");

        let Outcome::Check(id) = lock.key(Key::Submit, 4.0) else {
            panic!("a check should start");
        };
        lock.checked(id, Verdict::Failed, 4.0);
        let caps = Facts {
            caps_lock: true,
            ..Facts::default()
        };
        assert_eq!(
            lock.view(&caps, 4.0).hint,
            "Couldn't check the password · Caps Lock is on"
        );
    }

    #[test]
    fn only_media_and_vt_keys_act() {
        assert_eq!(
            allowed(Keysym::XF86_AudioMute),
            Some(Action::ToggleMute { microphone: false })
        );
        assert_eq!(
            allowed(Keysym::XF86_MonBrightnessUp),
            Some(Action::Brightness(5))
        );
        assert_eq!(allowed(Keysym::XF86_Switch_VT_3), Some(Action::SwitchVt(3)));
        for key in [
            Keysym::Return,
            Keysym::Tab,
            Keysym::Escape,
            Keysym::F4,
            Keysym::l,
            Keysym::Super_L,
            // Screenshots, and Super+D.
            Keysym::Print,
            Keysym::s,
            Keysym::d,
        ] {
            assert_eq!(allowed(key), None, "{key:?}");
        }
        let plain = ModifiersState::default();
        let ctrl = ModifiersState {
            ctrl: true,
            ..plain
        };
        let logo = ModifiersState {
            logo: true,
            ..plain
        };
        assert_eq!(key_for(Keysym::a, Keysym::A, &plain), Key::Type('A'));
        assert_eq!(
            key_for(Keysym::a, Keysym::Cyrillic_ef, &plain),
            Key::Type('ф'),
            "the layout's own character"
        );
        assert_eq!(key_for(Keysym::u, Keysym::u, &ctrl), Key::Clear);
        assert_eq!(
            key_for(Keysym::BackSpace, Keysym::BackSpace, &ctrl),
            Key::Clear
        );
        assert_eq!(key_for(Keysym::Return, Keysym::Return, &logo), Key::Submit);
        assert_eq!(
            key_for(Keysym::l, Keysym::l, &logo),
            Key::Nothing,
            "no chord types"
        );
        assert_eq!(key_for(Keysym::Tab, Keysym::Tab, &plain), Key::Nothing);
    }

    #[test]
    fn no_pam_file_no_lock() {
        assert!(!can_lock(|_| false));
        assert!(can_lock(
            |file| file == Path::new("/usr/lib/pam.d/slipstream")
        ));
        assert!(can_lock(|file| file == Path::new("/etc/pam.d/slipstream")));
        assert!(!can_lock(|file| file == Path::new("/etc/pam.d/kde")));
    }

    #[test]
    fn lock_steps_are_off_by_default() {
        // No test turns the switch on, so this is how the login session starts.
        assert!(!steps_allowed());
        assert!(!steps_allowed_for(false));
        assert!(steps_allowed_for(true));
    }

    #[test]
    fn unlocking_fades_the_card_and_brings_the_desktop_forward() {
        let unlocking = Lock::<()>::new(None, 0.0, false).leave(&Facts::default(), 10.0);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
        let (zoom, opacity) = unlocking.desktop(10.0);
        assert!(close(zoom, 0.93) && close(opacity as f64, 0.3));
        let (alpha, grow) = unlocking.card_state(10.0);
        assert!(close(alpha as f64, 1.0) && close(grow, 1.0));
        let (alpha, grow) = unlocking.card_state(10.0 + CARD_OUT);
        assert!(close(alpha as f64, 0.0) && close(grow, 1.05));
        let (zoom, opacity) = unlocking.desktop(10.0 + DESKTOP_IN);
        assert!(close(zoom, 1.0) && close(opacity as f64, 1.0));
        assert!(unlocking.done(10.0 + DESKTOP_IN));
        let reduced = Lock::<()>::new(None, 0.0, true).leave(&Facts::default(), 0.0);
        assert_eq!(reduced.desktop(0.04).0, 1.0, "no zoom");
        assert_eq!(reduced.card_state(0.04).1, 1.0);
        assert!(reduced.done(REDUCED_FADE));
        let mut lock = Lock::<()>::new(None, 0.0, true);
        lock.key(Key::Submit, 0.0);
        assert_eq!(lock.shake(0.05), 0.0, "no shake");
    }
}
