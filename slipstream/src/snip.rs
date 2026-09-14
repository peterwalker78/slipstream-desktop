//! Super+Shift+S: a snip of part of the screen.
//!
//! The focused screen freezes on its next frame and dims. Drag out a region, press the letter on a
//! window (or click it) for that window, or press Enter for the whole screen; Esc, or a right
//! click, snips nothing. Nothing moves underneath while it's up, so what is chosen is what was on
//! the screen when the keys were pressed.
//!
//! The snip is saved and copied as Print's screenshots are. On screen, the chosen part breaks
//! into a grid of fragments that scatter, turn into rain glyphs and fly into the toast saying
//! where it went. The snip is over the moment it's chosen; the fragments only paint afterwards.

use std::sync::Arc;

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
    render::{self, OutputElement},
    text::{self, Face, Style},
};

/// How far the pointer has to move before a press becomes a drag rather than a click.
const DRAG_MIN: f64 = 6.0;
/// The shade over everything not chosen.
const DIM: f32 = 0.5;
/// The fragments' flight, before each one's own delay.
const FLIGHT: f64 = 0.42;
/// With reduced motion the chosen part just fades.
const REDUCED_FLIGHT: f64 = 0.08;
/// Logical pixels per mockup pixel, which the fragment grid is sized in.
const MOCKUP_PX: f64 = 0.8;

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
/// fragment per 170 mockup pixels, three to six across and three to five down.
pub fn grid(w: f64, h: f64) -> (usize, usize) {
    let cols = ((w / MOCKUP_PX / 170.0).round() as usize).clamp(3, 6);
    let rows = ((h / MOCKUP_PX / 170.0).round() as usize).clamp(3, 5);
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
        if self.snip.is_some() {
            self.snip = None;
            return;
        }
        let Some(output) = self.screens.focused_output() else {
            return;
        };
        self.close_panels();
        tracing::info!(screen = output.name(), "snip asked for");
        self.snip = Some(Snip {
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
                self.snip = None;
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
        if let Some(snip) = self.snip.as_mut() {
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

    /// The windows on `screen` as drawn, topmost first, clipped to it, each given a letter.
    fn snip_targets(&self, screen: Rectangle<i32, Logical>) -> Vec<Target> {
        let screen = screen.to_f64();
        self.space
            .elements()
            .rev()
            .filter_map(|window| {
                let drawn = self
                    .fitted
                    .iter()
                    .find(|(fitted, _)| fitted == window)
                    .map(|(_, drawn)| *drawn)
                    .or_else(|| self.space.element_geometry(window).map(|r| r.to_f64()))?;
                let visible = drawn.intersection(screen)?;
                let rect = Rectangle::new(visible.loc - screen.loc, visible.size);
                (rect.size.w >= 8.0 && rect.size.h >= 8.0).then(|| (window.clone(), rect))
            })
            .zip(ALPHABET.chars())
            .map(|((window, rect), letter)| Target {
                label: letter.to_string(),
                name: self.window_name(&window),
                rect,
            })
            .collect()
    }

    pub fn snip_key(&mut self, key: Keysym) {
        let act = match self.snip.as_ref().and_then(|snip| snip.frozen.as_ref()) {
            Some(frozen) => frozen.key(key),
            // Still waiting for the frame: only Esc does anything.
            None if key == Keysym::Escape => Act::Cancel,
            None => Act::Nothing,
        };
        self.act_on_snip(act);
    }

    /// A pointer button while snipping: true when the snip took it.
    pub fn snip_button(&mut self, button: u32, pressed: bool) -> bool {
        let Some(snip) = self.snip.as_ref() else {
            return false;
        };
        let origin = self
            .space
            .outputs()
            .find(|output| output.name() == snip.output)
            .and_then(|output| self.space.output_geometry(output))
            .map_or_else(Point::default, |geo| geo.loc.to_f64());
        let at = self.pointer_location() - origin;
        let act = match self.snip.as_mut().and_then(|snip| snip.frozen.as_mut()) {
            Some(frozen) => frozen.button(button == 0x110, pressed, at),
            None => Act::Nothing,
        };
        self.act_on_snip(act);
        true
    }

    pub fn snip_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        let Some(snip) = self.snip.as_ref() else {
            return;
        };
        let origin = self
            .space
            .outputs()
            .find(|output| output.name() == snip.output)
            .and_then(|output| self.space.output_geometry(output))
            .map_or_else(Point::default, |geo| geo.loc.to_f64());
        if let Some(frozen) = self.snip.as_mut().and_then(|snip| snip.frozen.as_mut()) {
            frozen.moved(pos - origin);
        }
    }

    fn act_on_snip(&mut self, act: Act) {
        match act {
            Act::Nothing => {}
            Act::Cancel => {
                tracing::info!("snip cancelled");
                self.snip = None;
            }
            Act::Take(rect) => {
                let Some(Snip {
                    output,
                    frozen: Some(frozen),
                }) = self.snip.take()
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
                    crate::toast::TOP_LOGICAL + 24.0,
                ));
                let flight = Flight::new(
                    output,
                    &frozen,
                    rect,
                    target,
                    now,
                    self.clock.reduced_motion,
                    self.ring_rgb,
                );
                self.snip_flights.push(flight);
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
                h - 28.0 * MOCKUP_PX - legend.logical.h as f64,
            ));
            elements.extend(legend.element(renderer, at, 1.0).map(OutputElement::Memory));
        }

        let chosen = frozen.chosen();
        let dragging = frozen.is_dragging();
        // Letters on the windows, unless a region is being dragged out.
        if !dragging {
            for target in &frozen.windows {
                let centre = Point::from((
                    target.rect.loc.x + target.rect.size.w / 2.0,
                    target.rect.loc.y + target.rect.size.h / 2.0,
                ));
                elements.extend(
                    overview
                        .hint(
                            renderer,
                            &target.label,
                            Some(&target.name),
                            centre,
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
        self.snip_flights.retain(|flight| !flight.done(now));
        let name = output.name();
        self.snip_flights
            .iter()
            .filter(|flight| flight.output == name)
            .flat_map(|flight| flight.elements(renderer, now))
            .collect()
    }
}

/// One fragment: its own picture and glyph tile, where it started, where it scatters to, and when
/// it sets off.
struct Piece {
    picture: Painted,
    tile: Option<Painted>,
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
    fn new(
        output: String,
        frozen: &Frozen,
        rect: Rectangle<f64, Logical>,
        target: Point<f64, Logical>,
        now: f64,
        reduced: bool,
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
        let mut glyphs = if reduced { None } else { Glyphs::load() };
        let mut pieces = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let from = Rectangle::new(
                    (rect.loc.x + col as f64 * cw, rect.loc.y + row as f64 * ch).into(),
                    (cw, ch).into(),
                );
                let Some(picture) = crop_painted(frozen, from) else {
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
                pieces.push(Piece {
                    picture,
                    tile,
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
            if let Some(tile) = piece.tile.as_ref() {
                elements.extend(
                    sized(
                        renderer,
                        tile,
                        place.rect.loc,
                        size,
                        place.glyph * place.alpha,
                    )
                    .map(OutputElement::Memory),
                );
            }
            elements.extend(
                sized(renderer, &piece.picture, place.rect.loc, size, place.alpha)
                    .map(OutputElement::Memory),
            );
        }
        elements
    }
}

/// Where a fragment is at eased progress `p`, as the mockup's keyframes have it: scattered by
/// a fraction of a cell and shrunk to .78 at .38, then shrunk to .12 at the target and gone.
struct Place {
    rect: Rectangle<f64, Logical>,
    alpha: f32,
    /// How far the glyph tile over it has faded in.
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

/// The frozen picture inside `rect` (screen-local, logical) as a surface of its own.
fn crop_painted(frozen: &Frozen, rect: Rectangle<f64, Logical>) -> Option<Painted> {
    let px = crate::screenshot::to_pixels(rect, frozen.scale);
    let (pixels, w, h) = crate::screenshot::cropped(&frozen.pixels, frozen.size, px)?;
    Some(Painted {
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
        0x0b0d12ff,
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
    })
}

fn paint_legend(scale: f64) -> Option<Painted> {
    let style = Style::new(Face::Mono, 13.0, 0x8f98a8ff);
    let legend = "drag a region · a–z a window · ⏎ whole screen · Esc cancel";
    let w = (text::width(legend, &style) + 32.0) * MOCKUP_PX as f32;
    let h = (13.0 * 1.2 + 16.0) * MOCKUP_PX as f32;
    let logical = Size::from((w.ceil() as i32, h.ceil() as i32));
    Painted::new(logical, scale, |p| {
        let f = MOCKUP_PX as f32;
        p.fill(0.0, 0.0, w, h, 10.0 * f, 0x0a0c11e6);
        let style = Style::new(Face::Mono, 13.0 * f, 0x8f98a8ff);
        p.text(legend, 16.0 * f, h / 2.0, &style);
    })
}

fn paint_readout(label: &str, scale: f64) -> Option<Painted> {
    let style = Style::new(Face::Mono, 12.0, 0xe8edf5ff);
    let w = text::width(label, &style) + 16.0;
    let h = 22.0;
    let logical = Size::from((w.ceil() as i32, h as i32));
    Painted::new(logical, scale, |p| {
        p.fill(0.0, 0.0, w, h, 6.0, 0x0a0c11e6);
        p.text(label, 8.0, h / 2.0, &style);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
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

    #[test]
    fn a_fragment_ends_small_and_gone_at_the_target() {
        let piece = Piece {
            picture: Painted {
                buffer: MemoryRenderBuffer::default(),
                logical: Size::from((100, 100)),
                device: (100, 100),
                scale: 1.0,
            },
            tile: None,
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
