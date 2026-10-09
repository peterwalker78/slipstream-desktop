//! The tour's miniature of the desktop: a `tour::Scene` painted as a small copy of the real thing,
//! with the bar across the top, apps drawn as apps, the ring in the ring's own colour, the streams
//! at the edge in the chosen effects, and the key being pressed shown underneath.
//!
//! Everything is laid out in the stage's own design pixels and scaled with the window it sits in,
//! so the same painting serves a tile, a pane in Alt+Tab's deck and a workspace far off in
//! bullet time.

use slipstream_config::Effects;

use crate::{
    icons,
    paint::Painter,
    panel,
    text::{self, Face, Style},
    tour::{Deck, Kind, Rect, Scene, Stream, Typed},
};

/// The miniature's own grid, which the caller scales to whatever size it's shown at: a screen of
/// the real one's shape.
pub const WIDTH: f32 = 640.0;
pub const HEIGHT: f32 = 400.0;

/// What the miniature follows from the real desktop, and the clock its rain falls by.
pub struct Look {
    pub ring: u32,
    pub effects: Effects,
    pub secs: f32,
}

// The miniature's measurements, in its design pixels.
const BAR_H: f32 = 15.0;
/// Around the tiling area, as the real one keeps an outer gap.
const EDGE: f32 = 7.0;
const STREAM_W: f32 = 17.0;
const STREAM_GAP: f32 = 3.0;
/// Between one workspace and the next as they slide past.
const WORKSPACE_GAP: f32 = 28.0;
/// How small the desktop gets in bullet time.
const OVERVIEW: f32 = 0.56;

// Colours. The wallpaper's and the bar's are the real ones; the apps' are their own.
const WALL: u32 = 0x0b0d12ff;
const CYAN: u32 = 0x56c7d9ff;
const GREEN: u32 = 0x7fd962ff;
const RED: u32 = 0xe8607aff;
const ORANGE: u32 = 0xe5a05bff;
const STRING: u32 = 0x98c379ff;
const DIM: u32 = 0x6b7383ff;
const TERMINAL_INK: u32 = 0xd7dce4ff;
const RAIN_BUSY: u32 = 0x3cf07aff;
const RAIN_HEAD: u32 = 0xd8ffe4ff;
const RAIN_IDLE: u32 = 0x7d8594ff;

/// `rgba` with its opacity multiplied by `alpha`.
fn fade(rgba: u32, alpha: f32) -> u32 {
    let a = ((rgba & 0xff) as f32 * alpha.clamp(0.0, 1.0)).round() as u32;
    (rgba & 0xffff_ff00) | a.min(255)
}

fn style(face: Face, px: f32, rgba: u32, alpha: f32) -> Style {
    Style::new(face, px, fade(rgba, alpha))
}

/// Text that stops at `right` rather than running past it. Returns where it ended.
fn clipped(p: &mut Painter, s: &str, x: f32, centre: f32, right: f32, style: &Style) -> f32 {
    let room = right - x;
    if room < style.px {
        return x;
    }
    let s = text::ellipsize(s, style, room);
    x + p.text(&s, x, centre, style)
}

/// The colour an app's icon is drawn in, wherever it's shown small.
fn tint(kind: Kind) -> u32 {
    match kind {
        Kind::Terminal => 0x2f3542ff,
        Kind::Browser => 0x3b82f6ff,
        Kind::Editor => 0x7c5cffff,
        Kind::Files => 0x62a0eaff,
        Kind::Music => 0xe5566cff,
        Kind::Chat => 0x2fb67cff,
    }
}

/// A small square app icon, with a mark inside that says which app.
fn app_icon(p: &mut Painter, kind: Kind, x: f32, y: f32, size: f32, alpha: f32) {
    p.fill(x, y, size, size, size * 0.24, fade(tint(kind), alpha));
    let ink = fade(0xffffffd8, alpha);
    let u = size / 10.0;
    match kind {
        Kind::Terminal => {
            p.fill(x + 2.0 * u, y + 3.0 * u, 2.5 * u, 1.2 * u, 0.0, ink);
            p.fill(x + 5.0 * u, y + 6.2 * u, 3.0 * u, 1.2 * u, 0.0, ink);
        }
        Kind::Browser => {
            p.border(x + 2.0 * u, y + 2.0 * u, 6.0 * u, 6.0 * u, 3.0 * u, u, ink);
        }
        Kind::Music => {
            p.fill(x + 3.0 * u, y + 5.5 * u, 2.6 * u, 2.6 * u, 1.3 * u, ink);
            p.fill(x + 4.8 * u, y + 2.0 * u, 1.0 * u, 5.0 * u, 0.0, ink);
        }
        Kind::Files => {
            p.fill(x + 2.0 * u, y + 3.5 * u, 6.0 * u, 4.0 * u, u * 0.6, ink);
        }
        _ => {
            p.fill(x + 2.5 * u, y + 2.5 * u, 5.0 * u, 5.0 * u, 1.2 * u, ink);
        }
    }
}

/// Where things are: the tiling area, and how bullet time and sliding workspaces move them.
struct Frame {
    w: f32,
    h: f32,
    area: (f32, f32, f32, f32),
    camera: f32,
    scale: f32,
    lift: f32,
}

impl Frame {
    fn new(w: f32, h: f32, scene: &Scene) -> Self {
        let taken: f32 = scene
            .streams
            .iter()
            .map(|stream| stream.open * (STREAM_W + STREAM_GAP))
            .sum();
        let (x, y) = (EDGE, BAR_H + EDGE);
        Self {
            w,
            h,
            area: (x, y, w - 2.0 * EDGE - taken, h - y - EDGE),
            camera: scene.camera,
            scale: 1.0 - (1.0 - OVERVIEW) * scene.zoom,
            lift: 8.0 * scene.zoom,
        }
    }

    /// A point on workspace `workspace`'s own screen, where it lands on the stage.
    fn point(&self, x: f32, y: f32, workspace: u8) -> (f32, f32) {
        let x = x + (workspace as f32 - self.camera) * (self.w + WORKSPACE_GAP);
        let (cx, cy) = (self.w / 2.0, (self.h + BAR_H) / 2.0);
        (
            cx + (x - cx) * self.scale,
            cy + (y - cy) * self.scale + self.lift,
        )
    }

    /// A window's place in the tiling area, on the stage.
    fn place(&self, at: Rect, workspace: u8) -> (f32, f32, f32, f32) {
        let (ax, ay, aw, ah) = self.area;
        let (x, y) = self.point(ax + at.x * aw, ay + at.y * ah, workspace);
        (x, y, at.w * aw * self.scale, at.h * ah * self.scale)
    }

    /// A window on its way up to the bar, `dock` of the way there: it leaves the workspaces'
    /// sliding behind, and rises the gap the tiles keep under the bar to hang from it.
    fn hung(&self, at: Rect, dock: f32) -> (f32, f32, f32, f32) {
        let (ax, ay, aw, ah) = self.area;
        (
            ax + at.x * aw,
            ay + at.y * ah - EDGE * dock,
            at.w * aw,
            at.h * ah,
        )
    }

    /// A whole workspace's screen, on the stage.
    fn screen(&self, workspace: u8) -> (f32, f32, f32, f32) {
        let (x, y) = self.point(0.0, BAR_H, workspace);
        (x, y, self.w * self.scale, (self.h - BAR_H) * self.scale)
    }
}

