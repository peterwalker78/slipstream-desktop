//! The bar's parts drawn by its own shaders: the lit edge along its foot, and the workspace
//! marker over its lit numbers. Each keeps its numbers as plain values and builds the shader's
//! list of them on the stack as it draws, so a frame allocates nothing for them, and it counts as
//! changed only when they do, so a still part costs the screen nothing.

use std::marker::PhantomData;

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement},
        gles::{GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture},
        utils::CommitCounter,
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform},
};

use super::{light, marker};

/// One of the bar's shader-drawn parts, ready to draw.
#[derive(Debug, Clone)]
pub struct Shaded {
    id: Id,
    commit: CommitCounter,
    area: Rectangle<i32, Logical>,
    alpha: f32,
    look: Look,
}

#[derive(Debug, Clone)]
enum Look {
    Edge {
        program: GlesPixelProgram,
        uniforms: light::Uniforms,
    },
    Marker {
        program: GlesTexProgram,
        texture: GlesTexture,
        uniforms: marker::Uniforms,
    },
}

impl Shaded {
    /// The lit edge's strip over `area`, which carries its alpha in its numbers.
    pub fn edge(
        program: GlesPixelProgram,
        id: Id,
        commit: CommitCounter,
        area: Rectangle<i32, Logical>,
        uniforms: light::Uniforms,
    ) -> Self {
        Self {
            id,
            commit,
            area,
            alpha: 1.0,
            look: Look::Edge { program, uniforms },
        }
    }

    /// The marker over `area`, drawn from `texture` (the lit numbers, one texel per screen
    /// pixel) at `alpha`.
    pub fn marker(
        (program, texture): (GlesTexProgram, GlesTexture),
        id: Id,
        commit: CommitCounter,
        area: Rectangle<i32, Logical>,
        alpha: f32,
        uniforms: marker::Uniforms,
    ) -> Self {
        Self {
            id,
            commit,
            area,
            alpha,
            look: Look::Marker {
                program,
                texture,
                uniforms,
            },
        }
    }
}

impl Element for Shaded {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let size = match &self.look {
            Look::Edge { .. } => self.area.size.to_f64().to_buffer(1.0, Transform::Normal),
            Look::Marker { uniforms, .. } => {
                let [w, h] = uniforms.size();
                Size::from((w as f64, h as f64))
            }
        };
        Rectangle::from_size(size)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.area.to_physical_precise_round(scale)
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Shaded {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        match &self.look {
            Look::Edge { program, uniforms } => frame.render_pixel_shader_to(
                program,
                src,
                dst,
                self.area.size.to_buffer(1, Transform::Normal),
                Some(damage),
                self.alpha,
                &uniforms.list(),
            ),
            Look::Marker {
                program,
                texture,
                uniforms,
            } => frame.render_texture_from_to(
                texture,
                src,
                dst,
                damage,
                &[],
                Transform::Normal,
                self.alpha,
                Some(program),
                &uniforms.list(),
            ),
        }
    }
}

/// A part's stable id, made the first time it's drawn.
#[derive(Default)]
pub struct Kept(Option<Id>);

impl Kept {
    pub fn get(&mut self) -> Id {
        self.0.get_or_insert_with(Id::new).clone()
    }
}

/// A program kept in a renderer, marked by `T`: compiled once, or `None` if that failed.
struct Compiled<T, P>(Option<P>, PhantomData<T>);

/// The program marked by `T` for `renderer`, compiled by `compile` the first time it's asked
/// for. A program that doesn't compile is said once and not tried again.
pub fn program<T: 'static, P: Clone + 'static>(
    renderer: &mut GlesRenderer,
    compile: impl FnOnce(&mut GlesRenderer) -> Result<P, GlesError>,
    what: &str,
) -> Option<P> {
    if let Some(kept) = renderer.egl_context().user_data().get::<Compiled<T, P>>() {
        return kept.0.clone();
    }
    let compiled = compile(renderer)
        .map_err(|err| tracing::warn!("{what} shader didn't compile: {err}"))
        .ok();
    let kept = compiled.clone();
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(move || Compiled::<T, P>(kept, PhantomData));
    compiled
}
