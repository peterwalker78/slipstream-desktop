//! Super+Shift+S: a snip of part of the screen.
//!
//! The focused screen freezes on its next frame and dims. Drag out a region, press the letter on a
//! window (or click it) for that window, or press Enter for the whole screen; Esc, or a right
//! click, snips nothing. Nothing moves underneath while it's up, so what is chosen is what was on
//! the screen when the keys were pressed.
//!
//! The snip is saved and copied as Print's screenshots are. On screen, the chosen part breaks
//! into a grid of fragments that scatter and fly into the toast saying where it went. With
//! Slipstream's effects a shower of streaks pours through each one, as through a closing window,
//! and carries it into the toast; with the Matrix's each turns into a rain glyph. The snip is over the moment it's chosen; the fragments
//! only paint afterwards.

use std::sync::Arc;

use resvg::tiny_skia::Pixmap;
use slipstream_config::Effects;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Element, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    input::keyboard::Keysym,
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};

use crate::{
    Slipstream,
    anim::Easing,
    bullet::ALPHABET,
    glmatrix::{self, Glyphs},
    overview,
    paint::{self, Painted, Painter},
    panel,
    render::{self, OutputElement},
    streaks,
    text::{self, Face, Style},
};

/// The middle of the largest part of `rect` that none of `over` covers, if any part big enough
/// to aim at is left.
fn spot(
    rect: Rectangle<f64, Logical>,
    over: &[Rectangle<f64, Logical>],
) -> Option<Point<f64, Logical>> {
    rect.subtract_rects(over.iter().copied())
        .into_iter()
        .filter(|part| part.size.w >= 8.0 && part.size.h >= 8.0)
        .max_by(|a, b| (a.size.w * a.size.h).total_cmp(&(b.size.w * b.size.h)))
        .map(|part| part.loc + Point::from((part.size.w / 2.0, part.size.h / 2.0)))
}

/// How far the pointer has to move before a press becomes a drag rather than a click.
const DRAG_MIN: f64 = 6.0;
/// The shade over everything not chosen.
const DIM: f32 = 0.5;
/// The fragments' flight, before each one's own delay.
const FLIGHT: f64 = 0.42;
/// With reduced motion the chosen part just fades.
const REDUCED_FLIGHT: f64 = 0.08;
/// With Slipstream's effects each fragment is poured through by a shower of streaks, as a
/// closing window is: three layers of them in cells this many logical pixels wide, nearest
/// first, with some cells left empty.
const SHOWER_CELLS: [f64; 3] = [22.0, 17.0, 12.0];
const SHOWER_EMPTY: f64 = 0.3;
/// A streak's length, core width and head glow radius in logical pixels, farthest to nearest.
const STREAK_LENGTH: (f64, f64) = (24.0, 96.0);
const STREAK_WIDTH: (f64, f64) = (2.5, 9.0);
const STREAK_HEAD: (f64, f64) = (4.5, 11.0);
/// How many of a streak's pixels it has to each logical pixel. Light is soft and bloomed, so a
/// shower is painted coarser than the screen and spread over it.
const STREAK_PIXELS: f64 = 0.6;
/// How much of the light covers what is underneath; the rest is added to it.
const STREAK_COVER: f32 = 0.6;
/// How far behind the bite the fragment glows, in logical pixels.
const SEAR: f64 = 10.0;
/// When, in a fragment's eased flight, its shower shows, and starts and finishes eating it.
const SHOWER_IN: (f64, f64) = (0.22, 0.4);
const EAT: (f64, f64) = (0.3, 0.75);
/// When, in the fragment's flight by the clock, its streaks turn from the fragment's own colours
/// to the rain's green: the eased flight is nearly over by half time, which would leave the
/// colours no time to be seen.
const GREEN: (f64, f64) = (0.25, 0.6);
/// Logical pixels per design pixel, which the fragment grid is sized in.
const DESIGN_PX: f64 = panel::DESIGN_PX as f64;

/// A snip while it's being chosen.
pub struct Snip {
    /// The screen it's of.
    pub output: String,
    /// The frozen picture and the choosing, once the screen's frame has been read back.
    pub frozen: Option<Frozen>,
}

pub struct Frozen {
    pixels: Arc<Vec<u8>>,
    size: Size<i32, Physical>,
    scale: f64,
    buffer: MemoryRenderBuffer,
    logical: Size<i32, Logical>,
    /// The windows on the screen as they were drawn, topmost first, each with its letter.
    windows: Vec<Target>,
    /// Where a press started and where the pointer is now, screen-local.
    press: Option<Point<f64, Logical>>,
    pointer: Point<f64, Logical>,
    legend: Option<Painted>,
    readout: Option<(String, Painted)>,
}

pub struct Target {
    pub label: String,
    pub name: String,
    /// Screen-local.
    pub rect: Rectangle<f64, Logical>,
    /// Where its letter goes: the middle of the largest part of it nothing covers.
    pub spot: Point<f64, Logical>,
}

/// What a key or a click did.
#[derive(Debug, PartialEq)]
pub enum Act {
    Nothing,
    Cancel,
    Take(Rectangle<f64, Logical>),
}

/// The rectangle between two corners, whichever way it was dragged.
pub fn between(a: Point<f64, Logical>, b: Point<f64, Logical>) -> Rectangle<f64, Logical> {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
}

/// How many columns and rows a fragment grid has for a part `w` × `h` logical pixels: about one
/// fragment per 170 design pixels, three to six across and three to five down.
pub fn grid(w: f64, h: f64) -> (usize, usize) {
    let cols = ((w / DESIGN_PX / 170.0).round() as usize).clamp(3, 6);
    let rows = ((h / DESIGN_PX / 170.0).round() as usize).clamp(3, 5);
    (cols, rows)
}

impl Frozen {
    fn is_dragging(&self) -> bool {
        self.press.is_some_and(|start| {
            (self.pointer.x - start.x).abs() >= DRAG_MIN
                || (self.pointer.y - start.y).abs() >= DRAG_MIN
        })
    }

    /// What is chosen right now: the region being dragged, or the window under the pointer.
    fn chosen(&self) -> Option<Rectangle<f64, Logical>> {
        if let Some(start) = self.press.filter(|_| self.is_dragging()) {
            return Some(between(start, self.pointer));
        }
        self.window_at(self.pointer).map(|target| target.rect)
    }

    fn window_at(&self, at: Point<f64, Logical>) -> Option<&Target> {
        self.windows.iter().find(|target| target.rect.contains(at))
    }

    fn whole(&self) -> Rectangle<f64, Logical> {
        Rectangle::from_size(self.logical.to_f64())
    }

    pub fn key(&self, key: Keysym) -> Act {
        match key {
            Keysym::Escape => Act::Cancel,
            Keysym::Return | Keysym::KP_Enter => Act::Take(self.whole()),
            _ => {
                let typed = smithay::input::keyboard::xkb::keysym_to_utf8(key).to_lowercase();
                self.windows
                    .iter()
                    .find(|target| !typed.is_empty() && target.label == typed)
                    .map_or(Act::Nothing, |target| Act::Take(target.rect))
            }
        }
    }

    /// A left press or release, or any other button, at `at` on the screen.
    pub fn button(&mut self, left: bool, pressed: bool, at: Point<f64, Logical>) -> Act {
        self.pointer = at;
        if !left {
            return if pressed { Act::Cancel } else { Act::Nothing };
        }
        if pressed {
            self.press = Some(at);
            return Act::Nothing;
        }
        let dragged = self.is_dragging();
        let start = self.press.take();
        match start {
            Some(start) if dragged => Act::Take(between(start, at)),
            Some(_) => self
                .window_at(at)
                .map_or(Act::Nothing, |target| Act::Take(target.rect)),
            None => Act::Nothing,
        }
    }