/// Paints `scene` on the miniature's grid, its corner at the painter's origin.
pub fn paint(p: &mut Painter, scene: &Scene, look: &Look) {
    let (w, h) = (WIDTH, HEIGHT);
    let frame = Frame::new(w, h, scene);
    wallpaper(p, w, h, look.secs);
    if scene.zoom > 0.0 {
        p.fill(0.0, 0.0, w, h, 0.0, fade(0x000000ff, 0.55 * scene.zoom));
        let last = scene
            .windows
            .iter()
            .map(|win| win.workspace)
            .max()
            .unwrap_or(0);
        for workspace in 0..=last {
            let (x, y, sw, sh) = frame.screen(workspace);
            p.fill(x, y, sw, sh, 4.0, fade(0x11151cff, scene.zoom));
            let edge = if workspace == scene.bar_workspace {
                fade(look.ring, scene.zoom)
            } else {
                fade(0xffffff22, scene.zoom)
            };
            p.border(x, y, sw, sh, 4.0, 1.0, edge);
        }
    }
    // Tiles first, then whatever floats above them, then the pane docked in front of both.
    for layer in 0..3 {
        for (index, win) in scene.windows.iter().enumerate() {
            let docked = win.dock > 0.0;
            let its_layer = match (docked, win.floating) {
                (true, _) => 2,
                (false, true) => 1,
                (false, false) => 0,
            };
            if its_layer != layer || win.alpha <= 0.01 {
                continue;
            }
            let (x, y, ww, wh) = if docked {
                frame.hung(win.at, win.dock)
            } else {
                frame.place(win.at, win.workspace)
            };
            if x > w || x + ww < 0.0 || ww < 1.0 || wh < 1.0 {
                continue;
            }
            if win.floating {
                p.shadow(x, y, ww, wh, 4.0, 6.0, 14.0, fade(0x000000aa, win.alpha));
            }
            if docked {
                // The bar's own material comes down round it, with a shadow on what is behind.
                let rim = 2.0 * win.dock;
                p.shadow(x, y, ww, wh, 4.0, 5.0, 12.0, fade(0x00000070, win.dock));
                p.fill(
                    x - rim,
                    y,
                    ww + 2.0 * rim,
                    wh + rim,
                    3.0,
                    fade(WALL, win.dock),
                );
                p.border(
                    x - rim,
                    y,
                    ww + 2.0 * rim,
                    wh + rim,
                    3.0,
                    0.6,
                    fade(0xffffff18, win.dock),
                );
            }
            window(p, win.kind, x, y, ww, wh, frame.scale, win.alpha, scene);
            if scene.focus == Some(index) && scene.deck.is_none() {
                p.border(
                    x - 1.0,
                    y - 1.0,
                    ww + 2.0,
                    wh + 2.0,
                    0.0,
                    1.6,
                    fade(look.ring, win.alpha),
                );
            }
        }
    }
    if scene.zoom > 0.5 {
        labels(p, scene, &frame);
    }
    streams(p, &scene.streams, &frame, look);
    bar(p, scene, w, look);
    if let Some(deck) = &scene.deck {
        deal(p, deck, scene, &frame, look);
    }
    if let Some(typed) = scene.explorer {
        explorer(p, typed, w, look);
    }
    if let Some(typed) = scene.sheet {
        sheet(p, typed, w, h);
    }
    if scene.digest > 0.0 {
        digest(p, scene.digest, w, h);
    }
    keys(p, scene, w, h, look);
}

/// The desktop behind everything: the wallpaper's own background with a few faint lines of air
/// drifting across it.
fn wallpaper(p: &mut Painter, w: f32, h: f32, secs: f32) {
    p.fill(0.0, 0.0, w, h, 0.0, WALL);
    for n in 0..9 {
        let n = n as f32;
        let y = BAR_H + 18.0 + (n * 37.0) % (h - BAR_H - 24.0);
        let length = 40.0 + (n * 53.0) % 90.0;
        let x = ((n * 131.0) + secs * (6.0 + n)) % (w + length) - length;
        p.fill(x, y, length, 0.8, 0.0, 0xffffff0c);
    }
}

/// The bar across the top: logo, workspaces, what has the keyboard, the clock and the tray.
fn bar(p: &mut Painter, scene: &Scene, w: f32, look: &Look) {
    p.fill(0.0, 0.0, w, BAR_H, 0.0, WALL);
    p.fill(0.0, BAR_H - 0.6, w, 0.6, 0.0, 0xffffff10);
    let centre = BAR_H / 2.0;
    p.icon(icons::LOGO, 4.0, 2.5, 10.0, None);
    let digits = Style::new(Face::Mono, 6.5, panel::TERTIARY);
    let lit = Style::new(Face::Mono, 6.5, panel::INK);
    for n in 0..5u8 {
        let x = 20.0 + n as f32 * 10.0;
        if n == scene.bar_workspace {
            p.fill(x - 2.6, 2.5, 8.8, 10.0, 2.0, 0xffffff14);
            p.fill(x - 1.4, 12.0, 6.4, 0.9, 0.4, look.ring | 0xff);
        }
        let label = (n + 1).to_string();
        p.text(
            &label,
            x,
            centre,
            if n == scene.bar_workspace {
                &lit
            } else {
                &digits
            },
        );
    }
    if let Some(win) = scene.focus.and_then(|index| scene.windows.get(index)) {
        let title = Style::new(Face::Body, 6.0, panel::SECONDARY);
        clipped(p, win.kind.title(), 74.0, centre, w * 0.4, &title);
    }
    let clock = Style::new(Face::Mono, 6.5, panel::INK);
    let date = Style::new(Face::Body, 6.0, panel::SECONDARY);
    let width = text::width("09:41", &clock) + 4.0 + text::width("Tue 29 Sep", &date);
    let x = w / 2.0 - width / 2.0;
    let x = x + p.text("09:41", x, centre, &clock) + 4.0;
    p.text("Tue 29 Sep", x, centre, &date);
    let ink = Some(panel::PROSE);
    if let Some(strip) = &scene.slot {
        slot(p, strip, scene, w - 106.0, look);
    }
    p.icon(icons::WIFI, w - 64.0, 3.5, 8.0, ink);
    p.icon(icons::VOLUME, w - 54.0, 3.5, 8.0, ink);
    p.border(w - 43.0, 5.2, 8.0, 4.6, 1.0, 0.7, panel::PROSE);
    p.fill(w - 41.8, 6.4, 4.4, 2.2, 0.4, panel::PROSE);
    p.text(
        "88%",
        w - 33.0,
        centre,
        &Style::new(Face::Mono, 5.5, panel::INK),
    );
    p.icon(icons::BELL, w - 13.0, 3.5, 8.0, ink);
    if scene.bell > 0 {
        p.fill(w - 8.5, 1.8, 6.0, 6.0, 3.0, panel::AMBER);
        let count = Style::new(Face::MonoBold, 4.5, panel::DARK);
        let label = scene.bell.to_string();
        let lw = text::width(&label, &count);
        p.text(&label, w - 5.5 - lw / 2.0, 4.8, &count);
    }
}

