//! A lit line, drawn by one small pixel shader: a line in the focus ring's colour taken towards
//! white, with a glow above and below it that is light added to what's behind, banded on every
//! other screen row as a tube's scanlines are, its red split off just above and its blue just
//! below, and ends that can taper off. The drawer's leading edge and the slot a panel comes out of
//! are lit lines, and the same shader lays faint scanlines over a drawer while it moves.
//!
//! Everything is worked out per pixel from sub-pixel positions, so a line moving down the screen
//! glides rather than stepping a row at a time, and nothing is baked on the CPU's side.

use smithay::{
    backend::renderer::gles::{GlesRenderer, Uniform, UniformName, UniformType},
    utils::{Logical, Physical, Rectangle},
};

use super::shader::{self, Kept, Shared, Uniforms};

/// The glow's strength where it leaves the line.
pub const GLOW_PEAK: f32 = 0.55;
/// The fringes' strength, as a share of the line's red and blue.
pub const FRINGE: f32 = 0.3;
/// A tapered line fades in and out over this share of its length at either end, and over no
/// more than `TAPER_MOST` logical pixels.
pub const TAPER_SHARE: f64 = 0.15;
pub const TAPER_MOST: f64 = 40.0;

/// How far a tapered line `length` long fades in and out at either end, in the same units.
pub fn taper(length: f64, scale: f64) -> f64 {
    (TAPER_SHARE * length.max(0.0)).min(TAPER_MOST * scale)
}

/// The shader. Its constants come from the Rust ones above and the tube's (`fx`), so the two
/// sides can't drift apart.
fn source() -> String {
    format!(
        "precision highp float;\n\
         const float GLOW_PEAK = {GLOW_PEAK:?};\n\
         const float FRINGE = {FRINGE:?};\n\
         const float SCANLINE = {scanline:?};\n\
         {BODY}",
        scanline = super::fx::SCANLINE,
    )
}

const BODY: &str = r#"
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The area's top-left corner and size, in screen pixels.
uniform vec4 frame;
// The line's left and right ends and its middle, and the row above which nothing shows (the
// slot a drawer comes out of); for scanlines the third is their foot instead.
uniform vec4 ends;
// The line's thickness, how far its glow reaches either way, how far its ends taper, and for
// scanlines how dark they are, all in screen pixels.
uniform vec4 shape;
// The line's own colour and how strongly it shows.
uniform vec4 core;
// The glow's colour.
uniform vec3 halo;
// Whether its red and blue split off either side, and by how many screen pixels.
uniform vec2 tube;

void main() {
    vec2 at = v_coords * frame.zw + frame.xy;
    float row = floor(at.y);
    // Coverage across, with tapered or crisp ends.
    float across = shape.z > 0.0
        ? smoothstep(ends.x, ends.x + shape.z, at.x) * (1.0 - smoothstep(ends.y - shape.z, ends.y, at.x))
        : clamp(min(at.x + 0.5, ends.y) - max(at.x - 0.5, ends.x), 0.0, 1.0);
    float shown = across * clamp(at.y + 0.5 - ends.w, 0.0, 1.0);
    vec4 color;
    if (shape.w > 0.0) {
        // Scanlines: every other screen row darkened, between the top and the foot.
        float inside = clamp(ends.z - (at.y - 0.5), 0.0, 1.0);
        float dark = mod(row, 2.0) < 1.0 ? shape.w : 0.0;
        color = vec4(0.0, 0.0, 0.0, dark * inside);
    } else {
        float past = abs(at.y - ends.z) - 0.5 * shape.x;
        float cover = clamp(0.5 - past, 0.0, 1.0);
        float glow = shape.y > 0.0 ? GLOW_PEAK * pow(clamp(1.0 - past / shape.y, 0.0, 1.0), 2.0) : 0.0;
        if (mod(row, 2.0) >= 1.0) glow *= SCANLINE;
        vec3 light = halo * glow;
        if (tube.x > 0.0 && past > 0.0 && past <= tube.y) {
            if (at.y < ends.z) light.r += core.r * FRINGE;
            else light.b += core.b * FRINGE;
        }
        color = vec4(core.rgb * cover + light * (1.0 - cover), cover);
    }
    color *= shown * core.a * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

fn uniform_names() -> [UniformName<'static>; 6] {
    [
        UniformName::new("frame", UniformType::_4f),
        UniformName::new("ends", UniformType::_4f),
        UniformName::new("shape", UniformType::_4f),
        UniformName::new("core", UniformType::_4f),
        UniformName::new("halo", UniformType::_3f),
        UniformName::new("tube", UniformType::_2f),
    ]
}

/// A line to light, in logical pixels on a screen at `scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub left: f64,
    pub right: f64,
    /// Its middle, across the screen.
    pub y: f64,
    /// How thick it is, and how far its glow reaches either side of it.
    pub thick: f64,
    pub glow: f64,
    /// Nothing shows above this.
    pub above: f64,
    /// Whether its ends fade in and out rather than stopping crisply.
    pub tapered: bool,
    /// Whether its red and blue split off either side, as a tube's do.
    pub fringed: bool,
    /// Its own colour and its glow's, 0xRRGGBBAA, and how strongly it shows.
    pub core: u32,
    pub halo: u32,
    pub strength: f32,
}

