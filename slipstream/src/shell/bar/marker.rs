//! The current workspace's marker, drawn over the bar's picture rather than into it: a pill in
//! the focus ring's colour, with the numbers under it lit. The lit numbers are painted once with
//! the bar, into a strip as tall as the pill and as long as the workspaces; a shader draws the
//! pill and shows that strip through it wherever the marker is. A slide moves the shader's
//! numbers alone, so the bar's picture is neither repainted nor uploaded while it does.

use resvg::tiny_skia::{Pixmap, PixmapPaint, Transform};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    utils::{Logical, Physical, Point, Rectangle},
};

use crate::effects::shader::{self, Shaded, Values};

/// Where the pill is drawn down the bar, and how tall, in the bar's units.
pub const TOP: f32 = 7.0;
pub const TALL: f32 = 26.0;
/// How round its corners are, and how deep the band along its foot, in the bar's units.
const RADIUS: f32 = 6.0;
const BAND: f32 = 2.0;
/// The pill's fill over the ring's colour: stronger on the screen with the keyboard, which alone
/// has the band.
const FILL_HERE: u32 = 0x26;
const FILL_ELSEWHERE: u32 = 0x14;

/// The pill over the lit numbers: everything in screen pixels from the strip's corner.
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

// The strip's size.
uniform vec2 size;
// The pill's left, right, top and bottom edges.
uniform vec4 pill;
// Its corners' radius, the band's depth, the fill's alpha and the band's.
uniform vec4 look;
uniform vec3 ring;

void main() {
    vec2 p = v_coords * size;
    vec2 middle = 0.5 * vec2(pill.x + pill.y, pill.z + pill.w);
    vec2 half_size = 0.5 * vec2(pill.y - pill.x, pill.w - pill.z);
    float radius = min(look.x, min(half_size.x, half_size.y));
    vec2 q = abs(p - middle) - half_size + radius;
    float outside = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    float inside = clamp(0.5 - outside, 0.0, 1.0);
    float band = clamp(p.y - (pill.w - look.y) + 0.5, 0.0, 1.0) * look.w;
    float cover = inside * (band + (1.0 - band) * look.z);
    vec4 fill = vec4(ring * cover, cover);

    // The numbers are lit only between the pill's ends.
    float lit = clamp(p.x - pill.x + 0.5, 0.0, 1.0) * clamp(pill.y - p.x + 0.5, 0.0, 1.0);
    vec4 numbers = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    numbers = vec4(numbers.rgb, 1.0);
#endif
    numbers *= lit;
    vec4 color = (numbers + fill * (1.0 - numbers.a)) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// Marks the marker's program in a renderer's keeping.
struct Program;

/// The shader's numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    size: [f32; 2],
    pill: [f32; 4],
    look: [f32; 4],
    ring: [f32; 3],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("size", self.size)
            .with("pill", self.pill)
            .with("look", self.look)
            .with("ring", self.ring)
    }
}

/// The lit numbers as painted with the bar, for a texture still to be made from them.
pub struct Numbers {
    /// The whole bar's size, with only the lit numbers on it.
    pub pixmap: Pixmap,
    /// The workspaces' left and right ends, in the bar's units.
    pub span: (f32, f32),
    /// Screen pixels to one of the bar's units.
    pub f: f32,
    pub ring: u32,
    pub keyboard_here: bool,
}

/// The strip of lit numbers on the screen, and what the pill over it looks like.
struct Strip {
    texture: GlesTexture,
    place: Place,
}

/// Where the strip is and how the pill on it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    /// Where the strip is, in logical pixels, and its size in screen pixels.
    area: Rectangle<i32, Logical>,
    size: [f32; 2],
    /// Its corner in the bar picture's pixels.
    corner: (f32, f32),
    /// Screen pixels to one of the bar's units.
    f: f32,
    ring: [f32; 3],
    keyboard_here: bool,
}

/// One bar's marker.
#[derive(Default)]
pub struct Marker {
    strip: Option<Strip>,
    kept: shader::Kept,
}

impl Marker {
    /// Takes the lit numbers of a bar just painted at `bar` (its corner on the screen, logical
    /// pixels) at `scale`, as a strip just big enough for the pill. New numbers are a change to
    /// draw even where the pill's own numbers stay the same.
    pub fn repainted(
        &mut self,
        renderer: &mut GlesRenderer,
        numbers: Numbers,
        bar: Point<i32, Logical>,
        scale: f64,
    ) {
        self.strip = strip(renderer, numbers, bar, scale);
        self.kept.changed();
    }

