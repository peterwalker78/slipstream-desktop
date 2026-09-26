//! The code rain on the right of the screen, where minimised windows go. Each window becomes a
//! stream of GLMatrix rain (`glmatrix.rs`) under a header with its icon and a meter. The
//! tiling area gives up the width.
//!
//! **A stream is about its app, top to bottom.** How hard that app is working is one figure, and
//! the whole column says it: the meter's length in the header, and the colour and the speed of
//! its own rain. A column headed by an app's icon can't show a number the whole machine
//! shares — a file manager doing nothing under fast green rain is a lie about the file manager,
//! whatever the rain is actually measuring.
//!
//! So every stream paints its own band at its own speed. A quiet app's steps a few times a
//! second where a busy one's steps thirty, which means what a stream costs to draw is what its
//! app is doing — six idle streams and six flat out measure the same as the one shared band they
//! replaced.

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    desktop::Window,
    utils::{Logical, Physical, Point, Rectangle, Size},
};

use crate::{
    glmatrix::{self, Band, Glyphs, Look},
    layout::Rect,
    paint::{self, Painter},
    text::{self, Face, Style},
    usage::Meter,
};

// Sizes in the mockup's pixels. Streams are two-thirds of the mockup's 68 wide, and their
// headers shrink to match.
const STREAM: f32 = 45.0;
const GLOW: f32 = 16.0;
/// How far below the bar a stream's header card starts, in the mockup's pixels. The card its rain
/// falls down stops the same distance above the foot of the screen.
const HEAD_GAP: f32 = 12.0;
/// The header card's width and corner radius, in the mockup's pixels before `HEADER`.
const BUTTON_W: f32 = 58.0;
const RADIUS: f32 = 12.0;
/// The gap between an app's button and the card under it, in logical pixels.
const CARD_GAP: f64 = 2.0;
/// How far inside the card's edge the rain stops, in logical pixels.
const CARD_INSET: f64 = 2.0;
/// The cards' colour, the same as the headers'.
const CARD: u32 = 0x0b0d12ff;
/// Logical pixels per mockup pixel.
const MOCKUP_PX: f32 = 0.8;
/// The header's size against the mockup's.
const HEADER: f32 = 0.75;
/// The app's own meter at the foot of its header, in the mockup's pixels: as wide as the icon,
/// and a hairline tall — it is read as how full it is, never as a number.
const METER_H: f32 = 5.0;
/// How finely the meter is quantised. A header is a painted buffer, so it is repainted only when
/// the meter moves by a step of this; at 40 mockup pixels wide a step is about a pixel.
const METER_STEPS: f32 = 40.0;

/// The size, in buffer pixels, to load a stream header's icon at.
pub fn icon_px(scale: f64) -> u32 {
    (40.0 * MOCKUP_PX as f64 * HEADER as f64 * scale).round() as u32
}
/// A stream fades in over the time its window takes to pour into it.
const APPEAR: f64 = 0.34;
const REDUCED_FADE: f64 = 0.08;

struct Painted {
    buffer: MemoryRenderBuffer,
    logical: Size<i32, Logical>,
    device: (i32, i32),
    scale: f64,
}

impl Painted {
    /// With its corner on screen pixel `at`.
    fn element_px<R>(
        &self,
        renderer: &mut R,
        at: Point<i32, Physical>,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_f64(),
            &self.buffer,
            Some(alpha),
            Some(Rectangle::from_size(
                (self.device.0 as f64, self.device.1 as f64).into(),
            )),
            Some(self.logical),
            Kind::Unspecified,
        )
        .ok()
    }
}

/// A stream's dark card with nothing on it yet, and the shape inside its edge that the rain may
/// light: the card's own rounded rectangle, `CARD_INSET` smaller all round.
struct Card {
    base: Pixmap,
    inset: usize,
    /// Width and height of the shape inside the edge.
    inner: (usize, usize),
    /// How much of each of its pixels is inside it, one byte a pixel.
    cover: Vec<u8>,
}

