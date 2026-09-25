//! Bullet time's 3D. The overview is drawn flat into a texture, then shown leaning back like a
//! floor, in perspective, darker in the distance and faintly reflected below, by a small shader.
//! The same maths maps clicks on the screen back onto the flat overview.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Offscreen, Renderer,
            damage::OutputDamageTracker,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::{
                GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
                UniformValue, element::TextureShaderElement,
            },
        },
    },
    utils::{Buffer, Logical, Physical, Rectangle, Size, Transform},
};

use crate::render::OutputElement;

/// How the flat overview is turned in 3D and seen in perspective.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tilt {
    /// Leaning back, in radians: the top goes away.
    pub pitch: f64,
    /// Turning, in radians: positive sends the right-hand side away.
    pub yaw: f64,
    /// How far the eye is from the screen, in logical pixels. Nearer is stronger perspective.
    pub distance: f64,
    /// The point it turns about, the same on the flat overview and on the screen.
    pub centre: (f64, f64),
    /// How far down the screen the turned overview is moved.
    pub lift: f64,
    /// A fish-eye lens over it all, 0 for none: the middle bulges and the edges are squeezed.
    pub lens: f64,
}

impl Default for Tilt {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            yaw: 0.0,
            distance: 2000.0,
            centre: (0.0, 0.0),
            lift: 0.0,
            lens: 0.0,
        }
    }
}

impl Tilt {
    /// The homography from a flat point to the screen, both relative to the centre, row by row:
    /// CSS's `perspective(d) rotateX(pitch) rotateY(yaw)` applied to points on the z = 0 plane.
    fn matrix(&self) -> [f64; 9] {
        let (sa, ca) = self.pitch.sin_cos();
        let (sb, cb) = self.yaw.sin_cos();
        let d = self.distance;
        [cb, 0.0, 0.0, sa * sb, ca, 0.0, sb * ca / d, -sa / d, 1.0]
    }

    /// Where a point on the flat overview shows on the screen.
    pub fn project(&self, (x, y): (f64, f64)) -> Option<(f64, f64)> {
        let (cx, cy) = self.centre;
        let (sx, sy) = apply(&self.matrix(), x - cx, y - cy)?;
        let (ox, oy) = self.under_lens((sx, sy + self.lift));
        Some((cx + ox, cy + oy))
    }

    /// The point on the flat overview seen at a point on the screen.
    pub fn unproject(&self, (x, y): (f64, f64)) -> Option<(f64, f64)> {
        let (cx, cy) = self.centre;
        let (ox, oy) = self.through_lens((x - cx, y - cy));
        let (fx, fy) = apply(&invert(&self.matrix())?, ox, oy - self.lift)?;
        Some((cx + fx, cy + fy))
    }

    /// What the lens shows at `offset` from the centre of the screen, as an offset in the picture
    /// behind it. Half the screen's width is radius 1, and radius √½ keeps its size.
    fn through_lens(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let radius = self.centre.0.max(1.0);
        let k = self.lens;
        let stretch = (1.0 + k * (x * x + y * y) / (radius * radius)) / (1.0 + 0.5 * k);
        (x * stretch, y * stretch)
    }

    /// The inverse of `through_lens`: where the lens shows a point of the picture.
    fn under_lens(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let far = x.hypot(y);
        if self.lens == 0.0 || far == 0.0 {
            return (x, y);
        }
        let radius2 = self.centre.0.max(1.0).powi(2);
        let (k, norm) = (self.lens, 1.0 + 0.5 * self.lens);
        // Solve s·(1 + k·s²/r²)/norm = far by Newton's method; it's monotonic, so a few steps do.
        let mut s = far;
        for _ in 0..8 {
            let f = s * (1.0 + k * s * s / radius2) / norm - far;
            let slope = (1.0 + 3.0 * k * s * s / radius2) / norm;
            s -= f / slope;
        }
        (x * s / far, y * s / far)
    }
}

