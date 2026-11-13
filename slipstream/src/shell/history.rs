//! Clipboard history (Super+V): the last things copied, text and pictures, any of which can be
//! pasted again.
//!
//! Whenever an app puts something on the clipboard, Slipstream reads a copy for itself, on a
//! thread of its own, and keeps it in memory only: nothing is written to disk, and logging out
//! forgets it all. Anything a password manager marks as secret (`x-kde-passwordManagerHint`) is
//! never read at all. Screenshots and snips join the list as they're copied.
//!
//! Super+V lists them, newest first. ↑ and ↓ choose, Enter puts the chosen one back on the
//! clipboard and pastes it into the focused app (Ctrl+V, or Ctrl+Shift+V in a terminal), Delete
//! forgets it, Esc closes. The chosen row's text decodes out of rain glyphs as the selection
//! reaches it, whichever look the streams have.

use std::{
    io::Read,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
    time::{Duration, Instant},
};

use resvg::tiny_skia::{
    Color, FilterQuality, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Rect,
    SpreadMode, Transform,
};
use slipstream_config::Effects;
use smithay::{
    backend::renderer::gles::GlesRenderer,
    input::keyboard::Keysym,
    reexports::calloop::channel::Sender,
    utils::{Logical, Point, Rectangle, Size},
    wayland::selection::{
        SelectionTarget,
        data_device::{request_data_device_client_selection, set_data_device_selection},
    },
};

use crate::{
    Slipstream,
    paint::{self, Painted, Painter},
    panel::{self, DESIGN_PX},
    screenshot::Selection,
    text::{self, Face, Style},
};

/// How many things are kept.
pub const KEPT: usize = 25;
/// The most text read from one copy, in bytes. More is cut off.
const TEXT_LIMIT: usize = 256 << 10;
/// The largest picture read from one copy, in bytes of PNG.
const IMAGE_LIMIT: usize = 16 << 20;
/// How long a reading waits for an app to hand the copy over.
const READ_WAIT: Duration = Duration::from_secs(2);
/// A password manager's mark on a copy that must not be kept.
const SECRET: &str = "x-kde-passwordManagerHint";
/// Text types, best first: what the history asks an app for, and what it offers when pasting.
pub const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];
const PNG: &str = "image/png";

// The card, in design pixels.
const WIDTH: f32 = 760.0;
const TOP: f32 = 130.0;
const MARGIN: f32 = 64.0;
const PADDING: f32 = 18.0;
const HEAD_H: f32 = 60.0;
const FOOT_H: f32 = 44.0;
const TEXT_ROW: f32 = 64.0;
const IMAGE_ROW: f32 = 92.0;
const ROW_GAP: f32 = 6.0;
const THUMB_H: f32 = 72.0;
const THUMB_W: f32 = 128.0;
/// How long the chosen row's text takes to arrive.
const DECODE: f64 = 0.32;
/// With Slipstream's effects, how long each character takes to be carried in, and how far
/// upwind, to its left, it starts, in design pixels.
const ARRIVE: f64 = 0.12;
const UPWIND: f32 = 8.0;
/// With the Matrix's, what undecoded characters show: the rain's own digits and marks.
const GLYPHS: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', '.', '=', '*', '+', '-', '<', '>', '|',
    '¦', '"', '_',
];

/// Something copied.
#[derive(Clone)]
pub enum Clip {
    Text(Arc<str>),
    Image {
        png: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        /// Fitted into `THUMB_W` × `THUMB_H`, two pixels to each design pixel.
        thumb: Option<Arc<Pixmap>>,
    },
}