impl Card {
    fn paint(device: (i32, i32), scale: f64) -> Option<Self> {
        let (w, h) = (device.0.max(1) as u32, device.1.max(1) as u32);
        let radius = RADIUS * MOCKUP_PX * HEADER * scale as f32;
        let mut base = Painter::new(w, h, 1.0)?;
        base.fill(0.0, 0.0, w as f32, h as f32, radius, CARD);
        let inset = (CARD_INSET * scale).floor().max(1.0) as u32;
        let inner = (
            w.saturating_sub(2 * inset).max(1),
            h.saturating_sub(2 * inset).max(1),
        );
        let mut shape = Painter::new(inner.0, inner.1, 1.0)?;
        shape.fill(
            0.0,
            0.0,
            inner.0 as f32,
            inner.1 as f32,
            (radius - inset as f32).max(0.0),
            0xffffffff,
        );
        let cover = shape.pixmap.data().chunks_exact(4).map(|p| p[3]).collect();
        Some(Self {
            base: base.pixmap,
            inset: inset as usize,
            inner: (inner.0 as usize, inner.1 as usize),
            cover,
        })
    }
}

/// Where a stream's card goes, in screen pixels, for a button whose buffer has its corner at
/// `button` and whose card is `button_h` mockup pixels tall: as wide as the button and straight
/// under it, `CARD_GAP` below its lower edge, down to `HEAD_GAP` above the foot of a screen
/// `screen_h` logical pixels tall. The size is in whole logical pixels, so the buffer is shown one
/// to one.
/// The size, in screen pixels at `scale`, that the code rain's glyphs are drawn at: a third of a
/// stream card's inner width across, in GLMatrix's cell proportions. Anything else drawn as rain
/// uses this, so it is the same rain.
pub fn glyph_size(scale: f64) -> (usize, usize) {
    let card = ((BUTTON_W * MOCKUP_PX * HEADER).round() as f64 * scale).round() as i32;
    let inset = (CARD_INSET * scale).floor().max(1.0) as i32;
    crate::glmatrix::glyph_size((card - 2 * inset).max(3) as f32)
}

fn card_frame(
    button: Point<i32, Physical>,
    button_h: f32,
    screen_h: i32,
    scale: f64,
) -> (Point<i32, Physical>, Size<i32, Logical>) {
    let f = scale * (MOCKUP_PX * HEADER) as f64;
    let x = button.x + (GLOW as f64 * f).round() as i32;
    let edge = (button.y as f64 + (GLOW + button_h) as f64 * f).round() as i32;
    let y = edge + (CARD_GAP * scale).floor().max(1.0) as i32;
    let bottom = (screen_h as f64 * scale).round() as i32
        - ((HEAD_GAP * MOCKUP_PX) as f64 * scale).round() as i32;
    let w = (BUTTON_W * MOCKUP_PX * HEADER).round() as i32;
    let h = (((bottom - y) as f64 / scale).floor() as i32).max(1);
    (Point::from((x, y)), Size::from((w, h)))
}

pub struct Stream {
    pub window: Window,
    pub name: String,
    colour: u32,
    /// Drawn at 40 mockup pixels on the screen it was minimised on.
    icon: Option<Pixmap>,
    /// When it was minimised, on the animation clock.
    since: f64,
    header: Option<Painted>,
    /// The header card's height in mockup pixels.
    button_h: f32,
    card: Option<Card>,
    /// How hard this app is working, 0 to 1, and what the painted header and the band's look are
    /// showing, quantised, so neither is remade while the reading holds still.
    meter: Meter,
    painted_load: Option<u16>,
    look_load: Option<u16>,
    /// This app's own rain: its colour and speed are its load.
    band: Option<Band>,
    /// The card with the rain on it.
    painted: Option<Painted>,
    stepped_to: f64,
    /// Keeps two streams of the same app from falling in step.
    seed: u64,
}

pub struct Rain {
    /// Oldest first: stream 0 is the rightmost.
    pub streams: Vec<Stream>,
    glyphs: Option<Glyphs>,
    pub reduced_motion: bool,
    /// A load to show instead of every app's own, for the `demand:` debug step.
    pinned: Option<f32>,
    /// The names on the headers are in the focus ring's colour: the app's own colour can be close
    /// enough to the rain's green to vanish into it, and the ring's is chosen to stand off it.
    name_colour: u32,
    /// The same colour as channels, for the names the rain spells.
    ring_rgb: [f32; 3],
}

