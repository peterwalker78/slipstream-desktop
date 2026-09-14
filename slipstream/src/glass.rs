//! The glass fade. The UI and the living wallpaper are two panes of glass, the wallpaper's a little
//! way behind. While the UI fades to the wallpaper, or back, it's drawn whole into a texture and
//! shown as its pane tipping back, rigidly, about its bottom right corner, and sinking away. Where
//! it passes through the wallpaper's pane the two meet along a line, and that line sweeps
//! diagonally across the screen from the top left: along it the glass blurs and catches the
//! light, and past it the UI is behind the wallpaper, darkening and falling away
//! under the wallpaper's light. Coming back, the same fall runs in reverse.

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

/// How far the eye is from the screen, in screen widths.
const DISTANCE: f32 = 1.25;
/// How far behind the UI's pane the wallpaper's is, in the eye's distances.
const GLASS: f32 = 0.15;
/// How far the UI's pane has tipped back by the end, in radians (35 degrees).
const TILT: f32 = 0.61;
/// How far its corner has sunk back by the end, in the eye's distances: it ends at five sixths
/// of its size there.
const PUSH: f32 = 0.2;
/// Half the depth over which the two panes mix where they meet, in the eye's distances. It's
/// deep: across the screen the two worlds should mix over a wide band, not meet on a line.
const EDGE: f32 = 0.08;
/// The depths between which the UI fades out, in the eye's distances.
const FADE: (f32, f32) = (0.25, 0.45);
/// The last stretch of the fade, over which whatever is left of the UI goes out.
const LAST: f32 = 0.8;

/// How far the UI's pane has tipped back and how deep its corner is, in logical pixels,
/// `progress` of the way through the fade. Tipping and sinking go together, as one movement.
fn pose(progress: f32, width: f32) -> (f32, f32) {
    let p = progress.clamp(0.0, 1.0);
    (TILT * p, PUSH * DISTANCE * width * p)
}

/// How much of the UI is left over the last stretch of the fade, `progress` of the way through.
fn remaining(progress: f32) -> f32 {
    let t = ((progress - LAST) / (1.0 - LAST)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Where the two panes meet, as a share of the way from the UI pane's bottom right corner (0) to
/// its top left (1), `progress` of the way through; `None` before it tips at all.
#[cfg(test)]
fn meeting(progress: f32, width: f32, height: f32) -> Option<f32> {
    let (tilt, push) = pose(progress, width);
    (tilt > 0.0).then(|| (GLASS * DISTANCE * width - push) / tilt.sin() / width.hypot(height))
}

fn uniforms(logical: Size<i32, Logical>, progress: f32) -> Vec<Uniform<'static>> {
    let (w, h) = (logical.w as f32, logical.h as f32);
    let (tilt, push) = pose(progress, w);
    let distance = DISTANCE * w;
    vec![
        Uniform::new("size", (w, h)),
        Uniform::new("tilt", (tilt.sin(), tilt.cos())),
        Uniform::new("push", push),
        Uniform::new("distance", distance),
        Uniform::new("glass", (GLASS * distance, EDGE * distance)),
        Uniform::new("fade", (FADE.0 * distance, FADE.1 * distance)),
        Uniform::new("remaining", remaining(progress)),
    ]
}

fn uniform_names() -> [UniformName<'static>; 7] {
    [
        UniformName::new("size", UniformType::_2f),
        UniformName::new("tilt", UniformType::_2f),
        UniformName::new("push", UniformType::_1f),
        UniformName::new("distance", UniformType::_1f),
        UniformName::new("glass", UniformType::_2f),
        UniformName::new("fade", UniformType::_2f),
        UniformName::new("remaining", UniformType::_1f),
    ]
}

/// Smithay's texture shader's preamble, and the pane's pose: which point of the UI is seen at a
/// point on the screen, and how deep it is.
macro_rules! pane_shader {
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

// The screen in logical pixels; the sine and cosine of how far the UI's pane has tipped back; how
// deep its bottom right corner is; the eye's distance; the wallpaper pane's depth, and half the
// depth the two mix over; the depths the UI fades out between; how much of it is left at the end.
uniform vec2 size;
uniform vec2 tilt;
uniform float push;
uniform float distance;
uniform vec2 glass;
uniform vec2 fade;
uniform float remaining;

// How far the edge between the two wanders, as a share of the way across the screen, so the two
// worlds interleave along it rather than meeting on a ruled line.
const float RIPPLE = 0.05;

// The point of the UI seen at `screen`, how deep it is, and how deep it counts as against the
// wallpaper's pane, ripple and all. The pane turns about the line through its bottom right corner
// square to the diagonal; each ray from the eye meets it once, so this is solved outright.
bool seen(vec2 screen, out vec2 at, out float z, out float against) {
    vec2 n = normalize(size);
    vec2 across = vec2(-n.y, n.x);
    vec2 o = screen - size * 0.5;
    float sa = dot(o, n);
    float sb = dot(o, across);
    float h = dot(abs(n), size * 0.5);
    float den = sa * tilt.x + distance * tilt.y;
    if (den <= 0.0) {
        return false;
    }
    // How far the point is from the corner's line, along the diagonal.
    float u = (h * distance - sa * (distance + push)) / den;
    z = push + u * tilt.x;
    float b = sb * (distance + z) / distance;
    at = size * 0.5 + (h - u) * n + b * across;
    float span = 2.0 * h;
    float wave = RIPPLE * span * (sin(b / span * 7.3) * 0.6 + sin(b / span * 17.9 + 1.7) * 0.4);
    against = z + wave * tilt.x;
    return at.x >= 0.0 && at.y >= 0.0 && at.x <= size.x && at.y <= size.y;
}
"#
    };
}