impl Clip {
    fn same_as(&self, other: &Clip) -> bool {
        match (self, other) {
            (Clip::Text(a), Clip::Text(b)) => a == b,
            (Clip::Image { png: a, .. }, Clip::Image { png: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// What to ask an app for, from the types its copy is offered as. `None` for a secret, or for
/// nothing the history can show.
pub fn wanted(types: &[String]) -> Option<&'static str> {
    if types.iter().any(|kind| kind == SECRET) {
        return None;
    }
    TEXT_TYPES
        .into_iter()
        .chain([PNG])
        .find(|wanted| types.iter().any(|kind| kind == wanted))
}

/// A picture's size from its PNG header.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(16..24)?;
    if png.get(1..4)? != b"PNG" {
        return None;
    }
    let width = u32::from_be_bytes(header[0..4].try_into().ok()?);
    let height = u32::from_be_bytes(header[4..8].try_into().ok()?);
    Some((width, height))
}

/// A picture's thumbnail, fitted into the row's box at two pixels to each design pixel.
fn thumbnail(png: &[u8]) -> Option<Arc<Pixmap>> {
    let source = paint::decode_png(png)?;
    let (box_w, box_h) = (THUMB_W * 2.0, THUMB_H * 2.0);
    let (w, h) = (source.width() as f32, source.height() as f32);
    let k = (box_w / w).min(box_h / h).min(2.0);
    let (tw, th) = ((w * k).round().max(1.0), (h * k).round().max(1.0));
    let mut out = Pixmap::new(tw as u32, th as u32)?;
    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    out.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &paint,
        Transform::from_scale(k, k),
        None,
    );
    Some(Arc::new(out))
}

/// A picture as a clip, with its thumbnail. Decodes the whole picture, so it belongs on a thread.
pub fn image_clip(png: Arc<Vec<u8>>) -> Option<Clip> {
    let (width, height) = png_size(&png)?;
    let thumb = thumbnail(&png);
    Some(Clip::Image {
        png,
        width,
        height,
        thumb,
    })
}

/// Reads a copy an app is writing into `fd`, as `kind`, giving up after `READ_WAIT`.
fn read_clip(fd: OwnedFd, kind: &str) -> Option<Clip> {
    let limit = if kind == PNG { IMAGE_LIMIT } else { TEXT_LIMIT };
    let deadline = Instant::now() + READ_WAIT;
    let mut file = std::fs::File::from(fd);
    let mut data = Vec::new();
    let mut chunk = [0u8; 16 << 10];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, owned by this frame, for a descriptor this thread owns.
        let ready =
            unsafe { libc::poll(&mut poll, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        if ready <= 0 {
            return None;
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&chunk[..n]);
                if data.len() > limit {
                    if kind == PNG {
                        // A cut picture is no picture.
                        return None;
                    }
                    data.truncate(limit);
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    if data.is_empty() {
        return None;
    }
    if kind == PNG {
        return image_clip(Arc::new(data));
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    (!text.trim().is_empty()).then(|| Clip::Text(text.into()))
}

/// A pipe for an app to write a copy into: the end to read, and the end to hand over.
fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `pipe2` fills the two descriptors on success, which are then owned here.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: both descriptors were just opened and nothing else owns them.
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// The first lines of a text copy, whitespace run together, for its row.
pub fn preview(text: &str, lines: usize) -> Vec<String> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .take(lines)
        .collect()
}

/// Where character `i` of a row `count` long comes in the arrival, from 0 for the first to just
/// under 1 for the last: roughly its place along the row, a little early or late. Also returns
/// the hash its glyphs are picked by.
fn order(i: usize, count: usize, seed: u64) -> (f64, u64) {
    let hash = mix(seed ^ (i as u64).wrapping_mul(0x9e37_79b9));
    let jitter = (hash % 1000) as f64 / 1000.0;
    (0.7 * i as f64 / count.max(1) as f64 + 0.3 * jitter, hash)
}

/// How far one character has arrived: from 0 (not yet) to 1 (in place), and, with the Matrix's
/// effects, the rain glyph it shows until then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrival {
    pub progress: f64,
    pub glyph: Option<char>,
}

/// How far each character of a row `count` long has arrived `age` seconds after it was chosen.
/// The row takes `DECODE` either way. With the Matrix's effects each character settles at once
/// out of rain glyphs, which change as it runs; with Slipstream's each is carried in over
/// `ARRIVE` seconds (see `arriving`), landing by the end, and shows no glyph.
pub fn arrivals(effects: Effects, count: usize, age: f64, seed: u64) -> Vec<Arrival> {
    let step = (age * 30.0) as u64;
    (0..count)
        .map(|i| {
            let (order, hash) = order(i, count, seed);
            match effects {
                Effects::Matrix if age >= DECODE * order => Arrival {
                    progress: 1.0,
                    glyph: None,
                },
                Effects::Matrix => Arrival {
                    progress: 0.0,
                    glyph: Some(GLYPHS[(mix(hash ^ step) % GLYPHS.len() as u64) as usize]),
                },
                Effects::Slipstream => Arrival {
                    progress: ((age - (DECODE - ARRIVE) * order) / ARRIVE).clamp(0.0, 1.0),
                    glyph: None,
                },
            }
        })
        .collect()
}

/// One character carried in on the air, `progress` of the way, with its left edge landing at
/// `x`: it slides in from upwind, to its left, fading up, led by a hairline streak in the ring's
/// colour that shrinks into it as it lands. Nothing shows before it starts. Returns its width.
pub fn arriving(
    p: &mut Painter,
    ch: char,
    x: f32,
    centre: f32,
    style: &Style,
    ring: u32,
    progress: f64,
) -> f32 {
    let mut buffer = [0; 4];
    let ch = ch.encode_utf8(&mut buffer);
    let w = text::width(ch, style);
    if progress <= 0.0 {
        return w;
    }
    if progress >= 1.0 {
        p.text(ch, x, centre, style);
        return w;
    }
    // Quick at first, settling gently.
    let settled = 1.0 - (1.0 - progress as f32).powi(3);
    let at = x - UPWIND * (1.0 - settled);
    let mut faded = *style;
    let [r, g, b, a] = style.color;
    faded.color = [r, g, b, (a as f32 * settled).round() as u8];
    p.text(ch, at, centre, &faded);
    // The streak's bright head is at the letter's leading edge; behind it the line thins away
    // upwind, shorter the nearer the letter is to landing.
    let head = at + w * 0.9;
    let length = (UPWIND + 1.5 * w) * (1.0 - settled);
    let light = (progress as f32 * 5.0).min(1.0) * 0.9;
    let [r, g, b, _] = ring.to_be_bytes();
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(r, g, b, 0)),
        GradientStop::new(1.0, Color::from_rgba8(r, g, b, (255.0 * light) as u8)),
    ];
    let shader = LinearGradient::new(
        (head - length, centre).into(),
        (head, centre).into(),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    let (Some(shader), Some(rect)) = (
        shader,
        Rect::from_xywh(head - length, centre - 0.5, length, 1.0),
    ) else {
        return w;
    };
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    p.pixmap
        .fill_rect(rect, &paint, Transform::from_scale(p.f, p.f), None);
    w
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x
}

/// Each row shown, by its place in the list, and where it is.
type Rows = Vec<(usize, Rectangle<f64, Logical>)>;

/// What a key did.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    Close,
    Paste(usize),
}

#[derive(Default)]
pub struct History {
    /// Newest first.
    clips: Vec<Clip>,
    open: bool,
    opened_at: f64,
    /// Its way in and out.
    pub(crate) drawer: panel::Drawer,
    selected: usize,
    /// The first row shown.
    scroll: usize,
    /// When the selection last moved, on wall time, for the decode.
    chosen_at: f64,
    pub reduced_motion: bool,
    painted: Option<(Look, Painted)>,
    /// Where the painted card's corner settles, outside the margin round the card.
    at: Point<f64, Logical>,
    /// The card, and each row, in logical pixels on the screen.
    frame: Rectangle<f64, Logical>,
    rows: Rows,
    /// Each row as painted, from the painted surface's corner.
    card_rows: Rows,
}

#[derive(Clone, PartialEq, Default)]
struct Look {
    screen: Size<i32, Logical>,
    scale: f64,
    selected: usize,
    scroll: usize,
    generation: usize,
    /// How far the decode is, in thirtieths of a second; `None` once it's done.
    decode_step: Option<u64>,
    ring: u32,
    /// The key that opens and closes the history.
    key: String,
}

impl History {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, now: f64) {
        self.open = true;
        self.opened_at = now;
        self.drawer.open(now, panel::Hang::Own);
        self.selected = 0;
        self.scroll = 0;
        self.chosen_at = now;
        self.painted = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        let at = self.at;
        let picture = self
            .painted
            .take()
            .map(|(_, painted)| crate::flow::Layer::new(painted, at));
        self.drawer.close(picture.into_iter().collect());
    }

    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.painted = None;
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Forgets everything, as turning the history off does.
    pub fn clear(&mut self) {
        self.clips.clear();
        self.selected = 0;
        self.scroll = 0;
        self.painted = None;
    }

    /// A new copy goes to the top; one already kept moves there instead.
    pub fn add(&mut self, clip: Clip) {
        self.clips.retain(|kept| !kept.same_as(&clip));
        self.clips.insert(0, clip);
        self.clips.truncate(KEPT);
        self.painted = None;
    }

    pub fn get(&self, index: usize) -> Option<&Clip> {
        self.clips.get(index)
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.clips.len() {
            self.clips.remove(index);
            self.selected = self.selected.min(self.clips.len().saturating_sub(1));
            self.painted = None;
        }
    }

    fn choose(&mut self, index: usize, now: f64) {
        let index = index.min(self.clips.len().saturating_sub(1));
        if index != self.selected {
            self.selected = index;
            self.chosen_at = now;
        }
    }

    pub fn key(&mut self, sym: Keysym, now: f64) -> Outcome {
        let last = self.clips.len().saturating_sub(1);
        match sym {
            Keysym::Escape => return Outcome::Close,
            Keysym::Return | Keysym::KP_Enter | Keysym::space if !self.clips.is_empty() => {
                return Outcome::Paste(self.selected);
            }
            Keysym::Down | Keysym::Tab => self.choose((self.selected + 1).min(last), now),
            Keysym::Up | Keysym::ISO_Left_Tab => self.choose(self.selected.saturating_sub(1), now),
            Keysym::Home => self.choose(0, now),
            Keysym::End => self.choose(last, now),
            Keysym::Next => self.choose((self.selected + 4).min(last), now),
            Keysym::Prior => self.choose(self.selected.saturating_sub(4), now),
            Keysym::Delete | Keysym::KP_Delete => {
                let index = self.selected;
                self.remove(index);
                self.chosen_at = now;
            }
            _ => {}
        }
        Outcome::Nothing
    }

    /// A click at `pos` on the screen: a row pastes, outside the card closes.
    pub fn click(&self, pos: Point<f64, Logical>) -> Outcome {
        if let Some((index, _)) = self.rows.iter().find(|(_, rect)| rect.contains(pos)) {
            return Outcome::Paste(*index);
        }
        if self.frame.contains(pos) {
            Outcome::Nothing
        } else {
            Outcome::Close
        }
    }

    pub fn contains(&self, pos: Point<f64, Logical>) -> bool {
        self.frame.contains(pos)
    }

    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: crate::panel::Frame,
        ring: u32,
        key: String,
        out: &mut impl Extend<crate::flow::FlowElement>,
    ) -> Option<crate::flow::Reveal> {
        self.drawn(renderer, frame, ring, key, out)
    }

