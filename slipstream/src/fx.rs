//! Rain transitions: the desktop condensing out of the code rain at login and unlock (arrival),
//! and a closed window read out into falling code (derez).
//!
//! Both are drawn by a shader over a picture taken of what they reveal or take away. The rain's
//! glyphs travel with that picture: a strip below it holds each of Matrix mode's 26 glyphs, drawn
//! from the rain's own font, so one texture carries both and the shader needs nothing else.

use resvg::tiny_skia::Pixmap;
use smithay::backend::renderer::{
    Renderer,
    element::{
        Id, Kind,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        texture::TextureRenderElement,
    },
    gles::{
        GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType,
        element::TextureShaderElement,
    },
};
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};

use crate::{glmatrix::Glyphs, render::OutputElement, tilt};

/// A glyph cell, in logical pixels: GLMatrix's 32:46 proportions, about the size the rain's
/// streams draw theirs.
pub const CELL: (f64, f64) = (20.0, 28.0);
/// Matrix mode's glyphs, by GLMatrix's atlas numbering: the digits, then the kana.
const GLYPHS: [usize; 26] = [
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170,
    171, 172, 173, 174, 175,
];
/// How long the desktop takes to condense, in seconds of wall time.
pub const ARRIVAL: f64 = 1.3;
/// How long a closed window takes to fall away, in animation seconds.
pub const DEREZ: f64 = 1.2;
/// How fast falling glyphs speed up, in logical pixels a second a second.
const GRAVITY: f64 = 3400.0;

/// The glyph strip at one scale.
struct Atlas {
    scale: f64,
    buffer: MemoryRenderBuffer,
    /// Its size in screen pixels.
    device: (i32, i32),
}

/// The shaders and textures for both transitions on one screen.
#[derive(Default)]
pub struct Fx {
    arrival: Option<GlesTexProgram>,
    derez: Option<GlesTexProgram>,
    broken: bool,
    glyphs: Option<Glyphs>,
    atlas: Option<Atlas>,
    /// The desktop's picture while it arrives.
    arrival_texture: Option<(GlesTexture, Size<i32, Physical>)>,
}