/// Faint scanlines over a box, in logical pixels: every other screen row darkened by `dark`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scanlines {
    pub rect: Rectangle<f64, Logical>,
    pub dark: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LineUniforms {
    frame: [f32; 4],
    ends: [f32; 4],
    shape: [f32; 4],
    core: [f32; 4],
    halo: [f32; 3],
    tube: [f32; 2],
}

impl Uniforms for LineUniforms {
    fn list(&self) -> Vec<Uniform<'static>> {
        vec![
            Uniform::new("frame", self.frame),
            Uniform::new("ends", self.ends),
            Uniform::new("shape", self.shape),
            Uniform::new("core", self.core),
            Uniform::new("halo", self.halo),
            Uniform::new("tube", self.tube),
        ]
    }
}

fn rgb(c: u32) -> [f32; 3] {
    [
        (c >> 24) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
    ]
}

/// The whole logical pixels around `rect`, and where that area lands on the screen.
fn area_around(
    rect: Rectangle<f64, Logical>,
    scale: f64,
) -> (Rectangle<i32, Logical>, Rectangle<i32, Physical>) {
    let (left, top) = (rect.loc.x.floor(), rect.loc.y.floor());
    let (right, bottom) = (
        (rect.loc.x + rect.size.w).ceil(),
        (rect.loc.y + rect.size.h).ceil(),
    );
    let area = Rectangle::<i32, Logical>::new(
        (left as i32, top as i32).into(),
        ((right - left) as i32, (bottom - top) as i32).into(),
    );
    (area, area.to_physical_precise_round(scale))
}

fn frame_of(screen: Rectangle<i32, Physical>) -> [f32; 4] {
    [
        screen.loc.x as f32,
        screen.loc.y as f32,
        screen.size.w as f32,
        screen.size.h as f32,
    ]
}

impl Line {
    fn uniforms(&self, scale: f64) -> Option<(Rectangle<i32, Logical>, LineUniforms)> {
        if self.strength <= 0.0 || self.right - self.left < 1.0 {
            return None;
        }
        let reach = self.thick / 2.0 + self.glow;
        let top = (self.y - reach).max(self.above);
        let bottom = self.y + reach;
        if bottom <= top {
            return None;
        }
        let (area, screen) = area_around(
            Rectangle::new(
                (self.left, top).into(),
                (self.right - self.left, bottom - top).into(),
            ),
            scale,
        );
        let core = rgb(self.core);
        let taper = if self.tapered {
            taper((self.right - self.left) * scale, scale) as f32
        } else {
            0.0
        };
        Some((
            area,
            LineUniforms {
                frame: frame_of(screen),
                ends: [
                    (self.left * scale) as f32,
                    (self.right * scale) as f32,
                    (self.y * scale) as f32,
                    (self.above * scale) as f32,
                ],
                shape: [
                    (self.thick * scale) as f32,
                    (self.glow * scale) as f32,
                    taper,
                    0.0,
                ],
                core: [core[0], core[1], core[2], self.strength.min(1.0)],
                halo: rgb(self.halo),
                tube: [
                    if self.fringed { 1.0 } else { 0.0 },
                    (super::fx::SPLIT * scale) as f32,
                ],
            },
        ))
    }
}

