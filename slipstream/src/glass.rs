//! The glass fade. While the UI fades to the living wallpaper, or back, it's drawn whole into a
//! texture and turned away like a page: a crease sweeps diagonally across the screen from the top
//! right, and past it the page bends back through the screen's glass and falls away into the
//! distance, darkening and going out of focus. Light glints along the crease, with a sheen on the
//! flat page just ahead of it. The wallpaper lies on the glass itself, so wherever the page has
//! bent behind the glass the wallpaper's light is over it, and where it's still flat the page is in
//! front. Coming back, the page turns in again the same way.

use smithay::{
    backend::renderer::{
        Color32F, Renderer,
        element::{
            Id, Kind, memory::MemoryRenderBufferRenderElement, texture::TextureRenderElement,
        },
        gles::{
            GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
            element::TextureShaderElement,
        },
    },
    utils::{Logical, Physical, Rectangle, Size, Transform},
};

use crate::{render::OutputElement, tilt};

/// The way across the screen the page turns, towards the corner that goes first: right and up.
const DIRECTION: (f32, f32) = (0.9439, -0.3304);
/// How far the eye is from the screen, in screen widths.
const DISTANCE: f32 = 1.25;
/// The crease's radius, in screen widths.
const RADIUS: f32 = 0.11;
/// How far past upright the page turns back into the screen, in radians (about 72 degrees).
const TURN: f32 = 1.25;
/// How far the whole page sinks back as it turns, in the eye's distances.
const PUSH: f32 = 0.12;
/// The depths, in the eye's distances, over which the turned page fades out.
const FADE: (f32, f32) = (0.04, 0.2);
/// How far ahead of the crease the sheen reaches on the flat page, in screen widths.
const SHEEN: f32 = 0.3;

/// Where the crease is, `progress` of the way through the turn: its distance from the screen's
/// centre along `DIRECTION`, in logical pixels, and how far the whole page has sunk back.
fn crease(progress: f32, width: f32, height: f32) -> (f32, f32) {
    let reach = reach(width, height);
    let distance = DISTANCE * width;
    // Before: the sheen hasn't reached the page. After: every part of it has turned deep enough
    // to have faded out.
    let before = reach + SHEEN * width;
    let after = -reach - RADIUS * width * TURN - FADE.1 * distance / TURN.sin();
    (
        before + (after - before) * progress,
        PUSH * distance * progress,
    )
}

/// How far the page reaches from the screen's centre along `DIRECTION`.
fn reach(width: f32, height: f32) -> f32 {
    DIRECTION.0.abs() * width / 2.0 + DIRECTION.1.abs() * height / 2.0
}

/// A point `along` the turn's direction from the screen's centre, bent round a crease at `at`:
/// where it is along the same line, how deep behind the glass, and at what angle. Mirrors `bend`
/// in the shader.
#[cfg(test)]
fn bend(along: f32, at: f32, width: f32) -> (f32, f32, f32) {
    let r = RADIUS * width;
    let s = along - at;
    if s <= 0.0 {
        return (along, 0.0, 0.0);
    }
    let arc = r * TURN;
    if s <= arc {
        let angle = s / r;
        return (at + r * angle.sin(), r * (1.0 - angle.cos()), angle);
    }
    let t = s - arc;
    (
        at + r * TURN.sin() + t * TURN.cos(),
        r * (1.0 - TURN.cos()) + t * TURN.sin(),
        TURN,
    )
}

/// The uniforms both shaders take.
fn uniforms(logical: Size<i32, Logical>, progress: f32) -> Vec<Uniform<'static>> {
    let (w, h) = (logical.w as f32, logical.h as f32);
    let (at, push) = crease(progress, w, h);
    let distance = DISTANCE * w;
    vec![
        Uniform::new("size", (w, h)),
        Uniform::new("fold", at),
        Uniform::new("push", push),
        Uniform::new("direction", DIRECTION),
        Uniform::new("distance", distance),
        Uniform::new(
            "shape",
            (RADIUS * w, TURN, FADE.0 * distance, FADE.1 * distance),
        ),
        Uniform::new("sheen", SHEEN * w),
    ]
}