/// The UI's pane.
const SHADER: &str = concat!(
    pane_shader!(),
    r#"
// Where the panes meet: the blur's radius, in logical pixels. The picture isn't pulled aside there:
// the meeting line sweeps across it as the pane moves, and a pull that comes and goes with the line
// makes the picture stall and then lurch.
const float BLUR = 14.0;
// How much light catches there.
const float LIGHT = 0.2;
// Blur and darkening in the distance.
const float BLUR_DEEP = 6.0;
const float FOG = 0.45;

vec4 pane_at(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture2D(tex, uv);
}

void main() {
    vec2 at;
    float z;
    float against;
    vec4 color = vec4(0.0);
    if (seen(v_coords * size, at, z, against)) {
        float edge = clamp(1.0 - abs(against - glass.x) / glass.y, 0.0, 1.0);
        edge = edge * edge * (3.0 - 2.0 * edge);
        float far = smoothstep(glass.x, fade.y, z);
        float radius = BLUR * edge + BLUR_DEEP * far;
        // Samples spread over a disc, a golden angle apart: enough that thin lines blur rather
        // than double.
        for (int i = 0; i < 24; i++) {
            float f = float(i);
            float r = radius * sqrt((f + 0.5) / 24.0);
            float a = f * 2.39996;
            color += pane_at(at + vec2(cos(a), sin(a)) * r);
        }
        color /= 24.0;
        color.rgb += vec3(0.85, 0.95, 1.0) * LIGHT * edge * color.a;
        color.rgb *= 1.0 - FOG * far;
        color *= (1.0 - smoothstep(fade.x, fade.y, z)) * remaining;
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

/// The screen with the wallpaper over the UI, kept only where the UI has gone behind its pane.
const THROUGH_SHADER: &str = concat!(
    pane_shader!(),
    r#"
void main() {
    vec2 at;
    float z;
    float against;
    // Off the UI's pane both orders look the same, so either will do.
    float behind = 1.0;
    if (seen(v_coords * size, at, z, against)) {
        behind = smoothstep(glass.x - glass.y, glass.x + glass.y, against);
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

/// Draws the UI into a texture and shows it as its pane of glass.
#[derive(Default)]
pub struct Glass {
    program: Option<GlesTexProgram>,
    through_program: Option<GlesTexProgram>,
    /// A shader or texture failed, so the UI fades plainly instead.
    broken: bool,
    /// The UI, flat.
    texture: Option<(GlesTexture, Size<i32, Physical>)>,
    /// The screen with the wallpaper over the UI.
    through: Option<(GlesTexture, Size<i32, Physical>)>,
}

impl Glass {
    /// Whether the pane can be drawn, compiling the shaders the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.program.is_some() {
            return !self.broken;
        }
        let compiled = renderer
            .compile_custom_texture_shader(SHADER, &uniform_names())
            .and_then(|pane| {
                renderer
                    .compile_custom_texture_shader(THROUGH_SHADER, &uniform_names())
                    .map(|through| (pane, through))
            });
        match compiled {
            Ok((pane, through)) => {
                self.program = Some(pane);
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

    /// `elements`, the UI drawn at full strength, shown as its pane `progress` of the way gone.
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

    /// The whole screen again with the wallpaper lying over the UI instead of behind it, kept only
    /// where the UI has gone behind the wallpaper's pane, to be shown over the pane drawn by
    /// `element`. Both are drawn onto `background`, so where there's no UI the two agree.
    /// `None` if it can't be drawn.
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
        if self.broken {
            return None;
        }
        let pane = {
            let (texture, _) = self.texture.as_ref()?;
            TextureShaderElement::new(
                whole_screen(renderer, texture, logical, physical),
                self.program.clone()?,
                uniforms(logical, progress),
            )
        };
        let elements = [
            OutputElement::Memory(wallpaper),
            OutputElement::Shaded(pane),
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
    fn the_pane_starts_flat_where_the_ui_is() {
        assert_eq!(pose(0.0, W), (0.0, 0.0));
        assert_eq!(meeting(0.0, W, H), None);
    }

    #[test]
    fn the_meeting_line_crosses_the_screen_in_the_middle_of_the_fade() {
        let crossing = |p: f32| meeting(p, W, H).unwrap();
        assert!(crossing(0.1) > 1.0, "not on the screen yet");
        assert!(crossing(0.3) < 1.0 && crossing(0.3) > 0.5);
        assert!(crossing(0.5) < 0.5 && crossing(0.5) > 0.0);
        assert!(crossing(0.8) < 0.0, "gone past the corner");
        let mut last = crossing(0.05);
        for i in 2..=20 {
            let now = crossing(i as f32 * 0.05);
            assert!(now < last, "it only ever moves one way");
            last = now;
        }
    }

    #[test]
    fn the_whole_pane_has_faded_by_the_end() {
        assert_eq!(remaining(0.5), 1.0);
        assert_eq!(remaining(1.0), 0.0);
    }

    #[test]
    fn tipping_and_sinking_keep_pace() {
        let (tilt_end, push_end) = pose(1.0, W);
        for p in [0.1, 0.25, 0.5, 0.75] {
            let (tilt, push) = pose(p, W);
            assert!((tilt / tilt_end - push / push_end).abs() < 1e-4);
        }
    }
}
