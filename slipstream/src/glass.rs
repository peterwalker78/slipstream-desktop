//! The glass fade. While the UI fades to the living wallpaper, or back, it's drawn whole into a
//! texture and shown as a pane of glass falling away into the screen, in perspective, tipping back
//! a little, darkening and going out of focus with distance. The wallpaper stays on the glass at
//! the front, so the two pass through each other: at first the UI is in front of the wallpaper,
//! and as it falls the wallpaper's light comes to lie over it instead. Nothing is drawn over the
//! crossing itself. Coming back, the same fall runs in reverse.

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

/// Smithay's texture shader with the sampling replaced by the falling pane's.
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

// The screen in logical pixels, and how far the pane has gone: 0 at the front, 1 gone.
uniform vec2 size;
uniform float progress;

// How far the eye is from the screen, in screen widths.
const float DISTANCE = 1.25;
// How far back the pane falls, in the eye's distances: at 0.9 it ends a little over half size.
const float PUSH = 0.9;
// How far it tips back as it falls, top away, in radians (about 12 degrees).
const float PITCH = 0.21;
// Out of focus by the time it's gone: the blur's radius on the screen, in logical pixels.
const float BLUR = 9.0;
// How much darker it is at the back.
const float FOG = 0.6;

vec4 pane_at(vec2 uv) {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture2D(tex, uv);
}

// The point of the flat pane seen at `offset` from the screen's centre, once it has fallen
// `depth` eye distances back and tipped `tip` radians: a plane pushed back and turned about its
// middle row, seen in perspective.
vec2 flat_at(vec2 offset, vec2 centre, float d, float depth, float tip) {
    float w = (1.0 + depth) / (1.0 + offset.y * tan(tip) / d);
    return (centre + vec2(offset.x * w, offset.y * w / cos(tip))) / size;
}

void main() {
    vec2 centre = size * 0.5;
    float d = DISTANCE * size.x;
    float depth = PUSH * progress;
    float tip = PITCH * progress;
    vec2 offset = v_coords * size - centre;

    // Samples spread over a disc on the screen, a golden angle apart, each followed back onto the
    // pane: the blur grows with distance, and a fraction of a pixel of it smooths the shrunken
    // text and the slanted edges from the start.
    float radius = BLUR * progress + 0.45 * min(progress * 12.0, 1.0);
    vec4 color = vec4(0.0);
    for (int i = 0; i < 16; i++) {
        float f = float(i);
        float r = radius * sqrt((f + 0.5) / 16.0);
        float a = f * 2.39996;
        color += pane_at(flat_at(offset + vec2(cos(a), sin(a)) * r, centre, d, depth, tip));
    }
    color /= 16.0;
    color.rgb *= 1.0 - FOG * progress;
    // It goes out as it reaches the back, not before.
    color *= 1.0 - smoothstep(0.45, 1.0, progress);

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

/// How far through the wallpaper the pane has fallen, 0 in front of it to 1 behind it, `progress`
/// of the way gone. The crossing comes early, once the fall is clearly under way.
pub fn behind(progress: f32) -> f32 {
    let t = ((progress - 0.08) / (0.45 - 0.08)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Draws the UI into a texture and shows it as the falling pane.
#[derive(Default)]
pub struct Glass {
    program: Option<GlesTexProgram>,
    /// The shader or texture failed, so the UI fades plainly instead.
    broken: bool,
    /// The UI, flat.
    texture: Option<(GlesTexture, Size<i32, Physical>)>,
    /// The wallpaper lying over the fallen pane.
    through: Option<(GlesTexture, Size<i32, Physical>)>,
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
        self.pane(renderer, logical, physical, progress)
    }

    /// The pane as last drawn by `element`, fallen `progress` of the way.
    fn pane(
        &self,
        renderer: &GlesRenderer,
        logical: Size<i32, Logical>,
        physical: Size<i32, Physical>,
        progress: f32,
    ) -> Option<TextureShaderElement> {
        let (texture, _) = self.texture.as_ref()?;
        let inner = TextureRenderElement::from_static_texture(
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
        );
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

    /// The whole screen with the wallpaper lying over the fallen pane rather than behind it, to be
    /// shown over the pane drawn by `element` at `behind(progress)`, so the screen mixes from one
    /// order to the other. Both are drawn onto `background`, so where there's no pane the two
    /// agree and nothing changes. `None` before the crossing starts, or if it can't be drawn.
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
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let mix = behind(progress);
        if self.broken || mix <= 0.0 {
            return None;
        }
        let pane = self.pane(renderer, logical, physical, progress)?;
        let elements = [
            OutputElement::Memory(wallpaper),
            OutputElement::Shaded(pane),
        ];
        let mut broken = false;
        let element = tilt::draw_offscreen_over(
            renderer,
            &mut self.through,
            &mut broken,
            &elements,
            logical,
            physical,
            scale,
            "the glass fade's crossing",
            background,
            Some(mix),
        );
        if broken {
            self.through = None;
        }
        element
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pane_crosses_the_wallpaper_early_in_the_fall() {
        assert_eq!(behind(0.0), 0.0);
        assert_eq!(behind(0.08), 0.0);
        assert!((behind(0.265) - 0.5).abs() < 1e-3);
        assert_eq!(behind(0.45), 1.0);
        assert_eq!(behind(1.0), 1.0);
    }
}
