//! A still picture of your own behind the windows, in place of the living wallpaper.
//!
//! Settings turns whatever image was chosen into a PNG at `slipstream_config::picture_path()`,
//! so this only ever reads that one PNG. It is decoded off the main thread, uploaded once into a
//! texture, and every screen draws that same texture fitted to itself. Nothing about it changes
//! from frame to frame, so a desktop at rest has nothing to redraw. If the file is missing or
//! can't be read, the living wallpaper stays and the log says why.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::SystemTime,
};

use resvg::tiny_skia::Pixmap;
use slipstream_config::PictureFit;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ContextId, Renderer,
            element::{
                Kind,
                solid::{SolidColorBuffer, SolidColorRenderElement},
                texture::{TextureBuffer, TextureRenderElement},
            },
            gles::{GlesRenderer, GlesTexture},
        },
    },
    utils::{Logical, Rectangle, Size, Transform},
};

/// Which prepared file is shown: its path, and when it was last written, so a new picture
/// prepared at the same path is read again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    path: PathBuf,
    written: Option<SystemTime>,
}

#[derive(Default)]
pub struct Picture {
    wanted: Option<Wanted>,
    fit: PictureFit,
    /// The decoded picture on its way from the loading thread.
    loading: Option<Receiver<Option<Pixmap>>>,
    /// Decoded and waiting to be uploaded into a renderer that has no texture of it yet.
    decoded: Option<Pixmap>,
    /// The picture's size in pixels, once known.
    size: Option<(u32, u32)>,
    /// One texture for each renderer that has drawn it; nearly always just the one.
    textures: Vec<(ContextId<GlesTexture>, TextureBuffer<GlesTexture>)>,
    /// The file couldn't be read, so the living wallpaper shows until a new one is chosen.
    failed: bool,
    /// The bars round a fitted picture, one per screen.
    bars: HashMap<String, SolidColorBuffer>,
}

impl Picture {
    pub fn new(wallpaper: &slipstream_config::Wallpaper) -> Self {
        let mut picture = Self::default();
        picture.set(wallpaper);
        picture
    }

    /// The settings changed: start reading the picture if one is now asked for, or a new one has
    /// been prepared since, and forget it when the living wallpaper is back.
    pub fn set(&mut self, wallpaper: &slipstream_config::Wallpaper) {
        self.fit = wallpaper.picture_fit;
        let wanted = wallpaper.shows_picture().then(|| {
            let path = slipstream_config::picture_path();
            let written = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok();
            Wanted { path, written }
        });
        if wanted == self.wanted {
            return;
        }
        self.loading = None;
        self.decoded = None;
        self.size = None;
        self.textures.clear();
        self.failed = false;
        self.wanted = wanted;
        self.read_on_thread();
    }

    /// Starts reading the wanted file on a thread of its own, so a large picture never holds up
    /// a frame.
    fn read_on_thread(&mut self) {
        let Some(path) = self.wanted.as_ref().map(|wanted| wanted.path.clone()) else {
            return;
        };
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("wallpaper picture".into())
            .spawn(move || {
                let _ = send.send(read(&path));
            });
        match spawned {
            Ok(_) => self.loading = Some(receive),
            Err(err) => {
                tracing::warn!("couldn't start reading the wallpaper picture: {err}");
                self.failed = true;
            }
        }
    }

