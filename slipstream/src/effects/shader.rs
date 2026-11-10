//! Plumbing for the desktop's small pixel shaders: one compiled program per renderer, kept with
//! its GL context, and elements kept from frame to frame that change only when their numbers do
//! and go into every frame without copying them.

use std::{marker::PhantomData, rc::Rc};

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform, UniformName,
            element::PixelShaderElement,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{
        Buffer, Logical, Physical, Point, Rectangle, Scale, Transform, user_data::UserDataMap,
    },
};

/// A program compiled once for a GL context, found again by the type `K` names it by; `None`
/// if it didn't compile, which is logged once.
struct Program<K>(Option<GlesPixelProgram>, PhantomData<K>);

/// The program `K` names, compiled from `source` the first time this renderer asks for it.
pub fn program<K: 'static>(
    renderer: &mut GlesRenderer,
    source: impl FnOnce() -> String,
    uniforms: &[UniformName<'_>],
) -> Option<GlesPixelProgram> {
    if let Some(Program(kept, _)) = renderer.egl_context().user_data().get::<Program<K>>() {
        return kept.clone();
    }
    let compiled = renderer
        .compile_custom_pixel_shader(source(), uniforms)
        .map_err(|err| tracing::warn!("a shader didn't compile, so it isn't drawn: {err}"))
        .ok();
    let kept = compiled.clone();
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(move || Program::<K>(kept, PhantomData));
    compiled
}

/// A shader's numbers, all plain values, listed for its element only when they change.
pub trait Uniforms: Copy + PartialEq {
    fn list(&self) -> Vec<Uniform<'static>>;
}

/// A shader element kept from frame to frame: moved and given new numbers only where they differ
/// from the last, so one that stands still has nothing to redraw.
#[derive(Debug)]
pub struct Kept<U> {
    element: Option<Shared>,
    shown: Option<(Rectangle<i32, Logical>, U)>,
}

impl<U> Default for Kept<U> {
    fn default() -> Self {
        Self {
            element: None,
            shown: None,
        }
    }
}

impl<U: Uniforms> Kept<U> {
    /// The element over `area` with `uniforms`, made from `program` the first time.
    pub fn element(
        &mut self,
        program: impl FnOnce() -> Option<GlesPixelProgram>,
        area: Rectangle<i32, Logical>,
        uniforms: U,
    ) -> Option<Shared> {
        let element = match self.element.as_mut() {
            Some(element) => element,
            None => self.element.insert(Shared(Rc::new(PixelShaderElement::new(
                program()?,
                area,
                None,
                1.0,
                uniforms.list(),
                Kind::Unspecified,
            )))),
        };
        if let Some((at, shown)) = self.shown {
            if at != area {
                element.get_mut().resize(area, None);
            }
            if shown != uniforms {
                element.get_mut().update_uniforms(uniforms.list());
            }
        }
        self.shown = Some((area, uniforms));
        Some(element.clone())
    }
}

/// A pixel shader element handed to each frame without a copy: the frame holds the same
/// element, and a change made once the frame is done touches the one kept.
#[derive(Debug, Clone)]
pub struct Shared(Rc<PixelShaderElement>);

impl Shared {
    /// The element to change. A frame still holding it gets its own copy first.
    fn get_mut(&mut self) -> &mut PixelShaderElement {
        Rc::make_mut(&mut self.0)
    }
}

impl Element for Shared {
    fn id(&self) -> &Id {
        self.0.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.0.location(scale)
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }
    fn transform(&self) -> Transform {
        self.0.transform()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.0.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.0.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.0.alpha()
    }
    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl RenderElement<GlesRenderer> for Shared {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(
            self.0.as_ref(),
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