pub(crate) fn apply(m: &[f64; 9], x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[6] * x + m[7] * y + m[8];
    (w > 1e-9).then(|| {
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    })
}

pub(crate) fn invert(m: &[f64; 9]) -> Option<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let (ei_fh, fg_di, dh_eg) = (e * i - f * h, f * g - d * i, d * h - e * g);
    let det = a * ei_fh + b * fg_di + c * dh_eg;
    (det.abs() > 1e-12).then(|| {
        [
            ei_fh / det,
            (c * h - b * i) / det,
            (b * f - c * e) / det,
            fg_di / det,
            (a * i - c * g) / det,
            (c * d - a * f) / det,
            dh_eg / det,
            (b * g - a * h) / det,
            (a * e - b * d) / det,
        ]
    })
}

/// The look beyond the turn itself.
pub struct Look {
    /// How much darker the far edge is, 0 to 1.
    pub fog: f32,
    /// The reflection: the flat y it mirrors about, how far below that it shows, and how strong
    /// it is (0 for none).
    pub mirror: (f32, f32, f32),
}

/// Smithay's texture shader with the sampling replaced: each screen pixel is mapped back onto the
/// flat overview, four times a quarter-pixel apart so slanted edges come out smooth.
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

uniform vec2 size;
uniform vec2 centre;
uniform float lift;
uniform mat3 unproject;
uniform float pixel;
uniform float fog;
uniform vec3 mirror;
uniform float lens;

vec4 flat_at(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4(0.0);
    }
    return texture2D(tex, uv);
}

vec4 seen_at(vec2 screen) {
    vec2 offset = screen - centre;
    vec2 q = offset / centre.x;
    offset *= (1.0 + lens * dot(q, q)) / (1.0 + 0.5 * lens);
    vec3 p = unproject * vec3(offset - vec2(0.0, lift), 1.0);
    if (p.z <= 0.0) {
        return vec4(0.0);
    }
    vec2 at = centre + p.xy / p.z;
    vec4 colour = flat_at(at);
    float below = at.y - mirror.x;
    if (mirror.z > 0.0 && below > 0.0 && below < mirror.y) {
        float fade = 1.0 - below / mirror.y;
        colour += flat_at(vec2(at.x, mirror.x - below)) * mirror.z * fade * fade * (1.0 - colour.a);
    }
    float far = clamp(0.5 - (at.y - centre.y) / size.y, 0.0, 1.0);
    colour.rgb *= 1.0 - fog * far;
    return colour;
}

