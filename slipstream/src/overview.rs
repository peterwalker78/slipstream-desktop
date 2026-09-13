//! What bullet time draws over the zoomed-out workspaces: each workspace's frame and number, a
//! note on empty ones, letter hints on windows and streams, the selection ring, the vignette and
//! the amber border. `render.rs` places them; this paints and caches them.

use std::collections::{HashMap, hash_map::Entry};

use smithay::{
    backend::renderer::{
        Color32F, ImportMem, Renderer,
        element::{
            Kind,
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
        },
    },
    utils::{Logical, Point, Rectangle, Scale, Size},
};

use crate::{
    paint::Painted,
    text::{self, Face, Style},
};

// Premultiplied colours.
/// A workspace's frame.
pub const FRAME: Color32F = Color32F::new(0.18, 0.18, 0.18, 0.18);
/// Over the windows of workspaces not being looked at.
pub const SHADE: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);

/// The ring around what's chosen, in the border colour from the settings.
pub fn ring(rgb: [f32; 3]) -> Color32F {
    Color32F::new(rgb[0], rgb[1], rgb[2], 1.0)
}

/// The border around the whole zoomed-out screen: the same colour at nine tenths.
pub fn border(rgb: [f32; 3]) -> Color32F {
    shade(rgb, 0.9)
}

/// The frame around the workspace being looked at: the same colour at just over two thirds, so
/// the ring stays the brightest thing in the overview.
pub fn frame_viewed(rgb: [f32; 3]) -> Color32F {
    shade(rgb, 0.69)
}

/// Premultiplied, so a colour at `alpha` is dimmed by the same factor.
fn shade(rgb: [f32; 3], alpha: f32) -> Color32F {
    Color32F::new(rgb[0] * alpha, rgb[1] * alpha, rgb[2] * alpha, alpha)
}

#[derive(Default)]
pub struct Overview {
    /// Solid rectangles, handed out in order each frame.
    solids: Vec<SolidColorBuffer>,
    used: usize,
    hints: HashMap<(String, Option<String>, u64), Painted>,
    labels: HashMap<(String, bool, String, u64), Painted>,
    notes: HashMap<(usize, u64), Painted>,
    /// The close button, plain and with the pointer on it, by scale.
    closes: HashMap<(bool, u64), Painted>,
    /// Bullet time's highlight colour, 0xRRGGBBAA, which the letter hints, the viewed workspace's
    /// number and a hovered close button are drawn in. `None` until it's set: amber.
    accent: Option<u32>,
    vignette: Option<(Size<i32, Logical>, Painted)>,
}

