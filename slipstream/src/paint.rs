//! Painting Slipstream's own surfaces (the bar, the explorer, the key hint) on the CPU with
//! tiny-skia, laid out in whatever units the caller likes and drawn at the screen's own
//! resolution, so text stays sharp at fractional scales.

use std::path::Path;

use resvg::{
    tiny_skia::{
        self, FillRule, FilterQuality, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke,
        Transform,
    },
    usvg,
};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Logical, Point, Rectangle, Size, Transform as OutputTransform},
};

use crate::{
    icons,
    text::{self, Style},
};

/// A buffer the renderer can show, one pixel per screen pixel.
pub fn buffer(pixmap: &Pixmap) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        pixmap.data(),
        Fourcc::Abgr8888,
        (pixmap.width() as i32, pixmap.height() as i32),
        1,
        OutputTransform::Normal,
        None,
    )
}

fn paint_of(rgba: u32) -> Paint<'static> {
    let [r, g, b, a] = rgba.to_be_bytes();
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(
            x, y, w, h,
        )?));
    }
    // Cubic control points this far in from each corner approximate a quarter circle.
    let k = r * (1.0 - 0.552_284_8);
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - k, y, x + w, y + k, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(x + w, y + h - k, x + w - k, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.cubic_to(x + k, y + h, x, y + h - k, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + k, x + k, y, x + r, y);
    path.close();
    path.finish()
}

/// A drop shadow, and the glass card casting it if there is one, in layout units.
struct Layer {
    w: f32,
    h: f32,
    radius: f32,
    dy: f32,
    blur: f32,
    shadow: u32,
    /// The card's fill and the edge inside it.
    glass: Option<(u32, u32)>,
}

/// A layer's size, shape, scale and sub-pixel offset as exact bits, and its colours.
#[derive(PartialEq)]
struct LayerKey {
    bits: [u32; 8],
    colours: [u32; 3],
    glass: bool,
}

/// Painted layers are kept up to this many bytes. A panel's is a few megabytes at 1.25×, and
/// panels that grow with their content need one for each height they've been.
const LAYER_CACHE_BYTES: usize = 48 << 20;

thread_local! {
    static LAYERS: std::cell::RefCell<Vec<(LayerKey, Pixmap)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Copies `src` onto `dst` with its corner at (`x`, `y`), if everything it lands on is still
/// transparent: drawing over nothing gives the source itself, and a copy is many times faster than
/// blending. Returns false, having changed nothing, if it isn't.
fn copy_onto_empty(dst: &mut Pixmap, src: &Pixmap, x: i32, y: i32) -> bool {
    let (dst_w, dst_h) = (dst.width() as i32, dst.height() as i32);
    let (src_w, src_h) = (src.width() as i32, src.height() as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + src_w).min(dst_w), (y + src_h).min(dst_h));
    if x0 >= x1 || y0 >= y1 {
        return true;
    }
    let span = |row: i32, x0: i32, x1: i32, width: i32| {
        (row * width * 4 + x0 * 4) as usize..(row * width * 4 + x1 * 4) as usize
    };
    let data = dst.data_mut();
    if (y0..y1).any(|row| data[span(row, x0, x1, dst_w)].iter().any(|&byte| byte != 0)) {
        return false;
    }
    let from = src.data();
    for row in y0..y1 {
        data[span(row, x0, x1, dst_w)].copy_from_slice(&from[span(row - y, x0 - x, x1 - x, src_w)]);
    }
    true
}

/// Paints in layout units onto a pixmap with `f` device pixels to each. Colours are written as
/// in CSS: 0xRRGGBBAA.
pub struct Painter {
    pub pixmap: Pixmap,
    pub f: f32,
}