    pub fn moved(&mut self, at: Point<f64, Logical>) {
        self.pointer = at;
    }
}

impl Slipstream {
    /// Super+Shift+S: freeze the focused screen at its next frame and start choosing.
    pub fn start_snip(&mut self) {
        if self.lock.is_some() {
            return;
        }
        if self.shots.snip.is_some() {
            self.shots.snip = None;
            return;
        }
        let Some(output) = self.screens.focused_output() else {
            return;
        };
        // Whatever is open stays open, so it can be snipped too: the snip takes every key and
        // click first, and the panels are as they were afterwards.
        tracing::info!(screen = output.name(), "snip asked for");
        self.shots.snip = Some(Snip {
            output: output.name(),
            frozen: None,
        });
    }

    /// Reads back the frame `output` has just drawn for a snip waiting on it, without the pointer.
    pub fn serve_snip(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        elements: &[OutputElement],
        size: Size<i32, Physical>,
        scale: f64,
    ) {
        let waiting = self
            .shots
            .snip
            .as_ref()
            .is_some_and(|snip| snip.output == output.name() && snip.frozen.is_none());
        if !waiting {
            return;
        }
        let Some(screen) = self.space.output_geometry(output) else {
            return;
        };
        let without_pointer: Vec<&OutputElement> = elements
            .iter()
            .filter(|element| element.kind() != Kind::Cursor)
            .collect();
        let pixels = match render::copy_frame(renderer, &without_pointer, size, scale) {
            Ok(pixels) => Arc::new(pixels),
            Err(err) => {
                tracing::warn!("snip failed: {err}");
                self.shots.snip = None;
                self.show_toast("Snip not taken", "The screen couldn’t be read back.");
                return;
            }
        };
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Abgr8888,
            (size.w, size.h),
            1,
            Transform::Normal,
            None,
        );
        let windows = self.snip_targets(screen);
        let pointer = self.pointer_location() - screen.loc.to_f64();
        if let Some(snip) = self.shots.snip.as_mut() {
            snip.frozen = Some(Frozen {
                pixels,
                size,
                scale,
                buffer,
                logical: screen.size,
                windows,
                press: None,
                pointer,
                legend: None,
                readout: None,
            });
        }
    }

    /// The windows on `screen` as drawn that can be seen, topmost first, clipped to it, each
    /// given a letter. One too see-through to be seen, such as an X11 app's hidden helper, or
    /// covered by the windows over it, gets none.
    fn snip_targets(&self, screen: Rectangle<i32, Logical>) -> Vec<Target> {
        let screen = screen.to_f64();
        let drawn: Vec<_> = self
            .space
            .elements()
            .rev()
            .filter(|window| !crate::state::unseen(window))
            .filter_map(|window| {
                let drawn = self
                    .drawn
                    .fitted
                    .iter()
                    .find(|(fitted, _)| fitted == window)
                    .map(|(_, drawn)| *drawn)
                    .or_else(|| self.space.element_geometry(window).map(|r| r.to_f64()))?;
                let visible = drawn.intersection(screen)?;
                Some((
                    window,
                    Rectangle::new(visible.loc - screen.loc, visible.size),
                ))
            })
            .collect();
        let rects: Vec<_> = drawn.iter().map(|(_, rect)| *rect).collect();
        drawn
            .iter()
            .enumerate()
            .filter_map(|(at, (window, rect))| Some((window, *rect, spot(*rect, &rects[..at])?)))
            .zip(ALPHABET.chars())
            .map(|((window, rect, spot), letter)| Target {
                label: letter.to_string(),
                name: self.window_name(window),
                rect,
                spot,
            })
            .collect()
    }

    pub fn snip_key(&mut self, key: Keysym) {
        let act = match self
            .shots
            .snip
            .as_ref()
            .and_then(|snip| snip.frozen.as_ref())
        {
            Some(frozen) => frozen.key(key),
            // Still waiting for the frame: only Esc does anything.
            None if key == Keysym::Escape => Act::Cancel,
            None => Act::Nothing,
        };
        self.act_on_snip(act);
    }

    /// Where the screen being snipped sits in the layout.
    fn snip_origin(&self, snip: &Snip) -> Point<f64, Logical> {
        self.space
            .outputs()
            .find(|output| output.name() == snip.output)
            .and_then(|output| self.space.output_geometry(output))
            .map_or_else(Point::default, |geo| geo.loc.to_f64())
    }

    /// A pointer button while snipping: true when the snip took it.
    pub fn snip_button(&mut self, button: u32, pressed: bool) -> bool {
        let Some(snip) = self.shots.snip.as_ref() else {
            return false;
        };
        let at = self.pointer_location() - self.snip_origin(snip);
        let act = match self
            .shots
            .snip
            .as_mut()
            .and_then(|snip| snip.frozen.as_mut())
        {
            Some(frozen) => frozen.button(button == 0x110, pressed, at),
            None => Act::Nothing,
        };
        self.act_on_snip(act);
        true
    }

    pub fn snip_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        let Some(snip) = self.shots.snip.as_ref() else {
            return;
        };
        let origin = self.snip_origin(snip);
        if let Some(frozen) = self
            .shots
            .snip
            .as_mut()
            .and_then(|snip| snip.frozen.as_mut())
        {
            frozen.moved(pos - origin);
        }
    }

    fn act_on_snip(&mut self, act: Act) {
        match act {
            Act::Nothing => {}
            Act::Cancel => {
                tracing::info!("snip cancelled");
                self.shots.snip = None;
            }
            Act::Take(rect) => {
                let Some(Snip {
                    output,
                    frozen: Some(frozen),
                }) = self.shots.snip.take()
                else {
                    return;
                };
                let rect = rect
                    .intersection(frozen.whole())
                    .filter(|rect| rect.size.w >= 1.0 && rect.size.h >= 1.0);
                let Some(rect) = rect else {
                    return;
                };
                tracing::info!(?rect, "snip taken");
                self.save_screenshot(frozen.pixels.clone(), frozen.size, Some(rect), frozen.scale);
                let now = self.wall();
                let target = Point::from((
                    frozen.logical.w as f64 / 2.0,
                    crate::toast::one_line_centre(frozen.logical.h as f64),
                ));
                let flight = Flight::new(
                    output,
                    &frozen,
                    rect,
                    target,
                    now,
                    self.clock.reduced_motion,
                    self.settings.motion.effects,
                    self.ring_rgb,
                );
                self.shots.snip_flights.push(flight);
            }
        }
    }

    /// The snip being chosen on `output`, front to back: the legend, the window letters, the
    /// size of the region, its outline, the shade around it, and the frozen picture.
    pub fn snip_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        overview: &mut overview::Overview,
        scale: f64,
    ) -> Vec<OutputElement> {
        let mut elements = Vec::new();
        let ring = overview::ring(self.ring_rgb);
        let Some(frozen) = self
            .shots
            .snip
            .as_mut()
            .filter(|snip| snip.output == output.name())
            .and_then(|snip| snip.frozen.as_mut())
        else {
            return elements;
        };
        let scale2 = smithay::utils::Scale::from(scale);
        let (w, h) = (frozen.logical.w as f64, frozen.logical.h as f64);

        if frozen
            .legend
            .as_ref()
            .is_none_or(|legend| legend.scale != scale)
        {
            frozen.legend = paint_legend(scale);
        }
        if let Some(legend) = frozen.legend.as_ref() {
            let at = Point::from((
                (w - legend.logical.w as f64) / 2.0,
                h - 28.0 * DESIGN_PX - legend.logical.h as f64,
            ));
            elements.extend(legend.element(renderer, at, 1.0).map(OutputElement::Memory));
        }

        let chosen = frozen.chosen();
        let dragging = frozen.is_dragging();
        // Letters on the windows, unless a region is being dragged out.
        if !dragging {
            for target in &frozen.windows {
                elements.extend(
                    overview
                        .hint(
                            renderer,
                            &target.label,
                            Some(&target.name),
                            target.spot,
                            scale,
                            1.0,
                        )
                        .map(OutputElement::Memory),
                );
            }
        }
        if let Some(rect) = chosen {
            let label = format!(
                "{} × {}",
                (rect.size.w * scale).round() as i32,
                (rect.size.h * scale).round() as i32
            );
            if frozen
                .readout
                .as_ref()
                .is_none_or(|(text, painted)| *text != label || painted.scale != scale)
            {
                frozen.readout = paint_readout(&label, scale).map(|painted| (label, painted));
            }
            if let Some((_, readout)) = frozen.readout.as_ref() {
                let below = rect.loc.y + rect.size.h + 8.0;
                let y = if below + readout.logical.h as f64 > h - 8.0 {
                    (rect.loc.y - 8.0 - readout.logical.h as f64).max(8.0)
                } else {
                    below
                };
                let at = Point::from((rect.loc.x, y));
                elements.extend(
                    readout
                        .element(renderer, at, 1.0)
                        .map(OutputElement::Memory),
                );
            }
            elements.extend(
                overview
                    .outline(rect, 0.0, 2, ring, scale2, 1.0)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
        // The shade: everywhere but the chosen part.
        let rect = chosen.unwrap_or_default();
        let whole = |x: f64, y: f64, rw: f64, rh: f64| {
            Rectangle::<i32, Logical>::new(
                (x.round() as i32, y.round() as i32).into(),
                (rw.round() as i32, rh.round() as i32).into(),
            )
        };
        let (x0, y0) = (rect.loc.x, rect.loc.y);
        let (x1, y1) = (x0 + rect.size.w, y0 + rect.size.h);
        for shade in [
            whole(0.0, 0.0, w, y0),
            whole(0.0, y1, w, h - y1),
            whole(0.0, y0, x0, y1 - y0),
            whole(x1, y0, w - x1, y1 - y0),
        ] {
            if shade.size.w > 0 && shade.size.h > 0 {
                elements.push(OutputElement::Solid(overview.solid(
                    shade,
                    overview::SHADE,
                    scale2,
                    DIM,
                )));
            }
        }
        if let Ok(picture) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            &frozen.buffer,
            None,
            Some(Rectangle::from_size(
                (frozen.size.w as f64, frozen.size.h as f64).into(),
            )),
            Some(frozen.logical),
            Kind::Unspecified,
        ) {
            elements.push(OutputElement::Memory(picture));
        }
        elements
    }

    /// The fragments still flying on `output`, front to back.
    pub fn snip_flight_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
    ) -> Vec<OutputElement> {
        let now = self.wall();
        self.shots.snip_flights.retain(|flight| !flight.done(now));
        let name = output.name();
        self.shots
            .snip_flights
            .iter()
            .filter(|flight| flight.output == name)
            .flat_map(|flight| flight.elements(renderer, now))
            .collect()
    }
}