impl Rain {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        for stream in &mut self.streams {
            stream.painted_load = None;
        }
    }

    pub fn new(reduced_motion: bool, ring_rgb: [f32; 3]) -> Self {
        Self {
            streams: Vec::new(),
            glyphs: Glyphs::load(),
            reduced_motion,
            pinned: None,
            name_colour: rgb_colour(ring_rgb),
            ring_rgb,
        }
    }

    /// Writes the names in `ring_rgb` from the next frame, when the ring's colour changes.
    pub fn set_ring_colour(&mut self, ring_rgb: [f32; 3]) {
        let colour = rgb_colour(ring_rgb);
        self.ring_rgb = ring_rgb;
        if colour != self.name_colour {
            self.name_colour = colour;
            self.forget_painted_text();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Shows `value` as every app's load instead of reading it, so both ends of a stream — the
    /// bar, the colour and the speed — can be looked at without finding an app working that
    /// hard. `None` gives the apps their own readings back.
    pub fn pin_demand(&mut self, value: Option<f32>) {
        self.pinned = value;
        for stream in &mut self.streams {
            stream.look_load = None;
        }
    }

    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// Logical pixels the tiling area gives up on the right: 20 + 68 per stream, in the
    /// mockup's pixels.
    pub fn reserve(&self) -> i32 {
        if self.streams.is_empty() {
            0
        } else {
            ((20.0 + self.streams.len() as f32 * STREAM) * MOCKUP_PX).round() as i32
        }
    }

    pub fn contains(&self, window: &Window) -> bool {
        self.index_of(window).is_some()
    }

    /// Which stream is `window`'s, which is also which column it pours into.
    pub fn index_of(&self, window: &Window) -> Option<usize> {
        self.streams
            .iter()
            .position(|stream| stream.window == *window)
    }

    /// Where stream `index` sits on `screen`, below `top`: the column its window pours into.
    pub fn column(index: usize, screen: Rect, top: i32) -> Rect {
        let centre = (screen.x + screen.w) as f32
            - (10.0 + STREAM / 2.0 + index as f32 * STREAM) * MOCKUP_PX;
        let half = BUTTON_W * MOCKUP_PX * HEADER / 2.0;
        Rect {
            x: (centre - half).round() as i32,
            y: screen.y + top,
            w: (2.0 * half).round() as i32,
            h: (screen.h - top).max(1),
        }
    }

    /// The stream under (`x`, `y`) in the space's coordinates, where `screen` is the screen the
    /// rain is drawn on. A point on any other screen is no stream, even one lined up with a column.
    pub fn stream_hit(&self, x: f64, y: f64, screen: Rect, top: i32) -> Option<usize> {
        stream_hit(self.streams.len(), x, y, screen, top)
    }

    /// The stream under a point on `screen`.
    pub fn stream_at(&self, x: f64, y: f64, screen: Rect, top: i32) -> Option<usize> {
        (0..self.streams.len()).find(|&index| {
            let column = Self::column(index, screen, top);
            (column.x as f64..(column.x + column.w) as f64).contains(&x)
                && (column.y as f64..(column.y + column.h) as f64).contains(&y)
        })
    }

    /// Adds a stream for `window`.
    pub fn add(
        &mut self,
        window: Window,
        name: String,
        icon: Option<Pixmap>,
        pid: Option<u32>,
        now: f64,
    ) {
        let colour = icon
            .as_ref()
            .map_or_else(|| name_colour(&name), icon_colour);
        self.streams.push(Stream {
            window,
            name,
            colour,
            icon,
            since: now,
            header: None,
            button_h: 0.0,
            card: None,
            meter: Meter::new(pid),
            painted_load: None,
            look_load: None,
            band: None,
            painted: None,
            stepped_to: now,
            seed: now.to_bits() ^ ((self.streams.len() as u64 + 1) << 32) | 1,
        });
    }

    /// Takes `window`'s stream out, returning where it was.
    pub fn remove(&mut self, window: &Window) -> Option<usize> {
        let index = self
            .streams
            .iter()
            .position(|stream| stream.window == *window)?;
        self.streams.remove(index);
        Some(index)
    }

    /// Windows minimised in the last moment, still pouring into their streams.
    pub fn arriving(&self, now: f64) -> Vec<Window> {
        self.streams
            .iter()
            .filter(|stream| now - stream.since < APPEAR)
            .map(|stream| stream.window.clone())
            .collect()
    }

    /// The streams and their headers for a screen `size` logical pixels big, below `top`.
    pub fn elements<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        top: i32,
        scale: f64,
        now: f64,
        ui: f32,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let mut elements = Vec::new();
        if self.streams.is_empty() || ui <= 0.0 {
            // Nothing minimised, or the UI has faded into the wallpaper: none of this is on the
            // screen, so none of it is painted. Each band picks up where it left off.
            for stream in &mut self.streams {
                stream.stepped_to = now;
            }
            return elements;
        }
        let screen = Rect {
            x: 0,
            y: 0,
            w: size.w,
            h: size.h,
        };
        let fade = if self.reduced_motion {
            REDUCED_FADE
        } else {
            APPEAR
        };
        let Rain {
            streams,
            glyphs,
            pinned,
            reduced_motion,
            name_colour,
            ring_rgb,
        } = self;
        let Some(glyphs) = glyphs.as_mut() else {
            return elements;
        };

        for (index, stream) in streams.iter_mut().enumerate() {
            // What this app is doing, sampled at most once a second by its own meter. Everything
            // about the stream says the same thing: the bar's length, the rain's colour and how
            // fast it falls.
            stream.meter.refresh();
            let load = pinned.unwrap_or(stream.meter.load).clamp(0.0, 1.0);
            let step = (load * METER_STEPS).round() as u16;
            if stream
                .header
                .as_ref()
                .is_none_or(|header| header.scale != scale)
                || stream.painted_load != Some(step)
            {
                let painted = paint_header(
                    &stream.name,
                    stream.colour,
                    *name_colour,
                    stream.icon.as_ref(),
                    step as f32 / METER_STEPS,
                    scale,
                );
                stream.button_h = painted.as_ref().map_or(0.0, |(_, h)| *h);
                stream.header = painted.map(|(header, _)| header);
                stream.painted_load = Some(step);
            }
            let Some(header) = &stream.header else {
                continue;
            };

            let alpha = ((now - stream.since) / fade).clamp(0.0, 1.0) as f32 * ui;
            let column = Self::column(index, screen, top);
            let centre = column.x as f64 + column.w as f64 / 2.0;
            let button = Point::<f64, Logical>::from((
                centre - header.logical.w as f64 / 2.0,
                top as f64 + (HEAD_GAP * MOCKUP_PX) as f64 - (GLOW * MOCKUP_PX * HEADER) as f64,
            ))
            .to_physical(scale)
            .to_i32_round::<i32>();

            // The card hangs from the button and the rain is drawn on to it, so none of the rain
            // can fall outside it.
            let (card_at, card_size) = card_frame(button, stream.button_h, size.h, scale);
            let device = (
                (card_size.w as f64 * scale).round() as i32,
                (card_size.h as f64 * scale).round() as i32,
            );
            if stream.card.as_ref().is_none_or(|card| {
                (card.base.width(), card.base.height()) != (device.0 as u32, device.1 as u32)
            }) {
                stream.card = Card::paint(device, scale);
            }
            let Some(card) = &stream.card else {
                continue;
            };

            // Its own band, at its own speed and colour. A quiet app's rain steps a few times a
            // second and a busy one's thirty, so what a stream costs to paint is what its app is
            // doing — the same thing the rain is there to say. Now and then the rain spells out
            // the app's name, vowels small and the rest in capitals, lit in the ring's colour like the header's.
            let band = stream.band.get_or_insert_with(|| {
                Band::new(
                    &rain_case(&stream.name),
                    Look::for_demand(load),
                    card.inner.0 as f32,
                    card.inner.1 as f32,
                    stream.seed,
                )
            });
            let recoloured = band.set_name_colour(*ring_rgb);
            let changed = if *reduced_motion {
                // Reduced motion holds the rain still: nothing falls, and the glyphs where they
                // stand are repainted in the load's colour and brightness only when it moves a
                // step.
                let changed = stream.look_load != Some(step) || !band.is_still();
                if changed {
                    band.hold(load);
                    stream.look_load = Some(step);
                }
                changed
            } else {
                let was_still = band.is_still();
                if stream.look_load != Some(step) {
                    // A change in an app's load eases into its rain rather than stepping.
                    band.set_look(Look::for_demand(load));
                    stream.look_load = Some(step);
                }
                band.step(now - stream.stepped_to) || was_still
            };
            stream.stepped_to = now;
            let stale = stream
                .painted
                .as_ref()
                .is_none_or(|painted| painted.scale != scale || painted.device != device);
            if changed || stale || recoloured {
                stream.painted = Some(Painted {
                    buffer: paint::buffer(&compose(band, glyphs, card)),
                    logical: card_size,
                    device,
                    scale,
                });
            }

            // The card in front of the button's glow, which would otherwise wash over the gap
            // between them. Elements are listed front to back.
            if let Some(painted) = &stream.painted {
                elements.extend(painted.element_px(renderer, card_at, alpha));
            }
            elements.extend(header.element_px(renderer, button, alpha));
        }
        elements
    }
}

