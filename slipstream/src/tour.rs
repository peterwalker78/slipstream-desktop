//! The tour (Super+/, then Enter): what tiling is, why this desktop works the way it does, and
//! the places a window can be besides a tile, in short lessons, each one drawn happening.
//!
//! Someone coming from overlapping windows doesn't get stuck because there are too many keys.
//! They get stuck because the desktop does something they didn't ask for — a second window opens
//! and the first one shrinks — and nothing on screen says why. So each lesson says why first, and
//! then shows it on a miniature of the desktop: its bar, apps that look like apps, the ring in the
//! ring's own colour and the streams at the edge, going through the move on a loop while the key
//! that does it lights up underneath.
//!
//! Every lesson can be tried on the spot. Its keys play the move on the miniature rather than on
//! the real windows: a tour that rearranged your actual work would be a tour nobody runs twice.
//!
//! The scenes are plain data in unit squares, so what happens is decided and tested here without
//! a compositor; `miniature.rs` paints them and `sheet.rs` holds the card.

use slipstream_config::KeyStyle;
use smithay::input::keyboard::Keysym;

use crate::keys::{Action, App};

/// The parts the lessons are grouped into, named across the top of the card.
pub const CHAPTERS: [&str; 5] = [
    "Basics",
    "Arranging",
    "Getting around",
    "Beyond the tiles",
    "Good to know",
];

/// One lesson.
pub struct Lesson {
    /// Which of `CHAPTERS` it belongs to.
    pub chapter: usize,
    pub title: &'static str,
    /// Why it works this way. Said before what to press, because the why is what makes the keys
    /// stick. Keys are written in braces with the Windows style's names, and shown in the style
    /// the keys follow (`KeyNames::fill`).
    pub why: &'static str,
    /// What it's like for hands that know Windows, where that helps. Shown in the Windows style.
    pub habit: Option<&'static str>,
    /// The keys it teaches, drawn as keycaps, in the Windows style's names (`KeyNames::cap`).
    /// None where there is nothing to press.
    pub keys: &'static [&'static str],
    /// How many moves the miniature makes before it starts again.
    pub beats: usize,
    /// Where reduced motion holds the miniature, as a share of the loop: the arrangement the
    /// lesson is about.
    pub still: f32,
}

/// The lesson that asks whose keys to follow, first so that every lesson after it shows them.
pub const CHOOSE_KEYS: usize = 0;

/// The keys that choose a style on that lesson, each with its keycap.
pub const STYLE_KEYS: [(Keysym, &str, KeyStyle); 3] = [
    (Keysym::w, "W", KeyStyle::Windows),
    (Keysym::m, "M", KeyStyle::Mac),
    (Keysym::o, "O", KeyStyle::Omarchy),
];

