//! Closing windows falling away, drawn by a shader over the window's last picture.
//!
//! Derez, with the code rain: the window is read out into falling code, like a picture on a
//! failing CRT. The rain's glyphs travel with the picture: a strip below it holds each of Matrix
//! mode's 26 glyphs, drawn from the rain's own font, so one texture carries both and the shader
//! needs nothing else.
//!
//! Wake, with Slipstream's own effects: the window, still one rigid pane, drops away off the bottom
//! of the screen, leaving a slipstream behind it: streaks of light like the streams', coloured from
//! the window and turning to the rain's green as they fade.

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

use slipstream_config::Effects;

use crate::{
    glmatrix::{Glyphs, TINT},
    render::OutputElement,
    tilt,
};

/// Matrix mode's glyphs, by GLMatrix's atlas numbering: the digits, then the kana.
const GLYPHS: [usize; 26] = [
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170,
    171, 172, 173, 174, 175,
];
/// How much faster than the shader's own timeline it plays.
const PACE: f64 = 1.25;
/// How long a closed window takes to fall away, in animation seconds: the shader's 1.2 s timeline
/// at `PACE`.
pub const DEREZ: f64 = 1.2 / PACE;
/// How fast falling glyphs speed up, in logical pixels a second a second.
const GRAVITY: f64 = 3400.0;
/// How long a closed window takes to drop away, in animation seconds: it lets go after 0.06 s, is
/// off the screen 0.42 s later, and its wake has faded 0.4 s after that. The shader has the same
/// numbers.
pub const WAKE: f64 = 0.88;
const WAKE_GO: f64 = 0.42;

/// How long a closed window takes to fall away in `effects`.
pub fn length(effects: Effects) -> f64 {
    match effects {
        Effects::Matrix => DEREZ,
        Effects::Slipstream => WAKE,
    }
}

/// The glyph strip at one scale.
struct Atlas {
    scale: f64,
    buffer: MemoryRenderBuffer,
    /// Its size in screen pixels.
    device: (i32, i32),
}

/// The shader and glyphs for closing windows on one screen.
#[derive(Default)]
pub struct Fx {
    derez: Option<GlesTexProgram>,
    wake: Option<GlesTexProgram>,
    broken: bool,
    glyphs: Option<Glyphs>,
    atlas: Option<Atlas>,
}

impl Fx {
    /// Whether a closing window can fall away, compiling the shaders the first time.
    pub fn ready(&mut self, renderer: &mut GlesRenderer) -> bool {
        if self.broken || self.derez.is_some() {
            return !self.broken;
        }
        // Both shaders take the same inputs, the wake's `window_w` and `unit` besides; one that a
        // shader doesn't use is simply not set.
        let uniforms = [
            UniformName::new("texpx", UniformType::_2f),
            UniformName::new("content", UniformType::_2f),
            UniformName::new("cell", UniformType::_2f),
            UniformName::new("t", UniformType::_1f),
            UniformName::new("ink", UniformType::_3f),
            UniformName::new("window_h", UniformType::_1f),
            UniformName::new("gravity", UniformType::_1f),
            UniformName::new("unit", UniformType::_1f),
            UniformName::new("window_w", UniformType::_1f),
        ];
        let derez =
            renderer.compile_custom_texture_shader(format!("{HEAD}{DEREZ_MAIN}"), &uniforms);
        let wake = renderer.compile_custom_texture_shader(format!("{HEAD}{WAKE_MAIN}"), &uniforms);
        match (derez, wake) {
            (Ok(derez), Ok(wake)) => {
                self.derez = Some(derez);
                self.wake = Some(wake);
            }
            (Err(err), _) | (_, Err(err)) => {
                tracing::warn!("a closing shader didn't compile, so closed windows fade: {err}");
                self.broken = true;
            }
        }
        if self.glyphs.is_none() {
            self.glyphs = Glyphs::load();
            if self.glyphs.is_none() {
                tracing::warn!("the rain's font didn't load, so closed windows fade");
                self.broken = true;
            }
        }
        !self.broken
    }