/// The rain drawn on to its card, one screen pixel to a buffer pixel. Light is added only inside
/// the card's inner shape, softened along its rounded corners, so the rain stops the same
/// distance from the card's edge all the way round.
fn compose(band: &mut Band, glyphs: &mut Glyphs, card: &Card) -> Pixmap {
    let (iw, ih) = card.inner;
    let mut rain = vec![0u8; iw * ih * 4];
    band.resize(iw as f32, ih as f32);
    band.draw(&mut rain, iw, ih, iw as f32 / 2.0, glyphs);
    let mut out = card.base.clone();
    let w = out.width() as usize;
    let data = out.data_mut();
    for (i, (light, &cover)) in rain.chunks_exact(4).zip(&card.cover).enumerate() {
        if cover == 0 || light[..3] == [0, 0, 0] {
            continue;
        }
        let at = ((i / iw + card.inset) * w + i % iw + card.inset) * 4;
        for (channel, &light) in data[at..at + 3].iter_mut().zip(&light[..3]) {
            let added = *channel as u32 + (light as u32 * cover as u32 + 127) / 255;
            *channel = added.min(255) as u8;
        }
    }
    out
}

/// A stream's header: a dark card edged and lit in the app's colour, with its icon and the app's
/// own meter (`load`, 0 to 1) along the foot of it. Every header is the same height: the rain
/// spells the app's name already. An app with no icon gets its initial in `name_colour` instead.
fn paint_header(
    name: &str,
    colour: u32,
    name_colour: u32,
    icon: Option<&Pixmap>,
    load: f32,
    scale: f64,
) -> Option<(Painted, f32)> {
    let (w, h) = (BUTTON_W, 7.0 + 40.0 + 9.0 + METER_H + 9.0);
    let logical = Size::<i32, Logical>::from((
        ((w + 2.0 * GLOW) * MOCKUP_PX * HEADER).ceil() as i32,
        ((h + 2.0 * GLOW) * MOCKUP_PX * HEADER).ceil() as i32,
    ));
    let device = (
        (logical.w as f64 * scale).round() as i32,
        (logical.h as f64 * scale).round() as i32,
    );
    let f = scale as f32 * MOCKUP_PX * HEADER;
    let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;
    let (x, y) = (GLOW, GLOW);
    let [r, g, b, _] = colour.to_be_bytes();
    p.shadow(
        x,
        y,
        w,
        h,
        12.0,
        0.0,
        22.0,
        u32::from_be_bytes([r, g, b, 0x47]),
    );
    // Opaque: the rain runs down the card behind it, and a header you can see it through
    // reads as a hole in the stream rather than the button it is.
    p.fill(x, y, w, h, RADIUS, CARD);
    p.border(x, y, w, h, RADIUS, 1.0, colour);
    if let Some(icon) = icon {
        p.image(icon, x + 9.0, y + 7.0);
    } else if let Some(initial) = name.chars().find(|c| c.is_alphanumeric()) {
        let style = Style::new(Face::MonoBold, 26.0, name_colour);
        let letter: String = initial.to_uppercase().collect();
        let letter_w = text::width(&letter, &style);
        p.text(&letter, x + (w - letter_w) / 2.0, y + 7.0 + 20.0, &style);
    }
    // The app's own meter along the foot: a dark groove the width of the icon, filled from the
    // left in the same grey-to-green the rain runs through, so a quiet app under a busy machine
    // reads as exactly that.
    let (meter_x, meter_y) = (x + 9.0, y + h - 9.0 - METER_H);
    let meter_w = 40.0;
    p.fill(
        meter_x,
        meter_y,
        meter_w,
        METER_H,
        METER_H / 2.0,
        0x20262eff,
    );
    let load = load.clamp(0.0, 1.0);
    if load > 0.0 {
        // Never narrower than it is tall: a sliver of colour still has to read as a rounded end
        // rather than as a speck of dust on the screen.
        let filled = (meter_w * load).max(METER_H);
        p.fill(
            meter_x,
            meter_y,
            filled,
            METER_H,
            METER_H / 2.0,
            meter_colour(load),
        );
    }
    Some((
        Painted {
            buffer: paint::buffer(&p.pixmap),
            logical,
            device,
            scale,
        },
        h,
    ))
}