/// The docked window's strip in the bar, beside the tray: its stream laid on its side, running
/// at the pace of its app's load, with the ring's colour round it while the keyboard is in the
/// pane.
fn slot(p: &mut Painter, strip: &Stream, scene: &Scene, x: f32, look: &Look) {
    const W: f32 = 36.0;
    const H: f32 = 9.0;
    let open = strip.open.clamp(0.0, 1.0);
    let (y, w) = ((BAR_H - H) / 2.0, W * open);
    if w < 2.0 {
        return;
    }
    p.fill(x, y, w, H, 2.0, fade(0x151a23ff, open));
    let busy = strip.busy > 0.5;
    let colour = if busy { RAIN_BUSY } else { RAIN_IDLE };
    for n in 0..4 {
        let depth = 0.55 + 0.45 * ((n * 7) % 4) as f32 / 3.0;
        let length = 5.0 + 6.0 * depth;
        let run = W + length;
        let head = (look.secs * fall_speed(strip.busy) * depth + n as f32 * 23.0) % run;
        let (from, to) = ((head - length).max(1.0), head.min(w - 1.0));
        if to <= from {
            continue;
        }
        let sy = y + 1.6 + 1.9 * n as f32;
        p.fill(
            x + from,
            sy,
            to - from,
            0.9,
            0.4,
            fade(colour, open * 0.7 * depth),
        );
        if head < w - 1.0 {
            p.fill(
                x + head - 1.2,
                sy - 0.2,
                1.3,
                1.3,
                0.6,
                fade(if busy { RAIN_HEAD } else { colour }, open * depth),
            );
        }
    }
    let keyboard_in_it = scene
        .focus
        .and_then(|index| scene.windows.get(index))
        .is_some_and(|win| win.dock > 0.0);
    let edge = if keyboard_in_it {
        fade(look.ring | 0xff, open)
    } else {
        fade(0xffffff24, open)
    };
    p.border(x, y, w, H, 2.0, 0.7, edge);
}

/// Bullet time's letters, one on each window.
fn labels(p: &mut Painter, scene: &Scene, frame: &Frame) {
    let alpha = ((scene.zoom - 0.5) / 0.5).clamp(0.0, 1.0);
    let letter = Style::new(Face::MonoBold, 7.5, fade(panel::DARK, alpha));
    for win in &scene.windows {
        let Some(label) = win.label else { continue };
        let (x, y, w, h) = frame.place(win.at, win.workspace);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        p.fill(
            cx - 6.0,
            cy - 6.0,
            12.0,
            12.0,
            3.0,
            fade(panel::AMBER, alpha),
        );
        let s = label.to_ascii_uppercase().to_string();
        let lw = text::width(&s, &letter);
        p.text(&s, cx - lw / 2.0, cy, &letter);
    }
}

/// The minimised apps' streams down the right edge, each under its app's button, raining in the
/// chosen effects at the pace of its app's load.
fn streams(p: &mut Painter, streams: &[Stream], frame: &Frame, look: &Look) {
    let (ax, ay, aw, ah) = frame.area;
    let mut x = ax + aw + STREAM_GAP + 2.0;
    for (index, stream) in streams.iter().enumerate() {
        let open = stream.open.clamp(0.0, 1.0);
        if open <= 0.01 {
            continue;
        }
        let sw = STREAM_W * open;
        let button = STREAM_W * 1.2;
        p.fill(x, ay, sw, button, 3.0, fade(0x151a23ff, open));
        let icon = 7.0 * open;
        app_icon(p, stream.kind, x + (sw - icon) / 2.0, ay + 2.5, icon, open);
        let meter = sw * 0.6;
        let colour = if stream.busy > 0.5 {
            RAIN_BUSY
        } else {
            0x5ad78eff
        };
        p.fill(
            x + (sw - meter) / 2.0,
            ay + button - 3.2,
            meter * stream.busy.max(0.15),
            1.1,
            0.5,
            fade(colour, open),
        );
        let top = ay + button + 2.0;
        let bottom = ay + ah;
        p.fill(x, top, sw, bottom - top, 3.0, fade(0x0f131aff, open));
        match look.effects {
            Effects::Matrix => glyphs(p, stream, index, x, sw, top, bottom, look.secs, open),
            Effects::Slipstream => streaks(p, stream, index, x, sw, top, bottom, look.secs, open),
        }
        x += (STREAM_W + STREAM_GAP) * open;
    }
}

/// How fast a stream falls: slow when its app is idle, fast when it's busy.
fn fall_speed(busy: f32) -> f32 {
    14.0 + 70.0 * busy
}

/// Code rain: three columns of glyphs, a bright head and a fading tail.
#[allow(clippy::too_many_arguments)]
fn glyphs(
    p: &mut Painter,
    stream: &Stream,
    index: usize,
    x: f32,
    w: f32,
    top: f32,
    bottom: f32,
    secs: f32,
    alpha: f32,
) {
    const CHARS: &[u8] = b"01Z7E3K9T4=:+<>*";
    const PITCH: f32 = 7.4;
    const TAIL: usize = 16;
    let busy = stream.busy > 0.5;
    let run = bottom - top + TAIL as f32 * PITCH;
    for column in 0..3 {
        let phase = (index * 3 + column) as f32 * 53.0;
        let head = top + (secs * fall_speed(stream.busy) + phase) % run;
        let cx = x + w * (0.18 + 0.32 * column as f32);
        for n in 0..TAIL {
            let y = head - n as f32 * PITCH;
            if y < top + 2.0 || y > bottom - 2.0 {
                continue;
            }
            let fall = 1.0 - n as f32 / TAIL as f32;
            let colour = match (n, busy) {
                (0, true) => RAIN_HEAD,
                (_, true) => RAIN_BUSY,
                _ => RAIN_IDLE,
            };
            let which = (n + column * 5 + (head / PITCH) as usize) % CHARS.len();
            let ch = (CHARS[which] as char).to_string();
            p.text(
                &ch,
                cx - 2.0,
                y,
                &Style::new(Face::Mono, 6.6, fade(colour, alpha * fall)),
            );
        }
    }
}

/// Slipstream's own effects: streaks of light falling at different depths.
#[allow(clippy::too_many_arguments)]
fn streaks(
    p: &mut Painter,
    stream: &Stream,
    index: usize,
    x: f32,
    w: f32,
    top: f32,
    bottom: f32,
    secs: f32,
    alpha: f32,
) {
    let busy = stream.busy > 0.5;
    let colour = if busy { RAIN_BUSY } else { RAIN_IDLE };
    for n in 0..5 {
        let depth = 0.55 + 0.45 * ((n * 7 + index * 3) % 5) as f32 / 4.0;
        let length = 10.0 + 16.0 * depth;
        let run = bottom - top + length;
        let phase = (index * 5 + n) as f32 * 41.0;
        let head = top + (secs * fall_speed(stream.busy) * depth + phase) % run;
        let sx = x + w * (0.15 + 0.7 * ((n * 3) % 5) as f32 / 4.0);
        let from = (head - length).max(top);
        let to = head.min(bottom);
        if to <= from {
            continue;
        }
        let width = 0.6 + 1.1 * depth;
        p.fill(
            sx,
            from,
            width,
            to - from,
            width / 2.0,
            fade(colour, alpha * 0.55 * depth),
        );
        if head < bottom {
            p.fill(
                sx - width * 0.4,
                head - width * 1.8,
                width * 1.8,
                width * 1.8,
                width,
                fade(if busy { RAIN_HEAD } else { colour }, alpha * depth),
            );
        }
    }
}

