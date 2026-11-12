//! The slot a panel comes out of, lit in the line along the bar's foot: a stretch of the line in
//! the focus ring's colour, tapering at its ends, with a glow a little above it and further below.
//! One shader strip along the foot draws it, in every style of bar.

use smithay::{
    backend::renderer::gles::{GlesPixelProgram, GlesRenderer},
    utils::{Logical, Physical, Rectangle},
};

use super::{BarSlot, HEIGHT, TALL};
use crate::effects::{
    lit_line,
    shader::{self, Values},
};

/// How far the strip reaches above and below the line, in logical pixels: room for the glow
/// either side.
const ABOVE: i32 = 5;
const BELOW: i32 = 10;

/// The strip's shader, its numbers in the bar's units unless they say otherwise. Its colour is
/// light added to what's under it.
const SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Screen pixels to one of the bar's units, the bar's height in screen pixels, and where the
// strip's top-left corner is from the bar's, in screen pixels.
uniform vec4 frame;
// The strip's size in screen pixels.
uniform vec2 extent;
uniform vec3 ring;
// The slot: its left and right ends, how strongly it is lit, and how far its ends taper, as a lit
// line's do.
uniform vec4 slot;
// The strip's own alpha.
uniform float fade;

// What a lit slot adds to the line, and the knee above which it brightens ever more slowly
// towards the ceiling, so it never glares.
const float SLOT = 0.8;
const float KNEE = 0.62;
const float CEILING = 0.88;

void main() {
    float f = frame.x;
    // This pixel's middle, in screen pixels from the bar's top-left corner.
    vec2 at = v_coords * extent + frame.zw;
    float x = at.x / f;
    // The line fills the bar's last unit, the same rows its picture's would.
    float line_top = frame.y - f;
    float cover = clamp(min(at.y + 0.5, frame.y) - max(at.y - 0.5, line_top), 0.0, 1.0);
    // Units from the line's middle, downwards.
    float below = (at.y - (frame.y - 0.5 * f)) / f;

    float lit = smoothstep(slot.x, slot.x + slot.w, x) * (1.0 - smoothstep(slot.y - slot.w, slot.y, x));
    float charge = SLOT * slot.z * lit;
    if (charge > KNEE) {
        float room = CEILING - KNEE;
        charge = KNEE + room * (1.0 - exp(-(charge - KNEE) / room));
    }
    // The glow beside the line: a little above, into the bar, and further below.
    float glow = below > 0.0
        ? 0.55 * pow(clamp(1.0 - below / 12.0, 0.0, 1.0), 2.2)
        : 0.30 * pow(clamp(1.0 + below / 4.0, 0.0, 1.0), 2.0);
    vec3 light = ring * charge * (cover + (1.0 - cover) * glow);
    // A glint at its flare, on the line alone.
    light += vec3(0.4 * max(slot.z - 1.0, 0.0) * lit * cover);

    vec4 color = vec4(light, 0.0) * fade * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// The strip's program for `renderer`, compiled once for it; `None` if it didn't compile, which
/// is said once.
fn program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    shader::program::<Program, _>(
        renderer,
        |renderer| renderer.compile_custom_pixel_shader(SHADER, &shader::names::<Uniforms>()),
        "a panel's slot's",
    )
}

/// Marks the slot's program in a renderer's keeping.
struct Program;

/// What the bar tells the slot's strip each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Where the bar is, in logical pixels from the screen's corner, and the screen's scale.
    pub bar: Rectangle<f64, Logical>,
    pub scale: f64,
    /// The focus ring's colour, 0xRRGGBBAA.
    pub ring: u32,
    /// The slot a panel is coming out of, its ends in logical pixels across the screen.
    pub slot: BarSlot,
    pub alpha: f32,
}

/// The shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    frame: [f32; 4],
    extent: [f32; 2],
    ring: [f32; 3],
    slot: [f32; 4],
    fade: f32,
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("extent", self.extent)
            .with("ring", self.ring)
            .with("slot", self.slot)
            .with("fade", [self.fade])
    }
}