    fn drawn(
        &mut self,
        renderer: &mut GlesRenderer,
        frame: crate::panel::Frame,
        ring: u32,
        key: String,
        out: &mut impl Extend<crate::flow::FlowElement>,
    ) -> Option<crate::flow::Reveal> {
        let (screen, scale, now) = (frame.size, frame.scale, frame.time);
        if !self.open {
            return None;
        }
        let rows_fit = self.rows_that_fit(screen);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows_fit {
            self.scroll = self.selected + 1 - rows_fit;
        }
        let age = now - self.chosen_at;
        let look = Look {
            screen,
            scale,
            selected: self.selected,
            scroll: self.scroll,
            generation: self.clips.len()
                ^ self.clips.first().map_or(0, |clip| match clip {
                    Clip::Text(text) => text.len() << 8,
                    Clip::Image { png, .. } => png.len() << 8,
                }),
            decode_step: (!self.reduced_motion && age < DECODE).then_some((age * 30.0) as u64),
            ring,
            key,
        };
        if self
            .painted
            .as_ref()
            .is_none_or(|(shown, _)| *shown != look)
        {
            let seed = (self.chosen_at * 1000.0) as u64;
            let (painted, rows) = self.paint(&look, rows_fit, age, seed)?;
            self.card_rows = rows;
            self.painted = Some((look, painted));
        }
        let (_, painted) = self.painted.as_ref()?;
        let at = Point::<f64, Logical>::from((
            ((screen.w - painted.logical.w) / 2) as f64,
            ((TOP - MARGIN) * DESIGN_PX) as f64,
        ));
        self.at = at;
        self.frame = crate::flow::card_in(at, painted.logical, MARGIN, 0.0);
        self.rows = self
            .card_rows
            .iter()
            .map(|(index, rect)| (*index, Rectangle::new(rect.loc + at, rect.size)))
            .collect();
        let shown = [crate::flow::Shown::new(painted, at)];
        Some(self.drawer.show(renderer, frame, self.frame, &shown, out))
    }

