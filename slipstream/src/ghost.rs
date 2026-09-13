//! A closed window's last picture, fading away where it was while the others retile into its
//! space.
//!
//! By the time a toplevel is destroyed, most clients have already let go of its buffers, so there
//! is nothing left to draw. Instead, every frame keeps a handle on the textures each window was
//! just drawn from (`Picture`): a reference, not a copy. When the window goes, its last picture
//! outlives the client and is drawn as plain textures until the fade ends.

use smithay::{
    backend::renderer::{
        ContextId,
        element::{Id, Kind, texture::TextureRenderElement},
        gles::GlesTexture,
        utils::RendererSurfaceStateUserData,
    },
    desktop::Window,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::{TraversalAction, with_surface_tree_downward},
        seat::WaylandFocus,
    },
};

use crate::motion::HYPR;

/// Transparent over 220 ms, shrinking a little about its middle.
const FADE: f64 = 0.22;
const REDUCED_FADE: f64 = 0.08;
const END_SCALE: f64 = 0.94;

/// One surface of the window: its texture, where it sat from the window's corner, and how it
/// was drawn.
#[derive(Clone)]
struct Piece {
    id: Id,
    texture: GlesTexture,
    offset: Point<f64, Logical>,
    src: Rectangle<f64, Logical>,
    size: Size<i32, Logical>,
    buffer_scale: i32,
    transform: Transform,
}

/// The textures a window was last drawn from, and its own size then.
#[derive(Clone)]
pub struct Picture {
    pieces: Vec<Piece>,
    own: Size<i32, Logical>,
}

pub struct Ghost {
    pieces: Vec<Piece>,
    /// Where the window was drawn, in the space's logical pixels.
    rect: Rectangle<f64, Logical>,
    /// The window's own size, which the pieces are laid out in.
    own: Size<i32, Logical>,
    started: f64,
    reduced_motion: bool,
}

impl Picture {
    /// What `window` is drawn from now. `None` when none of it has been imported by this
    /// renderer.
    pub fn of(window: &Window, context: &ContextId<GlesTexture>) -> Option<Self> {
        let surface = window.wl_surface()?;
        let corner = window.geometry().loc.to_f64();
        let mut pieces = Vec::new();
        with_surface_tree_downward(
            &surface,
            Point::<f64, Logical>::default(),
            // The walk holds each surface's state already, so it's read from what the walk hands
            // over: asking for it again would wait on the walk's own lock for ever.
            |_, states, location| {
                let offset = states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .and_then(|state| state.lock().unwrap().view())
                    .map(|view| view.offset.to_f64());
                match offset {
                    Some(offset) => TraversalAction::DoChildren(*location + offset),
                    None => TraversalAction::SkipChildren,
                }
            },
            |_, states, location| {
                let Some(state) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                    return;
                };
                let state = state.lock().unwrap();
                let (Some(view), Some(texture)) =
                    (state.view(), state.texture::<GlesTexture>(context.clone()))
                else {
                    return;
                };
                pieces.push(Piece {
                    id: Id::new(),
                    texture: texture.clone(),
                    offset: *location + view.offset.to_f64() - corner,
                    src: view.src,
                    size: view.dst,
                    buffer_scale: state.buffer_scale(),
                    transform: state.buffer_transform(),
                });
            },
            |_, _, _| true,
        );
        if pieces.is_empty() {
            return None;
        }
        Some(Self {
            pieces,
            own: window.geometry().size,
        })
    }
}

impl Picture {
    /// The window's own size when this was taken.
    pub fn own(&self) -> Size<i32, Logical> {
        self.own
    }
}

impl Ghost {
    /// A window's last picture, fading from `rect`, where it was drawn.
    pub fn new(
        picture: Picture,
        rect: Rectangle<f64, Logical>,
        now: f64,
        reduced_motion: bool,
    ) -> Self {
        Self {
            pieces: picture.pieces,
            rect,
            own: picture.own,
            started: now,
            reduced_motion,
        }
    }

    /// How far through its fade, from 0 to 1.
    fn progress(&self, now: f64) -> f64 {
        let length = if self.reduced_motion {
            REDUCED_FADE
        } else {
            FADE
        };
        ((now - self.started) / length).clamp(0.0, 1.0)
    }

    pub fn done(&self, now: f64) -> bool {
        self.progress(now) >= 1.0
    }

    /// Whether any of it is on a screen at `screen`, in the space's logical pixels.
    pub fn overlaps(&self, screen: Rectangle<f64, Logical>) -> bool {
        self.rect.overlaps(screen)
    }

    /// Its pieces for a screen whose corner is at `origin`, at `scale`, front first.
    pub fn elements(
        &self,
        context: &ContextId<GlesTexture>,
        origin: Point<f64, Logical>,
        scale: Scale<f64>,
        now: f64,
    ) -> Vec<TextureRenderElement<GlesTexture>> {
        let t = self.progress(now);
        let alpha = (1.0 - t) as f32;
        let shrink = if self.reduced_motion {
            1.0
        } else {
            1.0 - (1.0 - END_SCALE) * HYPR.at(t).clamp(0.0, 1.0)
        };
        // The window's own size to where it was drawn, then the shrink about its middle.
        let stretch = (
            self.rect.size.w / self.own.w.max(1) as f64 * shrink,
            self.rect.size.h / self.own.h.max(1) as f64 * shrink,
        );
        let corner = Point::<f64, Logical>::from((
            self.rect.loc.x + self.rect.size.w * (1.0 - shrink) / 2.0,
            self.rect.loc.y + self.rect.size.h * (1.0 - shrink) / 2.0,
        )) - origin;
        self.pieces
            .iter()
            .rev()
            .map(|piece| {
                let at = Point::<f64, Logical>::from((
                    corner.x + piece.offset.x * stretch.0,
                    corner.y + piece.offset.y * stretch.1,
                ));
                let size = Size::<i32, Logical>::from((
                    (piece.size.w as f64 * stretch.0).round().max(1.0) as i32,
                    (piece.size.h as f64 * stretch.1).round().max(1.0) as i32,
                ));
                let location: Point<f64, Physical> = at.to_physical(scale);
                TextureRenderElement::from_static_texture(
                    piece.id.clone(),
                    context.clone(),
                    location,
                    piece.texture.clone(),
                    piece.buffer_scale,
                    piece.transform,
                    Some(alpha),
                    Some(piece.src),
                    Some(size),
                    None,
                    Kind::Unspecified,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ghost(reduced_motion: bool) -> Ghost {
        let picture = Picture {
            pieces: Vec::new(),
            own: Size::from((800, 600)),
        };
        let rect = Rectangle::new((100.0, 40.0).into(), (800.0, 600.0).into());
        Ghost::new(picture, rect, 10.0, reduced_motion)
    }

    #[test]
    fn a_ghost_fades_for_its_time_and_then_goes() {
        let full = ghost(false);
        assert!(!full.done(10.0 + FADE / 2.0));
        assert!(full.done(10.0 + FADE));
        let reduced = ghost(true);
        assert!(reduced.done(10.0 + REDUCED_FADE));
        assert!(!full.done(10.0 + REDUCED_FADE), "the full fade is longer");
    }

    #[test]
    fn a_ghost_is_drawn_only_on_the_screen_it_was_on() {
        let ghost = ghost(false);
        let left = Rectangle::new((0.0, 0.0).into(), (1536.0, 960.0).into());
        let right = Rectangle::new((1536.0, 0.0).into(), (1920.0, 1080.0).into());
        assert!(ghost.overlaps(left));
        assert!(!ghost.overlaps(right));
    }
}