/// One screen's strip, kept from frame to frame: it counts as changed only when its numbers do.
#[derive(Default)]
pub struct Light(shader::Kept);

impl Light {
    /// The strip along the bar's foot with the slot `edge` says. `None` if the shader didn't
    /// compile.
    pub fn element(&mut self, renderer: &mut GlesRenderer, edge: &Edge) -> Option<shader::Shaded> {
        let program = program(renderer)?;
        let (area, uniforms) = uniforms(edge);
        Some(self.0.pixel(program, area, &uniforms))
    }
}

/// The strip's place and its shader's numbers for `edge`.
fn uniforms(edge: &Edge) -> (Rectangle<i32, Logical>, Uniforms) {
    let (bar, scale) = (edge.bar, edge.scale);
    // The bar's picture: one pixel per screen pixel from its rounded corner.
    let (left, top) = ((bar.loc.x * scale).round(), (bar.loc.y * scale).round());
    let tall = (HEIGHT as f64 * scale).round();
    let f = tall / TALL as f64;
    let area = Rectangle::<i32, Logical>::new(
        (bar.loc.x as i32, bar.loc.y as i32 + HEIGHT - ABOVE).into(),
        (bar.size.w as i32, ABOVE + BELOW).into(),
    );
    let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
    let units = |x: f64| (((x - bar.loc.x) * scale) / f) as f32;
    let rgb = |c: u32| {
        [
            (c >> 24) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
        ]
    };
    let (slot_left, slot_right) = (units(edge.slot.left), units(edge.slot.right));
    // Tapered as a lit line is, at least a unit long so its ends never meet.
    let length = f64::from(slot_right - slot_left).max(1.0) * f;
    let taper = lit_line::taper(length, scale) / f;
    let uniforms = Uniforms {
        frame: [
            f as f32,
            tall as f32,
            (screen.loc.x as f64 - left) as f32,
            (screen.loc.y as f64 - top) as f32,
        ],
        extent: [screen.size.w as f32, screen.size.h as f32],
        ring: rgb(edge.ring),
        slot: [slot_left, slot_right, edge.slot.strength, taper as f32],
        fade: edge.alpha,
    };
    (area, uniforms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge() -> Edge {
        Edge {
            bar: Rectangle::new((0.0, 0.0).into(), (1536.0, HEIGHT as f64).into()),
            scale: 1.0,
            ring: 0x42d3_ffff,
            slot: BarSlot {
                left: 100.0,
                right: 420.0,
                strength: 1.0,
            },
            alpha: 1.0,
        }
    }

    #[test]
    fn a_panels_slot_is_passed_in_the_bars_units_with_its_ends_tapered() {
        let mut edge = edge();
        edge.scale = 1.25;
        // At 1.25 a unit of the bar is one screen pixel; its ends taper as a lit line's do.
        let taper = lit_line::taper(400.0, 1.25) as f32;
        assert_eq!(uniforms(&edge).1.slot, [125.0, 525.0, 1.0, taper]);
    }

    #[test]
    fn the_strip_lines_up_with_the_bars_foot_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let mut edge = edge();
            edge.scale = scale;
            let (area, u) = uniforms(&edge);
            let screen: Rectangle<i32, Physical> = area.to_physical_precise_round(scale);
            // The line's rows are the bar's last unit, within the strip.
            let line_top = u.frame[1] - u.frame[0];
            assert!(line_top > u.frame[3]);
            assert!(u.frame[1] < u.frame[3] + screen.size.h as f32);
            assert_eq!(u.frame[1], (HEIGHT as f64 * scale).round() as f32);
        }
    }

    #[test]
    fn the_glow_fits_inside_the_strip_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            // Logical pixels to one of the bar's units.
            let unit = (HEIGHT as f64 * scale).round() / TALL as f64 / scale;
            assert!(4.0 * unit + unit < ABOVE as f64, "above at {scale}");
            assert!(12.0 * unit <= BELOW as f64, "below at {scale}");
        }
    }
}