impl Overview {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.hints.clear();
        self.labels.clear();
        self.notes.clear();
    }

    /// Bullet time's highlight colour, as `[r, g, b]` from 0 to 1. What was painted in the old
    /// colour is painted again.
    pub fn set_accent(&mut self, rgb: [f32; 3]) {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        let accent =
            (channel(rgb[0]) << 24) | (channel(rgb[1]) << 16) | (channel(rgb[2]) << 8) | 0xff;
        if self.accent != Some(accent) {
            self.accent = Some(accent);
            self.hints.clear();
            self.labels.clear();
            self.closes.clear();
        }
    }

    fn accent(&self) -> u32 {
        self.accent.unwrap_or(0xffb547ff)
    }

    /// Call once per frame, before any solids.
    pub fn begin(&mut self) {
        self.used = 0;
        if self.hints.len() > 200 {
            self.hints.clear();
        }
    }

    pub fn solid(
        &mut self,
        rect: Rectangle<i32, Logical>,
        colour: Color32F,
        scale: Scale<f64>,
        alpha: f32,
    ) -> SolidColorRenderElement {
        if self.used == self.solids.len() {
            self.solids.push(SolidColorBuffer::new((0, 0), colour));
        }
        let buffer = &mut self.solids[self.used];
        self.used += 1;
        buffer.update(rect.size, colour);
        SolidColorRenderElement::from_buffer(
            buffer,
            rect.loc.to_physical_precise_round(scale),
            scale,
            alpha,
            Kind::Unspecified,
        )
    }

    /// Four edges `thickness` wide, `gap` outside `rect`.
    pub fn outline(
        &mut self,
        rect: Rectangle<f64, Logical>,
        gap: f64,
        thickness: i32,
        colour: Color32F,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<SolidColorRenderElement> {
        let t = thickness as f64;
        let (x, y) = (rect.loc.x - gap - t, rect.loc.y - gap - t);
        let (w, h) = (rect.size.w + 2.0 * (gap + t), rect.size.h + 2.0 * (gap + t));
        let edge = |x: f64, y: f64, w: f64, h: f64| {
            Rectangle::<i32, Logical>::new(
                (x.round() as i32, y.round() as i32).into(),
                (w.round().max(1.0) as i32, h.round().max(1.0) as i32).into(),
            )
        };
        [
            edge(x, y, w, t),
            edge(x, y + h - t, w, t),
            edge(x, y + t, t, h - 2.0 * t),
            edge(x + w - t, y + t, t, h - 2.0 * t),
        ]
        .into_iter()
        .map(|edge| self.solid(edge, colour, scale, alpha))
        .collect()
    }

    /// A letter hint centred on `centre`, with the target's name beneath. Without a name it's the
    /// smaller hint the code rain's streams get.
    #[allow(clippy::too_many_arguments)]
    pub fn hint<R>(
        &mut self,
        renderer: &mut R,
        label: &str,
        name: Option<&str>,
        centre: Point<f64, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let key = (label.to_string(), name.map(String::from), scale.to_bits());
        if !self.hints.contains_key(&key) {
            self.hints
                .insert(key.clone(), paint_hint(label, name, scale, self.accent())?);
        }
        let painted = self.hints.get(&key)?;
        let above = if name.is_some() {
            4.0 + 24.0
        } else {
            painted.logical.h as f64 / 2.0
        };
        let at = Point::from((centre.x - painted.logical.w as f64 / 2.0, centre.y - above));
        painted.element(renderer, at, alpha)
    }

    /// A workspace's name or number and a caption ("3 windows", "empty"), with its top-left at
    /// `at`.
    #[allow(clippy::too_many_arguments)]
    pub fn workspace_label<R>(
        &mut self,
        renderer: &mut R,
        label: &str,
        viewed: bool,
        caption: &str,
        at: Point<f64, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let key = (
            label.to_string(),
            viewed,
            caption.to_string(),
            scale.to_bits(),
        );
        if !self.labels.contains_key(&key) {
            let digits = Style::new(
                Face::Display,
                34.0,
                if viewed { self.accent() } else { 0xe8edf5ff },
            );
            let small = Style::new(Face::Mono, 15.0, 0xaab4c6ff);
            let number = label;
            let number_w = text::width(number, &digits);
            let w = number_w + 10.0 + text::width(caption, &small) + 4.0;
            let size = Size::from((w.ceil() as i32, 40));
            let painted = Painted::new(size, scale, |p| {
                p.text_on(number, 0.0, 32.0, &digits);
                p.text_on(caption, number_w + 10.0, 32.0, &small);
            })?;
            self.labels.insert(key.clone(), painted);
        }
        self.labels.get(&key)?.element(renderer, at, alpha)
    }

    /// A window's close button, filling `rect`: dark with a faint edge, or amber with a dark
    /// cross while the pointer is on it (`hot`).
    pub fn close_button<R>(
        &mut self,
        renderer: &mut R,
        rect: Rectangle<f64, Logical>,
        hot: bool,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let side = rect.size.w.round().max(1.0) as i32;
        let accent = self.accent();
        let painted = match self.closes.entry((hot, scale.to_bits() ^ side as u64)) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(slot) => {
                let s = side as f32;
                slot.insert(Painted::new(Size::from((side, side)), scale, |p| {
                    let (fill, ink) = if hot {
                        (accent, ink_on(accent))
                    } else {
                        (0x0d0f14ee, 0xe8edf5ff)
                    };
                    p.fill(0.0, 0.0, s, s, 8.0 * 0.8, fill);
                    if !hot {
                        p.border(0.0, 0.0, s, s, 8.0 * 0.8, 1.0, 0xffffff24);
                    }
                    let glyph = s * 0.6;
                    p.icon(
                        crate::icons::CLOSE,
                        (s - glyph) / 2.0,
                        (s - glyph) / 2.0,
                        glyph,
                        Some(ink),
                    );
                })?)
            }
        };
        painted.element(renderer, rect.loc, alpha)
    }

    /// "Empty workspace", and how to fill it, centred on `centre`.
    pub fn empty_note<R>(
        &mut self,
        renderer: &mut R,
        number: usize,
        centre: Point<f64, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let painted = match self.notes.entry((number, scale.to_bits())) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(slot) => {
                let title = Style::new(Face::Body, 24.0, 0x9aa4b6ff);
                let how = Style::new(Face::Mono, 13.0, crate::panel::HINT);
                let lines = (
                    "Empty workspace",
                    if number <= 9 {
                        format!("Shift+{number} sends the chosen window here")
                    } else {
                        "Shift+← → sends the chosen window here".to_string()
                    },
                );
                let (title_w, how_w) = (text::width(lines.0, &title), text::width(&lines.1, &how));
                let w = title_w.max(how_w).ceil() + 8.0;
                slot.insert(Painted::new(Size::from((w as i32, 60)), scale, |p| {
                    p.text(lines.0, (w - title_w) / 2.0, 18.0, &title);
                    p.text(&lines.1, (w - how_w) / 2.0, 46.0, &how);
                })?)
            }
        };
        let at = Point::from((
            centre.x - painted.logical.w as f64 / 2.0,
            centre.y - painted.logical.h as f64 / 2.0,
        ));
        painted.element(renderer, at, alpha)
    }

    /// The screen's edges darkened, as the mockup's radial vignette.
    pub fn vignette<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let stale = self
            .vignette
            .as_ref()
            .is_none_or(|(painted_for, painted)| *painted_for != size || painted.scale != scale);
        if stale {
            // A soft gradient has no detail, so it's painted small and stretched.
            let small = Size::from(((size.w / 4).max(1), (size.h / 4).max(1)));
            let mut painted = Painted::new(small, 1.0, |p| {
                let (w, h) = (p.pixmap.width() as usize, p.pixmap.height() as usize);
                for (i, pixel) in p.pixmap.data_mut().chunks_exact_mut(4).enumerate() {
                    let dx = ((i % w) as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                    let dy = ((i / w) as f32 + 0.5) / h as f32 * 2.0 - 1.0;
                    let distance = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
                    let dark = ((distance - 0.45) / 0.55).clamp(0.0, 1.0) * 0.6;
                    pixel.copy_from_slice(&[0, 0, 0, (dark * 255.0) as u8]);
                }
            })?;
            painted.logical = size;
            painted.scale = scale;
            self.vignette = Some((size, painted));
        }
        let (_, painted) = self.vignette.as_ref()?;
        painted.element(renderer, Point::from((0.0, 0.0)), alpha)
    }
}