    /// Takes the decoded picture once the loading thread has it.
    fn collect(&mut self) {
        let Some(loading) = self.loading.as_ref() else {
            return;
        };
        match loading.try_recv() {
            Ok(Some(pixmap)) => {
                self.size = Some((pixmap.width(), pixmap.height()));
                self.decoded = Some(pixmap);
                self.loading = None;
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {
                self.failed = true;
                self.loading = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// The texture for `renderer`, uploading the decoded picture into it the first time. A second
    /// renderer reads the file again rather than every one keeping a copy of it in memory.
    fn texture(&mut self, renderer: &mut GlesRenderer) -> Option<TextureBuffer<GlesTexture>> {
        let context = renderer.context_id();
        if let Some((_, texture)) = self
            .textures
            .iter()
            .find(|(made_in, _)| *made_in == context)
        {
            return Some(texture.clone());
        }
        let Some(pixmap) = self.decoded.take() else {
            if self.loading.is_none() && !self.failed {
                self.read_on_thread();
            }
            return None;
        };
        let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
        // Premultiplied RGBA, which is how the renderer blends.
        match TextureBuffer::from_memory(
            renderer,
            pixmap.data(),
            Fourcc::Abgr8888,
            (w, h),
            false,
            1,
            Transform::Normal,
            None,
        ) {
            Ok(texture) => {
                tracing::info!(width = w, height = h, "wallpaper picture shown");
                self.textures.push((context, texture.clone()));
                Some(texture)
            }
            Err(err) => {
                tracing::warn!("couldn't upload the wallpaper picture: {err}");
                self.failed = true;
                None
            }
        }
    }

    /// The picture for a screen `size` logical pixels big, at `alpha`, frontmost first: the
    /// picture, then the bars a fitted one leaves. None while there is no picture to show, so the
    /// living wallpaper stays: none is chosen, it is still being read, or it couldn't be.
    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        screen: &str,
        size: Size<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Option<Vec<crate::render::OutputElement>> {
        use crate::render::OutputElement;
        self.wanted.as_ref()?;
        self.collect();
        if self.failed {
            return None;
        }
        let texture = self.texture(renderer)?;
        let (w, h) = self.size?;
        let placed = fitted(
            (w as f64, h as f64),
            (size.w as f64, size.h as f64),
            self.fit,
        );
        let [sx, sy, sw, sh] = placed.src;
        let [dx, dy, dw, dh] = placed.dst;
        let mut elements = vec![OutputElement::Texture(
            TextureRenderElement::from_texture_buffer(
                ((dx * scale).round(), (dy * scale).round()),
                &texture,
                Some(alpha),
                Some(Rectangle::new((sx, sy).into(), (sw, sh).into())),
                Some(Size::from((dw.round() as i32, dh.round() as i32))),
                Kind::Unspecified,
            ),
        )];
        if self.fit == PictureFit::Fit {
            let bars = self
                .bars
                .entry(screen.to_string())
                .or_insert_with(|| SolidColorBuffer::new(size, crate::render::BACKGROUND));
            bars.update(size, crate::render::BACKGROUND);
            elements.push(OutputElement::Solid(SolidColorRenderElement::from_buffer(
                bars,
                (0, 0),
                scale,
                alpha,
                Kind::Unspecified,
            )));
        }
        Some(elements)
    }

    /// A screen went away.
    pub fn forget_screen(&mut self, screen: &str) {
        self.bars.remove(screen);
    }
}

/// Reads the prepared PNG, saying in the log why not when it can't.
fn read(path: &std::path::Path) -> Option<Pixmap> {
    let data = std::fs::read(path)
        .map_err(|err| {
            tracing::warn!(
                path = %path.display(),
                "no wallpaper picture, so the living wallpaper stays: {err}"
            )
        })
        .ok()?;
    let pixmap = crate::paint::decode_png(&data);
    if pixmap.is_none() {
        tracing::warn!(
            path = %path.display(),
            "the wallpaper picture isn't a PNG that can be shown, so the living wallpaper stays"
        );
    }
    pixmap
}

/// Where a picture goes on a screen: `src` is the part of it shown, in its own pixels, and
/// `dst` where that lands on the screen, in the screen's logical pixels. Both are x, y, w, h.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub src: [f64; 4],
    pub dst: [f64; 4],
}

/// Fits a `picture` (w, h) to a `screen` (w, h). Fill scales it to cover the screen and crops
/// the overhang evenly from both sides; Fit scales it to fit inside and centres it.
pub fn fitted(picture: (f64, f64), screen: (f64, f64), fit: PictureFit) -> Placed {
    let (pw, ph) = (picture.0.max(1.0), picture.1.max(1.0));
    let (sw, sh) = screen;
    match fit {
        PictureFit::Fill => {
            let k = (sw / pw).max(sh / ph);
            let (w, h) = ((sw / k).min(pw), (sh / k).min(ph));
            Placed {
                src: [(pw - w) / 2.0, (ph - h) / 2.0, w, h],
                dst: [0.0, 0.0, sw, sh],
            }
        }
        PictureFit::Fit => {
            let k = (sw / pw).min(sh / ph);
            let (w, h) = (pw * k, ph * k);
            Placed {
                src: [0.0, 0.0, pw, ph],
                dst: [(sw - w) / 2.0, (sh - h) / 2.0, w, h],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANDSCAPE: (f64, f64) = (4000.0, 3000.0);
    const PORTRAIT: (f64, f64) = (3000.0, 4000.0);
    const LAPTOP: (f64, f64) = (1536.0, 960.0);
    const ULTRA_WIDE: (f64, f64) = (3440.0, 1440.0);
    const UPRIGHT: (f64, f64) = (1080.0, 1920.0);

    fn close(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
    }

    #[test]
    fn fill_covers_the_screen_and_crops_evenly() {
        for picture in [LANDSCAPE, PORTRAIT] {
            for screen in [LAPTOP, ULTRA_WIDE, UPRIGHT] {
                let placed = fitted(picture, screen, PictureFit::Fill);
                let [x, y, w, h] = placed.src;
                assert!(close(placed.dst, [0.0, 0.0, screen.0, screen.1]));
                // The part shown is the screen's shape, inside the picture, and centred.
                assert!((w / h - screen.0 / screen.1).abs() < 1e-9);
                assert!(w <= picture.0 + 1e-9 && h <= picture.1 + 1e-9);
                assert!((x * 2.0 + w - picture.0).abs() < 1e-9);
                assert!((y * 2.0 + h - picture.1).abs() < 1e-9);
                // One side always uses the whole picture.
                assert!((w - picture.0).abs() < 1e-9 || (h - picture.1).abs() < 1e-9);
            }
        }
        // A landscape photo on an ultra-wide screen loses its top and bottom.
        let placed = fitted(LANDSCAPE, ULTRA_WIDE, PictureFit::Fill);
        assert!(close(
            placed.src,
            [0.0, 662.790_697_674_418_6, 4000.0, 1_674.418_604_651_162_7]
        ));
        // A portrait photo on a laptop keeps its middle band.
        let placed = fitted(PORTRAIT, LAPTOP, PictureFit::Fill);
        assert!(close(placed.src, [0.0, 1062.5, 3000.0, 1875.0]));
    }

    #[test]
    fn fit_shows_the_whole_picture_centred() {
        for picture in [LANDSCAPE, PORTRAIT] {
            for screen in [LAPTOP, ULTRA_WIDE, UPRIGHT] {
                let placed = fitted(picture, screen, PictureFit::Fit);
                let [x, y, w, h] = placed.dst;
                assert!(close(placed.src, [0.0, 0.0, picture.0, picture.1]));
                assert!(
                    (w / h - picture.0 / picture.1).abs() < 1e-9,
                    "its own shape"
                );
                assert!(w <= screen.0 + 1e-9 && h <= screen.1 + 1e-9);
                assert!((x * 2.0 + w - screen.0).abs() < 1e-9);
                assert!((y * 2.0 + h - screen.1).abs() < 1e-9);
                assert!((w - screen.0).abs() < 1e-9 || (h - screen.1).abs() < 1e-9);
            }
        }
        // A portrait photo on an ultra-wide screen stands in the middle, full height.
        let placed = fitted(PORTRAIT, ULTRA_WIDE, PictureFit::Fit);
        assert!(close(placed.dst, [1180.0, 0.0, 1080.0, 1440.0]));
        // A landscape photo on an upright screen spans its width.
        let placed = fitted(LANDSCAPE, UPRIGHT, PictureFit::Fit);
        assert!(close(placed.dst, [0.0, 555.0, 1080.0, 810.0]));
    }

    #[test]
    fn a_picture_the_screen_s_own_shape_fills_it_either_way() {
        for fit in [PictureFit::Fill, PictureFit::Fit] {
            let placed = fitted((3072.0, 1920.0), LAPTOP, fit);
            assert!(close(placed.src, [0.0, 0.0, 3072.0, 1920.0]));
            assert!(close(placed.dst, [0.0, 0.0, LAPTOP.0, LAPTOP.1]));
        }
    }

    #[test]
    fn a_missing_or_broken_picture_leaves_the_living_wallpaper() {
        let dir = crate::files::test_scratch("picture");
        assert!(read(&dir.join("none.png")).is_none());
        let broken = dir.join("broken.png");
        std::fs::write(&broken, b"not a picture").unwrap();
        assert!(read(&broken).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