pub const LESSONS: [Lesson; 14] = [
    Lesson {
        chapter: 0,
        title: "Keys your hands already know",
        why: "Coming from Windows, a Mac or Omarchy? Pick one and the desktop's keys follow the \
              habits you already have, in this tour and everywhere else. The Shortcuts page in \
              Settings changes it later.",
        habit: None,
        // The choice is drawn in place of keys to try.
        keys: &[],
        beats: 1,
        still: 0.66,
    },
    Lesson {
        chapter: 0,
        title: "Windows arrange themselves",
        why: "Elsewhere, new windows pile up on top of each other and the day goes on dragging \
              them apart. Here a new window takes a share of the screen and the others make room, \
              so nothing is ever buried and nothing needs the mouse.",
        habit: Some("Like snapping a window to the side, except every window is snapped, always."),
        keys: &["Super+⏎"],
        beats: 4,
        still: 1.0,
    },
    Lesson {
        chapter: 0,
        title: "Open anything by name",
        why: "{tap} and type a few letters of what you want, and Enter opens it beside what you \
              were doing. The same box finds recent files, does sums and converts units.",
        habit: Some("The Start menu habit — tap the key, type, Enter — works unchanged."),
        keys: &["Super"],
        beats: 3,
        still: 0.66,
    },
    Lesson {
        chapter: 0,
        title: "The keyboard goes where you point",
        why: "Super and an arrow send your typing to the window in that direction. You always know \
              where a window is, even when you don't remember when you last used it. The ring \
              shows which window has the keyboard, and clicking still works.",
        habit: Some(
            "Win+arrows snap a window elsewhere. Here windows are already in place, so \
                     the arrows move you between them.",
        ),
        keys: &["Super+← → ↑ ↓"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 1,
        title: "Move a window, not the mouse",
        why: "Hold {mod:Super+Alt+←} as well and the window comes with you, swapping places with \
              its neighbour. Layouts are built by moving windows into place, not by dragging \
              edges to the pixel, so they come out tidy every time.",
        habit: Some(
            "Instead of dragging a title bar, hold Super and drag one window onto another.",
        ),
        keys: &["Super+Alt+← → ↑ ↓"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 1,
        title: "Give it more room",
        why: "{Super+]} gives a window more of the space it shares and {Super+[} gives it less; \
              with Shift they do the same for height. Its neighbour always takes the rest, so \
              there is never a gap to tidy, and halves and thirds catch as you pass them.",
        habit: None,
        keys: &["Super+]", "Super+["],
        beats: 3,
        still: 1.0,
    },
    Lesson {
        chapter: 1,
        title: "Or let them place themselves",
        why: "Tiling keeps windows where you put them. Gravity arranges them round the one you're \
              in: in the centre, wide, in a spotlight or in a grid. {Super+T} switches between \
              the two; keep Super held and press {key:Super+T} again to step through them.",
        habit: None,
        keys: &["Super+T"],
        beats: 4,
        still: 0.5,
    },
    Lesson {
        chapter: 2,
        title: "A workspace for each task",
        why: "Give each task a workspace of its own: code on 1, messages on 2, music on 3. Super \
              and a number goes straight there, and with {mod:Super+Shift+1} the window comes \
              too. On a touchpad, swipe three fingers sideways. Each one keeps its layout while \
              you're away.",
        habit: Some("Windows calls these virtual desktops. Here each one is a single key away."),
        keys: &["Super+1–9", "Super+Shift+1–9"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 2,
        title: "See everything at once",
        why: "{Super+Tab} tips the desktop back so every workspace sits side by side, with a letter \
              on each window: press the letter and you're there. Everything slows to a quarter \
              speed while you look, so nothing moves under you. On a touchpad, swipe three \
              fingers up.",
        habit: None,
        keys: &["Super+Tab"],
        beats: 3,
        still: 0.32,
    },
    Lesson {
        chapter: 3,
        title: "Lift a window off the tiles",
        why: "A calculator, a video or a chat you only glance at doesn't want a tile. \
              {Super+Shift+V} lifts a window off to float above the rest, and again puts it back. \
              {Super+Ctrl+V} takes the keyboard down to the tiles, and back up.",
        habit: Some(
            "The kind of window Windows has. Hold Super and drag to move it with the \
                     mouse.",
        ),
        keys: &["Super+Shift+V", "Super+Ctrl+V"],
        beats: 4,
        still: 0.5,
    },
    Lesson {
        chapter: 3,
        title: "Out of sight, still running",
        why: "{Super+M} pours a window into a stream at the edge of the screen, where a taskbar \
              would be. The stream shows its app at work, slow and grey when idle and fast and \
              bright when busy, so a build finishing catches your eye. {Super+H} puts every \
              window there at once.",
        habit: Some("Minimise and Win+D, with the apps still in sight."),
        keys: &["Super+M", "Super+Shift+M", "Super+H"],
        beats: 3,
        still: 0.66,
    },
    Lesson {
        chapter: 3,
        title: "Keep one window in view",
        why: "A build, a call or a video is for watching, not working in. {Super+W} hangs the \
              window from the bar as a pane of glass, in front of every workspace. Super+Up from \
              the top row puts the keyboard in it; Super and any arrow brings it back out.",
        habit: Some("Always on top, for one window, on every desktop at once."),
        keys: &["Super+W", "Super+Shift+W"],
        beats: 4,
        still: 0.5,
    },
    Lesson {
        chapter: 4,
        title: "Nothing breaks your concentration",
        why: "Notifications that arrive while you're typing wait for a pause, then come as one \
              card. A window you didn't ask for goes to the edge instead of taking your \
              keyboard. Critical alerts still come straight through.",
        habit: None,
        // Nothing to press: the point is what happens when you do nothing.
        keys: &[],
        beats: 4,
        still: 0.9,
    },
    Lesson {
        chapter: 4,
        title: "Every key, one press away",
        why: "You don't need to remember any of this. {Super+/} lists every key, and typing \
              narrows the list to what you're after. Enter there brings this tour back.",
        habit: None,
        keys: &["Super+/"],
        beats: 2,
        still: 1.0,
    },
];

/// How long one beat of a lesson takes: the rest, the move and the rest after it.
pub const BEAT: f64 = 1.9;
/// The share of a beat spent still, at each end, so a move reads as a move.
const REST: f32 = 0.3;
/// Where in a beat its key goes down: just before the move, as the state changes on the press
/// and the animation follows.
const PRESS: f32 = REST - 0.06;
/// How long the key stays lit after the press, as a share of a beat.
const GLOW: f32 = 0.5;

/// How long lesson `step` runs before it starts again, in seconds.
pub fn loop_secs(step: usize) -> f64 {
    LESSONS.get(step).map_or(1, |lesson| lesson.beats) as f64 * BEAT
}

/// The kind of app a miniature window is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Terminal,
    Browser,
    Editor,
    Files,
    Music,
    Chat,
}

impl Kind {
    /// What the bar says of it when it has the keyboard.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Terminal => "Terminal — ~/code/app",
            Kind::Browser => "Reading list — Web",
            Kind::Editor => "main.rs — Editor",
            Kind::Files => "Documents",
            Kind::Music => "Night Drive — Music",
            Kind::Chat => "Messages",
        }
    }
}

/// A rectangle in a unit square: 0,0 top left to 1,1 bottom right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A window on the miniature.
#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    pub kind: Kind,
    /// Where it sits in its workspace's tiling area.
    pub at: Rect,
    /// 0 where it has gone, into the rain or closed.
    pub alpha: f32,
    pub workspace: u8,
    /// Floating above the tiles, with a shadow.
    pub floating: bool,
    /// How far up to the bar it is: 0 among the windows, 1 hanging from the bar in front of
    /// every workspace.
    pub dock: f32,
    /// Its letter in bullet time.
    pub label: Option<char>,
}

/// A minimised app's stream at the right edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub kind: Kind,
    /// How busy the app is, 0 to 1: the rain's colour and speed.
    pub busy: f32,
    /// How much of its width it has taken, 0 to 1, as it opens or closes.
    pub open: f32,
}

/// A key going down on the miniature, and how lit it still is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    pub keys: &'static str,
    pub glow: f32,
}

/// A box being typed into: how many characters are in, and how visible it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typed {
    pub chars: usize,
    pub alpha: f32,
}

/// Super+T's strip of arrangements while Super is held: which one is in force, counted along
/// `arrangements()`, and how visible the strip is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choosing {
    pub selected: usize,
    pub alpha: f32,
}

/// What the miniature shows at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Back to front.
    pub windows: Vec<Win>,
    /// Which window the ring is around, if any.
    pub focus: Option<usize>,
    pub streams: Vec<Stream>,
    /// Which workspace is in view, fractional while sliding between them.
    pub camera: f32,
    /// Bullet time, 0 on the desktop to 1 tipped right back.
    pub zoom: f32,
    /// The workspace the bar has lit, 0-based.
    pub bar_workspace: u8,
    pub key: Option<Press>,
    /// Words in the key's place, when what's happening isn't a key.
    pub caption: Option<&'static str>,
    pub explorer: Option<Typed>,
    pub choosing: Option<Choosing>,
    /// The bell's count on the bar.
    pub bell: u8,
    /// The card of notifications held while typing, 0 to 1.
    pub digest: f32,
    /// How much of the terminal's command has been typed, if it's being typed.
    pub typing: Option<f32>,
    pub sheet: Option<Typed>,
    /// The docked window's strip in the bar: its app, how busy it is and how far it has opened.
    pub slot: Option<Stream>,
}