impl Fx {
    /// Whether the transitions can be drawn, compiling their shaders the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.arrival.is_some() {
            return !self.broken;
        }
        let common = [
            UniformName::new("texpx", UniformType::_2f),
            UniformName::new("content", UniformType::_2f),
            UniformName::new("cell", UniformType::_2f),
            UniformName::new("t", UniformType::_1f),
        ];
        let mut arrival = common.to_vec();
        arrival.push(UniformName::new("dark", UniformType::_1f));
        let mut derez = common.to_vec();
        derez.push(UniformName::new("window_h", UniformType::_1f));
        derez.push(UniformName::new("gravity", UniformType::_1f));
        let compiled = (
            renderer.compile_custom_texture_shader(format!("{HEAD}{ARRIVAL_MAIN}"), &arrival),
            renderer.compile_custom_texture_shader(format!("{HEAD}{DEREZ_MAIN}"), &derez),
        );
        match compiled {
            (Ok(a), Ok(d)) => {
                self.arrival = Some(a);
                self.derez = Some(d);
            }
            (Err(err), _) | (_, Err(err)) => {
                tracing::warn!("the rain transitions' shaders didn't compile, so they fade: {err}");
                self.broken = true;
            }
        }
        if self.glyphs.is_none() {
            self.glyphs = Glyphs::load();
            if self.glyphs.is_none() {
                tracing::warn!("the rain's font didn't load, so the rain transitions fade");
                self.broken = true;
            }
        }
        !self.broken
    }

    /// The glyph strip, painted for `scale` the first time it's wanted there.
    fn atlas(&mut self, scale: f64) -> Option<&Atlas> {
        if self.atlas.as_ref().is_none_or(|atlas| atlas.scale != scale) {
            let cell = (
                (CELL.0 * scale).round().max(1.0) as usize,
                (CELL.1 * scale).round().max(1.0) as usize,
            );
            let glyphs = self.glyphs.as_mut()?;
            let mut pixmap = Pixmap::new((cell.0 * GLYPHS.len()) as u32, cell.1 as u32)?;
            let width = pixmap.width() as usize;
            let data = pixmap.data_mut();
            for (slot, index) in GLYPHS.iter().enumerate() {
                let Some(coverage) = glyphs.coverage(*index, cell) else {
                    continue;
                };
                for y in 0..cell.1 {
                    for x in 0..cell.0 {
                        let c = coverage[y * cell.0 + x];
                        let at = (y * width + slot * cell.0 + x) * 4;
                        // White, premultiplied: the shader colours it.
                        data[at..at + 4].copy_from_slice(&[c, c, c, c]);
                    }
                }
            }
            self.atlas = Some(Atlas {
                scale,
                device: (pixmap.width() as i32, pixmap.height() as i32),
                buffer: crate::paint::buffer(&pixmap),
            });
        }
        self.atlas.as_ref()
    }

    /// The glyph strip as an element, its top left at `top` screen pixels down.
    fn atlas_element(
        &mut self,
        renderer: &mut GlesRenderer,
        scale: f64,
        top: i32,
    ) -> Option<OutputElement> {
        let atlas = self.atlas(scale)?;
        let logical = Size::<i32, Logical>::from((
            (atlas.device.0 as f64 / scale).round() as i32,
            (atlas.device.1 as f64 / scale).round() as i32,
        ));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::<f64, Physical>::from((0.0, top as f64)),
            &atlas.buffer,
            None,
            Some(Rectangle::from_size(
                (atlas.device.0 as f64, atlas.device.1 as f64).into(),
            )),
            Some(logical),
            Kind::Unspecified,
        )
        .ok()
        .map(OutputElement::Memory)
    }

    /// The desktop, `elements` (front to back), condensing out of the rain `t` seconds in, on a
    /// screen `logical` big and `physical` pixels. `dark`: the screen starts black and the
    /// wallpaper comes up with the desktop, as at login; otherwise the wallpaper is already
    /// there, as behind the lock.
    #[allow(clippy::too_many_arguments)]
    pub fn arrival(
        &mut self,
        renderer: &mut GlesRenderer,
        mut elements: Vec<OutputElement>,
        logical: Size<i32, Logical>,
        physical: Size<i32, Physical>,
        scale: f64,
        t: f64,
        dark: bool,
    ) -> Option<TextureShaderElement> {
        if !self.ready(renderer) {
            return None;
        }
        let cell_px = (CELL.1 * scale).round() as i32;
        let atlas_w = (CELL.0 * scale).round() as i32 * GLYPHS.len() as i32;
        let texpx = Size::<i32, Physical>::from((physical.w.max(atlas_w), physical.h + cell_px));
        elements.extend(self.atlas_element(renderer, scale, physical.h));
        tilt::draw_offscreen(
            renderer,
            &mut self.arrival_texture,
            &mut self.broken,
            &elements,
            texpx.to_f64().to_logical(scale).to_i32_round(),
            texpx,
            scale,
            "the arrival",
        )?;
        // Shown over the screen only: the glyph strip below stays out of sight.
        let inner = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            (0.0, 0.0),
            self.arrival_texture.as_ref()?.0.clone(),
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
            Uniform::new("texpx", (texpx.w as f32, texpx.h as f32)),
            Uniform::new("content", (physical.w as f32, physical.h as f32)),
            Uniform::new(
                "cell",
                (
                    (CELL.0 * scale).round() as f32,
                    (CELL.1 * scale).round() as f32,
                ),
            ),
            Uniform::new("t", t as f32),
            Uniform::new("dark", if dark { 1.0f32 } else { 0.0 }),
        ];
        Some(TextureShaderElement::new(
            inner,
            self.arrival.clone()?,
            uniforms,
        ))
    }

    /// A closed window's last picture, `elements` laid out from its corner, read out into falling
    /// code `t` seconds in. It was drawn at `rect` on a screen whose bottom edge is `bottom`, both
    /// in the screen's own logical pixels; the glyphs fall off that edge.
    #[allow(clippy::too_many_arguments)]
    pub fn derez(
        &mut self,
        renderer: &mut GlesRenderer,
        texture: &mut Option<(GlesTexture, Size<i32, Physical>)>,
        mut elements: Vec<OutputElement>,
        rect: Rectangle<f64, Logical>,
        bottom: f64,
        scale: f64,
        t: f64,
    ) -> Option<TextureShaderElement> {
        if !self.ready(renderer) {
            return None;
        }
        let fall = (bottom - rect.loc.y).max(rect.size.h);
        let content = Size::<i32, Physical>::from((
            (rect.size.w * scale).round().max(1.0) as i32,
            (fall * scale).round().max(1.0) as i32,
        ));
        let window_h = (rect.size.h * scale).round() as f32;
        let cell = (
            (CELL.0 * scale).round() as i32,
            (CELL.1 * scale).round() as i32,
        );
        let atlas_w = cell.0 * GLYPHS.len() as i32;
        let texpx = Size::<i32, Physical>::from((content.w.max(atlas_w), content.h + cell.1));
        elements.extend(self.atlas_element(renderer, scale, content.h));
        tilt::draw_offscreen(
            renderer,
            texture,
            &mut self.broken,
            &elements,
            texpx.to_f64().to_logical(scale).to_i32_round(),
            texpx,
            scale,
            "a closing window",
        )?;
        let corner = rect.loc.to_physical(scale).to_i32_round::<i32>();
        let shown = Size::<i32, Logical>::from((
            (content.w as f64 / scale).round() as i32,
            (content.h as f64 / scale).round() as i32,
        ));
        let inner = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            corner.to_f64(),
            texture.as_ref()?.0.clone(),
            1,
            Transform::Normal,
            None,
            Some(Rectangle::from_size(
                (content.w as f64, content.h as f64).into(),
            )),
            Some(shown),
            None,
            Kind::Unspecified,
        );
        let uniforms = vec![
            Uniform::new("texpx", (texpx.w as f32, texpx.h as f32)),
            Uniform::new("content", (content.w as f32, content.h as f32)),
            Uniform::new("cell", (cell.0 as f32, cell.1 as f32)),
            Uniform::new("t", t as f32),
            Uniform::new("window_h", window_h),
            Uniform::new("gravity", (GRAVITY * scale) as f32),
        ];
        Some(TextureShaderElement::new(
            inner,
            self.derez.clone()?,
            uniforms,
        ))
    }
}

