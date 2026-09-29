//! The tour (Super+/, then Enter): what tiling is, why this desktop works the way it does, and
//! what it does that others don't, in sixteen short lessons, each one drawn happening.
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

use crate::keys::{Action, App};

/// The parts the lessons are grouped into, named across the top of the card.
pub const CHAPTERS: [&str; 4] = ["Basics", "Arranging", "Getting around", "Why Slipstream"];

/// One lesson.
pub struct Lesson {
    /// Which of `CHAPTERS` it belongs to.
    pub chapter: usize,
    pub title: &'static str,
    /// Why it works this way. Said before what to press, because the why is what makes the keys
    /// stick.
    pub why: &'static str,
    /// What it's like for hands that know Windows, where that helps.
    pub habit: Option<&'static str>,
    /// The keys it teaches, drawn as keycaps.
    pub keys: &'static [&'static str],
    /// How many moves the miniature makes before it starts again.
    pub beats: usize,
    /// Where reduced motion holds the miniature, as a share of the loop: the arrangement the
    /// lesson is about.
    pub still: f32,
}

pub const LESSONS: [Lesson; 16] = [
    Lesson {
        chapter: 0,
        title: "Windows arrange themselves",
        why: "Elsewhere, each new window lands on top of the last, and the day goes on dragging, \
              resizing and digging for the one underneath. Here a new window takes a share of the \
              screen and the others make room. Nothing is buried, and nothing needs the mouse to \
              put it where it goes.",
        habit: Some("Like snapping a window to the side, except every window is snapped, always."),
        keys: &["Super+⏎"],
        beats: 4,
        still: 1.0,
    },
    Lesson {
        chapter: 0,
        title: "Open anything by name",
        why: "Tap Super on its own and type a few letters of what you want. Enter opens it beside \
              what you were doing, so it's quicker than any menu once your hands know it. The \
              same box finds recent files, does sums and converts units.",
        habit: Some("The Start menu habit — tap the key, type, Enter — works unchanged."),
        keys: &["Super"],
        beats: 3,
        still: 0.66,
    },
    Lesson {
        chapter: 0,
        title: "The keyboard goes where you point",
        why: "Super and an arrow send your typing to the window in that direction. A direction is \
              easier than a list: you know where a window is without remembering when you last \
              used it. The ring shows which window has the keyboard, and clicking still works.",
        habit: Some(
            "Win+arrows snap a window elsewhere. Here windows are already in place, so \
                     the arrows move you between them.",
        ),
        keys: &["Super+← → ↑ ↓"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 0,
        title: "Your Windows keys still work",
        why: "Alt+Tab deals your windows as a deck of glass, each Tab sending the front one to the \
              back. Alt+F4 closes a window, and the rest close up the gap it leaves. Super+L \
              locks, Super+E opens your files and Print takes a screenshot.",
        habit: None,
        keys: &["Alt+Tab", "Alt+F4"],
        beats: 4,
        still: 0.3,
    },
    Lesson {
        chapter: 1,
        title: "Move a window, not the mouse",
        why: "Hold Alt as well and the window comes with you, swapping places with its neighbour. \
              Layouts are built by moving windows into place rather than by dragging edges to \
              the pixel, so they come out tidy every time.",
        habit: Some("With a mouse, hold Super and drag one window onto another."),
        keys: &["Super+Alt+← → ↑ ↓"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 1,
        title: "Give it more room",
        why: "Super+] gives a window more of the space it shares and Super+[ gives it less; with \
              Shift they do the same for height. The split moves, not the window, so its \
              neighbour always takes up the rest and there is never a gap to tidy. Halves and \
              thirds catch as you pass them.",
        habit: None,
        keys: &["Super+]", "Super+["],
        beats: 3,
        still: 1.0,
    },
    Lesson {
        chapter: 1,
        title: "Turn the whole layout",
        why: "When the shape is right but the wrong way round, with a wide window across the top \
              that you wanted down the side, Super+R turns the layout a quarter with every window \
              in it. Shift turns it back. Nothing has to be closed or moved by hand.",
        habit: None,
        keys: &["Super+R", "Super+Shift+R"],
        beats: 2,
        still: 0.5,
    },
    Lesson {
        chapter: 1,
        title: "One thing at a time",
        why: "Super+F lets a window fill the whole area while you concentrate on it. Press it \
              again and everything is back exactly where it was, because nothing else was closed \
              or moved.",
        habit: Some("Maximise, without losing the layout underneath."),
        keys: &["Super+F"],
        beats: 2,
        still: 0.5,
    },
    Lesson {
        chapter: 1,
        title: "Dialogs float",
        why: "Save dialogs, splash screens and picture-in-picture float above the tiles, centred \
              over the window they belong to, because squeezing a dialog into a tile helps \
              nobody. Super+Shift+V floats any window, or tiles it again.",
        habit: None,
        keys: &["Super+Shift+V"],
        beats: 4,
        still: 0.75,
    },
    Lesson {
        chapter: 1,
        title: "Or let them place themselves",
        why: "Tiling puts windows where you put them. Gravity arranges them around the one that \
              matters: in the centre, wide, in a spotlight or in a grid. Move a window into the \
              centre and it becomes the centre. Keep Super held after Super+T to choose the \
              arrangement.",
        habit: None,
        keys: &["Super+T"],
        beats: 3,
        still: 0.33,
    },
    Lesson {
        chapter: 2,
        title: "A workspace for each task",
        why: "Rather than one crowded screen, give each task its own: code on 1, messages on 2, \
              music on 3. Super and a number goes straight there, and with Shift the window comes \
              too. Each workspace keeps its layout while you're away, so changing task never \
              means rearranging.",
        habit: Some("Windows calls these virtual desktops. Here each one is a single key away."),
        keys: &["Super+1–9", "Super+Shift+1–9"],
        beats: 3,
        still: 0.34,
    },
    Lesson {
        chapter: 2,
        title: "See everything at once",
        why: "Super+Tab tips the desktop back so every workspace sits side by side, with a letter \
              on each window. Press the letter and you're there. While you look, everything slows \
              to a quarter of its speed so nothing moves under you, and catches up when you leave.",
        habit: None,
        keys: &["Super+Tab"],
        beats: 3,
        still: 0.32,
    },
    Lesson {
        chapter: 2,
        title: "Out of sight, still running",
        why: "Super+M pours a window into a stream at the edge of the screen instead of a taskbar. \
              The stream is a live readout of its app: slow and grey while it's idle, fast and \
              bright green when it's busy, so a build finishing catches your eye. Super+H puts \
              every window there at once.",
        habit: Some("Minimise and Win+D, with the apps still in sight."),
        keys: &["Super+M", "Super+Shift+M", "Super+H"],
        beats: 3,
        still: 0.66,
    },
    Lesson {
        chapter: 3,
        title: "Nothing breaks your concentration",
        why: "Most desktops let any app interrupt you mid-sentence. Here, notifications that \
              arrive while you're typing wait for a natural pause and then come as one card, and \
              a window you didn't ask for goes to the edge instead of taking your keyboard. \
              Critical alerts still come straight through.",
        habit: None,
        keys: &["Super+N"],
        beats: 4,
        still: 0.9,
    },
    Lesson {
        chapter: 3,
        title: "Yours, on any Linux",
        why: "No accounts and no telemetry. Settings take effect the moment you change them, and \
              are kept in one plain text file you can read, copy or keep with your dotfiles. The \
              same desktop runs on any distribution, with the apps and tools you already use.",
        habit: None,
        keys: &["Super+I"],
        beats: 2,
        still: 0.5,
    },
    Lesson {
        chapter: 3,
        title: "Every key, one press away",
        why: "You don't need to remember any of this. Super+/ lists every key, and typing narrows \
              the list to what you're after. Enter there brings this tour back, and an empty \
              workspace always says how to find it.",
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
    Settings,
    /// A terminal showing the settings file.
    Config,
    /// A save dialog.
    Dialog,
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
            Kind::Settings => "Settings",
            Kind::Config => "Terminal — ~/.config/slipstream",
            Kind::Dialog => "Save as",
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

/// Alt+Tab's deck: which windows are in it front to back, how far the front one has gone round
/// to the back, and how visible it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Deck {
    pub order: Vec<usize>,
    pub turn: f32,
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
    pub deck: Option<Deck>,
    /// The bell's count on the bar.
    pub bell: u8,
    /// The card of notifications held while typing, 0 to 1.
    pub digest: f32,
    /// How much of the terminal's command has been typed, if it's being typed.
    pub typing: Option<f32>,
    /// The switch being flipped in Settings, 0 off to 1 on.
    pub switch: f32,
    pub sheet: Option<Typed>,
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
            deck: None,
            bell: 0,
            digest: 0.0,
            typing: None,
            switch: 0.0,
            sheet: None,
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
        label: None,
    }
}

fn press(keys: &'static str, beat: &Beat) -> Option<Press> {
    Some(Press {
        keys,
        glow: beat.glow,
    })
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
        // Windows open one after another, each taking a share and the others making room.
        0 => {
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
        1 => {
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
        2 => {
            let path = [(0, 1, "Super+→"), (1, 2, "Super+↓"), (2, 0, "Super+←")];
            let (from, to, keys) = path[b.index.min(2)];
            let mut scene = Scene::desk(three(), Some(if b.pressed { to } else { from }));
            scene.key = press(keys, &b);
            scene
        }
        // Alt+Tab deals the deck twice, Alt lets go on the terminal, then Alt+F4 closes it.
        3 => {
            let mut windows = three();
            let mut scene;
            match b.index {
                0 => {
                    scene = Scene::desk(windows, Some(2));
                    if b.pressed {
                        scene.deck = Some(Deck {
                            order: vec![2, 1, 0],
                            turn: b.moved,
                            alpha: (b.moved * 3.0).clamp(0.2, 1.0),
                        });
                    }
                    scene.key = press("Alt+Tab", &b);
                }
                1 => {
                    scene = Scene::desk(windows, Some(2));
                    scene.deck = Some(Deck {
                        order: vec![1, 0, 2],
                        turn: if b.pressed { b.moved } else { 0.0 },
                        alpha: 1.0,
                    });
                    scene.key = press("Tab", &b);
                }
                2 => {
                    scene = Scene::desk(windows, Some(if b.pressed { 0 } else { 2 }));
                    scene.deck = Some(Deck {
                        order: vec![0, 2, 1],
                        turn: 0.0,
                        alpha: if b.pressed { 1.0 - b.moved } else { 1.0 },
                    });
                    scene.caption = Some("let go of Alt");
                }
                _ => {
                    let (top, bottom) = split_y(FULL, 0.5);
                    windows[0].alpha = if b.pressed { 1.0 - b.moved } else { 1.0 };
                    windows[0].at = shrunk(windows[0].at, 0.08 * b.moved);
                    windows[1].at = tween(windows[1].at, top, b.moved);
                    windows[2].at = tween(windows[2].at, bottom, b.moved);
                    scene = Scene::desk(windows, Some(if b.pressed { 1 } else { 0 }));
                    scene.key = press("Alt+F4", &b);
                }
            }
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
        // A wide window across the top turns to run down the right, and back.
        6 => {
            let (top, bottom) = split_y(FULL, 0.5);
            let (bottom_left, bottom_right) = split_x(bottom, 0.5);
            let (left, right) = split_x(FULL, 0.5);
            let (left_top, left_bottom) = split_y(left, 0.5);
            let across = [top, bottom_left, bottom_right];
            let down = [right, left_top, left_bottom];
            let (from, to, keys) = match b.index {
                0 => (across, down, "Super+R"),
                _ => (down, across, "Super+Shift+R"),
            };
            let kinds = [Kind::Browser, Kind::Terminal, Kind::Editor];
            let windows = (0..3)
                .map(|n| win(kinds[n], tween(from[n], to[n], b.moved)))
                .collect();
            let mut scene = Scene::desk(windows, Some(0));
            scene.key = press(keys, &b);
            scene
        }
        // The editor fills the area over the others, and goes back.
        7 => {
            let mut windows = three();
            let (from, to) = match b.index {
                0 => (windows[2].at, FULL),
                _ => (FULL, windows[2].at),
            };
            windows[2].at = tween(from, to, b.moved);
            let mut scene = Scene::desk(windows, Some(2));
            scene.key = press("Super+F", &b);
            scene
        }
        // A save dialog floats over its window and goes; then the browser floats, and tiles again.
        8 => {
            let (left, right) = split_x(FULL, 0.5);
            let floated = Rect {
                x: 0.24,
                y: 0.12,
                w: 0.52,
                h: 0.76,
            };
            let dialog = Rect {
                x: right.x + right.w * 0.16,
                y: 0.3,
                w: right.w * 0.68,
                h: 0.36,
            };
            let mut windows = vec![win(Kind::Terminal, left), win(Kind::Browser, right)];
            let mut scene;
            match b.index {
                0 | 1 => {
                    let alpha = match (b.index, b.pressed) {
                        (0, true) => b.moved,
                        (0, false) => 0.0,
                        (_, true) => 1.0 - b.moved,
                        (_, false) => 1.0,
                    };
                    windows.push(Win {
                        alpha,
                        floating: true,
                        ..win(Kind::Dialog, shrunk(dialog, 0.1 * (1.0 - alpha)))
                    });
                    let open = (b.index == 0) == b.pressed;
                    scene = Scene::desk(windows, Some(if open { 2 } else { 1 }));
                    scene.key = press(if b.index == 0 { "Ctrl+S" } else { "⏎" }, &b);
                }
                2 => {
                    windows[0].at = tween(left, FULL, b.moved);
                    windows[1].at = tween(right, floated, b.moved);
                    windows[1].floating = b.pressed;
                    scene = Scene::desk(windows, Some(1));
                    scene.key = press("Super+Shift+V", &b);
                }
                _ => {
                    windows[0].at = tween(FULL, left, b.moved);
                    windows[1].at = tween(floated, right, b.moved);
                    windows[1].floating = b.moved < 1.0;
                    scene = Scene::desk(windows, Some(1));
                    scene.key = press("Super+Shift+V", &b);
                }
            }
            scene
        }
        // Tiling to gravity's centre, the browser moved into the centre, and back to tiling.
        9 => {
            let tiled = dwindle(4);
            let (side_top, side_bottom) = split_y(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.2,
                    h: 1.0,
                },
                0.5,
            );
            let centre = Rect {
                x: 0.2 + GAP_X,
                y: 0.0,
                w: 0.6 - 2.0 * GAP_X,
                h: 1.0,
            };
            let far = Rect {
                x: 0.8,
                y: 0.0,
                w: 0.2,
                h: 1.0,
            };
            // Terminal, browser, editor, files: the editor has the keyboard and takes the centre.
            let gravity = [side_top, side_bottom, centre, far];
            let swapped = [side_top, centre, side_bottom, far];
            let swapped_tiled = [tiled[0], tiled[2], tiled[1], tiled[3]];
            let (from, to, keys) = match b.index {
                0 => (tiled.clone(), gravity.to_vec(), "Super+T"),
                1 => (gravity.to_vec(), swapped.to_vec(), "Super+Alt+↓"),
                _ => (swapped.to_vec(), swapped_tiled.to_vec(), "Super+T"),
            };
            let kinds = [Kind::Terminal, Kind::Browser, Kind::Editor, Kind::Files];
            let windows = (0..4)
                .map(|n| win(kinds[n], tween(from[n], to[n], b.moved)))
                .collect();
            let mut scene = Scene::desk(windows, Some(2));
            scene.key = press(keys, &b);
            scene
        }
        // Workspace 1 to 2 to 3 and home again, the screens sliding past.
        10 => {
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
        11 => {
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
        // The busy terminal pours into the rain, then the idle browser, then the browser comes back.
        12 => {
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
        // Typing, while three notifications arrive and only the bell counts them; then a pause, and
        // they come as one card.
        13 => {
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
        // A switch flipped in Settings, and the line in the settings file changing with it.
        14 => {
            let (left, right) = split_x(FULL, 0.52);
            let mut scene = Scene::desk(
                vec![win(Kind::Settings, left), win(Kind::Config, right)],
                Some(0),
            );
            let on = if b.pressed { b.moved } else { 0.0 };
            scene.switch = if b.index == 0 { on } else { 1.0 - on };
            scene.key = press("Space", &b);
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

/// Where a key pressed during lesson `step` takes its loop, if it's one the lesson teaches: to
/// the moment the miniature makes that move, so trying the key plays it at once. Anything else
/// isn't the tour's to answer.
pub fn practise(step: usize, action: &Action) -> Option<f32> {
    let index = match (step, action) {
        (0, Action::Launch(App::Terminal)) => 1,
        (1, Action::Explorer) => 0,
        (2, Action::Focus(_)) => 0,
        (3, Action::CycleWindows { .. }) => 0,
        (3, Action::Close) => 3,
        (4, Action::MoveTile(_)) => 0,
        (5, Action::Resize(_)) => 0,
        (6, Action::Rotate { clockwise: true }) => 0,
        (6, Action::Rotate { clockwise: false }) => 1,
        (7, Action::Maximise) => 0,
        (8, Action::ToggleFloating) => 2,
        (9, Action::ToggleGravity) => 0,
        (10, Action::Workspace(_) | Action::MoveToWorkspace(_)) => 0,
        (11, Action::BulletTime) => 0,
        (12, Action::Minimise | Action::HideAll) => 0,
        (12, Action::Restore) => 2,
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
                if let Some(deck) = &scene.deck {
                    assert!(deck.order.iter().all(|n| *n < scene.windows.len()));
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
        // The card's column holds about eight lines of it.
        for lesson in &LESSONS {
            assert!(
                lesson.why.len() <= 330,
                "{}: too long to read",
                lesson.title
            );
            assert!(
                !lesson.keys.is_empty(),
                "{}: nothing to press",
                lesson.title
            );
            assert!((0.0..=1.0).contains(&lesson.still));
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
        let before = scene(0, 0.0);
        assert_eq!(before.windows[0].alpha, 0.0, "the desktop starts empty");
        let end = scene(0, 1.0);
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
        let t = press_at(2, 0);
        assert_eq!(scene(2, t - 0.01).focus, Some(0));
        assert_eq!(scene(2, t + 0.001).focus, Some(1));
    }

    #[test]
    fn turning_the_layout_turns_it_a_quarter() {
        let before = scene(6, 0.0);
        assert!(
            before.windows[0].at.w > before.windows[0].at.h,
            "wide across the top"
        );
        let after = scene(6, 0.49);
        assert!(
            after.windows[0].at.h > after.windows[0].at.w,
            "tall down the side"
        );
        assert!(after.windows[0].at.x > 0.4, "on the right");
    }

    #[test]
    fn resizing_catches_on_two_thirds() {
        let end = scene(5, 1.0);
        let share = end.windows[0].at.w / (end.windows[0].at.w + end.windows[1].at.w);
        assert!((share - 2.0 / 3.0).abs() < 0.01, "{share}");
    }

    #[test]
    fn minimising_opens_a_stream_and_the_tiles_close_up() {
        let after = scene(12, 0.33);
        assert_eq!(after.streams.len(), 1);
        assert_eq!(after.streams[0].open, 1.0);
        assert_eq!(after.windows[0].alpha, 0.0);
        let still = scene(12, LESSONS[12].still);
        assert_eq!(still.streams.len(), 2, "a busy stream and an idle one");
        assert!(still.streams[0].busy > still.streams[1].busy);
    }

    #[test]
    fn notifications_wait_for_the_pause() {
        let typing = scene(13, 0.6);
        assert!(typing.bell >= 2, "the bell counts them");
        assert_eq!(typing.digest, 0.0, "but nothing pops up");
        let pause = scene(13, 1.0);
        assert_eq!(pause.bell, 3);
        assert_eq!(pause.digest, 1.0);
    }

    #[test]
    fn workspaces_slide_and_the_bar_follows_the_press() {
        let t = press_at(10, 0);
        assert_eq!(scene(10, t - 0.01).bar_workspace, 0);
        assert_eq!(scene(10, t + 0.001).bar_workspace, 1);
        assert_eq!(scene(10, 0.333).camera, 1.0);
    }

    #[test]
    fn trying_a_key_jumps_to_its_move() {
        let t = practise(2, &Action::Focus(Direction::Right)).unwrap();
        assert!(scene(2, t).key.is_some_and(|key| key.glow > 0.99), "lit");
        assert_eq!(
            practise(2, &Action::Minimise),
            None,
            "not this lesson's key"
        );
        assert_eq!(practise(12, &Action::Restore), Some(press_at(12, 2)));
        for step in 0..LESSONS.len() {
            for action in [Action::Explorer, Action::Maximise, Action::BulletTime] {
                if let Some(t) = practise(step, &action) {
                    assert!((0.0..1.0).contains(&t));
                }
            }
        }
    }

    #[test]
    fn reduced_motion_holds_what_the_lesson_is_about() {
        assert_eq!(scene(7, LESSONS[7].still).windows[2].at, FULL, "filled");
        assert!(scene(8, LESSONS[8].still).windows[1].floating, "floating");
        assert!(scene(11, LESSONS[11].still).zoom > 0.99, "in bullet time");
    }
}
