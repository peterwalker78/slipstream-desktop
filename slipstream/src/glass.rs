//! The glass fade. While the UI fades to the living wallpaper, or back, it's drawn whole into a
//! texture and shown as a pane of glass passing through the wallpaper's, the way a foldable
//! phone's two screens dissolve through each other. A soft edge sweeps diagonally across the
//! screen: along it the pane blurs, bends and catches the light, and behind it the pane is gone.
//! The whole pane drifts a little towards you as it goes.

use smithay::{
    backend::renderer::gles::{
        GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
        element::TextureShaderElement,
    },
    utils::{Logical, Physical, Size},
};

use crate::{render::OutputElement, tilt};

/// Smithay's texture shader with the sampling replaced by the pane's.
const SHADER: &str = r#"#version 100

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

// The screen in logical pixels, and how far the pane has gone: 0 whole, 1 gone.
uniform vec2 size;
uniform float progress;

// Half the soft edge's width, as a share of the way across the screen. It's wide: the two
// worlds should be mixed across a third of the screen as the pane passes, not divided by a line.
const float EDGE = 0.34;
// How far the edge wanders off its diagonal, in the same share of the screen, so the two worlds
// interleave along it rather than meeting on a ruled line.
const float RIPPLE = 0.055;
// At the edge: the blur's radius, and how far the picture is pulled along, in logical pixels.
const float BLUR = 14.0;
const float BEND = 22.0;
// How much bigger the pane has grown by the time it's gone.
const float DRIFT = 0.035;

vec4 pane_at(vec2 uv) {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture2D(tex, uv);
}

void main() {
    // How far across the screen this point is, from its top left corner to its bottom right.
    vec2 diagonal = normalize(size);
    float across = dot(v_coords * size, diagonal) / dot(size, diagonal);
    // Along the edge, not across it: two waves out of step, so the boundary is ragged rather
    // than ruled.
    float along = dot(v_coords * size, vec2(-diagonal.y, diagonal.x)) / dot(size, diagonal);
    across += RIPPLE * (sin(along * 7.3) * 0.6 + sin(along * 17.9 + 1.7) * 0.4);
    float front = mix(-EDGE, 1.0 + EDGE, progress);
    float edge = clamp(1.0 - abs(across - front) / EDGE, 0.0, 1.0);
    edge = edge * edge * (3.0 - 2.0 * edge);
    float remaining = smoothstep(front - EDGE, front + EDGE, across);

    vec2 uv = (v_coords - 0.5) / (1.0 + DRIFT * progress) + 0.5;
    uv -= diagonal * BEND * edge * edge / size;
    float radius = BLUR * edge + 4.0 * progress;
    vec4 color = vec4(0.0);
    // Samples spread over a disc, a golden angle apart: enough that thin lines blur rather than double.
    for (int i = 0; i < 24; i++) {
        float f = float(i);
        float r = radius * sqrt((f + 0.5) / 24.0);
        float a = f * 2.39996;
        color += pane_at(uv + vec2(cos(a), sin(a)) * r / size);
    }
    color /= 24.0;
    // The light caught along the edge, on the glass only.
    color.rgb += vec3(0.85, 0.95, 1.0) * 0.12 * edge * color.a;
    color *= remaining;

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
"#;

/// Draws the UI into a texture and shows it as the glass pane.
#[derive(Default)]
pub struct Glass {
    program: Option<GlesTexProgram>,
    /// The shader or texture failed, so the UI fades plainly instead.
    broken: bool,
    texture: Option<(GlesTexture, Size<i32, Physical>)>,
}

impl Glass {
    /// Whether the pane can be drawn, compiling the shader the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.program.is_some() {
            return !self.broken;
        }
        let uniforms = [
            UniformName::new("size", UniformType::_2f),
            UniformName::new("progress", UniformType::_1f),
        ];
        match renderer.compile_custom_texture_shader(SHADER, &uniforms) {
            Ok(program) => self.program = Some(program),
            Err(err) => {
                tracing::warn!(
                    "the glass fade's shader didn't compile, so the UI fades plainly: {err}"
                );
                self.broken = true;
            }
        }
        !self.broken
    }

    /// `elements`, the UI drawn at full strength, shown as the pane `progress` of the way gone.
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
        let inner = tilt::draw_offscreen(
            renderer,
            &mut self.texture,
            &mut self.broken,
            elements,
            logical,
            physical,
            scale,
            "the glass fade",
        )?;
        let uniforms = vec![
            Uniform::new("size", (logical.w as f32, logical.h as f32)),
            Uniform::new("progress", progress),
        ];
        Some(TextureShaderElement::new(
            inner,
            self.program.clone()?,
            uniforms,
        ))
    }
}