/// One window, drawn as its app, `k` times its natural size.
#[allow(clippy::too_many_arguments)]
fn window(
    p: &mut Painter,
    kind: Kind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    k: f32,
    alpha: f32,
    scene: &Scene,
) {
    match kind {
        Kind::Terminal => terminal(p, x, y, w, h, k, alpha, scene.typing),
        Kind::Browser => browser(p, x, y, w, h, k, alpha),
        Kind::Editor => editor(p, x, y, w, h, k, alpha),
        Kind::Files => files(p, x, y, w, h, k, alpha),
        Kind::Music => music(p, x, y, w, h, k, alpha),
        Kind::Chat => chat(p, x, y, w, h, k, alpha),
    }
}

type Span = (&'static str, u32);

/// A terminal's lines of coloured spans, top down, as many as fit.
#[allow(clippy::too_many_arguments)]
fn lines(
    p: &mut Painter,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    k: f32,
    alpha: f32,
    rows: &[&[Span]],
) -> f32 {
    let pitch = 8.6 * k;
    let right = x + w - 4.0 * k;
    let mut line_y = y + 7.0 * k;
    for row in rows {
        if line_y + pitch / 2.0 > y + h {
            break;
        }
        let mut cx = x + 5.0 * k;
        for (text, colour) in *row {
            cx = clipped(
                p,
                text,
                cx,
                line_y,
                right,
                &style(Face::Mono, 6.0 * k, *colour, alpha),
            );
        }
        line_y += pitch;
    }
    line_y
}

const PROMPT: Span = ("~/code/app", CYAN);
const DOLLAR: Span = (" $ ", DIM);
/// The command typed in the lesson about typing.
const COMMAND: &str = "git commit -m \"Fix the login redirect\"";

#[allow(clippy::too_many_arguments)]
fn terminal(
    p: &mut Painter,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    k: f32,
    alpha: f32,
    typing: Option<f32>,
) {
    p.fill(x, y, w, h, 0.0, fade(0x1d2026ff, alpha));
    let rows: &[&[Span]] = &[
        &[PROMPT, DOLLAR, ("cargo build --release", TERMINAL_INK)],
        &[("   Compiling", GREEN), (" serde v1.0.219", TERMINAL_INK)],
        &[("   Compiling", GREEN), (" tokio v1.47.1", TERMINAL_INK)],
        &[("   Compiling", GREEN), (" app v0.3.0", TERMINAL_INK)],
        &[("    Finished", GREEN), (" release in 41.2s", TERMINAL_INK)],
        &[PROMPT, DOLLAR, ("git status --short", TERMINAL_INK)],
        &[(" M", RED), (" src/login.rs", TERMINAL_INK)],
        &[(" M", RED), (" src/routes.rs", TERMINAL_INK)],
    ];
    let line_y = lines(p, x, y, w, h, k, alpha, rows);
    if line_y + 4.3 * k > y + h {
        return;
    }
    let mut cx = x + 5.0 * k;
    let right = x + w - 4.0 * k;
    for (text, colour) in [PROMPT, DOLLAR] {
        cx = clipped(
            p,
            text,
            cx,
            line_y,
            right,
            &style(Face::Mono, 6.0 * k, colour, alpha),
        );
    }
    if let Some(typed) = typing {
        let chars = (typed * COMMAND.len() as f32).ceil() as usize;
        let typed: String = COMMAND.chars().take(chars).collect();
        cx = clipped(
            p,
            &typed,
            cx,
            line_y,
            right,
            &style(Face::Mono, 6.0 * k, TERMINAL_INK, alpha),
        );
    }
    if cx + 3.6 * k < right {
        p.fill(
            cx + 0.6 * k,
            line_y - 3.4 * k,
            3.4 * k,
            6.8 * k,
            0.0,
            fade(0xe7ebf1d0, alpha),
        );
    }
}

/// Lines of prose as grey bars, `widths` long in shares of `w`, as many as fit above `bottom`.
#[allow(clippy::too_many_arguments)]
fn prose(
    p: &mut Painter,
    x: f32,
    mut y: f32,
    w: f32,
    bottom: f32,
    k: f32,
    widths: &[f32],
    rgba: u32,
) -> f32 {
    for share in widths {
        if y + 2.4 * k > bottom {
            break;
        }
        p.fill(x, y, w * share, 2.2 * k, 1.1 * k, rgba);
        y += 5.2 * k;
    }
    y
}

fn browser(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, k: f32, alpha: f32) {
    let chrome = 13.0 * k;
    p.fill(x, y, w, h, 0.0, fade(0xf4f5f7ff, alpha));
    p.fill(x, y, w, chrome, 0.0, fade(0x2a2e35ff, alpha));
    for n in 0..2 {
        p.fill(
            x + (4.0 + n as f32 * 6.0) * k,
            y + 4.8 * k,
            3.4 * k,
            3.4 * k,
            1.7 * k,
            fade(0x5a606bff, alpha),
        );
    }
    let bar_x = x + 18.0 * k;
    let bar_w = (w - 24.0 * k).max(0.0);
    p.fill(
        bar_x,
        y + 3.0 * k,
        bar_w,
        7.0 * k,
        3.5 * k,
        fade(0x1b1e23ff, alpha),
    );
    clipped(
        p,
        "example.org/reading",
        bar_x + 4.0 * k,
        y + 6.5 * k,
        bar_x + bar_w - 2.0 * k,
        &style(Face::Mono, 5.0 * k, panel::TERTIARY, alpha),
    );
    let pad = 9.0 * k;
    let (px, pw) = (x + pad, w - 2.0 * pad);
    let bottom = y + h - 4.0 * k;
    let mut py = y + chrome + 7.0 * k;
    if py + 6.0 * k < bottom {
        p.fill(px, py, 5.0 * k, 5.0 * k, 1.2 * k, fade(0x2f6fdeff, alpha));
        p.fill(
            px + 8.0 * k,
            py + 1.5 * k,
            pw * 0.2,
            2.0 * k,
            1.0 * k,
            fade(0x9aa1abff, alpha),
        );
        py += 13.0 * k;
    }
    if py + 5.0 * k < bottom {
        p.fill(px, py, pw * 0.7, 5.0 * k, 1.5 * k, fade(0x1f2329ff, alpha));
        py += 8.0 * k;
    }
    if py + 5.0 * k < bottom {
        p.fill(px, py, pw * 0.45, 5.0 * k, 1.5 * k, fade(0x1f2329ff, alpha));
        py += 11.0 * k;
    }
    let grey = fade(0xb9bec7ff, alpha);
    py = prose(p, px, py, pw, bottom, k, &[0.96, 0.92, 0.98, 0.64], grey);
    let picture = (h * 0.28).min(pw * 0.5);
    if py + picture + 4.0 * k < bottom {
        py += 3.0 * k;
        p.fill(px, py, pw, picture, 2.0 * k, fade(0x5b7cfaff, alpha));
        p.fill(
            px + pw * 0.62,
            py + picture * 0.2,
            picture * 0.3,
            picture * 0.3,
            picture * 0.15,
            fade(0xffd97aff, alpha),
        );
        p.fill(
            px,
            py + picture * 0.62,
            pw,
            picture * 0.38,
            0.0,
            fade(0x3553c9ff, alpha),
        );
        py += picture + 7.0 * k;
    }
    prose(
        p,
        px,
        py,
        pw,
        bottom,
        k,
        &[
            0.94, 0.97, 0.9, 0.55, 0.0, 0.96, 0.92, 0.98, 0.7, 0.93, 0.88, 0.6,
        ],
        grey,
    );
}

fn editor(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, k: f32, alpha: f32) {
    const KW: u32 = 0xc678ddff;
    const FUNC: u32 = 0x61afefff;
    const TYPE: u32 = 0xe5c07bff;
    const PLAIN: u32 = 0xabb2bfff;
    const NOTE: u32 = 0x5c6370ff;
    const CODE: &[(u8, &[(f32, u32)])] = &[
        (0, &[(8.0, KW), (18.0, TYPE)]),
        (0, &[(8.0, KW), (24.0, TYPE)]),
        (0, &[]),
        (0, &[(30.0, NOTE)]),
        (0, &[(6.0, KW), (18.0, FUNC), (14.0, PLAIN)]),
        (1, &[(8.0, KW), (10.0, PLAIN), (18.0, FUNC), (6.0, PLAIN)]),
        (1, &[(6.0, KW), (12.0, PLAIN), (4.0, KW), (14.0, FUNC)]),
        (2, &[(12.0, FUNC), (22.0, STRING), (4.0, PLAIN)]),
        (2, &[(16.0, PLAIN), (10.0, FUNC)]),
        (1, &[(3.0, PLAIN)]),
        (1, &[(14.0, FUNC), (8.0, TYPE), (4.0, PLAIN)]),
        (0, &[(3.0, PLAIN)]),
        (0, &[]),
        (0, &[(6.0, KW), (16.0, FUNC), (20.0, PLAIN)]),
        (1, &[(8.0, KW), (26.0, STRING)]),
        (0, &[(3.0, PLAIN)]),
    ];
    p.fill(x, y, w, h, 0.0, fade(0x1e2127ff, alpha));
    let tabs = 9.0 * k;
    p.fill(x, y, w, tabs, 0.0, fade(0x181a1fff, alpha));
    let tab_w = (38.0 * k).min(w * 0.6);
    p.fill(x, y, tab_w, tabs, 0.0, fade(0x1e2127ff, alpha));
    p.fill(
        x,
        y + tabs - 0.8 * k,
        tab_w,
        0.8 * k,
        0.0,
        fade(FUNC, alpha),
    );
    clipped(
        p,
        "main.rs",
        x + 4.0 * k,
        y + tabs / 2.0,
        x + tab_w - 2.0 * k,
        &style(Face::Mono, 5.0 * k, panel::PROSE, alpha),
    );
    let gutter = style(Face::Mono, 4.6 * k, 0x4b5263ff, alpha);
    let mut line_y = y + tabs + 6.0 * k;
    let right = x + w - 4.0 * k;
    for (n, (indent, spans)) in CODE.iter().cycle().enumerate() {
        if line_y + 3.0 * k > y + h {
            break;
        }
        clipped(
            p,
            &(n + 1).to_string(),
            x + 3.0 * k,
            line_y,
            x + 14.0 * k,
            &gutter,
        );
        let mut cx = x + 17.0 * k + *indent as f32 * 9.0 * k;
        for (length, colour) in *spans {
            // In characters' widths, roughly: code runs a good way across a window.
            let length = length * 2.2;
            let end = (cx + length * k).min(right);
            if end > cx {
                p.fill(
                    cx,
                    line_y - 1.1 * k,
                    end - cx,
                    2.2 * k,
                    1.1 * k,
                    fade(*colour, alpha),
                );
            }
            cx += length * k + 3.5 * k;
        }
        line_y += 6.4 * k;
    }
}

fn files(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, k: f32, alpha: f32) {
    p.fill(x, y, w, h, 0.0, fade(0x1f2229ff, alpha));
    let head = 12.0 * k;
    p.fill(x, y, w, head, 0.0, fade(0x25282fff, alpha));
    let side = (w * 0.26).min(44.0 * k);
    clipped(
        p,
        "Documents",
        x + side + 6.0 * k,
        y + head / 2.0,
        x + w - 4.0 * k,
        &style(Face::BodyBold, 6.0 * k, panel::INK, alpha),
    );
    p.fill(x, y + head, side, h - head, 0.0, fade(0x1a1c22ff, alpha));
    let mut sy = y + head + 6.0 * k;
    for (n, share) in [0.7, 0.55, 0.8, 0.6, 0.5, 0.65].iter().enumerate() {
        if sy + 4.0 * k > y + h {
            break;
        }
        if n == 1 {
            p.fill(
                x + 2.0 * k,
                sy - 3.0 * k,
                side - 4.0 * k,
                8.0 * k,
                2.0 * k,
                fade(0xffffff12, alpha),
            );
        }
        p.fill(
            x + 5.0 * k,
            sy - 1.5 * k,
            3.0 * k,
            3.0 * k,
            0.8 * k,
            fade(0x8a93a3ff, alpha),
        );
        p.fill(
            x + 10.0 * k,
            sy - 1.0 * k,
            (side - 14.0 * k) * share,
            2.0 * k,
            1.0 * k,
            fade(0x8a93a3ff, alpha),
        );
        sy += 9.0 * k;
    }
    let (cell_w, cell_h) = (28.0 * k, 28.0 * k);
    let gx = x + side + 5.0 * k;
    let columns = ((x + w - gx) / cell_w).floor().max(0.0) as usize;
    let rows = ((y + h - (y + head + 5.0 * k)) / cell_h).floor().max(0.0) as usize;
    for row in 0..rows {
        for column in 0..columns {
            let cx = gx + column as f32 * cell_w + cell_w / 2.0;
            let cy = y + head + 5.0 * k + row as f32 * cell_h;
            if (row * 3 + column) % 4 == 3 {
                p.fill(
                    cx - 5.5 * k,
                    cy + 2.0 * k,
                    11.0 * k,
                    14.0 * k,
                    1.2 * k,
                    fade(0xd9dde3ff, alpha),
                );
            } else {
                p.fill(
                    cx - 9.0 * k,
                    cy + 3.0 * k,
                    7.0 * k,
                    3.0 * k,
                    1.0 * k,
                    fade(0x4a88d6ff, alpha),
                );
                p.fill(
                    cx - 9.0 * k,
                    cy + 4.5 * k,
                    18.0 * k,
                    12.0 * k,
                    1.8 * k,
                    fade(0x62a0eaff, alpha),
                );
            }
            p.fill(
                cx - 7.0 * k,
                cy + 19.5 * k,
                14.0 * k,
                2.0 * k,
                1.0 * k,
                fade(0x8a93a3ff, alpha),
            );
        }
    }
}

fn music(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, k: f32, alpha: f32) {
    p.fill(x, y, w, h, 0.0, fade(0x16181dff, alpha));
    let pad = 9.0 * k;
    let art = (h * 0.5).min(w * 0.4);
    let (ax, ay) = (x + pad, y + pad);
    p.fill(ax, ay, art, art, 3.0 * k, fade(0xd9485fff, alpha));
    p.fill(
        ax,
        ay + art * 0.55,
        art,
        art * 0.45,
        0.0,
        fade(0x7a2a67ff, alpha),
    );
    p.fill(
        ax + art * 0.3,
        ay + art * 0.22,
        art * 0.4,
        art * 0.4,
        art * 0.2,
        fade(0xf7a541ff, alpha),
    );
    let tx = ax + art + 10.0 * k;
    let right = x + w - pad;
    if right - tx > 30.0 * k {
        let ty = ay + art * 0.3;
        clipped(
            p,
            "Night Drive",
            tx,
            ty,
            right,
            &style(Face::BodyBold, 8.0 * k, panel::BRIGHT, alpha),
        );
        clipped(
            p,
            "The Midnight Hours",
            tx,
            ty + 10.0 * k,
            right,
            &style(Face::Body, 6.0 * k, panel::SECONDARY, alpha),
        );
        let bar_y = ty + 24.0 * k;
        p.fill(
            tx,
            bar_y,
            right - tx,
            1.6 * k,
            0.8 * k,
            fade(0xffffff26, alpha),
        );
        p.fill(
            tx,
            bar_y,
            (right - tx) * 0.42,
            1.6 * k,
            0.8 * k,
            fade(ORANGE, alpha),
        );
        for n in 0..3 {
            let size = if n == 1 { 9.0 * k } else { 6.0 * k };
            let cx = tx + (n as f32 * 14.0 + 6.0) * k;
            p.fill(
                cx - size / 2.0,
                bar_y + 10.0 * k - size / 2.0,
                size,
                size,
                size / 2.0,
                fade(if n == 1 { 0xf2f4f8ff } else { 0x8a93a3ff }, alpha),
            );
        }
    }
    let mut ly = ay + art + 12.0 * k;
    let mut n = 1;
    while ly + 4.0 * k < y + h {
        let lit = n == 3;
        if lit {
            p.fill(
                x + 4.0 * k,
                ly - 4.0 * k,
                w - 8.0 * k,
                8.0 * k,
                2.0 * k,
                fade(0xffffff0e, alpha),
            );
        }
        clipped(
            p,
            &n.to_string(),
            x + pad,
            ly,
            x + pad + 8.0 * k,
            &style(Face::Mono, 5.0 * k, DIM, alpha),
        );
        let length = (w - 2.0 * pad - 14.0 * k) * [0.5, 0.35, 0.42, 0.3, 0.46][n % 5];
        p.fill(
            x + pad + 12.0 * k,
            ly - 1.0 * k,
            length.max(0.0),
            2.0 * k,
            1.0 * k,
            fade(if lit { ORANGE } else { 0x9aa3b3ff }, alpha),
        );
        ly += 9.0 * k;
        n += 1;
    }
}

fn chat(p: &mut Painter, x: f32, y: f32, w: f32, h: f32, k: f32, alpha: f32) {
    const FACES: [u32; 5] = [0xf59e0bff, 0x10b981ff, 0x6366f1ff, 0xec4899ff, 0x0ea5e9ff];
    p.fill(x, y, w, h, 0.0, fade(0x1b1e24ff, alpha));
    let side = w * 0.32;
    p.fill(x, y, side, h, 0.0, fade(0x17191eff, alpha));
    let mut sy = y + 5.0 * k;
    for (n, colour) in FACES.iter().cycle().enumerate() {
        if sy + 12.0 * k > y + h {
            break;
        }
        if n == 0 {
            p.fill(
                x + 2.0 * k,
                sy - 1.5 * k,
                side - 4.0 * k,
                13.0 * k,
                2.0 * k,
                fade(0xffffff10, alpha),
            );
        }
        p.fill(
            x + 4.0 * k,
            sy,
            9.0 * k,
            9.0 * k,
            4.5 * k,
            fade(*colour, alpha),
        );
        let bar = (side - 22.0 * k).max(0.0);
        p.fill(
            x + 16.0 * k,
            sy + 1.5 * k,
            bar * 0.7,
            2.0 * k,
            1.0 * k,
            fade(0xc9ced6ff, alpha),
        );
        p.fill(
            x + 16.0 * k,
            sy + 5.5 * k,
            bar,
            1.6 * k,
            0.8 * k,
            fade(0x6b7383ff, alpha),
        );
        sy += 15.0 * k;
    }
    let mx = x + side;
    let mw = w - side;
    clipped(
        p,
        "Sam",
        mx + 6.0 * k,
        y + 7.0 * k,
        x + w,
        &style(Face::BodyBold, 6.5 * k, panel::INK, alpha),
    );
    p.fill(mx, y + 13.0 * k, mw, 0.6 * k, 0.0, fade(0xffffff12, alpha));
    let bubbles: [(bool, f32, u8); 6] = [
        (false, 0.55, 1),
        (false, 0.4, 1),
        (true, 0.62, 2),
        (false, 0.5, 1),
        (true, 0.35, 1),
        (false, 0.6, 2),
    ];
    let mut by = y + 18.0 * k;
    let input = y + h - 13.0 * k;
    for (mine, share, lines) in bubbles {
        let bh = lines as f32 * 5.0 * k + 5.0 * k;
        if by + bh > input - 3.0 * k {
            break;
        }
        let bw = mw * share;
        let bx = if mine {
            mx + mw - bw - 6.0 * k
        } else {
            mx + 6.0 * k
        };
        let (fill, ink) = if mine {
            (0x3a6fd8ff, 0xffffffb0)
        } else {
            (0x2b303aff, 0xc9ced6a0)
        };
        p.fill(bx, by, bw, bh, 4.0 * k, fade(fill, alpha));
        for line in 0..lines {
            let length = if line + 1 == lines && lines > 1 {
                0.5
            } else {
                0.8
            };
            p.fill(
                bx + 4.0 * k,
                by + 3.5 * k + line as f32 * 5.0 * k,
                (bw - 8.0 * k) * length,
                1.8 * k,
                0.9 * k,
                fade(ink, alpha),
            );
        }
        by += bh + 4.0 * k;
    }
    if input > y + 20.0 * k {
        p.fill(
            mx + 5.0 * k,
            input,
            mw - 10.0 * k,
            9.0 * k,
            4.5 * k,
            fade(0x262a31ff, alpha),
        );
    }
}

/// Alt+Tab's deck: the windows as panes of glass stacked in depth, the front one going round to
/// the back as Tab is pressed.
fn deal(p: &mut Painter, deck: &Deck, scene: &Scene, frame: &Frame, look: &Look) {
    let (ax, ay, aw, ah) = frame.area;
    p.fill(
        ax - EDGE,
        ay - EDGE,
        aw + 2.0 * EDGE,
        ah + 2.0 * EDGE,
        0.0,
        fade(0x000000ff, 0.75 * deck.alpha),
    );
    let count = deck.order.len().max(1) as f32;
    // Each pane's depth: the front one travels to the back as the rest come forward.
    let mut panes: Vec<(f32, usize)> = deck
        .order
        .iter()
        .enumerate()
        .map(|(n, window)| {
            let depth = if n == 0 {
                deck.turn * (count - 1.0)
            } else {
                n as f32 - deck.turn
            };
            (depth, *window)
        })
        .collect();
    panes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (cx, cy) = (ax + aw / 2.0, ay + ah / 2.0);
    for (depth, window) in panes {
        let Some(win) = scene.windows.get(window) else {
            continue;
        };
        let k = 0.6 - depth * 0.04;
        let (pw, ph) = (aw * k, ah * k);
        // The pane going round dips out below the others on its way.
        let dip = if depth > 0.0 && window == deck.order[0] {
            (deck.turn * std::f32::consts::PI).sin() * ah * 0.18
        } else {
            0.0
        };
        // Fanned up and to the right, the fan centred on the area.
        let spread = depth - (count - 1.0) / 2.0;
        let x = cx - pw / 2.0 + spread * 46.0;
        let y = cy - ph / 2.0 - spread * 30.0 + dip;
        // Panes are solid glass: the ones further back are darker, not see-through.
        let alpha = deck.alpha;
        window_pane(p, win.kind, x, y, pw, ph, k, alpha, scene);
        p.fill(
            x,
            y,
            pw,
            ph,
            3.0,
            fade(0x000000ff, alpha * (depth * 0.28).min(0.7)),
        );
        let edge = if depth < 0.5 {
            fade(look.ring, alpha)
        } else {
            fade(0xffffff40, alpha)
        };
        p.border(x, y, pw, ph, 3.0, 1.0, edge);
    }
}

#[allow(clippy::too_many_arguments)]
fn window_pane(
    p: &mut Painter,
    kind: Kind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    k: f32,
    alpha: f32,
    scene: &Scene,
) {
    p.shadow(x, y, w, h, 3.0, 5.0, 12.0, fade(0x000000aa, alpha));
    window(p, kind, x, y, w, h, k, alpha, scene);
}

/// The app explorer, as tapping Super opens it: a search box, and what it finds.
fn explorer(p: &mut Painter, typed: Typed, w: f32, look: &Look) {
    const QUERY: &str = "music";
    let a = typed.alpha;
    if a <= 0.01 {
        return;
    }
    let cw = (w * 0.46).min(270.0);
    let (x, y) = ((w - cw) / 2.0, BAR_H + 28.0);
    let rows: &[(Kind, &str, &str)] = if typed.chars == 0 {
        &[
            (Kind::Terminal, "Terminal", "recent"),
            (Kind::Files, "Files", "recent"),
            (Kind::Browser, "Web", "recent"),
        ]
    } else {
        &[
            (Kind::Music, "Music", "app"),
            (Kind::Files, "music-notes.txt", "Documents"),
        ]
    };
    let ch = 20.0 + rows.len() as f32 * 15.0 + 6.0;
    p.shadow(x, y, cw, ch, 6.0, 8.0, 18.0, fade(0x000000aa, a));
    p.fill(x, y, cw, ch, 6.0, fade(panel::GLASS, a));
    p.border(x, y, cw, ch, 6.0, 0.8, fade(panel::GLASS_EDGE, a));
    p.icon(
        icons::SEARCH,
        x + 7.0,
        y + 5.5,
        9.0,
        Some(fade(panel::SECONDARY, a)),
    );
    let field = y + 10.0;
    let words = style(Face::Body, 8.0, panel::INK, a);
    let caret_x = if typed.chars == 0 {
        p.text(
            "Search apps and files",
            x + 20.0,
            field,
            &style(Face::Body, 8.0, panel::PLACEHOLDER, a),
        );
        x + 20.0
    } else {
        let query: String = QUERY.chars().take(typed.chars).collect();
        x + 20.0 + p.text(&query, x + 20.0, field, &words)
    };
    p.fill(
        caret_x + 0.8,
        field - 4.5,
        0.9,
        9.0,
        0.0,
        fade(look.ring | 0xff, a),
    );
    p.fill(x, y + 20.0, cw, 0.6, 0.0, fade(panel::DIVIDER, a));
    for (n, (kind, name, what)) in rows.iter().enumerate() {
        let ry = y + 23.0 + n as f32 * 15.0;
        if n == 0 && typed.chars > 0 {
            p.fill(
                x + 4.0,
                ry,
                cw - 8.0,
                14.0,
                3.0,
                fade(panel::selection_fill(look.ring), a),
            );
        }
        app_icon(p, *kind, x + 9.0, ry + 3.0, 8.0, a);
        p.text(
            name,
            x + 22.0,
            ry + 7.0,
            &style(Face::Body, 7.0, panel::INK, a),
        );
        let hint = style(Face::Body, 6.0, panel::TERTIARY, a);
        let hw = text::width(what, &hint);
        p.text(what, x + cw - 9.0 - hw, ry + 7.0, &hint);
    }
}

/// The key list, as Super+/ opens it, narrowed as it's typed into.
fn sheet(p: &mut Painter, typed: Typed, w: f32, h: f32) {
    const QUERY: &str = "work";
    const ROWS: [(&str, &str); 16] = [
        ("Super", "apps"),
        ("Super+← → ↑ ↓", "move focus"),
        ("Super+Alt+← → ↑ ↓", "move the window"),
        ("Alt+Tab", "switch window"),
        ("Alt+F4", "close window"),
        ("Super+F", "fill the area"),
        ("Super+R", "turn the layout"),
        ("Super+T", "tiling or gravity"),
        ("Super+1–9", "go to a workspace"),
        ("Super+Shift+1–9", "send window to a workspace"),
        ("Super+Ctrl+← →", "previous, next workspace"),
        ("Super+Tab", "bullet time"),
        ("Super+M", "minimise to the rain"),
        ("Super+H", "hide every window"),
        ("Super+N", "notifications"),
        ("Super+L", "lock the screen"),
    ];
    let a = typed.alpha;
    if a <= 0.01 {
        return;
    }
    let (cw, ch) = (w * 0.74, (h - BAR_H) * 0.74);
    let (x, y) = ((w - cw) / 2.0, BAR_H + (h - BAR_H - ch) / 2.0);
    p.shadow(x, y, cw, ch, 6.0, 8.0, 18.0, fade(0x000000aa, a));
    p.fill(x, y, cw, ch, 6.0, fade(panel::GLASS, a));
    p.border(x, y, cw, ch, 6.0, 0.8, fade(panel::GLASS_EDGE, a));
    let head = y + 12.0;
    let tx = x
        + 10.0
        + p.text(
            "Keyboard shortcuts",
            x + 10.0,
            head,
            &style(Face::Body, 9.0, panel::INK, a),
        );
    let query: String = QUERY.chars().take(typed.chars).collect();
    if query.is_empty() {
        p.text(
            "type to filter",
            tx + 8.0,
            head,
            &style(Face::Body, 7.0, panel::PLACEHOLDER, a),
        );
    } else {
        let end = tx
            + 8.0
            + p.text(
                &query,
                tx + 8.0,
                head,
                &style(Face::Body, 7.0, panel::INK, a),
            );
        p.fill(end + 0.8, head - 4.0, 0.9, 8.0, 0.0, fade(panel::PROSE, a));
    }
    p.fill(x, y + 23.0, cw, 0.6, 0.0, fade(panel::DIVIDER, a));
    let shown: Vec<&(&str, &str)> = ROWS
        .iter()
        .filter(|(_, does)| does.contains(query.as_str()))
        .collect();
    let columns = if query.is_empty() { 2 } else { 1 };
    let per_column = shown.len().div_ceil(columns);
    let column_w = (cw - 20.0) / columns as f32;
    let cap = style(Face::Mono, 5.2, panel::PROSE, a);
    let does = style(Face::Body, 6.4, panel::SECONDARY, a);
    for (n, (keys, what)) in shown.iter().enumerate() {
        let (column, row) = (n / per_column, n % per_column);
        let rx = x + 10.0 + column as f32 * column_w;
        let ry = y + 32.0 + row as f32 * 12.5;
        if ry + 5.0 > y + ch {
            continue;
        }
        let label = keys.to_uppercase();
        let kw = text::width(&label, &cap) + 6.0;
        p.fill(rx, ry - 4.5, kw, 9.0, 2.0, fade(panel::QUIET, a));
        p.border(rx, ry - 4.5, kw, 9.0, 2.0, 0.6, fade(panel::EDGE, a));
        p.text(&label, rx + 3.0, ry, &cap);
        clipped(
            p,
            what,
            rx + column_w * 0.46,
            ry,
            rx + column_w - 4.0,
            &does,
        );
    }
}

/// The card that comes at a pause with what arrived while typing.
fn digest(p: &mut Painter, alpha: f32, w: f32, h: f32) {
    let rows = [
        (0x2fb67cff, "Messages", "Sam: still on for Thursday?"),
        (0xf59e0bff, "Calendar", "Stand-up in 10 minutes"),
        (0x62a0eaff, "Updates", "3 updates are ready"),
    ];
    let (cw, ch) = (160.0, 64.0);
    let x = w - cw - EDGE - 4.0;
    let y = h - ch - EDGE - 4.0 + (1.0 - alpha) * 8.0;
    p.shadow(x, y, cw, ch, 5.0, 6.0, 14.0, fade(0x000000aa, alpha));
    p.fill(x, y, cw, ch, 5.0, fade(panel::NOTICE, alpha));
    p.border(x, y, cw, ch, 5.0, 0.8, fade(panel::NOTICE_EDGE, alpha));
    p.icon(
        icons::BELL,
        x + 7.0,
        y + 5.0,
        8.0,
        Some(fade(panel::AMBER, alpha)),
    );
    p.text(
        "3 while you were typing",
        x + 19.0,
        y + 9.0,
        &style(Face::BodyBold, 7.0, panel::INK, alpha),
    );
    for (n, (colour, app, what)) in rows.iter().enumerate() {
        let ry = y + 24.0 + n as f32 * 13.0;
        p.fill(x + 8.0, ry - 3.0, 6.0, 6.0, 3.0, fade(*colour, alpha));
        let end = x
            + 19.0
            + p.text(
                app,
                x + 19.0,
                ry,
                &style(Face::BodyBold, 6.0, panel::PROSE, alpha),
            );
        clipped(
            p,
            what,
            end + 4.0,
            ry,
            x + cw - 6.0,
            &style(Face::Body, 6.0, panel::SECONDARY, alpha),
        );
    }
}

/// The key going down, drawn as keycaps under the miniature, lit in the ring's colour as it's
/// pressed; or, when what's happening isn't a key, a word or two in its place.
fn keys(p: &mut Painter, scene: &Scene, w: f32, h: f32, look: &Look) {
    let cap = Style::new(Face::Mono, 8.0, panel::BRIGHT);
    let plus = Style::new(Face::Body, 8.0, panel::TERTIARY);
    let (pill_h, y) = (20.0, h - EDGE - 24.0);
    if let Some(press) = scene.key {
        let parts: Vec<String> = press.keys.split('+').map(str::to_uppercase).collect();
        let widths: Vec<f32> = parts
            .iter()
            .map(|part| text::width(part, &cap) + 10.0)
            .collect();
        let plus_w = text::width("+", &plus) + 8.0;
        let total = widths.iter().sum::<f32>() + plus_w * (parts.len() - 1) as f32 + 12.0;
        let mut x = (w - total) / 2.0;
        let glow = press.glow;
        p.shadow(x, y, total, pill_h, 6.0, 4.0, 10.0, 0x00000088);
        p.fill(x, y, total, pill_h, 6.0, 0x0b0d12e8);
        p.border(
            x,
            y,
            total,
            pill_h,
            6.0,
            1.0,
            fade(look.ring | 0xff, 0.25 + 0.75 * glow),
        );
        x += 6.0;
        for (n, (part, width)) in parts.iter().zip(&widths).enumerate() {
            if n > 0 {
                p.text("+", x + 4.0, y + pill_h / 2.0, &plus);
                x += plus_w;
            }
            // Pressed, the cap sinks a little and fills with the ring's colour.
            let sink = glow * 1.0;
            p.fill(
                x,
                y + 3.5 + sink,
                *width,
                13.0 - sink,
                3.0,
                fade(look.ring | 0xff, 0.12 + 0.35 * glow),
            );
            p.border(x, y + 3.5 + sink, *width, 13.0 - sink, 3.0, 0.8, 0xffffff30);
            p.text(part, x + 5.0, y + 10.0 + sink / 2.0, &cap);
            x += width;
        }
    } else if let Some(words) = scene.caption {
        let style = Style::new(Face::Body, 8.0, panel::SECONDARY);
        let dots = match (look.secs * 3.0) as usize % 3 {
            0 => ".",
            1 => "..",
            _ => "...",
        };
        let label = if words == "typing" {
            format!("typing{dots}")
        } else {
            words.to_string()
        };
        let total = text::width("typing...", &style).max(text::width(&label, &style)) + 20.0;
        let x = (w - total) / 2.0;
        p.fill(x, y, total, pill_h, 6.0, 0x0b0d12d0);
        p.border(x, y, total, pill_h, 6.0, 0.8, 0xffffff1c);
        p.text(&label, x + 10.0, y + pill_h / 2.0, &style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tour;

    #[test]
    fn fading_scales_only_the_opacity() {
        assert_eq!(fade(0x11223380, 0.5), 0x11223340);
        assert_eq!(fade(0x112233ff, 1.0), 0x112233ff);
        assert_eq!(fade(0x112233ff, 0.0), 0x11223300);
    }

    #[test]
    fn workspaces_sit_side_by_side_and_bullet_time_shrinks_them() {
        let mut scene = tour::scene(7, 0.0);
        scene.camera = 0.0;
        let frame = Frame::new(WIDTH, HEIGHT, &scene);
        let (first, ..) = frame.screen(0);
        let (second, ..) = frame.screen(1);
        assert!(second >= WIDTH, "the next workspace is off to the right");
        assert_eq!(first, 0.0);
        scene.zoom = 1.0;
        let frame = Frame::new(WIDTH, HEIGHT, &scene);
        let (_, _, w, _) = frame.screen(1);
        assert!(w < 400.0, "smaller in bullet time");
    }

    #[test]
    fn every_lesson_paints() {
        let look = Look {
            ring: 0x42d3ffcc,
            effects: Effects::Matrix,
            secs: 1.3,
        };
        for step in 0..tour::LESSONS.len() {
            for tenth in 0..=10 {
                let scene = tour::scene(step, tenth as f32 / 10.0);
                let mut p = Painter::new(300, 190, 0.5).expect("a pixmap");
                paint(&mut p, &scene, &look);
            }
        }
    }
}