/// What a fragment turns into as it flies, painted once when it's made.
enum Turn {
    /// The rain's dark card with one glyph, the fragment's own size.
    Glyph(Painted),
    /// A shower of streaks that pours through it along its flight and eats it away.
    Shower(Box<Shower>),
}

/// A fragment's shower, painted once: its streaks in the fragment's own colours and again in the
/// rain's green, both drawn at the fragment's scale, and the glow just behind the bite.
struct Shower {
    own: Painted,
    green: Painted,
    /// Where in the streaks' sprite the bite's middle is, in logical pixels: their heads line up
    /// along the bite.
    anchor: Point<f64, Logical>,
    sear: Option<Painted>,
    /// The way the fragment flies, and the side of it the bite crosses towards.
    along: (f64, f64),
    axis: Axis,
}

/// The edge of a fragment its shower eats towards: the one its flight points at most.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Down,
    Up,
    Right,
    Left,
}

impl Axis {
    fn of(along: (f64, f64)) -> Self {
        match along {
            (x, y) if y.abs() >= x.abs() && y >= 0.0 => Axis::Down,
            (x, y) if y.abs() >= x.abs() => Axis::Up,
            (x, _) if x >= 0.0 => Axis::Right,
            _ => Axis::Left,
        }
    }

    fn unit(self) -> (f64, f64) {
        match self {
            Axis::Down => (0.0, 1.0),
            Axis::Up => (0.0, -1.0),
            Axis::Right => (1.0, 0.0),
            Axis::Left => (-1.0, 0.0),
        }
    }

    /// Across the axis, a quarter turn from it.
    fn across(self) -> (f64, f64) {
        let (x, y) = self.unit();
        (-y, x)
    }

    /// How deep a `w`×`h` fragment is along the axis, and how wide across it.
    fn depth(self, (w, h): (f64, f64)) -> (f64, f64) {
        match self {
            Axis::Down | Axis::Up => (h, w),
            Axis::Right | Axis::Left => (w, h),
        }
    }

    /// The part of a `w`×`h` fragment from `from` to `to` along the axis, measured from its
    /// middle, as a rectangle from its top left corner.
    fn part(self, (w, h): (f64, f64), from: f64, to: f64) -> Rectangle<f64, Logical> {
        let (x0, x1, y0, y1) = match self {
            Axis::Down => (0.0, w, from + h / 2.0, to + h / 2.0),
            Axis::Up => (0.0, w, h / 2.0 - to, h / 2.0 - from),
            Axis::Right => (from + w / 2.0, to + w / 2.0, 0.0, h),
            Axis::Left => (w / 2.0 - to, w / 2.0 - from, 0.0, h),
        };
        let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
        let (y0, y1) = (y0.clamp(0.0, h), y1.clamp(0.0, h));
        Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
    }
}

/// One fragment: its own picture and what it turns into, where it started, where it scatters to,
/// and when it sets off.
struct Piece {
    picture: Painted,
    turn: Option<Turn>,
    from: Rectangle<f64, Logical>,
    scatter: Point<f64, Logical>,
    delay: f64,
}

/// The chosen part breaking up and flying into the toast.
pub struct Flight {
    output: String,
    started: f64,
    reduced: bool,
    target: Point<f64, Logical>,
    pieces: Vec<Piece>,
}