void main() {
    vec2 screen = v_coords * size;
    vec4 color = 0.25 * (
        seen_at(screen + pixel * vec2(0.125, 0.375)) +
        seen_at(screen + pixel * vec2(-0.375, 0.125)) +
        seen_at(screen + pixel * vec2(-0.125, -0.375)) +
        seen_at(screen + pixel * vec2(0.375, -0.125)));

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

/// Draws the overview into a texture and shows it tilted.
#[derive(Default)]
pub struct Stage {
    program: Option<GlesTexProgram>,
    /// The shader or texture failed, so the overview is drawn flat instead.
    broken: bool,
    texture: Option<(GlesTexture, Size<i32, Physical>)>,
}

impl Stage {
    /// Whether the overview can be tilted, compiling the shader the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.program.is_some() {
            return !self.broken;
        }
        let uniforms = [
            UniformName::new("size", UniformType::_2f),
            UniformName::new("centre", UniformType::_2f),
            UniformName::new("lift", UniformType::_1f),
            UniformName::new("unproject", UniformType::Matrix3x3),
            UniformName::new("pixel", UniformType::_1f),
            UniformName::new("fog", UniformType::_1f),
            UniformName::new("mirror", UniformType::_3f),
            UniformName::new("lens", UniformType::_1f),
        ];
        match renderer.compile_custom_texture_shader(SHADER, &uniforms) {
            Ok(program) => self.program = Some(program),
            Err(err) => {
                tracing::warn!("bullet time's 3D shader didn't compile, so it's flat: {err}");
                self.broken = true;
            }
        }
        !self.broken
    }

    /// `elements`, drawn flat `dense` times the screen's own resolution, then shown turned by
    /// `tilt`. The caller builds `elements` at `scale * dense` to match (see `supersample`).
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
        logical: Size<i32, Logical>,
        physical: Size<i32, Physical>,
        scale: f64,
        dense: f64,
        tilt: &Tilt,
        look: &Look,
    ) -> Option<TextureShaderElement> {
        if !self.ready(renderer) {
            return None;
        }
        let sampled = Size::<i32, Physical>::from((
            (physical.w as f64 * dense).round() as i32,
            (physical.h as f64 * dense).round() as i32,
        ));
        let inner = draw_offscreen(
            renderer,
            &mut self.texture,
            &mut self.broken,
            elements,
            logical,
            sampled,
            scale * dense,
            "bullet time's 3D",
        )?;
        // GLSL takes matrices column by column.
        let m = invert(&tilt.matrix())?.map(|v| v as f32);
        let columns = [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]];
        let uniforms = vec![
            Uniform::new("size", (logical.w as f32, logical.h as f32)),
            Uniform::new("centre", (tilt.centre.0 as f32, tilt.centre.1 as f32)),
            Uniform::new("lift", tilt.lift as f32),
            Uniform::new(
                "unproject",
                UniformValue::Matrix3x3 {
                    matrices: vec![columns],
                    transpose: false,
                },
            ),
            Uniform::new("pixel", (1.0 / scale) as f32),
            Uniform::new("fog", look.fog),
            Uniform::new("mirror", look.mirror),
            Uniform::new("lens", tilt.lens as f32),
        ];
        Some(TextureShaderElement::new(
            inner,
            self.program.clone()?,
            uniforms,
        ))
    }
}

/// How much denser than the screen the flat overview is drawn, given the screen's own size in
/// pixels.
///
/// Text passes through two resamplings on its way to the screen: a window is drawn zoomed out into
/// the flat overview, and the shader then samples that through the perspective warp. At the
/// screen's own resolution both steps throw detail away, and the result reads as blurred. Drawing
/// the overview denser keeps it: the zoomed-out window still lands on more texels than its buffer
/// has, and the shader's four taps average down from that rather than guessing between pixels.
///
/// Twice over is enough; the steps down are for screens large enough that four times the pixels
/// would cost more fill rate and memory than the sharpness is worth (a 4K screen at 2x would be 33
/// megapixels, 133 MB).
pub fn supersample(physical: Size<i32, Physical>) -> f64 {
    match (physical.w as i64 * physical.h as i64).max(0) {
        0..=4_000_000 => 2.0,
        4_000_001..=9_000_000 => 1.5,
        _ => 1.0,
    }
}

/// Draws `elements` into `texture` (made, or made again at a new size, as needed) and returns it
/// as an element covering the screen, for a shader to show. A texture that can't be made sets
/// `broken`, so the effect gives way to plain drawing. `what` names the effect in the log.
#[allow(clippy::too_many_arguments)]
pub fn draw_offscreen(
    renderer: &mut GlesRenderer,
    texture: &mut Option<(GlesTexture, Size<i32, Physical>)>,
    broken: &mut bool,
    elements: &[OutputElement],
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
    scale: f64,
    what: &str,
) -> Option<TextureRenderElement<GlesTexture>> {
    draw_offscreen_over(
        renderer,
        texture,
        broken,
        elements,
        logical,
        physical,
        scale,
        what,
        Color32F::new(0.0, 0.0, 0.0, 0.0),
        None,
    )
}

