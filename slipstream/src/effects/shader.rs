//! Plumbing for the desktop's small shaders: the lit lines, the bar's lit edge and its workspace
//! marker, and the closing windows' tube.
//!
//! Each program is compiled once for a GL context and kept with it (`program`), until it is let go
//! (`release`) when nothing will draw with it for a while. A shader's
//! numbers are plain values (`Values`), listed for the renderer on the stack as it draws, so a
//! frame allocates nothing for them. Each element is kept from frame to frame under one id
//! (`Kept`), and its count moves only when its place or numbers change, so one that stands still
//! costs the screen nothing. Constants a shader shares with the Rust side are written into its
//! source from the Rust ones (`with_constants`), so the two can't drift apart.

use std::{cell::RefCell, fmt::Debug, marker::PhantomData};

use smithay::{
    backend::renderer::{
        Texture,
        element::{Element, Id, Kind, RenderElement},
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture,
            Uniform, UniformName, UniformType, UniformValue,
        },
        utils::CommitCounter,
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform, user_data::UserDataMap},
};

/// A program kept in a renderer, marked by `K`: not compiled yet (or let go), compiled, or
/// `Some(None)` if that failed.
struct Compiled<K, P>(RefCell<Option<Option<P>>>, PhantomData<K>);

/// The program marked by `K` for `renderer`, compiled by `compile` the first time it's asked
/// for. One that doesn't compile is said once, as `what` shader, and not tried again until it is
/// let go.
pub fn program<K: 'static, P: Clone + 'static>(
    renderer: &mut GlesRenderer,
    compile: impl FnOnce(&mut GlesRenderer) -> Result<P, GlesError>,
    what: &str,
) -> Option<P> {
    let kept = |renderer: &GlesRenderer| {
        let data = renderer.egl_context().user_data();
        data.insert_if_missing(|| Compiled::<K, P>(RefCell::new(None), PhantomData));
        data.get::<Compiled<K, P>>()
            .and_then(|compiled| compiled.0.borrow().clone())
    };
    if let Some(kept) = kept(renderer) {
        return kept;
    }
    let compiled = compile(renderer)
        .map_err(|err| tracing::warn!("{what} shader didn't compile, so it isn't drawn: {err}"))
        .ok();
    if let Some(slot) = renderer.egl_context().user_data().get::<Compiled<K, P>>() {
        *slot.0.borrow_mut() = Some(compiled.clone());
    }
    compiled
}

/// Lets the program marked by `K` go, if `renderer` keeps it, so the driver frees it; it is
/// compiled again the next time it is asked for.
pub fn release<K: 'static, P: 'static>(renderer: &GlesRenderer) {
    if let Some(kept) = renderer.egl_context().user_data().get::<Compiled<K, P>>() {
        kept.0.borrow_mut().take();
    }
}

/// The line every shader here has once, after which shared constants are declared.
const PRECISION: &str = "precision highp float;\n";

/// A Rust number a shader is given as a `const float`, written as Rust writes it.
pub trait Float: Debug {}
impl Float for f32 {}
impl Float for f64 {}

/// `source` with each of `constants` declared as a `const float` straight after its precision
/// line, so a shader and the Rust side that times or places what it draws share one number.
pub fn with_constants(source: &str, constants: &[(&str, &dyn Float)]) -> String {
    debug_assert_eq!(source.matches(PRECISION).count(), 1, "one precision line");
    let declared: String = constants
        .iter()
        .map(|(name, value)| format!("const float {name} = {value:?};\n"))
        .collect();
    source.replacen(PRECISION, &format!("{PRECISION}{declared}"), 1)
}

/// The most numbers one of these shaders takes.
const MOST: usize = 20;

/// One of a shader's numbers: its name and one to four floats.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Value {
    name: &'static str,
    value: [f32; 4],
    len: u8,
}

const UNSET: Value = Value {
    name: "",
    value: [0.0; 4],
    len: 1,
};

/// A shader's numbers in the order it declares them, all plain values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    list: [Value; MOST],
    len: usize,
}

impl Default for Values {
    fn default() -> Self {
        Self {
            list: [UNSET; MOST],
            len: 0,
        }
    }
}