    /// The glyph strip, painted for `scale` the first time it's wanted there.
    fn atlas(&mut self, scale: f64) -> Option<&Atlas> {
        if self.atlas.as_ref().is_none_or(|atlas| atlas.scale != scale) {
            let cell = crate::rain::glyph_size(scale);
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

    /// A closed window's last picture, `elements` laid out from its corner, falling away in
    /// `effects` `t` seconds in. It was drawn at `rect` on a screen whose bottom edge is `bottom`,
    /// both in the screen's own logical pixels; what falls, falls off that edge.
    #[allow(clippy::too_many_arguments)]
    pub fn fall(
        &mut self,
        renderer: &mut GlesRenderer,
        effects: Effects,
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
        // Both fall off the foot of the screen.
        let fall = (bottom - rect.loc.y).max(rect.size.h);
        let content = Size::<i32, Physical>::from((
            (rect.size.w * scale).round().max(1.0) as i32,
            (fall * scale).round().max(1.0) as i32,
        ));
        let window_w = (rect.size.w * scale).round() as f32;
        let window_h = (rect.size.h * scale).round() as f32;
        let cell = crate::rain::glyph_size(scale);
        let cell = (cell.0 as i32, cell.1 as i32);
        // What falls speeds up at `gravity` screen pixels a second a second: a dropping window fast
        // enough to be clear of the screen in its time, however far it has to go.
        let (texpx, program, t, gravity) = match effects {
            Effects::Matrix => {
                let atlas_w = cell.0 * GLYPHS.len() as i32;
                elements.extend(self.atlas_element(renderer, scale, content.h));
                (
                    Size::<i32, Physical>::from((content.w.max(atlas_w), content.h + cell.1)),
                    self.derez.clone()?,
                    t * PACE,
                    GRAVITY * scale,
                )
            }
            Effects::Slipstream => (
                content,
                self.wake.clone()?,
                t,
                2.0 * content.h as f64 / (WAKE_GO * WAKE_GO),
            ),
        };
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
            Uniform::new("ink", TINT),
            Uniform::new("window_h", window_h),
            Uniform::new("gravity", gravity as f32),
            Uniform::new("unit", scale as f32),
            Uniform::new("window_w", window_w),
        ];
        Some(TextureShaderElement::new(inner, program, uniforms))
    }
}

/// What the shader starts with: Smithay's texture shader's inputs, and the code rain's own look
/// from GLMatrix — its glyphs, the brightness wave running down each column, each column's depth
/// fog, and light added in the rain's tint — so a closing window's rain is the minimised rain's.
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
uniform vec3 ink;

const float WAVE = 22.0;
// Font strokes are thinner than GLMatrix's glowing atlas, so they carry more light.
const float GAIN = 2.5;

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float hash3(vec3 p) {
    return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453);
}

// GLMatrix's brightness ramp: 0.2 + 0.8·sin((22 − k)/21 · π/2).
float ramp(float k) {
    return 0.2 + 0.8 * sin((WAVE - k) / (WAVE - 1.0) * 1.5707963);
}

// A column's fog, from the depth its strip falls at.
float fog(float col) {
    float z = 7.0 - hash(vec2(col, 5.0)) * 24.5;
    return 0.2 + 0.8 * (z / 35.0 + 0.5);
}

// The wave running down column `col`, stepping every two to four GLMatrix frames: how bright it
// leaves cell `row`. Where the wave's table runs out the cell is dark, as in GLMatrix.
float wave(float col, float row) {
    float period = 0.03 * (2.0 + floor(hash(vec2(col, 6.0)) * 3.0));
    float at = mod(floor(t / period), WAVE);
    float j = WAVE - mod(row + WAVE - at, WAVE);
    return j >= WAVE ? 0.0 : ramp(j);
}

// One cell in seven is empty.
bool empty_cell(vec2 cid) {
    return hash(vec2(cid.x + 11.0, cid.y * 1.3)) < 1.0 / 7.0;
}

// The glyph in a cell: fixed, or changing if the cell is one of the one in twenty that spin.
float glyph_of(vec2 cid) {
    float spin = hash(vec2(cid.x * 1.7 + 3.1, cid.y)) < 0.05 ? floor(t / 0.06) : 0.0;
    return floor(hash3(vec3(cid, spin)) * 26.0);
}

// Glyph `g` (0 to 25) at `f`, 0 to 1 across its cell, read from the strip below the picture.
float glyph_at(float g, vec2 f) {
    vec2 px = vec2((g + clamp(f.x, 0.02, 0.98)) * cell.x, content.y + clamp(f.y, 0.02, 0.98) * cell.y);
    return texture2D(tex, px / texpx).a;
}