fn uniform_names() -> [UniformName<'static>; 7] {
    [
        UniformName::new("size", UniformType::_2f),
        UniformName::new("fold", UniformType::_1f),
        UniformName::new("push", UniformType::_1f),
        UniformName::new("direction", UniformType::_2f),
        UniformName::new("distance", UniformType::_1f),
        UniformName::new("shape", UniformType::_4f),
        UniformName::new("sheen", UniformType::_1f),
    ]
}

/// Smithay's texture shader's preamble, and the page's shape: which point of the flat page is seen
/// at a point on the screen.
macro_rules! page_shader {
    () => {
        r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The screen in logical pixels; the crease's distance from the centre along the turn's direction;
// how far the page has sunk back; the eye's distance; the crease's radius, the turn's angle, and
// the depths the turned page fades out between; how far the sheen reaches ahead of the crease.
uniform vec2 size;
uniform float fold;
uniform float push;
uniform vec2 direction;
uniform float distance;
uniform vec4 shape;
uniform float sheen;

// A point `a` along the turn from the centre, bent round the crease: where it is along the same
// line, how deep behind the glass, and at what angle.
void bend(float a, out float x, out float z, out float th) {
    float r = shape.x;
    float turn = shape.y;
    float s = a - fold;
    if (s <= 0.0) {
        x = a;
        z = 0.0;
        th = 0.0;
        return;
    }
    float arc = r * turn;
    if (s <= arc) {
        th = s / r;
        x = fold + r * sin(th);
        z = r * (1.0 - cos(th));
        return;
    }
    float t = s - arc;
    th = turn;
    x = fold + r * sin(turn) + t * cos(turn);
    z = r * (1.0 - cos(turn)) + t * sin(turn);
}

// How much smaller something `z` behind the glass looks, the whole page having sunk `push`.
float lens(float z) {
    return distance / (distance + push + z);
}

// The point of the flat page seen at `screen`, with its depth, its angle, and how far along the
// turn it is. The flat part is in front of everything, so it's tried first; past the crease, the
// bent page is walked from the crease outwards and the first part found there is the nearest.
bool seen(vec2 screen, out vec2 at, out float z, out float th, out float a) {
    vec2 across = vec2(-direction.y, direction.x);
    vec2 o = screen - size * 0.5;
    float sa = dot(o, direction);
    float sb = dot(o, across);
    float reach = abs(direction.x) * size.x * 0.5 + abs(direction.y) * size.y * 0.5;
    float x;
    float k = lens(0.0);
    a = sa / k;
    z = 0.0;
    th = 0.0;
    if (a > fold) {
        float lo = max(fold, -reach);
        if (lo >= reach) {
            return false;
        }
        bend(lo, x, z, th);
        if (x * lens(z) - sa >= 0.0) {
            return false;
        }
        float step = (reach - lo) / 24.0;
        float hi = lo;
        bool hit = false;
        for (int i = 1; i <= 24; i++) {
            hi = lo + step * float(i);
            bend(hi, x, z, th);
            if (x * lens(z) - sa >= 0.0) {
                hit = true;
                break;
            }
        }
        if (!hit) {
            return false;
        }
        float low = hi - step;
        for (int j = 0; j < 8; j++) {
            float mid = 0.5 * (low + hi);
            bend(mid, x, z, th);
            if (x * lens(z) - sa >= 0.0) {
                hi = mid;
            } else {
                low = mid;
            }
        }
        a = 0.5 * (low + hi);
        bend(a, x, z, th);
        k = lens(z);
    }
    at = size * 0.5 + a * direction + (sb / k) * across;
    return at.x >= 0.0 && at.y >= 0.0 && at.x <= size.x && at.y <= size.y;
}
"#
    };
}