impl Flight {
    // What is snipped and where it goes, then the clock and the look it flies in.
    #[allow(clippy::too_many_arguments)]
    fn new(
        output: String,
        frozen: &Frozen,
        rect: Rectangle<f64, Logical>,
        target: Point<f64, Logical>,
        now: f64,
        reduced: bool,
        effects: Effects,
        ring_rgb: [f32; 3],
    ) -> Self {
        let (cols, rows) = if reduced {
            (1, 1)
        } else {
            grid(rect.size.w, rect.size.h)
        };
        let (cw, ch) = (rect.size.w / cols as f64, rect.size.h / rows as f64);
        let mut seed = (now.to_bits() ^ 0x9e37_79b9_7f4a_7c15) | 1;
        let base = seed;
        let mut random = move || {
            // xorshift: enough to scatter a few dozen fragments.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut glyphs = match effects {
            Effects::Matrix if !reduced => Glyphs::load(),
            _ => None,
        };
        let mut pieces = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let from = Rectangle::new(
                    (rect.loc.x + col as f64 * cw, rect.loc.y + row as f64 * ch).into(),
                    (cw, ch).into(),
                );
                let Some((picture, pixels)) = crop_painted(frozen, from) else {
                    continue;
                };
                let tile = glyphs.as_mut().and_then(|glyphs| {
                    // Kana and digits, as the rain falls in.
                    let index = if random() < 0.7 {
                        160 + (random() * 16.0) as usize
                    } else {
                        16 + (random() * 10.0) as usize
                    };
                    paint_tile(glyphs, index, from.size, frozen.scale, ring_rgb)
                });
                let scatter =
                    Point::from(((random() - 0.5) * 0.9 * cw, (random() - 0.5) * 0.9 * ch));
                let turn = match effects {
                    _ if reduced => None,
                    Effects::Matrix => tile.map(Turn::Glyph),
                    Effects::Slipstream => {
                        let centre = from.loc + Point::from((cw / 2.0, ch / 2.0)) + scatter;
                        let along = target - centre;
                        let index = ((row * cols + col) as u64 + 1).wrapping_mul(0x2545_f491);
                        paint_shower(&picture, &pixels, from.size, along, base ^ index)
                            .map(|shower| Turn::Shower(Box::new(shower)))
                    }
                };
                pieces.push(Piece {
                    picture,
                    turn,
                    from,
                    scatter,
                    delay: if reduced {
                        0.0
                    } else {
                        random() * FLIGHT * 0.35
                    },
                });
            }
        }
        Self {
            output,
            started: now,
            reduced,
            target,
            pieces,
        }
    }

    fn duration(&self) -> f64 {
        if self.reduced {
            REDUCED_FLIGHT
        } else {
            FLIGHT * 1.35
        }
    }

    fn done(&self, now: f64) -> bool {
        now - self.started >= self.duration()
    }

    fn elements(&self, renderer: &mut GlesRenderer, now: f64) -> Vec<OutputElement> {
        let age = now - self.started;
        let mut elements = Vec::new();
        for piece in &self.pieces {
            if self.reduced {
                let alpha = (1.0 - age / REDUCED_FLIGHT).clamp(0.0, 1.0) as f32;
                elements.extend(
                    piece
                        .picture
                        .element(renderer, piece.from.loc, alpha)
                        .map(OutputElement::Memory),
                );
                continue;
            }
            let t = ((age - piece.delay) / FLIGHT).clamp(0.0, 1.0);
            let p = Easing::Bezier(0.3, 0.7, 0.2, 1.0).at(t);
            let place = fragment_at(p, piece, self.target);
            let size = Size::<i32, Logical>::from((
                place.rect.size.w.round().max(1.0) as i32,
                place.rect.size.h.round().max(1.0) as i32,
            ));
            match piece.turn.as_ref() {
                Some(Turn::Glyph(tile)) => elements.extend(
                    sized(
                        renderer,
                        tile,
                        place.rect.loc,
                        size,
                        place.glyph * place.alpha,
                    )
                    .map(OutputElement::Memory),
                ),
                Some(Turn::Shower(shower)) => {
                    shower.elements(renderer, piece, &place, (p, t), &mut elements);
                    continue;
                }
                None => {}
            }
            elements.extend(
                sized(renderer, &piece.picture, place.rect.loc, size, place.alpha)
                    .map(OutputElement::Memory),
            );
        }
        elements
    }
}

/// Where a fragment is at eased progress `p`: scattered by a fraction of a cell and shrunk to .78
/// at .38, then shrunk to .12 at the target and gone.
struct Place {
    rect: Rectangle<f64, Logical>,
    alpha: f32,
    /// How far what it turns into has faded in over it.
    glyph: f32,
}

fn fragment_at(p: f64, piece: &Piece, target: Point<f64, Logical>) -> Place {
    const MID: f64 = 0.38;
    let centre = Point::<f64, Logical>::from((
        piece.from.loc.x + piece.from.size.w / 2.0,
        piece.from.loc.y + piece.from.size.h / 2.0,
    ));
    let scattered = centre + piece.scatter;
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let (at, scale, alpha): (Point<f64, Logical>, f64, f64) = if p < MID {
        let t = p / MID;
        (
            Point::from((
                lerp(centre.x, scattered.x, t),
                lerp(centre.y, scattered.y, t),
            )),
            lerp(1.0, 0.78, t),
            1.0,
        )
    } else {
        let t = (p - MID) / (1.0 - MID);
        (
            Point::from((
                lerp(scattered.x, target.x, t),
                lerp(scattered.y, target.y, t),
            )),
            lerp(0.78, 0.12, t),
            1.0 - t,
        )
    };
    let (w, h) = (piece.from.size.w * scale, piece.from.size.h * scale);
    Place {
        rect: Rectangle::new((at.x - w / 2.0, at.y - h / 2.0).into(), (w, h).into()),
        alpha: alpha as f32,
        glyph: (p / 0.3).clamp(0.0, 1.0) as f32,
    }
}

/// A painted surface drawn at `size` rather than its own.
fn sized(
    renderer: &mut GlesRenderer,
    painted: &Painted,
    at: Point<f64, Logical>,
    size: Size<i32, Logical>,
    alpha: f32,
) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at.to_physical(painted.scale),
        &painted.buffer,
        Some(alpha),
        Some(Rectangle::from_size(
            (painted.device.0 as f64, painted.device.1 as f64).into(),
        )),
        Some(size),
        Kind::Unspecified,
    )
    .ok()
}

/// The frozen picture inside `rect` (screen-local, logical) as a surface of its own, and its
/// pixels.
fn crop_painted(frozen: &Frozen, rect: Rectangle<f64, Logical>) -> Option<(Painted, Vec<u8>)> {
    let px = crate::screenshot::to_pixels(rect, frozen.scale);
    let (pixels, w, h) = crate::screenshot::cropped(&frozen.pixels, frozen.size, px)?;
    let painted = Painted {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Abgr8888,
            (w as i32, h as i32),
            1,
            Transform::Normal,
            None,
        ),
        logical: Size::from((rect.size.w.round() as i32, rect.size.h.round() as i32)),
        device: (w as i32, h as i32),
        scale: frozen.scale,
        pieces: Default::default(),
    };
    Some((painted, pixels))
}

/// How far `p` is from `a` to `b`, from 0 to 1.
fn ramp(p: f64, (a, b): (f64, f64)) -> f64 {
    ((p - a) / (b - a)).clamp(0.0, 1.0)
}