    /// The marker with its edges at `edges` in the bar's units, at `alpha`. `None` without a
    /// strip, or if the shader didn't compile.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        edges: (f32, f32),
        alpha: f32,
    ) -> Option<Shaded> {
        let strip = self.strip.as_ref()?;
        let program = shader::program::<Program, _>(
            renderer,
            |renderer| renderer.compile_custom_texture_shader(SHADER, &shader::names::<Uniforms>()),
            "the workspace marker's",
        )?;
        Some(self.kept.texture(
            (program, strip.texture.clone()),
            strip.place.area,
            alpha,
            &uniforms(&strip.place, edges),
        ))
    }
}

/// The pill with its edges at `edges` in the bar's units, over the strip at `place`.
fn uniforms(place: &Place, (left, right): (f32, f32)) -> Uniforms {
    let ((x, y), f) = (place.corner, place.f);
    let (fill, band) = if place.keyboard_here {
        (FILL_HERE, 1.0)
    } else {
        (FILL_ELSEWHERE, 0.0)
    };
    Uniforms {
        size: place.size,
        pill: [
            left * f - x,
            right * f - x,
            TOP * f - y,
            (TOP + TALL) * f - y,
        ],
        look: [RADIUS * f, BAND * f, fill as f32 / 255.0, band],
        ring: place.ring,
    }
}

/// The strip of `numbers` for a bar at `bar` at `scale`, uploaded: the logical pixels round the
/// workspaces and the pill's rows, one texel for each screen pixel they cover.
fn strip(
    renderer: &mut GlesRenderer,
    numbers: Numbers,
    bar: Point<i32, Logical>,
    scale: f64,
) -> Option<Strip> {
    let Numbers {
        pixmap,
        span: (left, right),
        f,
        ring,
        keyboard_here,
    } = numbers;
    let picture = bar.to_f64().to_physical(scale).to_i32_round::<i32>();
    // Logical pixels round the pill's reach, a unit spare at each end.
    let to_logical = |units: f32, physical: i32| (physical as f64 + (units * f) as f64) / scale;
    let x0 = to_logical(left - 1.0, picture.x).floor() as i32;
    let x1 = to_logical(right + 1.0, picture.x).ceil() as i32;
    let y0 = to_logical(TOP - 1.0, picture.y).floor() as i32;
    let y1 = to_logical(TOP + TALL + 1.0, picture.y).ceil() as i32;
    let area = Rectangle::<i32, Logical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into());
    let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
    let mut crop = Pixmap::new(screen.size.w as u32, screen.size.h as u32)?;
    let (dx, dy) = (screen.loc.x - picture.x, screen.loc.y - picture.y);
    crop.draw_pixmap(
        -dx,
        -dy,
        pixmap.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    let texture = renderer
        .import_memory(
            crop.data(),
            Fourcc::Abgr8888,
            (screen.size.w, screen.size.h).into(),
            false,
        )
        .map_err(|err| tracing::warn!("couldn't upload the marker's numbers: {err}"))
        .ok()?;
    let rgb = |c: u32| {
        [
            (c >> 24) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
        ]
    };
    Some(Strip {
        texture,
        place: Place {
            area,
            size: [screen.size.w as f32, screen.size.h as f32],
            corner: (dx as f32, dy as f32),
            f,
            ring: rgb(ring),
            keyboard_here,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(keyboard_here: bool) -> Place {
        Place {
            area: Rectangle::new((40, 5).into(), (160, 22).into()),
            size: [200.0, 28.0],
            corner: (50.0, 6.0),
            f: 1.0,
            ring: [0.0, 0.8, 1.0],
            keyboard_here,
        }
    }

    #[test]
    fn only_the_screen_with_the_keyboard_has_the_band_and_the_stronger_fill() {
        let here = uniforms(&place(true), (60.0, 90.0));
        let elsewhere = uniforms(&place(false), (60.0, 90.0));
        assert_eq!(here.look[3], 1.0);
        assert_eq!(elsewhere.look[3], 0.0);
        assert!(here.look[2] > elsewhere.look[2]);
    }

    #[test]
    fn the_pill_is_placed_from_the_strips_corner_and_a_slide_moves_only_its_ends() {
        let u = uniforms(&place(true), (60.0, 90.0));
        assert_eq!(u.pill, [10.0, 40.0, 1.0, 27.0]);
        let moved = uniforms(&place(true), (70.0, 110.0));
        assert_eq!(
            (moved.size, moved.look, moved.ring),
            (u.size, u.look, u.ring)
        );
        assert_ne!(moved.pill, u.pill);
    }
}