// The light a glyph adds at `brightness`, in the rain's `ink`, clamped as fixed-function GL
// clamps it. Added, not laid over: alpha stays 0, so what's behind shows through, as the rain
// glows on its card.
vec4 light(float brightness, float cover) {
    return vec4(ink * min(brightness * GAIN, 1.0) * cover, 0.0);
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

/// Derez. The window is read out into glyphs from the top down, like a picture on a failing CRT:
/// scanlines, a flicker, colour fringes and torn bands of picture shifting sideways, worst at the
/// bright line doing the reading. Each column of glyphs, once enough of it is read, lets go and
/// falls, faster and faster, fading as it goes. Below the reading line the window is still itself.
const DEREZ_MAIN: &str = r#"
uniform float window_h;
uniform float gravity;

const float READ = 0.42;

// The window and its glyphs at `p`, before the screen's own effects.
vec4 layer(vec2 p) {
    if (p.x < 0.0 || p.x >= content.x) {
        return vec4(0.0);
    }
    float col = floor(p.x / cell.x);
    float fx = p.x / cell.x - col;
    float let_go = READ * 0.55 + hash(vec2(col, 9.0)) * 0.25;
    float since = max(t - let_go, 0.0);
    float fall = 0.5 * gravity * since * since;
    float strength = 1.0 - clamp((t - let_go - 0.25) / 0.45, 0.0, 1.0);
    float y = p.y - fall;
    vec4 glow = vec4(0.0);
    if (y >= 0.0 && y < window_h && strength > 0.0) {
        vec2 cid = vec2(col, floor(y / cell.y));
        float read_at = READ * cid.y * cell.y / window_h;
        if (t >= read_at && !empty_cell(cid)) {
            vec2 f = vec2(fx, y / cell.y - cid.y);
            float g = glyph_of(cid);
            // Phosphor: a soft halo around each stroke.
            vec2 d = vec2(1.5) / cell;
            float halo = 0.25 * (glyph_at(g, f + vec2(d.x, 0.0)) + glyph_at(g, f - vec2(d.x, 0.0))
                + glyph_at(g, f + vec2(0.0, d.y)) + glyph_at(g, f - vec2(0.0, d.y)));
            float cover = min(glyph_at(g, f) + 0.45 * halo, 1.0);
            // The row just read is the head, lit half as much again.
            float bright = (t - read_at < 0.06 ? 1.5 : wave(cid.x, cid.y)) * fog(col);
            glow = light(bright, cover) * strength;
        }
    }
    float line = window_h * clamp(t / READ, 0.0, 1.0);
    vec4 still = (p.y >= line && p.y < window_h) ? picture(p) : vec4(0.0);
    return still + glow;
}

void main() {
    vec2 px = v_coords * texpx;
    float line = window_h * clamp(t / READ, 0.0, 1.0);
    float frame = floor(t / 0.04);
    // Torn bands: some slices of the picture jump sideways for a frame or two, more of them early
    // on, and always around the reading line.
    float band = floor(px.y / (cell.y * 0.6));
    float torn = step(0.8, hash(vec2(band, frame))) * (1.0 - smoothstep(0.35, 0.8, t));
    float near = 1.0 - clamp(abs(px.y - line) / (cell.y * 2.5), 0.0, 1.0);
    near *= step(t, READ + 0.05);
    float shift = (torn * 5.0 + near * 2.5) * (hash(vec2(band + 3.0, frame)) - 0.5) * cell.x;
    vec2 p = vec2(px.x + shift, px.y);
    // Colour fringes, wider where the picture tears.
    float split = 1.5 + 6.0 * max(torn, near);
    vec4 mid = layer(p);
    vec4 color = vec4(
        layer(p + vec2(split, 0.0)).r,
        mid.g,
        layer(p - vec2(split, 0.0)).b,
        mid.a);
    // The reading line itself, bright and jittering.
    float jitter = (hash(vec2(frame, 4.0)) - 0.5) * 3.0;
    float beam = (1.0 - clamp(abs(px.y - line - jitter) / 2.5, 0.0, 1.0)) * step(t, READ)
        * step(px.y, window_h);
    color.rgb += ink * beam * 0.9;
    // Scanlines, every other screen row, and the tube's flicker.
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : 0.62;
    float flicker = 0.88 + 0.12 * hash(vec2(frame, 1.0));
    color.rgb *= scan * flicker;
    finish(color);
}
"#;

/// Wake. The window holds still for a moment, then drops off the bottom of the screen as one rigid
/// pane, speeding up and drawing back a little into the distance as it goes. Behind it pours its
/// slipstream, drawn as the streams draw their light: streaks at different depths, near ones wider,
/// brighter and quicker to follow, far ones fine and dim and lagging, each with a glowing head
/// close behind the pane. Each takes its colour from the part of the window it came from, starting
/// in the window's own colours and turning to the rain's green, and all of them fade once the pane
/// has gone.
const WAKE_MAIN: &str = r#"
uniform float window_h;
uniform float window_w;
uniform float gravity;
// One logical pixel, in screen pixels.
uniform float unit;

// The same timings as `WAKE_GO` and `WAKE` in `fx.rs`.
const float LEAN = 0.06;
const float GO = 0.42;
const float WAKE = 0.4;
// How far into the distance the pane draws back as it leaves.
const float RECEDE = 0.08;
// Three layers of streaks, each in cells this many logical pixels wide, some left empty.
const float CELL = 22.0;
const float EMPTY = 0.3;

// How far the pane has dropped, `u` seconds in, and how large it is.
float pane_y(float u) {
    float a = max(u - LEAN, 0.0);
    return 0.5 * gravity * a * a;
}

float pane_scale(float u) {
    return 1.0 - RECEDE * clamp(pane_y(u) / content.y, 0.0, 1.0);
}

// The pane's picture at point `p`, `u` seconds in: nothing outside it.
vec4 pane_at(vec2 p, float u) {
    vec2 middle = vec2(window_w, window_h) * 0.5;
    vec2 local = (p - vec2(0.0, pane_y(u)) - middle) / pane_scale(u) + middle;
    if (local.x < 0.0 || local.y < 0.0 || local.x >= window_w || local.y >= window_h) {
        return vec4(0.0);
    }
    return picture(local);
}

// One layer's streak at `px`, if its cell has one.
vec3 streak(vec2 px, float layer) {
    float cw = (CELL - 5.0 * layer) * unit;
    float off = hash(vec2(layer, 3.0)) * cw;
    float cell = floor((px.x + off) / cw);
    float seed = layer * 7.0;
    if (hash(vec2(cell, seed + 1.0)) < EMPTY) {
        return vec3(0.0);
    }
    // Nearer in the first layer, farther in the last.
    float near = clamp(hash(vec2(cell, seed + 2.0)) * 0.6 + (2.0 - layer) * 0.2, 0.0, 1.0);
    float cx = (cell + 0.25 + 0.5 * hash(vec2(cell, seed + 3.0))) * cw - off;
    // Far streaks fall behind the pane; near ones keep close to it.
    float lag = 0.01 + 0.16 * (1.0 - near) * hash(vec2(cell, seed + 4.0));
    float u = t - lag;
    if (u <= LEAN) {
        return vec3(0.0);
    }
    float head = pane_y(u) + 5.0 * unit;
    float speed = gravity * (u - LEAN);
    float len = (60.0 + 220.0 * near) * unit + speed * 0.08;
    float width = mix(2.5, 9.0, near) * unit;
    float radius = mix(4.5, 11.0, near) * unit;
    float dx = abs(px.x - cx);
    // The tail, brightening towards the head, and the head's glow.
    float along = (px.y - (head - len)) / len;
    float body = (along >= 0.0 && along <= 1.0)
        ? pow(along, 1.6) * clamp(width * 0.5 + 0.5 - dx, 0.0, 1.0) : 0.0;
    float d = length(vec2(px.x - cx, px.y - head)) / radius;
    float glow = pow(max(1.0 - d, 0.0), 2.0);
    if (body <= 0.0 && glow <= 0.0) {
        return vec3(0.0);
    }
    // Its colour: the window's own at first, from the part it came from, then the rain's green,
    // brighter where the window was.
    vec4 c = picture(vec2(clamp(cx, 0.0, window_w - 1.0), hash(vec2(cell, seed + 5.0)) * window_h));
    float l = dot(c.rgb, vec3(0.3, 0.59, 0.11));
    vec3 green = ink * (0.55 + 0.6 * l);
    vec3 own = c.rgb * 1.2 + ink * 0.2;
    vec3 colour = mix(own, green, clamp((t - LEAN) / 0.18, 0.0, 1.0));
    vec3 hot = mix(colour, vec3(1.0), 0.6);
    float bright = 0.4 + 0.6 * near;
    return (colour * body + hot * glow) * bright;
}

void main() {
    vec2 px = v_coords * texpx;
    vec4 color = vec4(0.0);
    if (px.x < 0.0 || px.x >= content.x) {
        finish(color);
        return;
    }
    // The pane itself, whole.
    color = pane_at(px, t);
    // Its wake, fading once the pane is gone.
    float fade = 1.0 - clamp((t - LEAN - GO) / WAKE, 0.0, 1.0);
    vec3 shower = streak(px, 0.0) + streak(px, 1.0) + streak(px, 2.0);
    color.rgb += shower * fade;
    finish(color);
}
"#;