impl Values {
    /// These numbers and `name`, a float or a vector of up to four.
    pub fn with<const N: usize>(mut self, name: &'static str, value: [f32; N]) -> Self {
        const { assert!(N >= 1 && N <= 4) };
        assert!(self.len < MOST, "a shader with more than {MOST} numbers");
        let mut padded = [0.0; 4];
        padded[..N].copy_from_slice(&value);
        self.list[self.len] = Value {
            name,
            value: padded,
            len: N as u8,
        };
        self.len += 1;
        self
    }

    fn each(&self) -> &[Value] {
        &self.list[..self.len]
    }

    /// The names and types, as a program is compiled with them.
    pub fn names(&self) -> Vec<UniformName<'static>> {
        self.each()
            .iter()
            .map(|value| {
                let kind = match value.len {
                    1 => UniformType::_1f,
                    2 => UniformType::_2f,
                    3 => UniformType::_3f,
                    _ => UniformType::_4f,
                };
                UniformName::new(value.name, kind)
            })
            .collect()
    }

    /// The numbers as the renderer takes them, for a part drawn outside an element: the first
    /// of the two counts are these.
    pub fn listed(&self) -> ([Uniform<'static>; MOST], usize) {
        (self.uniforms(), self.len)
    }

    /// The numbers as the renderer takes them, on the stack: the first `len` are these.
    fn uniforms(&self) -> [Uniform<'static>; MOST] {
        std::array::from_fn(|n| {
            let Value { name, value, len } = self.list[n];
            let [a, b, c, d] = value;
            let value = match len {
                1 => UniformValue::_1f(a),
                2 => UniformValue::_2f(a, b),
                3 => UniformValue::_3f(a, b, c),
                _ => UniformValue::_4f(a, b, c, d),
            };
            Uniform::new(name, value)
        })
    }
}

/// A shader's numbers, kept by each shader as its own named fields.
pub trait Uniforms {
    fn values(&self) -> Values;
}

/// The names and types of the numbers `U` lists, to compile its program with.
pub fn names<U: Uniforms + Default>() -> Vec<UniformName<'static>> {
    U::default().values().names()
}

/// One of the desktop's shader-drawn parts, ready to draw.
#[derive(Debug, Clone)]
pub struct Shaded {
    id: Id,
    commit: CommitCounter,
    area: Rectangle<i32, Logical>,
    alpha: f32,
    values: Values,
    look: Look,
}

/// The name a lit line's element goes by where the drawer draws it.
pub type Shared = Shaded;

#[derive(Debug, Clone)]
enum Look {
    /// A pixel shader over the whole area, which carries its own alpha in its numbers.
    Pixel(GlesPixelProgram),
    /// A part of a texture drawn through a shader of its own.
    Texture(GlesTexProgram, GlesTexture, Rectangle<f64, Buffer>),
}

impl Element for Shaded {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match &self.look {
            Look::Pixel(_) => {
                Rectangle::from_size(self.area.size.to_f64().to_buffer(1.0, Transform::Normal))
            }
            Look::Texture(_, _, part) => *part,
        }
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
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let uniforms = self.values.uniforms();
        let uniforms = &uniforms[..self.values.len];
        match &self.look {
            Look::Pixel(program) => frame.render_pixel_shader_to(
                program,
                src,
                dst,
                self.area.size.to_buffer(1, Transform::Normal),
                Some(damage),
                self.alpha,
                uniforms,
            ),
            Look::Texture(program, texture, _) => frame.render_texture_from_to(
                texture,
                src,
                dst,
                damage,
                &[],
                Transform::Normal,
                self.alpha,
                Some(program),
                uniforms,
            ),
        }
    }
}

/// A shader-drawn part kept from frame to frame: one id for as long as it lives, and a count that
/// moves only when what it draws changes.
#[derive(Debug, Default)]
pub struct Kept {
    id: Option<Id>,
    commit: CommitCounter,
    shown: Option<(Rectangle<i32, Logical>, Values)>,
}

impl Kept {
    /// Notes that the part is drawn over `area` with `uniforms`, counted as a change only when
    /// either differs from the last.
    pub fn note(&mut self, area: Rectangle<i32, Logical>, uniforms: &impl Uniforms) -> Values {
        let values = uniforms.values();
        if self.shown != Some((area, values)) {
            self.shown = Some((area, values));
            self.commit.increment();
        }
        values
    }