/// Part of a painted surface, `src` in logical pixels of the `whole` it was painted at, drawn at
/// `at` and `size`.
fn part(
    renderer: &mut GlesRenderer,
    painted: &Painted,
    whole: (f64, f64),
    src: Rectangle<f64, Logical>,
    at: Point<f64, Logical>,
    size: (f64, f64),
    alpha: f32,
) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
    if src.size.w <= 0.0 || src.size.h <= 0.0 || alpha <= 0.0 {
        return None;
    }
    let (sx, sy) = (
        painted.device.0 as f64 / whole.0.max(1.0),
        painted.device.1 as f64 / whole.1.max(1.0),
    );
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at.to_physical(painted.scale),
        &painted.buffer,
        Some(alpha),
        Some(Rectangle::new(
            (src.loc.x * sx, src.loc.y * sy).into(),
            (src.size.w * sx, src.size.h * sy).into(),
        )),
        Some(Size::from((
            size.0.round().max(1.0) as i32,
            size.1.round().max(1.0) as i32,
        ))),
        Kind::Unspecified,
    )
    .ok()
}

impl Shower {
    /// The fragment at `place`, `p` into its eased flight and `t` by the clock, as its shower eats
    /// it from the trailing edge: the streaks, the glow just behind the bite, and what is left of
    /// the picture.
    fn elements(
        &self,
        renderer: &mut GlesRenderer,
        piece: &Piece,
        place: &Place,
        (p, t): (f64, f64),
        out: &mut Vec<OutputElement>,
    ) {
        let fragment = (piece.from.size.w, piece.from.size.h);
        let (depth, _) = self.axis.depth(fragment);
        let k = place.rect.size.w / fragment.0.max(1.0);
        // The bite crosses the fragment from its trailing edge to its leading one, and the
        // streaks run on ahead of it into the toast.
        let bite = depth * (ramp(p, EAT) + 0.3 * ramp(p, (EAT.1, 1.0)) - 0.5);
        let ahead = bite / lean(self.along, self.axis) * k;
        let centre = Point::<f64, Logical>::from((
            place.rect.loc.x + place.rect.size.w / 2.0,
            place.rect.loc.y + place.rect.size.h / 2.0,
        ));
        let at = centre + Point::from((self.along.0 * ahead, self.along.1 * ahead))
            - Point::from((self.anchor.x * k, self.anchor.y * k));
        let whole = (self.own.logical.w as f64, self.own.logical.h as f64);
        let size = (whole.0 * k, whole.1 * k);
        let all = Rectangle::from_size(whole.into());
        // Light all the way into the toast, going out only as it gets there.
        let light = (ramp(p, SHOWER_IN) * (place.alpha as f64).sqrt()) as f32;
        let green = ramp(t, GREEN);
        let green = (green * green * (3.0 - 2.0 * green)) as f32;
        for (painted, alpha) in [
            (&self.green, light * green),
            (&self.own, light * (1.0 - green)),
        ] {
            out.extend(
                part(renderer, painted, whole, all, at, size, alpha).map(OutputElement::Memory),
            );
        }
        let to_screen = |part: Rectangle<f64, Logical>| {
            (
                place.rect.loc + Point::from((part.loc.x * k, part.loc.y * k)),
                (part.size.w * k, part.size.h * k),
            )
        };
        if let Some(sear) = &self.sear {
            let band = sear_band(fragment, self.axis);
            let deep = SEAR.min(depth / 2.0 - bite);
            let src = self.axis.part(band, -SEAR / 2.0, deep - SEAR / 2.0);
            let (at, size) = to_screen(self.axis.part(fragment, bite, bite + deep));
            let alpha = (ramp(p, (0.0, 0.08)) as f32) * place.alpha;
            out.extend(part(renderer, sear, band, src, at, size, alpha).map(OutputElement::Memory));
        }
        let left = self.axis.part(fragment, bite, depth / 2.0);
        let (at, size) = to_screen(left);
        out.extend(
            part(
                renderer,
                &piece.picture,
                fragment,
                left,
                at,
                size,
                place.alpha,
            )
            .map(OutputElement::Memory),
        );
    }
}

/// How far a flight along `along` goes along `axis` for each pixel it travels.
fn lean(along: (f64, f64), axis: Axis) -> f64 {
    let (ax, ay) = axis.unit();
    (along.0 * ax + along.1 * ay).max(0.5)
}

/// One streak of a fragment's shower: its head, with the bite's middle as the origin, how near
/// it is from 0 (far) to 1, and its colours, the fragment's own and the rain's green.
struct Streak {
    head: (f64, f64),
    near: f64,
    own: [f32; 3],
    green: [f32; 3],
}

/// A fragment's shower for a flight along `along`, painted from its own `pixels` (RGBA, at
/// `picture`'s device size), `fragment` logical pixels big.
fn paint_shower(
    picture: &Painted,
    pixels: &[u8],
    fragment: Size<f64, Logical>,
    along: Point<f64, Logical>,
    seed: u64,
) -> Option<Shower> {
    let reach = (along.x * along.x + along.y * along.y).sqrt();
    let along = if reach > 1e-6 {
        (along.x / reach, along.y / reach)
    } else {
        (0.0, 1.0)
    };
    let axis = Axis::of(along);
    let fragment = (fragment.w, fragment.h);
    let sample = Sampler {
        pixels,
        device: picture.device,
        fragment,
    };
    let shower = shower_streaks(fragment, along, axis, &sample, seed);
    let ShowerLight {
        own,
        green,
        logical,
        anchor,
    } = paint_streaks(&shower, along)?;
    let painted = |pixmap: Pixmap| Painted {
        buffer: paint::buffer(&pixmap),
        logical,
        device: (pixmap.width() as i32, pixmap.height() as i32),
        scale: picture.scale,
        pieces: Default::default(),
    };
    Some(Shower {
        own: painted(own),
        green: painted(green),
        anchor,
        sear: paint_sear(fragment, axis, &sample, picture.scale),
        along,
        axis,
    })
}

/// A fragment's picture, read by where in the fragment a point is.
struct Sampler<'a> {
    pixels: &'a [u8],
    device: (i32, i32),
    fragment: (f64, f64),
}

impl Sampler<'_> {
    /// The colour at (`x`, `y`) logical pixels from the fragment's top left, from 0 to 1.
    fn at(&self, x: f64, y: f64) -> [f32; 3] {
        let (w, h) = (self.device.0.max(1) as usize, self.device.1.max(1) as usize);
        let px = ((x / self.fragment.0.max(1.0) * w as f64).max(0.0) as usize).min(w - 1);
        let py = ((y / self.fragment.1.max(1.0) * h as f64).max(0.0) as usize).min(h - 1);
        let at = (py * w + px) * 4;
        match self.pixels.get(at..at + 3) {
            Some(rgb) => std::array::from_fn(|i| rgb[i] as f32 / 255.0),
            None => [0.0; 3],
        }
    }
}

/// A closing window's streak colours for a pixel of `rgb`: its own, lifted a little and touched
/// with the rain's green, and the green it turns to, brighter where the picture was.
fn shower_colours(rgb: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let ink = glmatrix::TINT;
    let luma = 0.3 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2];
    (
        std::array::from_fn(|i| rgb[i] * 1.2 + ink[i] * 0.2),
        ink.map(|c| c * (0.55 + 0.6 * luma)),
    )
}