/// Dark ink on a light colour, light ink on a dark one.
fn ink_on(rgba: u32) -> u32 {
    let [r, g, b, _] = rgba.to_be_bytes().map(|c| c as f32 / 255.0);
    if 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.45 {
        0x1a1206ff
    } else {
        0xf2f4f8ff
    }
}

fn paint_hint(label: &str, name: Option<&str>, scale: f64, accent: u32) -> Option<Painted> {
    let small = name.is_none();
    let (box_h, px) = if small { (34.0, 22.0) } else { (48.0, 34.0) };
    let letter = Style::new(Face::MonoBold, px, accent);
    let letter_w = text::width(label, &letter);
    let box_w = (letter_w + if small { 16.0 } else { 26.0 }).max(box_h);
    let caption = Style::new(Face::Mono, 13.0, 0xe8edf5ff);
    let name = name.map(|name| text::ellipsize(name, &caption, 220.0));
    let name_w = name
        .as_ref()
        .map_or(0.0, |name| text::width(name, &caption));
    let w = box_w.max(name_w + 12.0).ceil();
    let h = if small { box_h } else { box_h + 6.0 + 22.0 };
    Painted::new(Size::from((w as i32 + 8, h as i32 + 8)), scale, |p| {
        let x = 4.0 + (w - box_w) / 2.0;
        p.shadow(x, 4.0, box_w, box_h, 10.0, 6.0, 16.0, 0x00000099);
        p.fill(x, 4.0, box_w, box_h, 10.0, 0x0d0f14ee);
        p.border(x, 4.0, box_w, box_h, 10.0, 2.5, accent);
        p.text(
            label,
            x + (box_w - letter_w) / 2.0,
            4.0 + box_h / 2.0,
            &letter,
        );
        if let Some(name) = &name {
            let x = 4.0 + (w - name_w - 12.0) / 2.0;
            p.fill(x, 4.0 + box_h + 6.0, name_w + 12.0, 22.0, 4.0, 0x0d0f14cc);
            p.text(name, x + 6.0, 4.0 + box_h + 6.0 + 11.0, &caption);
        }
    })
}
