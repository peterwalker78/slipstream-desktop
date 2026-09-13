//! The pointer on real hardware: the XCursor theme named by `XCURSOR_THEME` (at `XCURSOR_SIZE`),
//! including the shapes apps ask for, with a drawn arrow if no theme loads. Adapted from
//! Smithay's anvil (MIT).

use std::{collections::HashMap, time::Duration};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Logical, Point, Transform},
};
use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

pub struct Cursor {
    theme: CursorTheme,
    size: u32,
    /// Parsed shapes by name; `None` if the theme doesn't have that one.
    icons: HashMap<String, Option<Vec<Image>>>,
    /// Uploaded frames by shape, nominal size and animation frame.
    buffers: HashMap<(String, u32, usize), MemoryRenderBuffer>,
}

impl Cursor {
    pub fn load() -> Self {
        let theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        Self {
            theme: CursorTheme::load(&theme),
            size,
            icons: HashMap::new(),
            buffers: HashMap::new(),
        }
    }

    /// The image for shape `name` at output `scale`, and its hotspot in logical pixels.
    pub fn image(
        &mut self,
        name: &str,
        scale: f64,
        time: Duration,
    ) -> (MemoryRenderBuffer, Point<f64, Logical>) {
        // Draw from the next integer size down, so the pointer stays sharp at 1.25×.
        let int_scale = scale.ceil().max(1.0) as i32;
        let nominal = self.size * int_scale as u32;
        let name = if self.has(name) {
            name
        } else if self.has("default") {
            "default"
        } else {
            return self.fallback(int_scale);
        };
        let images = self.icons[name].as_ref().unwrap();
        let (index, image) = frame(time.as_millis() as u32, nominal, images);
        let hotspot = Point::from((
            image.xhot as f64 / int_scale as f64,
            image.yhot as f64 / int_scale as f64,
        ));
        let buffer = self
            .buffers
            .entry((name.to_string(), nominal, index))
            .or_insert_with(|| {
                MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    Fourcc::Abgr8888,
                    (image.width as i32, image.height as i32),
                    int_scale,
                    Transform::Normal,
                    None,
                )
            })
            .clone();
        (buffer, hotspot)
    }

    /// The default pointer at nominal size, for XWayland to show over X11 windows.
    pub fn x11_image(&mut self) -> Option<Image> {
        if !self.has("default") {
            return None;
        }
        let images = self.icons["default"].as_ref()?;
        Some(frame(0, self.size, images).1.clone())
    }

    fn has(&mut self, name: &str) -> bool {
        if !self.icons.contains_key(name) {
            let images = self
                .theme
                .load_icon(name)
                .and_then(|path| crate::files::read_small(&path, crate::files::CURSOR_LIMIT).ok())
                .and_then(|data| parse_xcursor(&data))
                .filter(|images| !images.is_empty());
            if images.is_none() {
                tracing::debug!(name, "cursor theme has no such shape");
            }
            self.icons.insert(name.to_string(), images);
        }
        self.icons[name].is_some()
    }

    fn fallback(&mut self, int_scale: i32) -> (MemoryRenderBuffer, Point<f64, Logical>) {
        let buffer = self
            .buffers
            .entry((String::new(), int_scale as u32, 0))
            .or_insert_with(|| {
                let (w, h, pixels) = arrow(int_scale);
                MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Abgr8888,
                    (w, h),
                    int_scale,
                    Transform::Normal,
                    None,
                )
            })
            .clone();
        (buffer, Point::from((0.0, 0.0)))
    }
}

/// The animation frame due at `millis`, from the images nearest the nominal `size`.
fn frame(millis: u32, size: u32, images: &[Image]) -> (usize, &Image) {
    let nearest = images
        .iter()
        .min_by_key(|image| (size as i32 - image.size as i32).abs())
        .unwrap();
    let frames: Vec<&Image> = images
        .iter()
        .filter(|image| image.width == nearest.width && image.height == nearest.height)
        .collect();
    let total: u32 = frames.iter().map(|image| image.delay).sum();
    if total > 0 {
        let mut t = millis % total;
        for (index, image) in frames.iter().enumerate() {
            if t < image.delay {
                return (index, image);
            }
            t -= image.delay;
        }
    }
    (0, frames[0])
}

/// A white arrow with a black edge, for systems with no cursor theme.
fn arrow(scale: i32) -> (i32, i32, Vec<u8>) {
    let (w, h) = (12 * scale, 19 * scale);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / scale as f32, y as f32 / scale as f32);
            let slope_edge = fy * 0.62;
            let bottom_edge = 17.5 - fx * 0.5;
            if fx > slope_edge || fy > bottom_edge {
                continue;
            }
            let edge = fx < 1.0 || fx > slope_edge - 1.2 || fy > bottom_edge - 1.2;
            let shade = if edge { 0 } else { 255 };
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[shade, shade, shade, 255]);
        }
    }
    (w, h, pixels)
}