/// The streaks of a fragment's shower, as a closing window's: three layers of cells across the
/// bite, nearest first, some empty, each streak lagging behind the bite the farther it is, and
/// coloured from the part of the picture it pours through. At a slant the shower is wider than
/// the fragment, so that every part of it is crossed.
fn shower_streaks(
    fragment: (f64, f64),
    along: (f64, f64),
    axis: Axis,
    sample: &Sampler,
    seed: u64,
) -> Vec<Streak> {
    let mut seed = seed | 1;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (depth, width) = axis.depth(fragment);
    let (ax, ay) = axis.unit();
    let (cx, cy) = axis.across();
    // How far a streak moves across the bite for each pixel the bite moves on.
    let drift = (along.0 * cx + along.1 * cy) / lean(along, axis);
    let half = width / 2.0 + depth / 2.0 * drift.abs();
    let mut streaks = Vec::new();
    for (layer, cell) in SHOWER_CELLS.into_iter().enumerate() {
        let offset = random() * cell;
        let cells = (2.0 * half / cell).ceil() as usize + 1;
        for i in 0..cells {
            let (empty, near, x, y) = (random(), random(), random(), random());
            let (lag, pace) = (random(), random());
            let across = -half - offset + (i as f64 + 0.25 + 0.5 * x) * cell;
            if empty < SHOWER_EMPTY || across.abs() > half {
                continue;
            }
            // Nearer in the first layer, farther in the last. Far ones fall behind the bite, and
            // each keeps a pace of its own, near ones closer to the bite's.
            let near = (near * 0.6 + (2 - layer) as f64 * 0.2).clamp(0.0, 1.0);
            let lag = (0.02 + 0.4 * (1.0 - near) * lag + 0.25 * (1.0 - near * 0.6) * pace) * depth;
            // The picture where it crosses the fragment, somewhere along its depth.
            let down = (y - 0.5) * depth;
            let side = (across + down * drift).clamp(-width / 2.0, width / 2.0);
            let (lx, ly) = (side * cx + down * ax, side * cy + down * ay);
            let (own, green) =
                shower_colours(sample.at(lx + fragment.0 / 2.0, ly + fragment.1 / 2.0));
            streaks.push(Streak {
                head: (across * cx - lag * along.0, across * cy - lag * along.1),
                near,
                own,
                green,
            });
        }
    }
    streaks
}

/// A streak measure for depth `near`, from its farthest to its nearest.
fn by_depth((far, near): (f64, f64), at: f64) -> f64 {
    far + (near - far) * at
}

/// The light of a `shower` of streaks running along `along`, twice: in their own colours and in
/// the rain's green. Each is a closing window's streak, a core brightening towards a glowing
/// head, with the streams' faint trail behind and their bloom.
fn paint_streaks(shower: &[Streak], along: (f64, f64)) -> Option<ShowerLight> {
    if shower.is_empty() {
        return None;
    }
    let bloom = (streaks::BLOOM as f64 * STREAK_PIXELS).round().max(1.0) as usize;
    let margin = bloom as f64 / STREAK_PIXELS + 1.0;
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for streak in shower {
        let reach = by_depth(STREAK_LENGTH, streak.near) * (1.0 + streaks::TRAIL as f64);
        let r = by_depth(STREAK_HEAD, streak.near) + margin;
        let (hx, hy) = streak.head;
        for (x, y) in [(hx, hy), (hx - along.0 * reach, hy - along.1 * reach)] {
            bounds = (
                bounds.0.min(x - r),
                bounds.1.min(y - r),
                bounds.2.max(x + r),
                bounds.3.max(y + r),
            );
        }
    }
    let (x0, y0) = (bounds.0.floor(), bounds.1.floor());
    let logical =
        Size::<i32, Logical>::from(((bounds.2 - x0).ceil() as i32, (bounds.3 - y0).ceil() as i32));
    let (w, h) = (
        (logical.w as f64 * STREAK_PIXELS).ceil().max(1.0) as usize,
        (logical.h as f64 * STREAK_PIXELS).ceil().max(1.0) as usize,
    );
    let mut own = vec![0f32; w * h * 3];
    let mut green = vec![0f32; w * h * 3];
    for streak in shower {
        light_streak(&mut own, &mut green, (w, h), streak, along, (x0, y0));
    }
    let mut across = Vec::new();
    streaks::bloom(&mut own, &mut across, w, h, bloom);
    streaks::bloom(&mut green, &mut across, w, h, bloom);
    Some(ShowerLight {
        own: light_pixmap(&own, w, h)?,
        green: light_pixmap(&green, w, h)?,
        logical,
        anchor: Point::from((-x0, -y0)),
    })
}

/// A shower's streaks as `paint_streaks` paints them.
struct ShowerLight {
    own: Pixmap,
    green: Pixmap,
    logical: Size<i32, Logical>,
    anchor: Point<f64, Logical>,
}

/// Adds one streak's light to `own` and `green`, `w`×`h` pixels of RGB, whose top left is at
/// `origin` in the streaks' logical pixels. Only the pixels near the streak are visited: along
/// each row it crosses, or each column when it runs flatter than steep.
fn light_streak(
    own: &mut [f32],
    green: &mut [f32],
    (w, h): (usize, usize),
    streak: &Streak,
    along: (f64, f64),
    origin: (f64, f64),
) {
    let near = streak.near;
    let length = by_depth(STREAK_LENGTH, near);
    let width = by_depth(STREAK_WIDTH, near);
    let radius = by_depth(STREAK_HEAD, near);
    let reach = length * (1.0 + streaks::TRAIL as f64);
    let bright = (0.4 + 0.6 * near) as f32;
    let hot = |rgb: [f32; 3]| rgb.map(|c| c + (1.0 - c) * 0.6);
    let (hot_own, hot_green) = (hot(streak.own), hot(streak.green));
    let head = (
        (streak.head.0 - origin.0) * STREAK_PIXELS,
        (streak.head.1 - origin.1) * STREAK_PIXELS,
    );
    let end = (
        head.0 - along.0 * reach * STREAK_PIXELS,
        head.1 - along.1 * reach * STREAK_PIXELS,
    );
    let r = radius.max(width / 2.0 + 1.0) * STREAK_PIXELS + 1.0;
    let steep = along.1.abs() >= along.0.abs();
    // The rows (or columns) the streak runs over, and how far across each it can light.
    let (from, to, runs, across) = if steep {
        (head.1.min(end.1) - r, head.1.max(end.1) + r, h, w)
    } else {
        (head.0.min(end.0) - r, head.0.max(end.0) + r, w, h)
    };
    let (slope, spread) = if steep {
        (along.0 / along.1, r / along.1.abs())
    } else {
        (along.1 / along.0, r / along.0.abs())
    };
    for run in (from.floor().max(0.0) as usize)..(to.ceil().max(0.0) as usize).min(runs) {
        let m = run as f64 + 0.5;
        let centre = if steep {
            head.0 + (m - head.1) * slope
        } else {
            head.1 + (m - head.0) * slope
        };
        let first = (centre - spread).floor().max(0.0) as usize;
        let last = ((centre + spread).ceil().max(0.0) as usize).min(across);
        for step in first..last {
            let (x, y) = if steep { (step, run) } else { (run, step) };
            // How far behind the head, and how far off the line, in logical pixels.
            let qx = (x as f64 + 0.5 - head.0) / STREAK_PIXELS;
            let qy = (y as f64 + 0.5 - head.1) / STREAK_PIXELS;
            let behind = -(qx * along.0 + qy * along.1);
            let off = (qx * along.1 - qy * along.0).abs();
            let cover = (width * 0.5 + 0.5 - off).clamp(0.0, 1.0);
            let body = if (0.0..=length).contains(&behind) {
                (1.0 - behind / length).powf(1.6)
            } else if behind > length && behind <= reach {
                (1.0 - (behind - length) / (reach - length)) * streaks::TRAIL_LIGHT as f64
            } else {
                0.0
            } * cover;
            let d = (behind * behind + off * off).sqrt() / radius;
            let glow = (1.0 - d).max(0.0).powi(2);
            if body <= 0.0 && glow <= 0.0 {
                continue;
            }
            let (body, glow) = (body as f32 * bright, glow as f32 * bright);
            let at = (y * w + x) * 3;
            for (sum, (c, hot)) in own[at..at + 3]
                .iter_mut()
                .zip(streak.own.iter().zip(&hot_own))
            {
                *sum += c * body + hot * glow;
            }
            for (sum, (c, hot)) in green[at..at + 3]
                .iter_mut()
                .zip(streak.green.iter().zip(&hot_green))
            {
                *sum += c * body + hot * glow;
            }
        }
    }
}