/// The turning page.
const SHADER: &str = concat!(
    page_shader!(),
    r#"
// Blur at the crease and in the distance, in logical pixels on the screen.
const float BLUR_BEND = 10.0;
const float BLUR_DEEP = 8.0;
// How much darker the page is turned away, and in the distance.
const float SHADE = 0.45;
const float FOG = 0.5;
// The light: the glint along the crease and the sheen ahead of it.
const float GLINT = 0.3;
const float SHEEN = 0.08;

vec4 page_at(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture2D(tex, uv);
}

void main() {
    vec2 at;
    float z;
    float th;
    float a;
    vec4 color = vec4(0.0);
    if (seen(v_coords * size, at, z, th, a)) {
        float far = smoothstep(0.0, shape.w, z);
        float radius = (BLUR_BEND * sin(th) + BLUR_DEEP * far) / lens(z);
        // Samples spread over a disc, a golden angle apart.
        for (int i = 0; i < 12; i++) {
            float f = float(i);
            float r = radius * sqrt((f + 0.5) / 12.0);
            float g = f * 2.39996;
            color += page_at(at + vec2(cos(g), sin(g)) * r);
        }
        color /= 12.0;
        color.rgb *= (1.0 - SHADE * (1.0 - cos(th))) * (1.0 - FOG * far);
        float s = a - fold;
        // Brightest where the bend has turned the glass a third of the way; nothing on the flat.
        float glint = smoothstep(0.0, 0.5, th) * (1.0 - smoothstep(0.5, 1.1, th));
        float ahead = (1.0 - smoothstep(0.0, sheen, max(-s, 0.0)))
            * (1.0 - smoothstep(0.0, shape.x * shape.y, max(s, 0.0)));
        color.rgb += vec3(0.85, 0.95, 1.0) * (GLINT * glint + SHEEN * ahead) * color.a;
        color *= 1.0 - smoothstep(shape.z, shape.w, z);
    }

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#
);

/// The screen with the wallpaper over the page, kept only where the page has gone behind the glass.
const THROUGH_SHADER: &str = concat!(
    page_shader!(),
    r#"
void main() {
    vec2 at;
    float z;
    float th;
    float a;
    // Off the page both orders look the same, so either will do.
    float behind = 1.0;
    if (seen(v_coords * size, at, z, th, a)) {
        behind = smoothstep(0.0, shape.z, z);
    }
    vec4 color = texture2D(tex, v_coords) * behind;

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#
);

/// Draws the UI into a texture and turns it away as the page.
#[derive(Default)]
pub struct Glass {
    program: Option<GlesTexProgram>,
    through_program: Option<GlesTexProgram>,
    /// A shader or texture failed, so the UI fades plainly instead.
    broken: bool,
    /// The UI, flat.
    texture: Option<(GlesTexture, Size<i32, Physical>)>,
    /// The screen with the wallpaper over the page.
    through: Option<(GlesTexture, Size<i32, Physical>)>,
}

impl Glass {
    /// Whether the page can be drawn, compiling the shaders the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.program.is_some() {
            return !self.broken;
        }
        let compiled = renderer
            .compile_custom_texture_shader(SHADER, &uniform_names())
            .and_then(|page| {
                renderer
                    .compile_custom_texture_shader(THROUGH_SHADER, &uniform_names())
                    .map(|through| (page, through))
            });
        match compiled {
            Ok((page, through)) => {
                self.program = Some(page);
                self.through_program = Some(through);
            }
            Err(err) => {
                tracing::warn!(
                    "the glass fade's shaders didn't compile, so the UI fades plainly: {err}"
                );
                self.broken = true;
            }
        }
        !self.broken
    }

    /// `elements`, the UI drawn at full strength, shown as the page `progress` of the way turned.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
        logical: Size<i32, Logical>,
        physical: Size<i32, Physical>,
        scale: f64,
        progress: f32,
    ) -> Option<TextureShaderElement> {
        if !self.ready(renderer) {
            return None;
        }
        tilt::draw_offscreen(
            renderer,
            &mut self.texture,
            &mut self.broken,
            elements,
            logical,
            physical,
            scale,
            "the glass fade",
        )?;
        let (texture, _) = self.texture.as_ref()?;
        Some(TextureShaderElement::new(
            whole_screen(renderer, texture, logical, physical),
            self.program.clone()?,
            uniforms(logical, progress),
        ))
    }

    /// The whole screen again with the wallpaper lying over the page instead of behind it, kept
    /// only where the page has turned behind the glass, to be shown over the page drawn by
    /// `element`. Both are drawn onto `background`, so where there's no page the two agree.
    /// `None` while no part of the page has reached the glass yet, or if it can't be drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn through(
        &mut self,
        renderer: &mut GlesRenderer,
        wallpaper: MemoryRenderBufferRenderElement<GlesRenderer>,
        logical: Size<i32, Logical>,
        physical: Size<i32, Physical>,
        scale: f64,
        progress: f32,
        background: Color32F,
    ) -> Option<TextureShaderElement> {
        let (w, h) = (logical.w as f32, logical.h as f32);
        if self.broken || crease(progress, w, h).0 >= reach(w, h) {
            return None;
        }
        let page = {
            let (texture, _) = self.texture.as_ref()?;
            TextureShaderElement::new(
                whole_screen(renderer, texture, logical, physical),
                self.program.clone()?,
                uniforms(logical, progress),
            )
        };
        let elements = [
            OutputElement::Memory(wallpaper),
            OutputElement::Shaded(page),
        ];
        let mut broken = false;
        let drawn = tilt::draw_offscreen_over(
            renderer,
            &mut self.through,
            &mut broken,
            &elements,
            logical,
            physical,
            scale,
            "the glass fade's crossing",
            background,
            None,
        );
        if broken {
            self.through = None;
        }
        Some(TextureShaderElement::new(
            drawn?,
            self.through_program.clone()?,
            uniforms(logical, progress),
        ))
    }
}

