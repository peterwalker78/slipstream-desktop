//! Super+Shift+S: a snip of part of the screen.
//!
//! The focused screen freezes on its next frame and dims. Drag out a region, press the letter on a
//! window (or click it) for that window, or press Enter for the whole screen; Esc, or a right
//! click, snips nothing. Nothing moves underneath while it's up, so what is chosen is what was on
//! the screen when the keys were pressed.
//!
//! The snip is saved and copied as Print's screenshots are. On screen, the chosen part breaks
//! into a grid of fragments that scatter and fly into the toast saying where it went. With
//! Slipstream's effects each one stretches into a streak of light along its flight; with the
//! Matrix's each turns into a rain glyph. The snip is over the moment it's chosen; the fragments
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
    glmatrix::Glyphs,
    overview,
    paint::{self, Painted, Painter},
    panel,
    render::{self, OutputElement},
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
/// A fragment's streak of light: how long it is against the fragment's larger side, how thick
/// its core is against the smaller (in logical pixels, within bounds), how far its head glows
/// against that, and how many of its pixels it has to each logical pixel. Light is soft, so a
/// streak is painted coarser than the screen and spread over it.
const STREAK_LENGTH: f64 = 1.6;
const STREAK_THICKNESS: f64 = 0.06;
const STREAK_THICKEST: (f64, f64) = (2.0, 8.0);
const STREAK_HEAD: f64 = 2.2;
const STREAK_PIXELS: f64 = 0.6;
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
    /// A streak of light along its flight, drawn at the fragment's scale round its centre.
    Streak(Painted),
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
                let Some((picture, average)) = crop_painted(frozen, from) else {
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
                        paint_streak(from.size, along, average, ring_rgb, frozen.scale)
                            .map(Turn::Streak)
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
            let place = fragment_at(Easing::Bezier(0.3, 0.7, 0.2, 1.0).at(t), piece, self.target);
            let size = Size::<i32, Logical>::from((
                place.rect.size.w.round().max(1.0) as i32,
                place.rect.size.h.round().max(1.0) as i32,
            ));
            let mut picture_alpha = place.alpha;
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
                Some(Turn::Streak(streak)) => {
                    // The streak is mostly clear, so the picture fades out as it fades in.
                    picture_alpha *= 1.0 - place.glyph;
                    let (at, size) = streak_at(&place, piece, streak.logical);
                    elements.extend(
                        sized(renderer, streak, at, size, place.glyph * place.alpha)
                            .map(OutputElement::Memory),
                    );
                }
                None => {}
            }
            elements.extend(
                sized(
                    renderer,
                    &piece.picture,
                    place.rect.loc,
                    size,
                    picture_alpha,
                )
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

/// Where a fragment's streak is drawn while the fragment is at `place`: round the same centre,
/// scaled as the fragment is.
fn streak_at(
    place: &Place,
    piece: &Piece,
    streak: Size<i32, Logical>,
) -> (Point<f64, Logical>, Size<i32, Logical>) {
    let k = place.rect.size.w / piece.from.size.w.max(1.0);
    let (w, h) = (streak.w as f64 * k, streak.h as f64 * k);
    let centre = place.rect.loc + Point::from((place.rect.size.w / 2.0, place.rect.size.h / 2.0));
    let size = Size::from((w.round().max(1.0) as i32, h.round().max(1.0) as i32));
    (centre - Point::from((w / 2.0, h / 2.0)), size)
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
/// average colour.
fn crop_painted(frozen: &Frozen, rect: Rectangle<f64, Logical>) -> Option<(Painted, [f32; 3])> {
    let px = crate::screenshot::to_pixels(rect, frozen.scale);
    let (pixels, w, h) = crate::screenshot::cropped(&frozen.pixels, frozen.size, px)?;
    let average = average(&pixels);
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
    Some((painted, average))
}

/// The average colour of RGBA pixels, from 0 to 1.
fn average(pixels: &[u8]) -> [f32; 3] {
    let mut sum = [0u64; 3];
    for pixel in pixels.chunks_exact(4) {
        for (total, &channel) in sum.iter_mut().zip(pixel) {
            *total += channel as u64;
        }
    }
    let count = (pixels.len() / 4).max(1) as f32;
    sum.map(|total| total as f32 / count / 255.0)
}

/// A fragment's streak of light, as the streams draw theirs: a soft line along `along` with a
/// brighter, glowing head at the front, thinning to nothing behind. Its colour is the fragment's
/// own `average`, brought up to a light's brightness, lifting into the ring's colour at the head.
/// The streak is centred in its sprite, which is just big enough to hold it at its angle.
fn paint_streak(
    fragment: Size<f64, Logical>,
    along: Point<f64, Logical>,
    average: [f32; 3],
    ring_rgb: [f32; 3],
    scale: f64,
) -> Option<Painted> {
    let (pixmap, logical) = streak_light(fragment, along, average, ring_rgb)?;
    Some(Painted {
        buffer: paint::buffer(&pixmap),
        logical,
        device: (pixmap.width() as i32, pixmap.height() as i32),
        scale,
        pieces: Default::default(),
    })
}

/// The pixels of `paint_streak`'s sprite, `STREAK_PIXELS` to each logical pixel, and its size.
fn streak_light(
    fragment: Size<f64, Logical>,
    along: Point<f64, Logical>,
    average: [f32; 3],
    ring_rgb: [f32; 3],
) -> Option<(Pixmap, Size<i32, Logical>)> {
    let length = STREAK_LENGTH * fragment.w.max(fragment.h);
    let (thinnest, thickest) = STREAK_THICKEST;
    let thickness = (STREAK_THICKNESS * fragment.w.min(fragment.h)).clamp(thinnest, thickest);
    let glow = STREAK_HEAD * thickness;
    let reach = (along.x * along.x + along.y * along.y).sqrt();
    let (dx, dy) = if reach > 1e-6 {
        (along.x / reach, along.y / reach)
    } else {
        (0.0, -1.0)
    };
    let logical = Size::<i32, Logical>::from((
        (dx.abs() * length + 2.0 * glow).ceil() as i32,
        (dy.abs() * length + 2.0 * glow).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * STREAK_PIXELS).ceil().max(1.0) as i32,
        (logical.h as f64 * STREAK_PIXELS).ceil().max(1.0) as i32,
    );
    let mut pixmap = Pixmap::new(device.0 as u32, device.1 as u32)?;
    let (cx, cy) = (logical.w as f64 / 2.0, logical.h as f64 / 2.0);
    let tail = (cx - dx * length / 2.0, cy - dy * length / 2.0);
    let head = (cx + dx * length / 2.0, cy + dy * length / 2.0);
    let body = lit(average);
    let head_rgb: [f32; 3] = std::array::from_fn(|i| ring_rgb[i] + (1.0 - ring_rgb[i]) * 0.4);
    let width = device.0 as usize;
    let data = pixmap.data_mut();
    for py in 0..device.1 as usize {
        for px in 0..width {
            let x = (px as f64 + 0.5) / STREAK_PIXELS;
            let y = (py as f64 + 0.5) / STREAK_PIXELS;
            let (rx, ry) = (x - tail.0, y - tail.1);
            // How far along from tail to head, and how far off the line.
            let u = (rx * dx + ry * dy) / length;
            let off = (ry * dx - rx * dy).abs();
            let mut line = 0.0;
            if (0.0..=1.0).contains(&u) {
                let wide = thickness * (0.3 + 0.7 * u);
                let core = (-(off / wide).powi(2)).exp();
                let halo = 0.35 * (-(off / (3.0 * wide)).powi(2)).exp();
                line = (core + halo) * u.powf(1.2);
            }
            let (hx, hy) = (x - head.0, y - head.1);
            let shine = 0.8 * (-((hx * hx + hy * hy) / (glow * glow))).exp();
            let light = (line + shine) as f32;
            if light < 1.0 / 255.0 {
                continue;
            }
            let lift = (u.clamp(0.0, 1.0) as f32).powi(2);
            let rgb: [f32; 3] = std::array::from_fn(|i| {
                let colour = body[i] + (ring_rgb[i] - body[i]) * lift;
                (colour * line as f32 + head_rgb[i] * shine as f32) / light
            });
            let alpha = light.min(1.0);
            let at = (py * width + px) * 4;
            for (i, channel) in rgb.into_iter().enumerate() {
                data[at + i] = (channel.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
            }
            data[at + 3] = (alpha * 255.0).round() as u8;
        }
    }
    Some((pixmap, logical))
}

/// A fragment's colour brought up to a light's brightness: its brightest channel at least 0.7,
/// keeping its hue, and a light grey for one with no colour to speak of.
fn lit(rgb: [f32; 3]) -> [f32; 3] {
    let brightest = rgb.into_iter().fold(0.0f32, f32::max);
    if brightest < 0.02 {
        [0.7; 3]
    } else if brightest < 0.7 {
        rgb.map(|c| c * 0.7 / brightest)
    } else {
        rgb
    }
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
        let streaks = |piece: &Piece| matches!(piece.turn, Some(Turn::Streak(_)));
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

    /// Bright at the head, nothing at the tail, and lifting from the fragment's own colour into
    /// the ring's towards the front.
    #[test]
    fn a_streak_has_a_bright_head_and_thins_to_nothing_behind() {
        let fragment = Size::from((200.0, 100.0));
        let ring = [0.0, 1.0, 0.0];
        let (pixmap, logical) =
            streak_light(fragment, point(5.0, 0.0), [0.6, 0.1, 0.1], ring).unwrap();
        // Lying flat: as long as it is, plus its glow either end.
        assert_eq!(
            logical.w,
            (STREAK_LENGTH * 200.0 + 2.0 * STREAK_HEAD * STREAK_THICKNESS * 100.0).ceil() as i32
        );
        let row = pixmap.height() / 2;
        let at = |x: u32| pixmap.pixel(x, row).unwrap();
        let w = pixmap.width();
        let (tail, middle, head) = (at(w / 16), at(w / 2), at(w * 7 / 8));
        assert!(tail.alpha() < 20, "tail {}", tail.alpha());
        assert!(middle.alpha() < head.alpha());
        assert!(head.alpha() > 200);
        assert!(middle.red() > middle.green(), "the fragment's red behind");
        assert!(head.green() > head.red(), "the ring's green at the head");
        // Turned to run straight up, it fits a sprite on its end.
        let (_, upright) = streak_light(fragment, point(0.0, -5.0), [0.5; 3], ring).unwrap();
        assert_eq!((upright.w, upright.h), (logical.h, logical.w));
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