/// Light as a sprite: its colour, with `STREAK_COVER` of its brightness covering what is under it
/// and the rest added on.
fn light_pixmap(sum: &[f32], w: usize, h: usize) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(w as u32, h as u32)?;
    for (pixel, light) in pixmap
        .data_mut()
        .chunks_exact_mut(4)
        .zip(sum.chunks_exact(3))
    {
        let rgb: [f32; 3] = std::array::from_fn(|i| light[i].clamp(0.0, 1.0));
        let alpha = rgb.into_iter().fold(0.0f32, f32::max) * STREAK_COVER;
        for (channel, c) in pixel.iter_mut().zip(rgb) {
            *channel = (c * 255.0).round() as u8;
        }
        pixel[3] = (alpha * 255.0).round() as u8;
    }
    Some(pixmap)
}

/// The band that glows behind a fragment's bite, in logical pixels: across the whole fragment,
/// `SEAR` deep.
fn sear_band(fragment: (f64, f64), axis: Axis) -> (f64, f64) {
    match axis {
        Axis::Down | Axis::Up => (fragment.0, SEAR),
        Axis::Right | Axis::Left => (SEAR, fragment.1),
    }
}

/// The glow behind a fragment's bite, as a closing window's: hottest at the bite and gone `SEAR`
/// pixels in, each line of it in the colour of the picture along it, taken halfway to the rain's
/// green and then halfway to white.
fn paint_sear(fragment: (f64, f64), axis: Axis, sample: &Sampler, scale: f64) -> Option<Painted> {
    /// How many points along each line its colour is read from.
    const READS: usize = 8;
    let band = sear_band(fragment, axis);
    let (w, h) = (
        (band.0 * STREAK_PIXELS).ceil().max(1.0) as usize,
        (band.1 * STREAK_PIXELS).ceil().max(1.0) as usize,
    );
    let (depth, _) = axis.depth(fragment);
    let (ax, ay) = axis.unit();
    let ink = glmatrix::TINT;
    let mut sum = vec![0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let (bx, by) = (
                (x as f64 + 0.5) / STREAK_PIXELS,
                (y as f64 + 0.5) / STREAK_PIXELS,
            );
            // How far in from the bite, as a share of the band.
            let a = (bx - band.0 / 2.0) * ax + (by - band.1 / 2.0) * ay;
            let amount = ((0.5 - a / SEAR).clamp(0.0, 1.0).powi(2) * 0.8) as f32;
            let mut own = [0f32; 3];
            for i in 0..READS {
                let along = (i as f64 + 0.5) / READS as f64 * depth;
                let (sx, sy) = if ax == 0.0 { (bx, along) } else { (along, by) };
                let (colour, _) = shower_colours(sample.at(sx, sy));
                for (sum, c) in own.iter_mut().zip(colour) {
                    *sum += c / READS as f32;
                }
            }
            let at = (y * w + x) * 3;
            for (sum, (c, k)) in sum[at..at + 3].iter_mut().zip(own.iter().zip(&ink)) {
                let hot = (c + k) / 2.0;
                *sum = (hot + (1.0 - hot) * 0.5) * amount;
            }
        }
    }
    let pixmap = light_pixmap(&sum, w, h)?;
    Some(Painted {
        buffer: paint::buffer(&pixmap),
        logical: Size::from((band.0.round() as i32, band.1.round() as i32)),
        device: (w as i32, h as i32),
        scale,
        pieces: Default::default(),
    })
}

/// A fragment's glyph tile: the rain's dark card edged in the ring's colour, with one glyph.
fn paint_tile(
    glyphs: &mut Glyphs,
    index: usize,
    size: Size<f64, Logical>,
    scale: f64,
    ring_rgb: [f32; 3],
) -> Option<Painted> {
    let logical = Size::<i32, Logical>::from((
        size.w.round().max(1.0) as i32,
        size.h.round().max(1.0) as i32,
    ));
    let [r, g, b] = ring_rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    let edge = u32::from_be_bytes([r, g, b, 0x33]);
    let device = (
        (logical.w as f64 * scale).round().max(1.0) as i32,
        (logical.h as f64 * scale).round().max(1.0) as i32,
    );
    let (w, h) = device;
    let mut card = Painter::new(w as u32, h as u32, scale as f32)?;
    card.fill(
        0.0,
        0.0,
        logical.w as f32,
        logical.h as f32,
        3.0,
        panel::CHIP | 0xff,
    );
    card.border(0.0, 0.0, logical.w as f32, logical.h as f32, 3.0, 1.0, edge);
    let side = ((w.min(h) as f64) * 0.7).round().max(1.0) as usize;
    let coverage = glyphs.coverage(index, (side, side))?;
    // The glyph laid over the opaque card in the ring's colour.
    let (ox, oy) = ((w as usize - side) / 2, (h as usize - side) / 2);
    let data = card.pixmap.data_mut();
    for gy in 0..side {
        for gx in 0..side {
            let cover = coverage[gy * side + gx] as u32;
            if cover == 0 {
                continue;
            }
            let at = ((oy + gy) * w as usize + ox + gx) * 4;
            for (channel, ink) in [r, g, b].into_iter().enumerate() {
                let under = data[at + channel] as u32;
                data[at + channel] = ((ink as u32 * cover + under * (255 - cover)) / 255) as u8;
            }
            data[at + 3] = 255;
        }
    }
    Some(Painted {
        buffer: paint::buffer(&card.pixmap),
        logical,
        device,
        scale,
        pieces: Default::default(),
    })
}

fn paint_legend(scale: f64) -> Option<Painted> {
    let style = Style::new(Face::Mono, 13.0, panel::HINT);
    let legend = "drag a region · a–z a window · ⏎ whole screen · Esc cancel";
    let w = (text::width(legend, &style) + 32.0) * DESIGN_PX as f32;
    let h = (13.0 * 1.2 + 16.0) * DESIGN_PX as f32;
    let logical = Size::from((w.ceil() as i32, h.ceil() as i32));
    Painted::new(logical, scale, |p| {
        let f = DESIGN_PX as f32;
        p.fill(0.0, 0.0, w, h, 10.0 * f, panel::CHIP | 0xe6);
        let style = Style::new(Face::Mono, 13.0 * f, panel::HINT);
        p.text(legend, 16.0 * f, h / 2.0, &style);
    })
}