    fn rows_that_fit(&self, screen: Size<i32, Logical>) -> usize {
        let room = screen.h as f32 / DESIGN_PX - TOP - 24.0 - HEAD_H - FOOT_H - 2.0 * PADDING;
        ((room / (IMAGE_ROW + ROW_GAP)).floor() as usize).clamp(1, 8)
    }

    fn paint(&self, look: &Look, rows_fit: usize, age: f64, seed: u64) -> Option<(Painted, Rows)> {
        let screen_w = look.screen.w as f32 / DESIGN_PX;
        let width = WIDTH.min(screen_w - 40.0).max(420.0);
        let shown: Vec<(usize, &Clip)> = self
            .clips
            .iter()
            .enumerate()
            .skip(look.scroll)
            .take(rows_fit)
            .collect();
        let row_h = |clip: &Clip| match clip {
            Clip::Text(_) => TEXT_ROW,
            Clip::Image { .. } => IMAGE_ROW,
        };
        let body_h = if shown.is_empty() {
            TEXT_ROW
        } else {
            shown
                .iter()
                .map(|(_, clip)| row_h(clip) + ROW_GAP)
                .sum::<f32>()
                - ROW_GAP
        };
        let height = HEAD_H + PADDING + body_h + PADDING + FOOT_H;
        let logical = Size::<i32, Logical>::from((
            ((width + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
            ((height + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
        ));
        let device = (
            (logical.w as f64 * look.scale).round().max(1.0) as i32,
            (logical.h as f64 * look.scale).round().max(1.0) as i32,
        );
        let f = look.scale as f32 * DESIGN_PX;
        let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;
        let (fx, fy) = (MARGIN, MARGIN);
        panel::glass(&mut p, fx, fy, width, height);

        // The head: what this is, and how many, lined up with the rows' boxes below it.
        let centre = fy + HEAD_H / 2.0;
        let title = Style::new(Face::Body, 22.0, panel::INK);
        let title_w = p.text("Clipboard", fx + PADDING, centre, &title);
        let count = match self.clips.len() {
            0 => "nothing yet".to_string(),
            1 => "1 thing".to_string(),
            n => format!("{n} things"),
        };
        p.text(
            &count,
            fx + PADDING + title_w + 16.0,
            centre + 1.0,
            &Style::new(Face::Body, 15.0, panel::PLACEHOLDER),
        );
        let key_w = paint::keycap_width(&look.key);
        p.keycap(&look.key, fx + width - PADDING - key_w, centre);
        p.fill(fx, fy + HEAD_H, width, 1.0, 0.0, panel::DIVIDER);

        let mut rows = Vec::new();
        p.piece(fx, fy, width, HEAD_H);
        p.piece(fx, fy + height - FOOT_H, width, FOOT_H);
        let mut y = fy + HEAD_H + PADDING;
        if shown.is_empty() {
            p.text(
                "Nothing copied yet. Whatever you copy shows up here.",
                fx + PADDING,
                y + TEXT_ROW / 2.0,
                &Style::new(Face::Body, 16.0, panel::HINT),
            );
            panel::piece(&mut p, fx + PADDING, y, width - 2.0 * PADDING, TEXT_ROW);
        }
        let mono = Style::new(Face::Mono, 14.0, panel::PROSE);
        let [r, g, b, _] = look.ring.to_be_bytes();
        let glyph = Style::new(Face::Mono, 14.0, u32::from_be_bytes([r, g, b, 0xff]));
        let char_w = text::width("M", &mono);
        for (index, clip) in shown {
            let h = row_h(clip);
            let (x, w) = (fx + PADDING, width - 2.0 * PADDING);
            panel::piece(&mut p, x, y, w, h);
            let selected = index == look.selected;
            if selected {
                panel::selected_row(&mut p, x, y, w, h, look.ring);
            }
            rows.push((
                index,
                Rectangle::new(
                    ((x * DESIGN_PX) as f64, (y * DESIGN_PX) as f64).into(),
                    ((w * DESIGN_PX) as f64, (h * DESIGN_PX) as f64).into(),
                ),
            ));
            match clip {
                Clip::Text(full) => {
                    let lines = preview(full, 2);
                    let room = ((w - 28.0) / char_w).floor().max(4.0) as usize;
                    let lines: Vec<String> = lines
                        .into_iter()
                        .map(|line| {
                            if line.chars().count() > room {
                                line.chars().take(room - 1).chain(['…']).collect()
                            } else {
                                line
                            }
                        })
                        .collect();
                    let total: usize = lines.iter().map(|line| line.chars().count()).sum();
                    let decoding = selected && look.decode_step.is_some();
                    let arrived = if decoding {
                        // Always the decode: it reads as text settling, not as the code rain.
                        arrivals(Effects::Matrix, total, age, seed)
                    } else {
                        Vec::new()
                    };
                    let pitch = 22.0;
                    let first = y + h / 2.0 - pitch * (lines.len().max(1) as f32 - 1.0) / 2.0;
                    let mut n = 0;
                    for (line_index, line) in lines.iter().enumerate() {
                        let centre = first + line_index as f32 * pitch;
                        if !decoding {
                            p.text(line, x + 14.0, centre, &mono);
                            n += line.chars().count();
                            continue;
                        }
                        for (column, ch) in line.chars().enumerate() {
                            let cx = x + 14.0 + column as f32 * char_w;
                            let arrival = arrived.get(n).copied().unwrap_or(Arrival {
                                progress: 1.0,
                                glyph: None,
                            });
                            match arrival.glyph {
                                Some(shown) => {
                                    p.text(&shown.to_string(), cx, centre, &glyph);
                                }
                                None => {
                                    let ring = look.ring;
                                    arriving(&mut p, ch, cx, centre, &mono, ring, arrival.progress);
                                }
                            }
                            n += 1;
                        }
                    }
                }
                Clip::Image {
                    width: pw,
                    height: ph,
                    thumb,
                    ..
                } => {
                    let thumb_x = x + 12.0;
                    let thumb_y = y + (h - THUMB_H) / 2.0;
                    p.fill(thumb_x, thumb_y, THUMB_W, THUMB_H, 6.0, panel::CHIP | 0xff);
                    if let Some(thumb) = thumb {
                        // The thumbnail has two pixels to each design pixel; drawn at the card's.
                        let k = f / 2.0;
                        let (tw, th) = (thumb.width() as f32 / 2.0, thumb.height() as f32 / 2.0);
                        let tx = thumb_x + (THUMB_W - tw) / 2.0;
                        let ty = thumb_y + (THUMB_H - th) / 2.0;
                        p.pixmap.draw_pixmap(
                            0,
                            0,
                            thumb.as_ref().as_ref(),
                            &PixmapPaint {
                                quality: FilterQuality::Bicubic,
                                ..PixmapPaint::default()
                            },
                            Transform::from_row(k, 0.0, 0.0, k, tx * f, ty * f),
                            None,
                        );
                    }
                    p.text(
                        "Picture",
                        thumb_x + THUMB_W + 16.0,
                        y + h / 2.0 - 10.0,
                        &Style::new(Face::Body, 16.0, panel::PROSE),
                    );
                    p.text(
                        &format!("{pw} × {ph}"),
                        thumb_x + THUMB_W + 16.0,
                        y + h / 2.0 + 12.0,
                        &Style::new(Face::Mono, 13.0, panel::HINT),
                    );
                }
            }
            y += h + ROW_GAP;
        }

        // The foot: the keys.
        let foot = fy + height - FOOT_H / 2.0;
        p.fill(fx, fy + height - FOOT_H, width, 1.0, 0.0, panel::DIVIDER);
        let mut right = fx + width - PADDING;
        for (keys, label) in [
            (&["Esc"][..], "close"),
            (&["Del"][..], "forget"),
            (&["⏎"][..], "paste"),
            (&["↑", "↓"][..], "choose"),
        ] {
            right = panel::key_hint(&mut p, right, foot, keys, label) - panel::HINT_GAP;
        }
        let painted = Painted::from_painter(p, logical, look.scale);
        Some((painted, rows))
    }
}

/// App ids of terminals, which paste with Ctrl+Shift+V.
const TERMINALS: [&str; 12] = [
    "foot",
    "kitty",
    "alacritty",
    "wezterm",
    "konsole",
    "gnome-terminal",
    "ptyxis",
    "xterm",
    "tilix",
    "terminator",
    "ghostty",
    "blackbox",
];

pub fn is_terminal(app_id: &str) -> bool {
    let id = app_id.to_lowercase();
    TERMINALS.iter().any(|terminal| id.contains(terminal))
}

impl Slipstream {
    /// Super+V: the history opens, or closes.
    pub fn toggle_history(&mut self) {
        if self.history.is_open() {
            self.history.close();
            return;
        }
        if !self.settings.clipboard.history {
            self.show_toast(
                "Clipboard history is off",
                "Settings › Session turns it on.",
            );
            return;
        }
        self.close_panels();
        let now = self.wall();
        self.history.open(now);
    }

    pub fn history_key(&mut self, sym: Keysym) {
        let now = self.wall();
        let outcome = self.history.key(sym, now);
        self.history_outcome(outcome);
    }

    pub fn history_click(&mut self, pos: Point<f64, Logical>) {
        let outcome = self.history.click(pos);
        self.history_outcome(outcome);
    }

    fn history_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Nothing => {}
            Outcome::Close => self.history.close(),
            Outcome::Paste(index) => {
                self.history.close();
                self.paste_clip(index);
            }
        }
    }

    /// Puts clip `index` back on the clipboard, at the top of the list, and pastes it into the
    /// focused app.
    fn paste_clip(&mut self, index: usize) {
        if let Some(clip) = self.history.get(index).cloned() {
            self.put_on_clipboard(clip, true);
        }
    }

    /// Puts `clip` on the clipboard as Slipstream's own, at the top of the history, and with
    /// `paste` sends the focused app the keys that paste it.
    pub fn put_on_clipboard(&mut self, clip: Clip, paste: bool) {
        let (types, selection): (Vec<String>, Selection) = match &clip {
            Clip::Text(text) => (
                TEXT_TYPES.iter().map(|kind| kind.to_string()).collect(),
                Selection::Text(text.clone()),
            ),
            Clip::Image { png, .. } => (vec![PNG.to_string()], Selection::Image(png.clone())),
        };
        set_data_device_selection(&self.display_handle, &self.seat, types.clone(), selection);
        if let Some(xwm) = self.xwm.as_mut()
            && let Err(err) = xwm.new_selection(SelectionTarget::Clipboard, Some(types))
        {
            tracing::warn!(?err, "couldn't offer the clip to X11 apps");
        }
        if self.settings.clipboard.history {
            self.history.add(clip);
        }
        if paste {
            self.send_edit(crate::keys::Edit::Paste);
        }
    }

    /// An app has put something on the clipboard: a copy is read for the history, and text for
    /// the plugins that need to see what's copied, unless neither wants it or the copy is marked
    /// secret.
    pub fn clipboard_changed(&mut self, types: Vec<String>, x11: bool) {
        let history = self.settings.clipboard.history;
        let plugins = self
            .plugins
            .wants(slipstream_config::plugins::Need::Clipboard);
        if !(history || plugins) || self.lock.is_some() {
            return;
        }
        let Some(kind) = wanted(&types).filter(|kind| history || TEXT_TYPES.contains(kind)) else {
            return;
        };
        let Some((read, write)) = pipe() else {
            return;
        };
        let asked = if x11 {
            self.xwm.as_mut().is_some_and(|xwm| {
                xwm.send_selection(SelectionTarget::Clipboard, kind.to_string(), write)
                    .is_ok()
            })
        } else {
            request_data_device_client_selection(&self.seat, kind.to_string(), write).is_ok()
        };
        if !asked {
            return;
        }
        let answers: Sender<Clip> = self.clip_answers.clone();
        std::thread::spawn(move || {
            if let Some(clip) = read_clip(read, kind) {
                let _ = answers.send(clip);
            }
        });
    }

    /// A copy has been read.
    pub fn clip_read(&mut self, clip: Clip) {
        if self.lock.is_some() {
            return;
        }
        if let Clip::Text(text) = &clip {
            self.plugins.copied(text);
        }
        if self.settings.clipboard.history {
            self.history.add(clip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|kind| kind.to_string()).collect()
    }

    #[test]
    fn a_password_managers_secret_is_never_read() {
        assert_eq!(
            wanted(&types(&[
                "text/plain;charset=utf-8",
                "x-kde-passwordManagerHint"
            ])),
            None
        );
    }

    #[test]
    fn text_is_asked_for_before_a_picture() {
        assert_eq!(
            wanted(&types(&["image/png", "text/html", "text/plain"])),
            Some("text/plain")
        );
        assert_eq!(
            wanted(&types(&["image/png", "text/html"])),
            Some("image/png")
        );
        assert_eq!(wanted(&types(&["text/uri-list"])), None);
    }

    #[test]
    fn the_newest_copy_goes_on_top_and_a_repeat_moves_up() {
        let mut history = History::default();
        for text in ["one", "two", "three"] {
            history.add(Clip::Text(text.into()));
        }
        history.add(Clip::Text("one".into()));
        let order: Vec<String> = history
            .clips
            .iter()
            .map(|clip| match clip {
                Clip::Text(text) => text.to_string(),
                Clip::Image { .. } => String::new(),
            })
            .collect();
        assert_eq!(order, ["one", "three", "two"]);
    }

    #[test]
    fn closing_lets_go_at_once_and_only_its_picture_goes_back_in() {
        let mut history = History::default();
        history.open(0.0);
        history.drawer.drawn_at(1.0, false);
        let painted = Painted::new((1, 1).into(), 1.0, |_| {}).unwrap();
        history.painted = Some((Look::default(), painted));
        history.close();
        assert!(
            !history.is_open(),
            "keys and clicks go to what's underneath now"
        );
        assert!(history.drawer.closing());
        history.open(1.0);
        assert!(!history.drawer.closing());
    }

    #[test]
    fn only_so_many_are_kept() {
        let mut history = History::default();
        for n in 0..KEPT + 10 {
            history.add(Clip::Text(n.to_string().into()));
        }
        assert_eq!(history.clips.len(), KEPT);
    }

    #[test]
    fn a_preview_runs_whitespace_together_and_skips_blank_lines() {
        assert_eq!(
            preview("\n  fn main() {\n\n    println!(\"hi\");\n}", 2),
            ["fn main() {", "println!(\"hi\");"]
        );
    }

    fn glyphs(arrivals: &[Arrival]) -> Vec<Option<char>> {
        arrivals.iter().map(|arrival| arrival.glyph).collect()
    }

    #[test]
    fn decoding_starts_as_glyphs_and_ends_as_text() {
        let first = arrivals(Effects::Matrix, 20, 0.0, 7);
        assert!(glyphs(&first).iter().filter(|c| c.is_some()).count() >= 18);
        let last = arrivals(Effects::Matrix, 20, DECODE, 7);
        assert!(last.iter().all(|a| a.glyph.is_none() && a.progress == 1.0));
    }

    /// The Matrix's decode, glyph for glyph, as it was before Slipstream's look was added.
    #[test]
    fn the_matrix_decode_is_unchanged() {
        let before = |count: usize, age: f64, seed: u64| -> Vec<Option<char>> {
            let step = (age * 30.0) as u64;
            (0..count)
                .map(|i| {
                    let hash = mix(seed ^ (i as u64).wrapping_mul(0x9e37_79b9));
                    let jitter = (hash % 1000) as f64 / 1000.0;
                    let at = DECODE * (0.7 * i as f64 / count.max(1) as f64 + 0.3 * jitter);
                    (age < at).then(|| GLYPHS[(mix(hash ^ step) % GLYPHS.len() as u64) as usize])
                })
                .collect()
        };
        for tenth in 0..=40 {
            let age = tenth as f64 * 0.01;
            let now = arrivals(Effects::Matrix, 37, age, 99);
            assert_eq!(glyphs(&now), before(37, age, 99), "at {age} s");
            for arrival in now {
                assert_eq!(
                    arrival.progress,
                    f64::from(u8::from(arrival.glyph.is_none()))
                );
            }
        }
    }

    #[test]
    fn slipstreams_look_never_shows_a_rain_glyph() {
        for tenth in 0..=40 {
            let age = tenth as f64 * 0.01;
            let now = arrivals(Effects::Slipstream, 37, age, 99);
            assert!(now.iter().all(|arrival| arrival.glyph.is_none()));
        }
    }

    /// Nothing shows at first and all of it has landed by the end; each character takes about
    /// `ARRIVE`, and they come roughly left to right.
    #[test]
    fn text_is_carried_in_left_to_right_in_the_same_time() {
        let count = 40;
        let progress = |age| -> Vec<f64> {
            arrivals(Effects::Slipstream, count, age, 3)
                .iter()
                .map(|arrival| arrival.progress)
                .collect()
        };
        assert!(progress(0.0).iter().all(|&p| p == 0.0));
        assert!(progress(DECODE).iter().all(|&p| p == 1.0));
        let middle = progress(DECODE / 2.0);
        let left: f64 = middle[..count / 2].iter().sum();
        let right: f64 = middle[count / 2..].iter().sum();
        assert!(left > right, "{left} then {right}");
        for i in 0..count {
            let started = (0..=320)
                .map(|ms| ms as f64 / 1000.0)
                .find(|&age| progress(age)[i] > 0.0);
            let landed = (0..=320)
                .map(|ms| ms as f64 / 1000.0)
                .find(|&age| progress(age)[i] == 1.0);
            let took = landed.unwrap() - started.unwrap();
            assert!(
                (took - ARRIVE).abs() < 0.0021,
                "character {i} took {took} s"
            );
        }
    }

    #[test]
    fn a_pictures_size_comes_from_its_header() {
        let mut png = Vec::new();
        crate::render::encode_png(&mut png, &[0; 3 * 2 * 4], 3, 2).unwrap();
        assert_eq!(png_size(&png), Some((3, 2)));
        assert_eq!(png_size(b"not a picture at all"), None);
    }

    #[test]
    fn terminals_are_known_by_their_app_ids() {
        assert!(is_terminal("org.kde.konsole"));
        assert!(is_terminal("foot"));
        assert!(!is_terminal("firefox"));
    }
}