/// `draw_offscreen` onto `clear` rather than nothing, the result shown at `alpha`.
#[allow(clippy::too_many_arguments)]
pub fn draw_offscreen_over(
    renderer: &mut GlesRenderer,
    texture: &mut Option<(GlesTexture, Size<i32, Physical>)>,
    broken: &mut bool,
    elements: &[OutputElement],
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
    scale: f64,
    what: &str,
    clear: Color32F,
    alpha: Option<f32>,
) -> Option<TextureRenderElement<GlesTexture>> {
    if texture.as_ref().is_none_or(|(_, size)| *size != physical) {
        let size = Size::<i32, Buffer>::from((physical.w, physical.h));
        match renderer.create_buffer(Fourcc::Abgr8888, size) {
            Ok(made) => *texture = Some((made, physical)),
            Err(err) => {
                tracing::warn!("no texture for {what}, so it's drawn plainly: {err}");
                *broken = true;
                return None;
            }
        }
    }
    let (texture, _) = texture.as_mut()?;
    {
        let mut target = renderer.bind(texture).ok()?;
        let mut damage = OutputDamageTracker::new(physical, scale, Transform::Normal);
        if let Err(err) = damage.render_output(renderer, &mut target, 0, elements, clear) {
            tracing::warn!("couldn't draw {what}: {err:?}");
            return None;
        }
    }
    Some(TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture.clone(),
        1,
        Transform::Normal,
        alpha,
        // The whole texture, which is drawn at the output's scale.
        Some(Rectangle::from_size(
            (physical.w as f64, physical.h as f64).into(),
        )),
        Some(logical),
        None,
        Kind::Unspecified,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tilt() -> Tilt {
        Tilt {
            pitch: 15f64.to_radians(),
            yaw: 4f64.to_radians(),
            distance: 1840.0,
            centre: (800.0, 500.0),
            lift: 27.0,
            lens: 0.14,
        }
    }

    #[test]
    fn the_lens_bulges_the_middle_and_squeezes_the_edges() {
        let lens = Tilt {
            centre: (800.0, 500.0),
            lens: 0.14,
            ..Tilt::default()
        };
        assert!(lens.project((810.0, 500.0)).unwrap().0 > 810.0);
        assert!(lens.project((1590.0, 500.0)).unwrap().0 < 1590.0);
    }

    #[test]
    fn clicks_map_back_to_where_things_were_drawn() {
        let tilt = tilt();
        for point in [(0.0, 0.0), (800.0, 500.0), (1500.0, 120.0), (300.0, 950.0)] {
            let shown = tilt.project(point).unwrap();
            let back = tilt.unproject(shown).unwrap();
            assert!(
                (back.0 - point.0).abs() < 1e-6 && (back.1 - point.1).abs() < 1e-6,
                "{point:?} came back as {back:?}"
            );
        }
    }

    #[test]
    fn leaning_back_makes_the_far_edge_narrower() {
        let tilt = Tilt {
            yaw: 0.0,
            lens: 0.0,
            ..tilt()
        };
        let width_at =
            |y| tilt.project((1400.0, y)).unwrap().0 - tilt.project((200.0, y)).unwrap().0;
        assert!(width_at(100.0) < 1200.0);
        assert!(width_at(900.0) > 1200.0);
    }

    #[test]
    fn turning_sends_the_right_side_away() {
        let tilt = Tilt {
            pitch: 0.0,
            lens: 0.0,
            ..tilt()
        };
        let height_at =
            |x| tilt.project((x, 900.0)).unwrap().1 - tilt.project((x, 100.0)).unwrap().1;
        assert!(height_at(1400.0) < height_at(200.0));
    }

    #[test]
    fn flat_changes_nothing() {
        let flat = Tilt {
            centre: (800.0, 500.0),
            ..Tilt::default()
        };
        assert_eq!(flat.project((10.0, 20.0)), Some((10.0, 20.0)));
        assert_eq!(flat.unproject((10.0, 20.0)), Some((10.0, 20.0)));
    }
}