impl Scene {
    fn desk(windows: Vec<Win>, focus: Option<usize>) -> Self {
        Self {
            windows,
            focus,
            streams: Vec::new(),
            camera: 0.0,
            zoom: 0.0,
            bar_workspace: 0,
            key: None,
            caption: None,
            explorer: None,
            choosing: None,
            bell: 0,
            digest: 0.0,
            typing: None,
            sheet: None,
            slot: None,
        }
    }
}

/// Where a loop is: which beat, how far its move has got (resting at both ends), whether its key
/// has gone down, and how lit that key still is.
struct Beat {
    index: usize,
    moved: f32,
    pressed: bool,
    glow: f32,
}

fn beat(t: f32, beats: usize) -> Beat {
    let at = t.clamp(0.0, 0.9999) * beats as f32;
    let local = at.fract();
    let pressed = local >= PRESS;
    Beat {
        index: at as usize,
        moved: move_along(local),
        pressed,
        glow: if pressed {
            (1.0 - (local - PRESS) / GLOW).clamp(0.0, 1.0)
        } else {
            0.0
        },
    }
}

/// The loop position at which beat `index` of lesson `step` presses its key.
fn press_at(step: usize, index: usize) -> f32 {
    let beats = LESSONS.get(step).map_or(1, |lesson| lesson.beats) as f32;
    // A hair past the press, so rounding can't land it just before.
    (index as f32 + PRESS + 0.001) / beats
}

/// Eases a beat's progress into a move that rests at both ends: still, moves, still.
fn move_along(t: f32) -> f32 {
    let t = ((t - REST) / (1.0 - 2.0 * REST)).clamp(0.0, 1.0);
    // Smoothstep, so it sets off and arrives gently rather than snapping.
    t * t * (3.0 - 2.0 * t)
}