impl Painter {
    pub fn new(width: u32, height: u32, f: f32) -> Option<Self> {
        Some(Self {
            pixmap: Pixmap::new(width, height)?,
            f,
        })
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.f, self.f)
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, rgba: u32) {
        if let Some(path) = rounded(x, y, w, h, radius) {
            let transform = self.transform();
            self.pixmap
                .fill_path(&path, &paint_of(rgba), FillRule::Winding, transform, None);
        }
    }

    /// A border `width` thick inside the box, as CSS draws one.
    #[allow(clippy::too_many_arguments)]
    pub fn border(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32, rgba: u32) {
        let half = width / 2.0;
        if let Some(path) = rounded(x + half, y + half, w - width, h - width, radius - half) {
            let transform = self.transform();
            let stroke = Stroke {
                width,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint_of(rgba), &stroke, transform, None);
        }
    }

    /// CSS `box-shadow: inset 0 -2px 0`: a band along the bottom, clipped to the box's corners.
    pub fn inset_bottom(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, rgba: u32) {
        let (Some(shape), Some(band), Some(mut mask)) = (
            rounded(x, y, w, h, radius),
            tiny_skia::Rect::from_xywh(x, y + h - 2.0, w, 2.0),
            Mask::new(self.pixmap.width(), self.pixmap.height()),
        ) else {
            return;
        };
        let transform = self.transform();
        mask.fill_path(&shape, FillRule::Winding, true, transform);
        self.pixmap
            .fill_rect(band, &paint_of(rgba), transform, Some(&mask));
    }

    /// A soft drop shadow, like CSS `box-shadow: 0 dy blur`, built from stacked translucent
    /// rounded rectangles, since tiny-skia can't blur.
    #[allow(clippy::too_many_arguments)]
    pub fn shadow(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        dy: f32,
        blur: f32,
        rgba: u32,
    ) {
        self.layer(
            Layer {
                w,
                h,
                radius,
                dy,
                blur,
                shadow: rgba,
                glass: None,
            },
            x,
            y,
        );
    }

    /// A glass card with its shadow: the shadow, a fill with a faint edge inside it, and a dark
    /// line just outside.
    #[allow(clippy::too_many_arguments)]
    pub fn card(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        (dy, blur, shadow): (f32, f32, u32),
        fill: u32,
        edge: u32,
    ) {
        self.layer(
            Layer {
                w,
                h,
                radius,
                dy,
                blur,
                shadow,
                glass: Some((fill, edge)),
            },
            x,
            y,
        );
    }

    /// A shadow or a card, with its box's corner at (`x`, `y`). A card's shadow is a dozen
    /// anti-aliased fills the size of the card, most of a frame's time on the CPU, so each is
    /// painted once into a pixmap of its own and copied in after that: byte for byte where the
    /// pixmap underneath is still empty, as it is when a panel starts with its card.
    fn layer(&mut self, layer: Layer, x: f32, y: f32) {
        let f = self.f;
        // Everything the layer covers, in layout units from the box's corner: the shadow, and a
        // card's line outside its edge.
        let mut left = -layer.blur / 2.0;
        let mut top = layer.dy - layer.blur / 2.0;
        let (mut right, mut bottom) = (layer.w - left, layer.h + layer.dy + layer.blur / 2.0);
        if layer.glass.is_some() {
            left = left.min(-1.0);
            top = top.min(-1.0);
            right = right.max(layer.w + 1.0);
            bottom = bottom.max(layer.h + 1.0);
        }
        // Its corner in device pixels, split into the whole pixel it's copied to and the fraction
        // it's painted at, so a cached copy lands exactly where painting in place would.
        let (dx, dy) = ((x + left) * f, (y + top) * f);
        let (col, row) = (dx.floor(), dy.floor());
        let offset = (dx - col, dy - row);
        let key = LayerKey {
            bits: [
                layer.w,
                layer.h,
                layer.radius,
                layer.dy,
                layer.blur,
                f,
                offset.0,
                offset.1,
            ]
            .map(f32::to_bits),
            colours: [
                layer.shadow,
                layer.glass.map_or(0, |(fill, _)| fill),
                layer.glass.map_or(0, |(_, edge)| edge),
            ],
            glass: layer.glass.is_some(),
        };
        LAYERS.with_borrow_mut(|cache| {
            if let Some(at) = cache.iter().position(|(k, _)| *k == key) {
                // The most recently used goes last, and the oldest first out.
                let entry = cache.remove(at);
                cache.push(entry);
            } else {
                let width = ((right - left) * f + offset.0).ceil() as u32 + 1;
                let height = ((bottom - top) * f + offset.1).ceil() as u32 + 1;
                let Some(mut painter) = Painter::new(width, height, f) else {
                    return;
                };
                painter.paint_layer(&layer, offset, (-left, -top));
                cache.push((key, painter.pixmap));
                while cache.len() > 1
                    && cache
                        .iter()
                        .map(|(_, pixmap)| pixmap.data().len())
                        .sum::<usize>()
                        > LAYER_CACHE_BYTES
                {
                    cache.remove(0);
                }
            }
            let (_, pixmap) = cache.last().expect("found or just added");
            if !copy_onto_empty(&mut self.pixmap, pixmap, col as i32, row as i32) {
                self.pixmap.draw_pixmap(
                    col as i32,
                    row as i32,
                    pixmap.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        });
    }

    /// Paints a layer with its box's corner at (`x`, `y`) in layout units, shifted by a fraction
    /// of a device pixel. The shadow is a dozen rounded rectangles, each grown by another step
    /// with its corner radius grown to match, so each is exactly the set of points within that
    /// distance of the box. Rather than fill a dozen paths, each pixel counts the ones it's inside
    /// from its distance to the box, which takes one pass however many there are. The card's fill
    /// and its two edge lines come from the same distance.
    fn paint_layer(&mut self, layer: &Layer, offset: (f32, f32), (x, y): (f32, f32)) {
        const STEPS: i32 = 12;
        let f = self.f;
        let colour = |rgba: u32| {
            let [r, g, b, a] = rgba.to_be_bytes().map(|c| c as f32 / 255.0);
            [r * a, g * a, b * a, a]
        };
        let [sr, sg, sb, sa] = layer.shadow.to_be_bytes();
        let step_alpha = (sa as u32 / STEPS as u32).max(1) as f32 / 255.0;
        let shadow = colour(u32::from_be_bytes([sr, sg, sb, 255]));
        // (1 − a)^n: what n whole steps leave of what's below.
        let left_after: Vec<f32> = (0..=STEPS).map(|n| (1.0 - step_alpha).powi(n)).collect();
        let step = layer.blur / 2.0 / STEPS as f32 * f;
        let (half_w, half_h, radius) = (layer.w * f / 2.0, layer.h * f / 2.0, layer.radius * f);
        let centre = (x * f + offset.0 + half_w, y * f + offset.1 + half_h);
        let glass = layer
            .glass
            .map(|(fill, edge)| (colour(fill), colour(edge), colour(0x000000ff)));
        // Signed distance from a pixel's centre to the box's outline, in device pixels, with the
        // box's centre `down` pixels lower.
        let distance = |px: f32, py: f32, down: f32| {
            let qx = (px - centre.0).abs() - (half_w - radius);
            let qy = (py - centre.1 - down).abs() - (half_h - radius);
            let (ox, oy) = (qx.max(0.0), qy.max(0.0));
            (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - radius
        };
        // How far either side of the centre a row `py` is within `t` of the box: the box grown by
        // `t` is a rounded rectangle too.
        let reach = |py: f32, down: f32, t: f32| -> Option<f32> {
            let (w, h, r) = (half_w + t, half_h + t, (radius + t).max(0.0));
            let v = (py - centre.1 - down).abs();
            if w <= 0.0 || v > h {
                return None;
            }
            let straight = h - r;
            Some(if v <= straight {
                w
            } else {
                w - r + (r * r - (v - straight).powi(2)).max(0.0).sqrt()
            })
        };
        // How much of the pixel around distance `d` lies in [from, to].
        let overlap =
            |d: f32, from: f32, to: f32| ((d + 0.5).min(to) - (d - 0.5).max(from)).clamp(0.0, 1.0);
        let over = |dst: [f32; 4], src: [f32; 4], amount: f32| {
            let keep = 1.0 - src[3] * amount;
            [0, 1, 2, 3].map(|i| src[i] * amount + dst[i] * keep)
        };
        let shadow_down = layer.dy * f;
        let pixel_at = |px: f32, py: f32| {
            // The shadow: steps grown by `step`, `2 × step` … `STEPS × step`. Every step grown
            // at least half a pixel past this one covers it whole; one more may cover it in part.
            let d = distance(px, py, shadow_down);
            let whole = (STEPS - ((d + 0.5) / step).ceil().max(0.0) as i32 + 1).clamp(0, STEPS);
            let partial_k = STEPS - whole;
            let partial = if partial_k >= 1 {
                (partial_k as f32 * step + 0.5 - d).clamp(0.0, 1.0) * step_alpha
            } else {
                0.0
            };
            let alpha = 1.0 - left_after[whole as usize] * (1.0 - partial);
            let mut out = [
                shadow[0] * alpha,
                shadow[1] * alpha,
                shadow[2] * alpha,
                alpha,
            ];
            if let Some((fill, edge, line)) = glass {
                let d = distance(px, py, 0.0);
                out = over(out, fill, overlap(d, f32::NEG_INFINITY, 0.0));
                out = over(out, edge, overlap(d, -f, 0.0));
                out = over(out, line, overlap(d, 0.0, f));
            }
            // Premultiplied, as tiny-skia keeps its pixels.
            let alpha = out[3].clamp(0.0, 1.0);
            out.map(|c| (c.clamp(0.0, alpha) * 255.0 + 0.5) as u8)
        };
        let (width, height) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        // Well inside the card every pixel is the same: under every step, and under the fill
        // only. Well outside everything the pixmap stays empty. Only the bands between are worked
        // out pixel by pixel.
        let solid_margin = step - 0.5;
        let column = |at: f32| (at - 0.5).clamp(0.0, width as f32);
        for row in 0..height {
            let py = row as f32 + 0.5;
            let outer = [
                reach(py, shadow_down, STEPS as f32 * step + 0.5),
                glass.and_then(|_| reach(py, 0.0, f + 0.5)),
            ]
            .into_iter()
            .flatten()
            .fold(None, |widest: Option<f32>, r| {
                Some(widest.map_or(r, |w| w.max(r)))
            });
            let Some(outer) = outer else {
                continue;
            };
            let inner = match glass {
                Some(_) => reach(py, 0.0, -(f + 0.5))
                    .zip(reach(py, shadow_down, solid_margin))
                    .map(|(a, b)| a.min(b)),
                None => reach(py, shadow_down, solid_margin),
            };
            let (from, to) = (
                column(centre.0 - outer).floor() as i32,
                column(centre.0 + outer).ceil() as i32,
            );
            let (solid_from, solid_to) = match inner {
                Some(inner) => (
                    column(centre.0 - inner).ceil() as i32 + 1,
                    column(centre.0 + inner).floor() as i32 - 1,
                ),
                None => (to, to),
            };
            let line = &mut data[(row * width * 4) as usize..((row + 1) * width * 4) as usize];
            if solid_from < solid_to {
                let solid = pixel_at(solid_from as f32 + 0.5, py);
                for x in from..solid_from {
                    let at = (x * 4) as usize;
                    line[at..at + 4].copy_from_slice(&pixel_at(x as f32 + 0.5, py));
                }
                for pixel in
                    line[(solid_from * 4) as usize..(solid_to * 4) as usize].chunks_exact_mut(4)
                {
                    pixel.copy_from_slice(&solid);
                }
                for x in solid_to..to {
                    let at = (x * 4) as usize;
                    line[at..at + 4].copy_from_slice(&pixel_at(x as f32 + 0.5, py));
                }
            } else {
                for x in from..to {
                    let at = (x * 4) as usize;
                    line[at..at + 4].copy_from_slice(&pixel_at(x as f32 + 0.5, py));
                }
            }
        }
    }

    /// Text on a line centred at `centre`. Returns its width.
    pub fn text(&mut self, s: &str, x: f32, centre: f32, style: &Style) -> f32 {
        let baseline = text::baseline(style, centre);
        self.text_on(s, x, baseline, style)
    }

    /// A key as a keycap, its left edge at `x` and centred on `centre`: Mono 12 in capitals on a
    /// faint rounded fill with a 1 px edge. Every surface that names a key draws it this way.
    /// Returns its width.
    pub fn keycap(&mut self, key: &str, x: f32, centre: f32) -> f32 {
        let style = text::Style::new(text::Face::Mono, 12.0, 0xdce3ecff);
        let label = key.to_uppercase();
        let (w, h) = (keycap_width(key), KEYCAP_H);
        self.fill(x, centre - h / 2.0, w, h, 5.0, 0xffffff12);
        self.border(x, centre - h / 2.0, w, h, 5.0, 1.0, 0xffffff24);
        self.text(&label, x + 7.0, centre, &style);
        w
    }

    /// Text with its baseline at `baseline`. Returns its width.
    pub fn text_on(&mut self, s: &str, x: f32, baseline: f32, style: &Style) -> f32 {
        text::draw(
            &mut self.pixmap,
            s,
            x * self.f,
            baseline * self.f,
            &style.scaled(self.f),
        ) / self.f
    }

    pub fn icon(&mut self, body: &str, x: f32, y: f32, size: f32, ink: Option<u32>) {
        icons::draw(
            &mut self.pixmap,
            body,
            x * self.f,
            y * self.f,
            size * self.f,
            ink,
        );
    }

    /// An image already rasterised at device pixels, with its corner at (`x`, `y`).
    pub fn image(&mut self, image: &Pixmap, x: f32, y: f32) {
        self.pixmap.draw_pixmap(
            (x * self.f).round() as i32,
            (y * self.f).round() as i32,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

/// A keycap's height, in the units it's painted in.
pub const KEYCAP_H: f32 = 20.0;

/// How wide `Painter::keycap` draws `key`, padding included.
pub fn keycap_width(key: &str) -> f32 {
    let style = text::Style::new(text::Face::Mono, 12.0, 0);
    text::width(&key.to_uppercase(), &style) + 14.0
}

/// A painted surface ready to show: its buffer, and its size in logical and buffer pixels.
pub struct Painted {
    pub buffer: MemoryRenderBuffer,
    pub logical: Size<i32, Logical>,
    pub device: (i32, i32),
    pub scale: f64,
}

impl Painted {
    /// Paints a surface `logical` pixels big at `scale`, laid out in logical pixels.
    pub fn new(
        logical: Size<i32, Logical>,
        scale: f64,
        draw: impl FnOnce(&mut Painter),
    ) -> Option<Self> {
        let device = (
            (logical.w as f64 * scale).round().max(1.0) as i32,
            (logical.h as f64 * scale).round().max(1.0) as i32,
        );
        let mut painter = Painter::new(device.0 as u32, device.1 as u32, scale as f32)?;
        draw(&mut painter);
        Some(Self {
            buffer: buffer(&painter.pixmap),
            logical,
            device,
            scale,
        })
    }

    /// The surface with its corner at `at`, on whole screen pixels so text stays sharp.
    pub fn element<R>(
        &self,
        renderer: &mut R,
        at: Point<f64, Logical>,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let location = at.to_physical(self.scale).to_i32_round::<i32>().to_f64();
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location,
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

/// An SVG or PNG file drawn into a `px`-pixel square, keeping its proportions.
pub fn load_image(path: &Path, px: u32) -> Option<Pixmap> {
    // The path comes from an app or a desktop entry, so it may name anything at all.
    let data = crate::files::read_small(path, crate::files::IMAGE_LIMIT).ok()?;
    let mut out = Pixmap::new(px, px)?;
    let side = px as f32;
    if path.extension().is_some_and(|ext| ext == "svg") {
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
        let size = tree.size();
        let k = side / size.width().max(size.height());
        let transform = Transform::from_row(
            k,
            0.0,
            0.0,
            k,
            (side - size.width() * k) / 2.0,
            (side - size.height() * k) / 2.0,
        );
        resvg::render(&tree, transform, &mut out.as_mut());
    } else {
        let source = decode_png(&data)?;
        let (w, h) = (source.width() as f32, source.height() as f32);
        let k = side / w.max(h);
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        let transform =
            Transform::from_row(k, 0.0, 0.0, k, (side - w * k) / 2.0, (side - h * k) / 2.0);
        out.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
    }
    Some(out)
}

/// Any PNG as premultiplied RGBA.
fn decode_png(data: &[u8]) -> Option<Pixmap> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    let channels = info.color_type.samples();
    let mut pixmap = Pixmap::new(info.width, info.height)?;
    for (dst, src) in pixmap
        .data_mut()
        .chunks_exact_mut(4)
        .zip(pixels.chunks_exact(channels))
    {
        let (r, g, b, a) = match *src {
            [r, g, b, a] => (r, g, b, a),
            [r, g, b] => (r, g, b, 255),
            [grey, a] => (grey, grey, grey, a),
            [grey] => (grey, grey, grey, 255),
            _ => return None,
        };
        let premultiply = |channel: u8| ((channel as u16 * a as u16 + 127) / 255) as u8;
        dst.copy_from_slice(&[premultiply(r), premultiply(g), premultiply(b), a]);
    }
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_loads_scaled_into_its_square() {
        let dir = crate::files::test_scratch("paint");
        let path = dir.join("red.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 4, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0].repeat(8))
            .unwrap();
        let image = load_image(&path, 16).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((image.width(), image.height()), (16, 16));
        let centre = image.pixel(8, 8).unwrap();
        assert_eq!((centre.red(), centre.alpha()), (255, 255));
        assert_eq!(
            image.pixel(8, 0).unwrap().alpha(),
            0,
            "letterboxed, not stretched"
        );
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    const SHADOW: (f32, f32, u32) = (40.0, 110.0, 0x000000b0);
    const FILL: u32 = 0x10131af8;
    const EDGE: u32 = 0xffffff1c;

    /// The card painted the slow way, straight onto the pixmap, a dozen fills and two strokes.
    fn in_place(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
        const STEPS: u32 = 12;
        let step = 0xb0 / STEPS;
        for i in 0..STEPS {
            let spread = 110.0 / 2.0 * (1.0 - i as f32 / STEPS as f32);
            p.fill(
                x - spread,
                y + 40.0 - spread,
                w + 2.0 * spread,
                h + 2.0 * spread,
                18.0 + spread,
                step,
            );
        }
        p.fill(x, y, w, h, 18.0, FILL);
        p.border(x, y, w, h, 18.0, 1.0, EDGE);
        p.border(x - 1.0, y - 1.0, w + 2.0, h + 2.0, 19.0, 1.0, 0x000000ff);
    }

    /// The largest difference in any channel between two paintings, leaving out the pixels
    /// within a line's width and a pixel of the card's outline, where tiny-skia's stroked lines and exact
    /// coverage round differently.
    fn difference_off_the_outline(
        a: &Painter,
        b: &Painter,
        (x, y, w, h): (f32, f32, f32, f32),
    ) -> u8 {
        let mut outline = Painter::new(a.pixmap.width(), a.pixmap.height(), a.f).unwrap();
        // Each line is a layout unit wide, anti-aliased half a device pixel either side.
        let band = 1.0 + 1.0 / a.f;
        outline.border(
            x - band,
            y - band,
            w + 2.0 * band,
            h + 2.0 * band,
            18.0 + band,
            2.0 * band,
            0xffffffff,
        );
        a.pixmap
            .data()
            .chunks(4)
            .zip(b.pixmap.data().chunks(4))
            .zip(outline.pixmap.data().chunks(4))
            .filter(|(_, mask)| mask[3] == 0)
            .map(|((a, b), _)| {
                a.iter()
                    .zip(b)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn a_cached_card_matches_one_painted_in_place() {
        // A fractional scale and corners off the pixel grid, as panels at 1.25× have.
        for (f, x, y) in [
            (1.0, 64.0, 64.0),
            (1.25 * 0.8, 64.3, 70.6),
            (1.5, 20.0, 30.2),
        ] {
            let mut slow = Painter::new(900, 700, f).unwrap();
            in_place(&mut slow, x, y, 400.0, 250.0);
            // Painted fresh, then copied from the cache.
            for _ in 0..2 {
                let mut fast = Painter::new(900, 700, f).unwrap();
                fast.card(x, y, 400.0, 250.0, 18.0, SHADOW, FILL, EDGE);
                let worst = difference_off_the_outline(&slow, &fast, (x, y, 400.0, 250.0));
                assert!(worst <= 4, "at {f}×, ({x}, {y}): {worst}");
            }
        }
    }

    #[test]
    fn a_card_over_something_drawn_blends_instead_of_copying() {
        let mut slow = Painter::new(600, 500, 1.0).unwrap();
        slow.fill(0.0, 0.0, 600.0, 40.0, 0.0, 0x3cf0c0ff);
        in_place(&mut slow, 60.0, 30.0, 300.0, 200.0);
        let mut fast = Painter::new(600, 500, 1.0).unwrap();
        fast.fill(0.0, 0.0, 600.0, 40.0, 0.0, 0x3cf0c0ff);
        fast.card(60.0, 30.0, 300.0, 200.0, 18.0, SHADOW, FILL, EDGE);
        assert!(difference_off_the_outline(&slow, &fast, (60.0, 30.0, 300.0, 200.0)) <= 4);
        // The green band still shows beside the card.
        assert_eq!(&fast.pixmap.data()[..4], &[0x3c, 0xf0, 0xc0, 0xff]);
    }

    #[test]
    fn a_plain_shadow_matches_one_painted_in_place() {
        let mut slow = Painter::new(300, 200, 1.25).unwrap();
        for i in 0..12u32 {
            let spread = 16.0 / 2.0 * (1.0 - i as f32 / 12.0);
            slow.fill(
                40.0 - spread,
                30.0 + 6.0 - spread,
                120.0 + 2.0 * spread,
                60.0 + 2.0 * spread,
                10.0 + spread,
                0x99 / 12,
            );
        }
        let mut fast = Painter::new(300, 200, 1.25).unwrap();
        fast.shadow(40.0, 30.0, 120.0, 60.0, 10.0, 6.0, 16.0, 0x00000099);
        let worst = slow
            .pixmap
            .data()
            .iter()
            .zip(fast.pixmap.data())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(worst <= 4, "{worst}");
    }

    #[test]
    fn a_layer_hanging_off_the_pixmap_is_clipped() {
        let mut p = Painter::new(100, 80, 1.0).unwrap();
        p.card(-50.0, 40.0, 300.0, 200.0, 18.0, SHADOW, FILL, EDGE);
        p.shadow(80.0, -30.0, 60.0, 60.0, 10.0, 6.0, 16.0, 0x00000099);
    }
}