impl Scanlines {
    fn uniforms(&self, scale: f64) -> Option<(Rectangle<i32, Logical>, LineUniforms)> {
        if self.dark <= 0.0 || self.rect.size.h < 1.0 || self.rect.size.w < 1.0 {
            return None;
        }
        let (area, screen) = area_around(self.rect, scale);
        let r = self.rect;
        Some((
            area,
            LineUniforms {
                frame: frame_of(screen),
                ends: [
                    (r.loc.x * scale) as f32,
                    ((r.loc.x + r.size.w) * scale) as f32,
                    ((r.loc.y + r.size.h) * scale) as f32,
                    (r.loc.y * scale) as f32,
                ],
                shape: [0.0, 0.0, 0.0, self.dark],
                core: [0.0, 0.0, 0.0, 1.0],
                halo: [0.0; 3],
                tube: [0.0; 2],
            },
        ))
    }
}

/// The program's name in the GL context's keeping.
struct Program;

fn program(
    renderer: &mut GlesRenderer,
) -> Option<smithay::backend::renderer::gles::GlesPixelProgram> {
    shader::program::<Program>(renderer, source, &uniform_names())
}

/// One lit line or set of scanlines, kept from frame to frame.
#[derive(Debug, Default)]
pub struct LitLine(Kept<LineUniforms>);

impl LitLine {
    /// `line` on a screen at `scale`; `None` when it doesn't show.
    pub fn line(&mut self, renderer: &mut GlesRenderer, line: &Line, scale: f64) -> Option<Shared> {
        let (area, uniforms) = line.uniforms(scale)?;
        self.0.element(|| program(renderer), area, uniforms)
    }

    /// `scanlines` on a screen at `scale`; `None` when they don't show.
    pub fn scanlines(
        &mut self,
        renderer: &mut GlesRenderer,
        scanlines: &Scanlines,
        scale: f64,
    ) -> Option<Shared> {
        let (area, uniforms) = scanlines.uniforms(scale)?;
        self.0.element(|| program(renderer), area, uniforms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Line {
        Line {
            left: 100.0,
            right: 400.0,
            y: 50.0,
            thick: 2.0,
            glow: 14.0,
            above: 0.0,
            tapered: false,
            fringed: true,
            core: 0x42d3ffff,
            halo: 0x42d3ffff,
            strength: 1.0,
        }
    }

    #[test]
    fn the_shaders_constants_are_the_ones_rust_uses() {
        let source = source();
        assert!(source.contains(&format!("const float GLOW_PEAK = {GLOW_PEAK:?};")));
        assert!(source.contains(&format!(
            "const float SCANLINE = {:?};",
            super::super::fx::SCANLINE
        )));
    }

    #[test]
    fn a_line_covers_its_glow_and_is_cut_off_above_its_slot() {
        let (area, _) = line().uniforms(1.25).unwrap();
        assert!(area.loc.y as f64 <= 50.0 - 15.0 && (area.loc.y + area.size.h) as f64 >= 65.0);
        let (cut, _) = Line {
            above: 60.0,
            ..line()
        }
        .uniforms(1.25)
        .unwrap();
        assert_eq!(cut.loc.y, 60, "nothing above the slot is in its area");
        assert!(
            Line {
                above: 70.0,
                ..line()
            }
            .uniforms(1.25)
            .is_none(),
            "wholly in the slot"
        );
    }

    #[test]
    fn a_line_moves_by_fractions_of_a_pixel() {
        let (_, a) = line().uniforms(1.25).unwrap();
        let (_, b) = Line { y: 50.3, ..line() }.uniforms(1.25).unwrap();
        assert!((b.ends[2] - a.ends[2] - 0.375).abs() < 1e-4);
    }

    #[test]
    fn a_tapered_lines_ends_fade_over_a_share_of_it_up_to_a_most() {
        assert_eq!(taper(100.0, 1.0), 15.0);
        assert_eq!(taper(4000.0, 1.25), TAPER_MOST * 1.25);
        let (_, crisp) = line().uniforms(1.0).unwrap();
        assert_eq!(crisp.shape[2], 0.0);
    }

    #[test]
    fn a_line_with_no_strength_or_length_is_not_drawn() {
        assert!(
            Line {
                strength: 0.0,
                ..line()
            }
            .uniforms(1.0)
            .is_none()
        );
        assert!(
            Line {
                right: 100.5,
                ..line()
            }
            .uniforms(1.0)
            .is_none()
        );
    }
}