/// The meter's colour at `load`: the rain's own scale, from the light grey of an idle machine to
/// its bright green, so the bar and the glyphs beside it mean the same thing.
fn meter_colour(load: f32) -> u32 {
    rgb_colour(glmatrix::colour_for(load))
}

/// Three channels from 0 to 1 as opaque `0xrrggbbff`.
fn rgb_colour(rgb: [f32; 3]) -> u32 {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    u32::from_be_bytes([r, g, b, 0xff])
}

/// An app's colour from its icon: the average of its visible pixels, brightened.
fn icon_colour(icon: &Pixmap) -> u32 {
    let (mut sum, mut weight) = ([0f64; 3], 0f64);
    for pixel in icon.data().chunks_exact(4) {
        let alpha = pixel[3] as f64 / 255.0;
        if alpha > 0.5 {
            for c in 0..3 {
                // Premultiplied, so divide the alpha back out.
                sum[c] += pixel[c] as f64 / alpha;
            }
            weight += 1.0;
        }
    }
    if weight == 0.0 {
        return 0x3cf0c0ff;
    }
    let mean = sum.map(|channel| channel / weight);
    let brightest = mean.iter().cloned().fold(1.0, f64::max);
    let [r, g, b] = mean.map(|channel| (channel / brightest * 255.0).round() as u8);
    if r.max(g).max(b) - r.min(g).min(b) < 40 {
        // Grey icons get Slipstream's mint rather than a muddy grey.
        return 0x3cf0c0ff;
    }
    u32::from_be_bytes([r, g, b, 0xff])
}