    /// Counts a change the numbers don't show, such as a new texture under them.
    pub fn changed(&mut self) {
        self.commit.increment();
    }

    /// The count that moves when the part changes.
    #[cfg(test)]
    pub fn commit(&self) -> CommitCounter {
        self.commit
    }

    fn shaded(
        &mut self,
        area: Rectangle<i32, Logical>,
        alpha: f32,
        values: Values,
        look: Look,
    ) -> Shaded {
        Shaded {
            id: self.id.get_or_insert_with(Id::new).clone(),
            commit: self.commit,
            area,
            alpha,
            values,
            look,
        }
    }

    /// `program` over `area` with `uniforms`, which carry its alpha.
    pub fn pixel(
        &mut self,
        program: GlesPixelProgram,
        area: Rectangle<i32, Logical>,
        uniforms: &impl Uniforms,
    ) -> Shaded {
        let values = self.note(area, uniforms);
        self.shaded(area, 1.0, values, Look::Pixel(program))
    }

    /// `texture` drawn over `area` through `program` with `uniforms`, at `alpha`.
    pub fn texture(
        &mut self,
        (program, texture): (GlesTexProgram, GlesTexture),
        area: Rectangle<i32, Logical>,
        alpha: f32,
        uniforms: &impl Uniforms,
    ) -> Shaded {
        let whole = Rectangle::from_size(texture.size().to_f64());
        self.texture_part((program, texture, whole), area, alpha, uniforms)
    }

    /// The `part` of `texture` drawn over `area` through `program` with `uniforms`, at `alpha`.
    pub fn texture_part(
        &mut self,
        (program, texture, part): (GlesTexProgram, GlesTexture, Rectangle<f64, Buffer>),
        area: Rectangle<i32, Logical>,
        alpha: f32,
        uniforms: &impl Uniforms,
    ) -> Shaded {
        let values = self.note(area, uniforms);
        self.shaded(area, alpha, values, Look::Texture(program, texture, part))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_declared_once_straight_after_the_precision() {
        let source = with_constants(
            "#version 100\nprecision highp float;\nvoid main() {}\n",
            &[("ONE", &1.0_f64), ("PEAK", &0.55_f32)],
        );
        assert_eq!(
            source,
            "#version 100\nprecision highp float;\nconst float ONE = 1.0;\n\
             const float PEAK = 0.55;\nvoid main() {}\n"
        );
    }

    #[derive(Default)]
    struct Two([f32; 2], f32);

    impl Uniforms for Two {
        fn values(&self) -> Values {
            Values::default().with("at", self.0).with("t", [self.1])
        }
    }

    #[test]
    fn numbers_are_named_and_typed_as_they_are_listed() {
        let names = names::<Two>();
        assert_eq!(
            names,
            [
                UniformName::new("at", UniformType::_2f),
                UniformName::new("t", UniformType::_1f),
            ]
        );
        let values = Two([1.0, 2.0], 3.0).values();
        let listed = values.uniforms();
        assert_eq!(listed[0].value, UniformValue::_2f(1.0, 2.0));
        assert_eq!(listed[1].value, UniformValue::_1f(3.0));
        assert_eq!(values.len, 2);
    }

    #[test]
    fn a_kept_part_counts_a_change_only_when_its_place_or_numbers_change() {
        let mut kept = Kept::default();
        let area = Rectangle::new((0, 0).into(), (10, 10).into());
        kept.note(area, &Two([1.0, 2.0], 3.0));
        let first = kept.commit();
        kept.note(area, &Two([1.0, 2.0], 3.0));
        assert_eq!(kept.commit(), first, "the same again is no change");
        kept.note(area, &Two([1.0, 2.0], 4.0));
        assert_ne!(kept.commit(), first);
        let numbers = kept.commit();
        kept.note(
            Rectangle::new((1, 0).into(), (10, 10).into()),
            &Two([1.0, 2.0], 4.0),
        );
        assert_ne!(kept.commit(), numbers, "moved");
        let moved = kept.commit();
        kept.changed();
        assert_ne!(kept.commit(), moved, "a change the numbers don't show");
    }
}