fn between(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn tween(from: Rect, to: Rect, t: f32) -> Rect {
    Rect {
        x: between(from.x, to.x, t),
        y: between(from.y, to.y, t),
        w: between(from.w, to.w, t),
        h: between(from.h, to.h, t),
    }
}

/// `r` shrunk by `by` of its size about its centre: how a window arriving or leaving is drawn.
fn shrunk(r: Rect, by: f32) -> Rect {
    Rect {
        x: r.x + r.w * by / 2.0,
        y: r.y + r.h * by / 2.0,
        w: r.w * (1.0 - by),
        h: r.h * (1.0 - by),
    }
}

/// The gaps between tiles, across and down. The area is wider than it is tall, so the gap down is
/// the larger share to look the same.
const GAP_X: f32 = 0.012;
const GAP_Y: f32 = 0.02;

const FULL: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

/// `r` split side by side, the first side getting `ratio` of it.
fn split_x(r: Rect, ratio: f32) -> (Rect, Rect) {
    let w = (r.w - GAP_X) * ratio;
    (
        Rect { w, ..r },
        Rect {
            x: r.x + w + GAP_X,
            w: r.w - w - GAP_X,
            ..r
        },
    )
}

/// `r` split one above the other, the top getting `ratio` of it.
fn split_y(r: Rect, ratio: f32) -> (Rect, Rect) {
    let h = (r.h - GAP_Y) * ratio;
    (
        Rect { h, ..r },
        Rect {
            y: r.y + h + GAP_Y,
            h: r.h - h - GAP_Y,
            ..r
        },
    )
}

/// The first `n` windows' places as each new window splits the one before it along its longer
/// side, as the tiling does: one fills the area, then halves, then the right half in two, then
/// its lower part in two.
fn dwindle(n: usize) -> Vec<Rect> {
    let (left, right) = split_x(FULL, 0.5);
    let (top_right, bottom_right) = split_y(right, 0.5);
    let (lower_left, lower_right) = split_x(bottom_right, 0.5);
    match n {
        0 => vec![],
        1 => vec![FULL],
        2 => vec![left, right],
        3 => vec![left, top_right, bottom_right],
        _ => vec![left, top_right, lower_left, lower_right],
    }
}

fn win(kind: Kind, at: Rect) -> Win {
    Win {
        kind,
        at,
        alpha: 1.0,
        workspace: 0,
        floating: false,
        dock: 0.0,
        label: None,
    }
}

/// Where a docked window hangs at `w` by `h` of the tiling area: under the bar, at the right-hand
/// end.
fn hung(w: f32, h: f32) -> Rect {
    Rect {
        x: 1.0 - w,
        y: 0.0,
        w,
        h,
    }
}

fn press(keys: &'static str, beat: &Beat) -> Option<Press> {
    Some(Press {
        keys,
        glow: beat.glow,
    })
}

/// `r` with the tiles' gap taken off each side that meets a neighbour rather than the edge of the
/// area.
fn gapped(r: Rect) -> Rect {
    let (left, top) = (r.x > 0.001, r.y > 0.001);
    let (right, bottom) = (r.x + r.w < 0.999, r.y + r.h < 0.999);
    let half = |on: bool, gap: f32| if on { gap / 2.0 } else { 0.0 };
    Rect {
        x: r.x + half(left, GAP_X),
        y: r.y + half(top, GAP_Y),
        w: r.w - half(left, GAP_X) - half(right, GAP_X),
        h: r.h - half(top, GAP_Y) - half(bottom, GAP_Y),
    }
}

/// Where the gravity lesson's four windows sit in each arrangement on Super+T's strip, in the
/// strip's order: tiling, grid, centre, wide, spotlight. The windows are a terminal, a browser,
/// an editor and files; the editor has the keyboard, so gravity arranges the rest round it.
pub fn arrangements() -> [[Rect; 4]; 5] {
    let at = |x: f32, y: f32, w: f32, h: f32| gapped(Rect { x, y, w, h });
    let tiled = dwindle(4);
    let third = 1.0 / 3.0;
    [
        [tiled[0], tiled[1], tiled[2], tiled[3]],
        [
            at(0.0, 0.0, 0.5, 0.5),
            at(0.5, 0.0, 0.5, 0.5),
            at(0.0, 0.5, 0.5, 0.5),
            at(0.5, 0.5, 0.5, 0.5),
        ],
        [
            at(0.0, 0.0, 0.205, 1.0),
            at(0.795, 0.0, 0.205, 0.655),
            at(0.205, 0.0, 0.59, 1.0),
            at(0.795, 0.655, 0.205, 0.345),
        ],
        [
            at(0.7, third, 0.3, third),
            at(0.7, 0.0, 0.3, third),
            at(0.0, 0.0, 0.7, 1.0),
            at(0.7, 2.0 * third, 0.3, third),
        ],
        [
            at(third, 0.75, third, 0.25),
            at(0.0, 0.75, third, 0.25),
            at(0.0, 0.0, 1.0, 0.75),
            at(2.0 * third, 0.75, third, 0.25),
        ],
    ]
}

/// The three windows most lessons start from: a terminal down the left, a browser and an editor
/// stacked on the right.
fn three() -> Vec<Win> {
    let places = dwindle(3);
    vec![
        win(Kind::Terminal, places[0]),
        win(Kind::Browser, places[1]),
        win(Kind::Editor, places[2]),
    ]
}

/// The workspaces of the lessons that go between them: code on 1, messages on 2, music on 3,
/// with bullet time's letters on each window.
fn workspaces() -> Vec<Win> {
    let (left, right) = split_x(FULL, 0.5);
    let (chat, web) = split_x(FULL, 0.42);
    let on = |mut w: Win, workspace: u8, label: char| {
        w.workspace = workspace;
        w.label = Some(label);
        w
    };
    vec![
        on(win(Kind::Terminal, left), 0, 'j'),
        on(win(Kind::Editor, right), 0, 'k'),
        on(win(Kind::Chat, chat), 1, 'l'),
        on(win(Kind::Browser, web), 1, 'f'),
        on(win(Kind::Music, FULL), 2, 'h'),
    ]
}

/// The window each workspace of `workspaces()` has the keyboard on.
fn focused_on(workspace: u8) -> usize {
    match workspace {
        0 => 1,
        1 => 3,
        _ => 4,
    }
}

/// What lesson `step` shows, `t` of the way through its loop (0 to 1).
pub fn scene(step: usize, t: f32) -> Scene {
    let beats = LESSONS.get(step).map_or(1, |lesson| lesson.beats);
    let b = beat(t, beats);
    match step {
        // The explorer opening on the chosen style's key, which the miniature names.
        CHOOSE_KEYS => {
            let (left, right) = split_x(FULL, 0.5);
            let windows = vec![win(Kind::Terminal, left), win(Kind::Browser, right)];
            let mut scene = Scene::desk(windows, Some(1));
            scene.explorer = Some(Typed {
                chars: 0,
                alpha: if b.pressed { b.moved.max(0.15) } else { 0.0 },
            });
            scene.key = press("Super", &b);
            scene
        }
        // Windows open one after another, each taking a share and the others making room.
        1 => {
            let kinds = [Kind::Terminal, Kind::Browser, Kind::Editor, Kind::Files];
            let (before, after) = (dwindle(b.index), dwindle(b.index + 1));
            let windows = (0..=b.index)
                .map(|i| match before.get(i) {
                    Some(from) => win(kinds[i], tween(*from, after[i], b.moved)),
                    None => Win {
                        alpha: b.moved,
                        ..win(kinds[i], shrunk(after[i], 0.08 * (1.0 - b.moved)))
                    },
                })
                .collect();
            let focus = match (b.pressed, b.index) {
                (true, i) => Some(i),
                (false, 0) => None,
                (false, i) => Some(i - 1),
            };
            let mut scene = Scene::desk(windows, focus);
            if !b.pressed && b.index == 0 {
                scene.windows[0].alpha = 0.0;
            }
            scene.key = press("Super+⏎", &b);
            scene
        }
        // The explorer: tap Super, type, Enter, and the app opens beside what you were doing.
        2 => {
            let (left, right) = split_x(FULL, 0.5);
            let (top, bottom) = split_y(right, 0.5);
            let mut windows = vec![win(Kind::Terminal, left), win(Kind::Browser, right)];
            let mut scene;
            match b.index {
                0 => {
                    scene = Scene::desk(windows, Some(1));
                    scene.explorer = Some(Typed {
                        chars: 0,
                        alpha: if b.pressed { b.moved.max(0.15) } else { 0.0 },
                    });
                    scene.key = press("Super", &b);
                }
                1 => {
                    scene = Scene::desk(windows, Some(1));
                    scene.explorer = Some(Typed {
                        chars: (b.moved * 5.0).ceil() as usize,
                        alpha: 1.0,
                    });
                    scene.caption = Some("typing");
                }
                _ => {
                    windows[1].at = tween(right, top, b.moved);
                    windows.push(Win {
                        alpha: b.moved,
                        ..win(Kind::Music, shrunk(bottom, 0.08 * (1.0 - b.moved)))
                    });
                    scene = Scene::desk(windows, Some(if b.pressed { 2 } else { 1 }));
                    scene.explorer = Some(Typed {
                        chars: 5,
                        alpha: if b.pressed { 1.0 - b.moved } else { 1.0 },
                    });
                    scene.key = press("⏎", &b);
                }
            }
            scene
        }
        // The ring goes right, down, then back left.
        3 => {
            let path = [(0, 1, "Super+→"), (1, 2, "Super+↓"), (2, 0, "Super+←")];
            let (from, to, keys) = path[b.index.min(2)];
            let mut scene = Scene::desk(three(), Some(if b.pressed { to } else { from }));
            scene.key = press(keys, &b);
            scene
        }
        // The terminal swaps its way right, down, then back left.
        4 => {
            let p = dwindle(3);
            let places = [
                [p[0], p[1], p[2]],
                [p[1], p[0], p[2]],
                [p[2], p[0], p[1]],
                [p[0], p[2], p[1]],
            ];
            let i = b.index.min(2);
            let mut windows = three();
            for (n, window) in windows.iter_mut().enumerate() {
                window.at = tween(places[i][n], places[i + 1][n], b.moved);
            }
            let mut scene = Scene::desk(windows, Some(0));
            scene.key = press(["Super+Alt+→", "Super+Alt+↓", "Super+Alt+←"][i], &b);
            scene
        }
        // Three presses widen the terminal, the last one catching on two thirds.
        5 => {
            let ratios = [0.5, 0.55, 0.6, 2.0 / 3.0];
            let i = b.index.min(2);
            let ratio = between(ratios[i], ratios[i + 1], b.moved);
            let (left, right) = split_x(FULL, ratio);
            let mut scene = Scene::desk(
                vec![win(Kind::Terminal, left), win(Kind::Browser, right)],
                Some(0),
            );
            scene.key = press("Super+]", &b);
            scene
        }
        // Super+T turns gravity on round the editor and, with Super still down, shows the
        // arrangements; each further T takes the next one; letting go of Super keeps it.
        6 => {
            let all = arrangements();
            let (from, to) = match b.index {
                0 => (0, 2),
                1 => (2, 3),
                2 => (3, 4),
                _ => (4, 4),
            };
            let kinds = [Kind::Terminal, Kind::Browser, Kind::Editor, Kind::Files];
            let windows = (0..4)
                .map(|n| win(kinds[n], tween(all[from][n], all[to][n], b.moved)))
                .collect();
            let mut scene = Scene::desk(windows, Some(2));
            // The strip comes up a moment after the first press, and goes as Super is let go.
            let alpha = match (b.index, b.pressed) {
                (0, false) => 0.0,
                (0, true) => (b.moved * 2.0).min(1.0),
                (3, true) => 1.0 - b.moved,
                _ => 1.0,
            };
            scene.choosing = Some(Choosing {
                selected: if b.pressed { to } else { from },
                alpha,
            });
            match b.index {
                0 => scene.key = press("Super+T", &b),
                1 | 2 => scene.key = press("T", &b),
                _ => scene.caption = Some("let go of Super"),
            }
            scene
        }
        // Workspace 1 to 2 to 3 and home again, the screens sliding past.
        7 => {
            let path = [(0u8, 1u8, "Super+2"), (1, 2, "Super+3"), (2, 0, "Super+1")];
            let (from, to, keys) = path[b.index.min(2)];
            let now = if b.pressed { to } else { from };
            let mut scene = Scene::desk(workspaces(), Some(focused_on(now)));
            scene.camera = between(from as f32, to as f32, b.moved);
            scene.bar_workspace = now;
            scene.key = press(keys, &b);
            scene
        }
        // Bullet time opens, F goes to the browser on 2, and Super+1 comes home.
        8 => {
            let mut scene = Scene::desk(workspaces(), Some(focused_on(0)));
            match b.index {
                0 => {
                    scene.zoom = if b.pressed { b.moved } else { 0.0 };
                    scene.key = press("Super+Tab", &b);
                }
                1 => {
                    scene.zoom = 1.0 - b.moved;
                    scene.camera = b.moved;
                    if b.pressed {
                        scene.focus = Some(3);
                        scene.bar_workspace = 1;
                    }
                    scene.key = press("F", &b);
                }
                _ => {
                    scene.camera = 1.0 - b.moved;
                    scene.focus = Some(if b.pressed { focused_on(0) } else { 3 });
                    scene.bar_workspace = if b.pressed { 0 } else { 1 };
                    scene.key = press("Super+1", &b);
                }
            }
            scene
        }
        // The browser lifts off its tile and the editor takes the room; it is nudged across and
        // made wider; then the keyboard goes down to the tiles under it.
        9 => {
            let tiled = dwindle(3);
            let (_, right) = split_x(FULL, 0.5);
            let lifted = Rect {
                x: 0.46,
                y: 0.12,
                ..tiled[1]
            };
            let nudged = Rect { x: 0.22, ..lifted };
            let wider = Rect {
                w: nudged.w + 0.14,
                ..nudged
            };
            let (from, to, keys) = match b.index {
                0 => (tiled[1], lifted, "Super+Shift+V"),
                1 => (lifted, nudged, "Super+Alt+←"),
                2 => (nudged, wider, "Super+]"),
                _ => (wider, wider, "Super+Ctrl+V"),
            };
            let mut windows = three();
            windows[1].at = tween(from, to, b.moved);
            windows[1].floating = b.index > 0 || b.pressed;
            windows[2].at = match b.index {
                0 => tween(tiled[2], right, b.moved),
                _ => right,
            };
            let below = b.index == 3 && b.pressed;
            let mut scene = Scene::desk(windows, Some(if below { 2 } else { 1 }));
            scene.key = press(keys, &b);
            scene
        }
        // The busy terminal pours into the rain, then the idle browser, then the browser comes back.
        10 => {
            let (top, bottom) = split_y(FULL, 0.5);
            let mut windows = three();
            let gone = |from: Rect| Rect {
                x: 1.0,
                w: 0.02,
                ..from
            };
            let mut streams = vec![Stream {
                kind: Kind::Terminal,
                busy: 1.0,
                open: 1.0,
            }];
            let idle = Stream {
                kind: Kind::Browser,
                busy: 0.05,
                open: 1.0,
            };
            let (focus, keys) = match b.index {
                0 => {
                    windows[0].at = tween(windows[0].at, gone(windows[0].at), b.moved);
                    windows[0].alpha = 1.0 - b.moved;
                    windows[1].at = tween(windows[1].at, top, b.moved);
                    windows[2].at = tween(windows[2].at, bottom, b.moved);
                    streams[0].open = b.moved;
                    (if b.pressed { 1 } else { 0 }, "Super+M")
                }
                1 => {
                    windows[0].alpha = 0.0;
                    windows[1].at = tween(top, gone(top), b.moved);
                    windows[1].alpha = 1.0 - b.moved;
                    windows[2].at = tween(bottom, FULL, b.moved);
                    streams.push(Stream {
                        open: b.moved,
                        ..idle
                    });
                    (if b.pressed { 2 } else { 1 }, "Super+M")
                }
                _ => {
                    windows[0].alpha = 0.0;
                    windows[1].at = tween(gone(top), top, b.moved);
                    windows[1].alpha = b.moved;
                    windows[2].at = tween(FULL, bottom, b.moved);
                    streams.push(Stream {
                        open: 1.0 - b.moved,
                        ..idle
                    });
                    (if b.pressed { 1 } else { 2 }, "Super+Shift+M")
                }
            };
            let mut scene = Scene::desk(windows, Some(focus));
            scene.streams = streams;
            scene.key = press(keys, &b);
            scene
        }
        // The terminal goes up to the bar and the others take its room; it grows a size; the
        // workspace changes behind it; and it comes down among the windows it finds there.
        11 => {
            let tiled = dwindle(3);
            let (top, bottom) = split_y(FULL, 0.5);
            let (chat, music) = split_x(FULL, 0.42);
            let (music_top, music_bottom) = split_y(music, 0.5);
            let (small, medium) = (hung(0.26, 0.3), hung(0.36, 0.42));
            let on_two = |mut w: Win| {
                w.workspace = 1;
                w
            };
            // The pane is last, in front of everything.
            let mut windows = vec![
                win(Kind::Browser, top),
                win(Kind::Editor, bottom),
                on_two(win(Kind::Chat, chat)),
                on_two(win(Kind::Music, music)),
                win(Kind::Terminal, medium),
            ];
            windows[4].dock = 1.0;
            let mut scene;
            match b.index {
                0 => {
                    windows[0].at = tween(tiled[1], top, b.moved);
                    windows[1].at = tween(tiled[2], bottom, b.moved);
                    windows[4].at = tween(tiled[0], small, b.moved);
                    windows[4].dock = b.moved;
                    scene = Scene::desk(windows, Some(4));
                    scene.key = press("Super+W", &b);
                }
                1 => {
                    windows[4].at = tween(small, medium, b.moved);
                    scene = Scene::desk(windows, Some(4));
                    scene.key = press("Super+W", &b);
                }
                2 => {
                    scene = Scene::desk(windows, Some(if b.pressed { 3 } else { 4 }));
                    scene.camera = b.moved;
                    scene.bar_workspace = b.pressed as u8;
                    scene.key = press("Super+2", &b);
                }
                _ => {
                    windows[3].at = tween(music, music_top, b.moved);
                    windows[4].at = tween(medium, music_bottom, b.moved);
                    windows[4].dock = 1.0 - b.moved;
                    windows[4].workspace = 1;
                    scene = Scene::desk(windows, Some(if b.pressed { 4 } else { 3 }));
                    scene.camera = 1.0;
                    scene.bar_workspace = 1;
                    scene.key = press("Super+Shift+W", &b);
                }
            }
            let strip = scene.windows[4].dock;
            scene.slot = (strip > 0.0).then_some(Stream {
                kind: Kind::Terminal,
                busy: 1.0,
                open: strip,
            });
            scene
        }
        // Typing, while three notifications arrive and only the bell counts them; then a pause, and
        // they come as one card.
        12 => {
            let (left, right) = split_x(FULL, 0.55);
            let mut scene = Scene::desk(
                vec![win(Kind::Terminal, left), win(Kind::Browser, right)],
                Some(0),
            );
            let typed = t * beats as f32 / 3.0;
            scene.typing = Some(typed.min(1.0));
            let arrivals = [0.5, 1.3, 2.1];
            let at = t * beats as f32;
            scene.bell = arrivals.iter().filter(|arrival| at >= **arrival).count() as u8;
            if b.index < 3 {
                scene.caption = Some("typing");
            } else {
                scene.caption = Some("a pause");
                scene.digest = b.moved;
            }
            scene
        }
        // The key list opens over the windows, and typing narrows it.
        _ => {
            let (left, right) = split_x(FULL, 0.5);
            let mut scene = Scene::desk(
                vec![win(Kind::Editor, left), win(Kind::Browser, right)],
                Some(0),
            );
            match b.index {
                0 => {
                    scene.sheet = Some(Typed {
                        chars: 0,
                        alpha: if b.pressed { b.moved.max(0.15) } else { 0.0 },
                    });
                    scene.key = press("Super+/", &b);
                }
                _ => {
                    scene.sheet = Some(Typed {
                        chars: (b.moved * 4.0).ceil() as usize,
                        alpha: 1.0,
                    });
                    scene.caption = Some("typing");
                }
            }
            scene
        }
    }
}

/// The beat a key that steps through a lesson plays next, pressed `t` of the way through its
/// loop: the one after the beat it has just played, as far as `last`, or the first again.
fn next_beat(step: usize, t: f32, last: usize) -> usize {
    let beats = LESSONS.get(step).map_or(1, |lesson| lesson.beats);
    let now = beat(t, beats);
    if now.pressed && now.index < last {
        now.index + 1
    } else {
        0
    }
}

/// Where a key pressed during lesson `step`, `t` of the way through its loop, takes that loop,
/// if it's one the lesson teaches: to the moment the miniature makes that move, so trying the
/// key plays it at once. Anything else isn't the tour's to answer.
pub fn practise(step: usize, action: &Action, t: f32) -> Option<f32> {
    let index = match (step, action) {
        (CHOOSE_KEYS, Action::Explorer) => 0,
        (1, Action::Launch(App::Terminal)) => 1,
        (2, Action::Explorer) => 0,
        (3, Action::Focus(_)) => 0,
        (4, Action::MoveTile(_)) => 0,
        (5, Action::Resize(_)) => 0,
        // Each T with Super still held is the next arrangement along.
        (6, Action::ToggleGravity) => next_beat(6, t, 2),
        (7, Action::Workspace(_) | Action::MoveToWorkspace(_)) => 0,
        (8, Action::BulletTime) => 0,
        (9, Action::ToggleFloating) => 0,
        (9, Action::MoveTile(_)) => 1,
        (9, Action::Resize(_)) => 2,
        (9, Action::SwitchFloatingFocus) => 3,
        (10, Action::Minimise | Action::HideAll) => 0,
        (10, Action::Restore) => 2,
        // On the docked window, the key again is its next size.
        (11, Action::Dock) => next_beat(11, t, 1),
        (11, Action::Undock) => 3,
        _ => return None,
    };
    Some(press_at(step, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Direction;

    fn inside(r: Rect) -> bool {
        r.w >= 0.0 && r.h >= 0.0 && r.x >= -0.001 && r.y >= -0.001
    }

    #[test]
    fn every_lesson_draws_something_whole() {
        for step in 0..LESSONS.len() {
            for hundredth in 0..=100 {
                let scene = scene(step, hundredth as f32 / 100.0);
                assert!(!scene.windows.is_empty(), "lesson {step} shows nothing");
                for window in &scene.windows {
                    assert!(inside(window.at), "lesson {step}: {window:?}");
                    assert!((0.0..=1.0).contains(&window.alpha), "{window:?}");
                }
                if let Some(focus) = scene.focus {
                    assert!(
                        focus < scene.windows.len(),
                        "lesson {step}: the ring is on no window"
                    );
                }
                if let Some(key) = scene.key {
                    assert!((0.0..=1.0).contains(&key.glow));
                }
            }
        }
    }

    #[test]
    fn lessons_run_through_the_chapters_in_order() {
        let chapters: Vec<usize> = LESSONS.iter().map(|lesson| lesson.chapter).collect();
        assert!(
            chapters
                .windows(2)
                .all(|pair| pair[1] == pair[0] || pair[1] == pair[0] + 1)
        );
        assert_eq!(chapters.last(), Some(&(CHAPTERS.len() - 1)));
    }

    #[test]
    fn every_lesson_says_why_in_a_few_lines() {
        // Five lines or so of the card's column, in whichever style's keys.
        for style in slipstream_config::KeyStyle::ALL {
            let names = crate::keynames::KeyNames::new(style);
            for lesson in &LESSONS {
                let why = names.fill(lesson.why);
                assert!(
                    why.len() <= 280,
                    "{style:?}, {}: too long to read",
                    lesson.title
                );
                // The card's prose face has no arrows: they belong on keycaps.
                let prose = [lesson.title, &why, lesson.habit.unwrap_or_default()].concat();
                assert!(
                    !prose.contains(['←', '→', '↑', '↓', '⏎', '{', '}']),
                    "{style:?}, {}: a symbol the prose can't draw",
                    lesson.title
                );
                assert!((0.0..=1.0).contains(&lesson.still));
            }
        }
    }

    #[test]
    fn a_move_rests_at_both_ends() {
        assert_eq!(move_along(0.0), 0.0);
        assert_eq!(move_along(REST), 0.0);
        assert_eq!(move_along(1.0 - REST), 1.0);
        assert_eq!(move_along(1.0), 1.0);
        assert!(move_along(0.5) > 0.0 && move_along(0.5) < 1.0);
    }

    #[test]
    fn each_new_window_takes_a_share_and_the_others_make_room() {
        let before = scene(1, 0.0);
        assert_eq!(before.windows[0].alpha, 0.0, "the desktop starts empty");
        let end = scene(1, 1.0);
        assert_eq!(end.windows.len(), 4);
        let area: f32 = end.windows.iter().map(|w| w.at.w * w.at.h).sum();
        assert!(
            area > 0.95,
            "four windows share the whole area, not overlap: {area}"
        );
        assert_eq!(end.focus, Some(3), "the newest window has the keyboard");
    }

    #[test]
    fn the_ring_moves_on_the_press_not_after_the_animation() {
        let t = press_at(3, 0);
        assert_eq!(scene(3, t - 0.01).focus, Some(0));
        assert_eq!(scene(3, t + 0.001).focus, Some(1));
    }

    #[test]
    fn resizing_catches_on_two_thirds() {
        let end = scene(5, 1.0);
        let share = end.windows[0].at.w / (end.windows[0].at.w + end.windows[1].at.w);
        assert!((share - 2.0 / 3.0).abs() < 0.01, "{share}");
    }

    #[test]
    fn minimising_opens_a_stream_and_the_tiles_close_up() {
        let after = scene(10, 0.33);
        assert_eq!(after.streams.len(), 1);
        assert_eq!(after.streams[0].open, 1.0);
        assert_eq!(after.windows[0].alpha, 0.0);
        let still = scene(10, LESSONS[10].still);
        assert_eq!(still.streams.len(), 2, "a busy stream and an idle one");
        assert!(still.streams[0].busy > still.streams[1].busy);
    }

    #[test]
    fn notifications_wait_for_the_pause() {
        let typing = scene(12, 0.6);
        assert!(typing.bell >= 2, "the bell counts them");
        assert_eq!(typing.digest, 0.0, "but nothing pops up");
        let pause = scene(12, 1.0);
        assert_eq!(pause.bell, 3);
        assert_eq!(pause.digest, 1.0);
    }

    #[test]
    fn workspaces_slide_and_the_bar_follows_the_press() {
        let t = press_at(7, 0);
        assert_eq!(scene(7, t - 0.01).bar_workspace, 0);
        assert_eq!(scene(7, t + 0.001).bar_workspace, 1);
        assert_eq!(scene(7, 0.333).camera, 1.0);
    }

    #[test]
    fn trying_a_key_jumps_to_its_move() {
        let t = practise(3, &Action::Focus(Direction::Right), 0.0).unwrap();
        assert!(scene(3, t).key.is_some_and(|key| key.glow > 0.99), "lit");
        assert_eq!(
            practise(3, &Action::Minimise, 0.0),
            None,
            "not this lesson's key"
        );
        assert_eq!(practise(10, &Action::Restore, 0.0), Some(press_at(10, 2)));
        for step in 0..LESSONS.len() {
            for action in [Action::Explorer, Action::Maximise, Action::BulletTime] {
                if let Some(t) = practise(step, &action, 0.0) {
                    assert!((0.0..1.0).contains(&t));
                }
            }
        }
    }

    #[test]
    fn reduced_motion_holds_what_the_lesson_is_about() {
        assert!(scene(8, LESSONS[8].still).zoom > 0.99, "in bullet time");
        assert!(scene(9, LESSONS[9].still).windows[1].floating, "floating");
        assert_eq!(scene(11, LESSONS[11].still).windows[4].dock, 1.0, "docked");
        let gravity = scene(6, LESSONS[6].still);
        assert!(
            gravity.choosing.is_some_and(|strip| strip.alpha == 1.0),
            "the arrangements on show"
        );
    }

    #[test]
    fn holding_super_and_tapping_t_goes_along_the_arrangements() {
        let on = |t: f32| scene(6, t).choosing.unwrap();
        assert_eq!(on(0.0).alpha, 0.0, "nothing shows before the key");
        let mut t = practise(6, &Action::ToggleGravity, 0.0).unwrap();
        let mut seen = vec![on(t).selected];
        for _ in 0..2 {
            t = practise(6, &Action::ToggleGravity, t).unwrap();
            seen.push(on(t).selected);
        }
        assert_eq!(seen, [2, 3, 4], "centre, wide, spotlight");
        let kept = scene(6, 1.0);
        assert_eq!(
            kept.choosing.unwrap().alpha,
            0.0,
            "the strip goes with Super"
        );
        assert_eq!(
            kept.windows[2].at,
            arrangements()[4][2],
            "and the arrangement stays"
        );
        // Every arrangement gives the four windows the whole area between them.
        for places in arrangements() {
            let area: f32 = places.iter().map(|r| r.w * r.h).sum();
            assert!(area > 0.93 && area <= 1.0, "{area}");
        }
    }

    #[test]
    fn the_dock_key_again_is_the_next_size() {
        let first = practise(11, &Action::Dock, 0.0).unwrap();
        assert_eq!(first, press_at(11, 0));
        assert_eq!(practise(11, &Action::Dock, first), Some(press_at(11, 1)));
    }

    #[test]
    fn a_floated_window_leaves_its_tile_to_the_others() {
        let before = scene(9, 0.0);
        assert!(!before.windows[1].floating);
        let after = scene(9, 0.24);
        assert!(after.windows[1].floating);
        assert!(
            after.windows[2].at.h > before.windows[2].at.h,
            "its neighbour takes the room"
        );
        let t = press_at(9, 3);
        assert_eq!(scene(9, t - 0.01).focus, Some(1), "in the floating window");
        assert_eq!(scene(9, t + 0.001).focus, Some(2), "down in the tiles");
    }

    #[test]
    fn a_docked_window_stays_put_while_the_workspace_changes() {
        let docked = scene(11, 0.5);
        let away = scene(11, 0.74);
        assert_eq!(away.camera, 1.0, "on the next workspace");
        assert_eq!(away.windows[4].at, docked.windows[4].at);
        assert_eq!(away.windows[4].dock, 1.0);
        assert!(away.slot.is_some(), "its strip stays in the bar");
        let small = scene(11, 0.24).windows[4].at;
        assert!(docked.windows[4].at.w > small.w, "the next size is larger");
        let down = scene(11, 1.0);
        assert_eq!(down.windows[4].dock, 0.0);
        assert_eq!(down.windows[4].workspace, 1, "down where it was looking");
        assert!(down.slot.is_none());
    }

    /// The keys a keycap stands for: `Super+← → ↑ ↓` is four, `Super+1–9` nine.
    fn spelled_out(cap: &str) -> Vec<String> {
        let (mods, keys) = match cap.rfind('+') {
            Some(plus) => cap.split_at(plus + 1),
            None => ("", cap),
        };
        let keys = match keys {
            "1–9" => (1..=9).map(|n| n.to_string()).collect(),
            "⏎" => vec!["Enter".to_string()],
            _ => keys.split(' ').map(str::to_string).collect::<Vec<_>>(),
        };
        keys.into_iter().map(|key| format!("{mods}{key}")).collect()
    }

    #[test]
    fn a_lessons_keys_play_on_the_miniature_never_on_the_real_windows() {
        for style in slipstream_config::KeyStyle::ALL {
            let bindings = crate::keys::defaults(style);
            let names = crate::keynames::KeyNames::new(style);
            let sheet = names.cap("Super+/");
            for (step, lesson) in LESSONS.iter().enumerate() {
                let caps = lesson.keys.iter().map(|cap| names.cap(cap));
                for cap in caps.flat_map(|cap| spelled_out(&cap)) {
                    let action = match cap.as_str() {
                        // A tap, which is no binding.
                        "Super" => Action::Explorer,
                        // The key list is where the last lesson says this goes, tour or no tour.
                        _ if cap == sheet => continue,
                        _ => {
                            bindings
                                .iter()
                                .find(|b| crate::keys::label(b.mods, b.key) == cap)
                                .unwrap_or_else(|| {
                                    panic!("{style:?}, {}: {cap} is no key", lesson.title)
                                })
                                .action
                        }
                    };
                    assert!(
                        practise(step, &action, 0.0).is_some(),
                        "{style:?}, {}: {cap} would leave the tour and act on the real desktop",
                        lesson.title
                    );
                }
            }
        }
    }

    #[test]
    fn a_lesson_marks_every_key_that_differs_between_styles() {
        // Keys every style shares may be written plainly; any other would read the same in
        // every style, and be wrong in all but one.
        const SHARED: [&str; 2] = ["Alt+Tab", "Super+Up"];
        for lesson in &LESSONS {
            let unmarked: String = lesson.why.split(['{', '}']).step_by(2).collect();
            let plain = SHARED
                .iter()
                .fold(unmarked.clone(), |text, shared| text.replace(shared, ""));
            for modifier in ["Super+", "Alt+", "Ctrl+"] {
                assert!(
                    !plain.contains(modifier),
                    "{}: a {modifier} key outside braces",
                    lesson.title
                );
            }
        }
    }
}