/// `texture`, drawn at the screen's scale, as an element covering the screen.
fn whole_screen(
    renderer: &GlesRenderer,
    texture: &GlesTexture,
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
) -> TextureRenderElement<GlesTexture> {
    TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture.clone(),
        1,
        Transform::Normal,
        None,
        Some(Rectangle::from_size(
            (physical.w as f64, physical.h as f64).into(),
        )),
        Some(logical),
        None,
        Kind::Unspecified,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1536.0;
    const H: f32 = 960.0;

    #[test]
    fn nothing_turns_or_shines_before_the_turn_starts() {
        let (at, push) = crease(0.0, W, H);
        assert_eq!(push, 0.0);
        assert!(at - SHEEN * W >= reach(W, H) - 0.01);
        let (_, depth, angle) = bend(reach(W, H), at, W);
        assert_eq!((depth, angle), (0.0, 0.0));
    }

    #[test]
    fn every_part_of_the_page_has_faded_by_the_end() {
        let (at, _) = crease(1.0, W, H);
        let far = FADE.1 * DISTANCE * W;
        for along in [-reach(W, H), 0.0, reach(W, H)] {
            let (_, depth, _) = bend(along, at, W);
            assert!(depth >= far - 0.5, "{along} is only {depth} deep");
        }
    }

    #[test]
    fn the_page_bends_smoothly_through_the_crease() {
        let at = 100.0;
        let r = RADIUS * W;
        for s in [0.0, r * TURN] {
            let (x0, z0, a0) = bend(at + s - 0.01, at, W);
            let (x1, z1, a1) = bend(at + s + 0.01, at, W);
            assert!((x1 - x0).abs() < 0.05 && (z1 - z0).abs() < 0.05 && (a1 - a0).abs() < 0.01);
        }
    }
}