fn name_colour(name: &str) -> u32 {
    const COLOURS: [u32; 5] = [0x3cf0c0ff, 0x33ccffff, 0xffb547ff, 0xff7a93ff, 0xa78bfaff];
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u32)
    });
    COLOURS[hash as usize % COLOURS.len()]
}

/// `name` as the rain spells it: vowels in lower case and everything else in capitals, so
/// "Visual Studio Code" is "ViSuaL STuDio CoDe".
fn rain_case(name: &str) -> String {
    let mut spelled = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u') {
            spelled.extend(c.to_lowercase());
        } else {
            spelled.extend(c.to_uppercase());
        }
    }
    spelled
}

/// Which of `count` streams is under (`x`, `y`), in the space's coordinates, with the rain drawn
/// on `screen`. Nothing on another screen is a stream.
pub fn stream_hit(count: usize, x: f64, y: f64, screen: Rect, top: i32) -> Option<usize> {
    let on_screen = (screen.x as f64..(screen.x + screen.w) as f64).contains(&x)
        && (screen.y as f64..(screen.y + screen.h) as f64).contains(&y);
    if !on_screen {
        return None;
    }
    (0..count).find(|&index| {
        let column = Rain::column(index, screen, top);
        (column.x as f64..(column.x + column.w) as f64).contains(&x)
            && (column.y as f64..(column.y + column.h) as f64).contains(&y)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_rain_s_glyph_size_is_a_third_of_a_stream_s_inner_width() {
        assert_eq!(glyph_size(1.0), (10, 15));
        assert_eq!(glyph_size(1.25), crate::glmatrix::glyph_size(40.0));
    }

    use super::*;

    #[test]
    fn the_rain_spells_vowels_small_and_the_rest_in_capitals() {
        assert_eq!(rain_case("konsole"), "KoNSoLe");
        assert_eq!(rain_case("Visual Studio Code"), "ViSuaL STuDio CoDe");
        assert_eq!(rain_case("KeePassXC"), "KeePaSSXC");
        assert_eq!(rain_case("kde-connect 2"), "KDe-CoNNeCT 2");
        assert_eq!(rain_case(""), "");
    }

    #[test]
    fn streams_stack_leftwards_from_the_right_edge() {
        let screen = Rect {
            x: 0,
            y: 0,
            w: 1536,
            h: 960,
        };
        let first = Rain::column(0, screen, 32);
        let second = Rain::column(1, screen, 32);
        assert!(first.x + first.w <= 1536 - 8);
        assert!(
            second.x + second.w <= first.x,
            "the next stream goes to its left"
        );
        assert_eq!((first.y, first.h), (32, 928));
    }

    #[test]
    fn the_card_hangs_two_pixels_under_its_button_and_fits_the_screen() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let button = Point::from((1000, 40));
            let button_h = 131.0;
            let (at, size) = card_frame(button, button_h, 768, scale);
            let edge = button.y as f64 + (GLOW + button_h) as f64 * scale * 0.6;
            let gap = at.y as f64 - edge;
            assert!(
                ((2.0 * scale).floor() - 0.5..=(2.0 * scale).floor() + 0.5).contains(&gap),
                "a {gap} px gap at {scale}×"
            );
            let bottom = at.y + (size.h as f64 * scale).round() as i32;
            let margin = (768.0 * scale).round() as i32 - bottom;
            let wanted = (9.6 * scale).round() as i32;
            assert!(
                (wanted..=wanted + scale.ceil() as i32 + 1).contains(&margin),
                "{margin} px above the foot at {scale}×, wanted {wanted}"
            );
        }
    }

    #[test]
    fn the_rain_stays_inside_its_card() {
        let mut glyphs = Glyphs::load().expect("the font loads");
        for scale in [1.0, 1.25, 2.0] {
            let device = (
                (35.0 * scale as f64).round() as i32,
                (600.0 * scale as f64).round() as i32,
            );
            let card = Card::paint(device, scale).expect("the card paints");
            let mut band = Band::new(
                "SLIPSTREAM",
                Look::for_demand(1.0),
                card.inner.0 as f32,
                card.inner.1 as f32,
                3,
            );
            // Warmed and stepped hard, so the band is full of lit glyphs everywhere.
            band.step(20.0);
            let out = compose(&mut band, &mut glyphs, &card);
            let (w, h) = (device.0 as usize, device.1 as usize);
            let inset = card.inset;
            assert_eq!(inset, (2.0 * scale).floor() as usize);
            let mut lit = 0;
            for y in 0..h {
                for x in 0..w {
                    let at = (y * w + x) * 4;
                    let (painted, base) = (&out.data()[at..at + 4], &card.base.data()[at..at + 4]);
                    let inside = (inset..w - inset).contains(&x) && (inset..h - inset).contains(&y);
                    if painted != base {
                        assert!(inside, "rain at ({x}, {y}) of {w}×{h}, inset {inset}");
                        let cover = card.cover[(y - inset) * card.inner.0 + x - inset];
                        assert!(cover > 0, "rain outside the rounded corner at ({x}, {y})");
                        lit += 1;
                    }
                }
            }
            assert!(lit > 1000, "rain is falling on the card: {lit} lit pixels");
        }
    }

    #[test]
    fn icon_colours_ignore_grey_and_keep_hue() {
        let mut orange = Pixmap::new(4, 4).unwrap();
        orange.fill(resvg::tiny_skia::Color::from_rgba8(240, 120, 20, 255));
        let [r, g, b, _] = icon_colour(&orange).to_be_bytes();
        assert!(r == 255 && g < 160 && b < 60);
        let mut grey = Pixmap::new(4, 4).unwrap();
        grey.fill(resvg::tiny_skia::Color::from_rgba8(120, 120, 120, 255));
        assert_eq!(icon_colour(&grey), 0x3cf0c0ff);
    }

    #[test]
    fn only_the_screen_the_rain_is_on_has_streams() {
        let left = Rect {
            x: 0,
            y: 0,
            w: 1536,
            h: 960,
        };
        let right = Rect { x: 1536, ..left };
        let column = Rain::column(0, left, 40);
        let (cx, cy) = (
            (column.x + column.w / 2) as f64,
            (column.y + column.h / 2) as f64,
        );
        assert_eq!(stream_hit(1, cx, cy, left, 40), Some(0));
        // The same spot on the screen to its right: a scrollbar, not a stream.
        let mirrored = cx + right.x as f64;
        assert_eq!(stream_hit(1, mirrored, cy, left, 40), None);
        assert_eq!(stream_hit(0, cx, cy, left, 40), None, "no streams at all");
    }
}