fn paint_readout(label: &str, scale: f64) -> Option<Painted> {
    let style = Style::new(Face::Mono, 12.0, panel::INK);
    let w = text::width(label, &style) + 16.0;
    let h = 22.0;
    let logical = Size::from((w.ceil() as i32, h as i32));
    Painted::new(logical, scale, |p| {
        p.fill(0.0, 0.0, w, h, 6.0, panel::CHIP | 0xe6);
        p.text(label, 8.0, h / 2.0, &style);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    /// A window hidden behind others gets no letter, and one partly covered has its letter on the
    /// part that shows.
    #[test]
    fn only_a_window_that_shows_is_given_a_letter() {
        let rect = |x, y, w, h| Rectangle::<f64, Logical>::new((x, y).into(), (w, h).into());
        let back = rect(0.0, 0.0, 400.0, 300.0);
        assert_eq!(spot(back, &[rect(0.0, 0.0, 400.0, 300.0)]), None);
        assert_eq!(spot(back, &[rect(-10.0, -10.0, 500.0, 400.0)]), None);
        // The front covers its left three quarters: its letter goes in the quarter left.
        let partly = spot(back, &[rect(0.0, 0.0, 300.0, 300.0)]).unwrap();
        assert_eq!((partly.x, partly.y), (350.0, 150.0));
        assert_eq!(spot(back, &[]), Some(point(200.0, 150.0)));
        // A sliver too thin to aim at doesn't count.
        assert_eq!(spot(back, &[rect(0.0, 0.0, 396.0, 300.0)]), None);
    }

    #[test]
    fn a_region_dragged_any_way_is_the_same_rectangle() {
        let a = between(point(10.0, 20.0), point(110.0, 70.0));
        let b = between(point(110.0, 70.0), point(10.0, 20.0));
        assert_eq!(a, b);
        assert_eq!(a.size, (100.0, 50.0).into());
    }

    #[test]
    fn the_fragment_grid_stays_between_three_and_six_across() {
        assert_eq!(grid(40.0, 40.0), (3, 3));
        assert_eq!(grid(1536.0, 960.0), (6, 5));
        assert_eq!(grid(544.0, 408.0), (4, 3));
    }

    fn frozen(w: i32, h: i32) -> Frozen {
        Frozen {
            pixels: Arc::new(vec![40; (w * h * 4) as usize]),
            size: Size::from((w, h)),
            scale: 1.0,
            buffer: MemoryRenderBuffer::default(),
            logical: Size::from((w, h)),
            windows: Vec::new(),
            press: None,
            pointer: point(0.0, 0.0),
            legend: None,
            readout: None,
        }
    }

    fn fly(effects: Effects, reduced: bool) -> Flight {
        let rect = Rectangle::new((0.0, 0.0).into(), (600.0, 400.0).into());
        let target = point(300.0, 40.0);
        let ring = [0.2, 0.9, 0.5];
        Flight::new(
            "screen".into(),
            &frozen(600, 400),
            rect,
            target,
            1.0,
            reduced,
            effects,
            ring,
        )
    }

    #[test]
    fn a_snip_streams_into_the_toast_with_no_rain_glyph_with_slipstreams_effects() {
        let flight = fly(Effects::Slipstream, false);
        assert!(!flight.pieces.is_empty());
        let streaks = |piece: &Piece| matches!(piece.turn, Some(Turn::Shower(_)));
        assert!(flight.pieces.iter().all(streaks));
        // Reduced motion only fades the picture, whichever the look.
        for effects in [Effects::Slipstream, Effects::Matrix] {
            assert!(
                fly(effects, true)
                    .pieces
                    .iter()
                    .all(|piece| piece.turn.is_none())
            );
        }
    }

    #[test]
    fn a_snip_still_turns_into_rain_glyphs_with_the_matrixs_effects() {
        let flight = fly(Effects::Matrix, false);
        let glyphs = |piece: &Piece| matches!(piece.turn, Some(Turn::Glyph(_)) | None);
        assert!(flight.pieces.iter().all(glyphs));
    }

    /// A red, white and black picture pours out in red, white and dim streaks, which turn to
    /// the rain's green; nothing is in the ring's colour.
    #[test]
    fn a_snips_streaks_are_coloured_from_its_own_picture_then_the_rains_green() {
        let (w, h) = (60usize, 60usize);
        let mut pixels = vec![0u8; w * h * 4];
        for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let rgba = match (i % w) * 3 / w {
                0 => [230, 20, 20, 255],
                1 => [255, 255, 255, 255],
                _ => [0, 0, 0, 255],
            };
            pixel.copy_from_slice(&rgba);
        }
        let picture = Painted {
            buffer: MemoryRenderBuffer::default(),
            logical: Size::from((60, 60)),
            device: (60, 60),
            scale: 1.0,
            pieces: Default::default(),
        };
        let sample = Sampler {
            pixels: &pixels,
            device: (60, 60),
            fragment: (60.0, 60.0),
        };
        let along = (0.0, 1.0);
        let shower = shower_streaks((60.0, 60.0), along, Axis::Down, &sample, 7);
        assert!(shower.len() >= 6, "a shower, not a streak or two");
        let red = |rgb: [f32; 3]| rgb[0] > 0.9 && rgb[1] < 0.35;
        let white = |rgb: [f32; 3]| rgb.iter().all(|&c| c > 0.9);
        assert!(shower.iter().any(|streak| red(streak.own)));
        assert!(shower.iter().any(|streak| white(streak.own)));
        for streak in &shower {
            assert!(streak.green[1] > streak.green[0] && streak.green[1] > streak.green[2]);
        }
        // Both sprites are painted once, here, and the heads line up along the bite.
        let shower = paint_shower(
            &picture,
            &pixels,
            Size::from((60.0, 60.0)),
            point(0.0, 9.0),
            7,
        )
        .unwrap();
        assert_eq!(shower.axis, Axis::Down);
        assert_eq!(shower.own.logical, shower.green.logical);
        assert!(shower.anchor.y > 0.0 && shower.anchor.y < shower.own.logical.h as f64);
    }

    /// The bite eats a fragment from the edge it flies away from.
    #[test]
    fn a_fragment_is_eaten_from_its_trailing_edge() {
        let size = (100.0, 50.0);
        assert_eq!(Axis::of((0.3, 0.9)), Axis::Down);
        assert_eq!(Axis::of((-0.9, 0.3)), Axis::Left);
        let down = Axis::Down.part(size, -10.0, 25.0);
        assert_eq!(
            down,
            Rectangle::new((0.0, 15.0).into(), (100.0, 35.0).into())
        );
        let up = Axis::Up.part(size, -10.0, 25.0);
        assert_eq!(up, Rectangle::new((0.0, 0.0).into(), (100.0, 35.0).into()));
        let left = Axis::Left.part(size, 0.0, 50.0);
        assert_eq!(left, Rectangle::new((0.0, 0.0).into(), (50.0, 50.0).into()));
    }

    #[test]
    fn a_fragment_ends_small_and_gone_at_the_target() {
        let piece = Piece {
            picture: Painted {
                buffer: MemoryRenderBuffer::default(),
                logical: Size::from((100, 100)),
                device: (100, 100),
                scale: 1.0,
                pieces: Default::default(),
            },
            turn: None,
            from: Rectangle::new((0.0, 0.0).into(), (100.0, 100.0).into()),
            scatter: point(10.0, -10.0),
            delay: 0.0,
        };
        let target = point(700.0, 60.0);
        let start = fragment_at(0.0, &piece, target);
        assert_eq!(start.rect, piece.from);
        assert_eq!(start.alpha, 1.0);
        let end = fragment_at(1.0, &piece, target);
        assert_eq!(end.alpha, 0.0);
        assert!((end.rect.size.w - 12.0).abs() < 1e-9);
        assert!((end.rect.loc.x + end.rect.size.w / 2.0 - 700.0).abs() < 1e-9);
    }
}