/// What both shaders start with: Smithay's texture shader's inputs, a hash, and a way to read a
/// glyph's coverage out of the strip below the picture.
const HEAD: &str = r#"#version 100

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

uniform vec2 texpx;
uniform vec2 content;
uniform vec2 cell;
uniform float t;

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float hash3(vec3 p) {
    return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453);
}

// Glyph `g` (0 to 25) at `f`, 0 to 1 across its cell.
float glyph_at(float g, vec2 f) {
    vec2 px = vec2((g + clamp(f.x, 0.02, 0.98)) * cell.x, content.y + clamp(f.y, 0.02, 0.98) * cell.y);
    return texture2D(tex, px / texpx).a;
}

vec4 picture(vec2 px) {
    return texture2D(tex, px / texpx);
}

void finish(vec4 color) {
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

/// Arrival. Every column of cells rains down from the top at its own speed. Where a column's head
/// passes over part of the desktop — a window, the bar, a card — the cell locks: a bright glyph
/// on a dark square, flickering, until it resolves into that part of the picture. Everywhere else
/// the rain falls on through, fading as the desktop settles.
const ARRIVAL_MAIN: &str = r#"
uniform float dark;

const float DONE = 1.3;
const float TRAIL = 14.0;

void main() {
    vec2 px = v_coords * texpx;
    vec2 cid = floor(px / cell);
    vec2 f = px / cell - cid;
    float top = cid.y * cell.y;
    vec4 here = picture(px);
    if (t >= DONE) {
        finish(here);
        return;
    }
    float delay = hash(vec2(cid.x, 1.0)) * 0.35;
    float travel = 0.5 + 0.4 * hash(vec2(cid.x, 2.0));
    float head = (t - delay) / travel * content.y;
    float wallpaper = clamp((t - 0.75) / 0.5, 0.0, 1.0);
    vec4 night = vec4(0.0, 0.0, 0.0, dark * (1.0 - wallpaper));
    if (head < top) {
        finish(night);
        return;
    }
    float step = floor(t * 10.0) + floor(hash(cid) * 4.0);
    float cov = glyph_at(floor(hash3(vec3(cid, step)) * 26.0), f);
    vec4 centre = picture(cid * cell + 0.5 * cell);
    vec4 color;
    if (centre.a > 0.5) {
        float settle = 0.55 + 0.4 * hash(cid + 7.0) + 0.25 * top / content.y;
        if (t >= settle) {
            float flash = 1.0 - clamp((t - settle) / 0.1, 0.0, 1.0);
            color = here + vec4(0.24, 0.94, 0.75, 0.0) * 0.18 * flash * here.a;
        } else {
            vec3 ink = hash(cid + 3.0) < 0.15 ? vec3(0.91, 1.0, 0.96) : vec3(0.24, 0.94, 0.75);
            vec4 square = vec4(0.043, 0.051, 0.07, 1.0) * 0.92;
            color = vec4(ink, 1.0) * cov + square * (1.0 - cov);
        }
    } else {
        float dist = (head - top) / cell.y;
        float fade = 1.0 - clamp((t - 0.85) / 0.4, 0.0, 1.0);
        float a = dist < TRAIL ? (1.0 - dist / TRAIL) * fade : 0.0;
        vec3 ink = dist < 1.0 ? vec3(0.91, 1.0, 0.96) : vec3(0.35, 1.0, 0.55);
        float reveal = clamp((t - 0.9) / 0.3, 0.0, 1.0);
        vec4 under = here * reveal + night * (1.0 - here.a * reveal);
        color = vec4(ink, 1.0) * cov * a + under * (1.0 - cov * a);
    }
    finish(color);
}
"#;

/// Derez. The window is read out into glyphs from the top down; each column, once enough of it is
/// read, lets go and falls, faster and faster, fading as it goes. Below the reading line the window
/// is still itself.
const DEREZ_MAIN: &str = r#"
uniform float window_h;
uniform float gravity;

const float READ = 0.42;

void main() {
    vec2 px = v_coords * texpx;
    float col = floor(px.x / cell.x);
    float fx = px.x / cell.x - col;
    float let_go = READ * 0.55 + hash(vec2(col, 9.0)) * 0.25;
    float since = max(t - let_go, 0.0);
    float fall = 0.5 * gravity * since * since;
    float strength = 1.0 - clamp((t - let_go - 0.25) / 0.45, 0.0, 1.0);
    float y = px.y - fall;
    vec4 glyph = vec4(0.0);
    if (y >= 0.0 && y < window_h && strength > 0.0) {
        float row = floor(y / cell.y);
        float read_at = READ * row * cell.y / window_h;
        if (t >= read_at) {
            float step = floor(t * 14.0);
            float cov = glyph_at(floor(hash3(vec3(col, row, step)) * 26.0), vec2(fx, y / cell.y - row));
            vec3 ink = t - read_at < 0.06
                ? vec3(0.91, 1.0, 0.96)
                : vec3(0.35, 1.0, 0.55) * (0.35 + 0.6 * hash(vec2(col, row)));
            glyph = vec4(ink, 1.0) * cov * strength;
        }
    }
    float line = window_h * clamp(t / READ, 0.0, 1.0);
    vec4 still = (px.y >= line && px.y < window_h) ? picture(px) : vec4(0.0);
    finish(glyph + still * (1.0 - glyph.a));
}
"#;
