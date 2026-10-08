//! Closing windows falling away, drawn by a shader over the window's last picture.
//!
//! Derez, with the code rain: the window is read out into falling code, like a picture on a
//! failing CRT. The rain's glyphs travel with the picture: a strip below it holds each of Matrix
//! mode's 26 glyphs, drawn from the rain's own font, so one texture carries both and the shader
//! needs nothing else.
//!
//! Wake, with Slipstream's own effects: the window, still one rigid pane, drops towards the bottom
//! of the screen while a slipstream pours down through it faster than it falls, eating it from the
//! top: streaks of light like the streams', coloured from the window and turning to the rain's
//! green as they fade.
//!
//! Either way it goes as a picture on a failing tube does: both are shown through the same
//! scanlines, flicker, colour fringes and torn bands, worst where the window is being taken apart.

use resvg::tiny_skia::Pixmap;
use smithay::backend::renderer::{
    Renderer,
    element::{
        Id, Kind,
        memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        texture::TextureRenderElement,
    },
    gles::{
        GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
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
/// eaten through 0.42 s later, and its wake has faded 0.4 s after that. The shader has the same
/// numbers.
pub const WAKE: f64 = 0.88;
const WAKE_GO: f64 = 0.42;
/// How far the pane drops while its wake eats through it, as a share of the way to the screen's
/// foot.
const WAKE_DROP: f64 = 0.5;

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
        texture: &mut Option<tilt::Canvas>,
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
            // Here `gravity` is the shower's: it reaches the pane's foot, or the screen's if that
            // comes first, as the pane finishes its drop, so the window is gone when a window
            // dropping clear of the screen would have been.
            Effects::Slipstream => {
                let reach = (WAKE_DROP * content.h as f64 + window_h as f64).min(content.h as f64);
                (
                    content,
                    self.wake.clone()?,
                    t,
                    2.0 * reach / (WAKE_GO * WAKE_GO),
                )
            }
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
            texture.as_ref()?.texture.clone(),
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
/// Then the failing CRT that either effect's `scene` is shown through.
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

// What the effect draws at `p`, before the tube's own effects.
vec4 scene(vec2 p);

// The scene at `px` as a failing CRT shows it: scanlines, a flicker, colour fringes and torn bands
// of picture shifting sideways. `line` is the row where the window is being taken apart, where the
// picture tears worst while `live` is 1, and which is itself lit as brightly as `beam`. `tearing`
// is how much of the rest of the picture tears.
vec4 crt(vec2 px, float line, float live, float beam, float tearing) {
    float frame = floor(t / 0.04);
    // Torn bands: some slices of the picture jump sideways for a frame or two, and always around
    // the line.
    float band = floor(px.y / (cell.y * 0.6));
    float torn = step(0.8, hash(vec2(band, frame))) * tearing;
    float near = (1.0 - clamp(abs(px.y - line) / (cell.y * 2.5), 0.0, 1.0)) * live;
    float shift = (torn * 5.0 + near * 2.5) * (hash(vec2(band + 3.0, frame)) - 0.5) * cell.x;
    vec2 p = vec2(px.x + shift, px.y);
    // Colour fringes, wider where the picture tears.
    float split = 1.5 + 6.0 * max(torn, near);
    vec4 mid = scene(p);
    vec4 color = vec4(
        scene(p + vec2(split, 0.0)).r,
        mid.g,
        scene(p - vec2(split, 0.0)).b,
        mid.a);
    // The line itself, bright and jittering.
    float jitter = (hash(vec2(frame, 4.0)) - 0.5) * 3.0;
    color.rgb += ink * (1.0 - clamp(abs(px.y - line - jitter) / 2.5, 0.0, 1.0)) * beam * 0.9;
    // Scanlines, every other screen row, and the tube's flicker.
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : 0.62;
    float flicker = 0.88 + 0.12 * hash(vec2(frame, 1.0));
    color.rgb *= scan * flicker;
    return color;
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

/// Derez. The window is read out into glyphs from the top down, like a picture on a failing CRT,
/// which tears worst at the bright line doing the reading. Each column of glyphs, once enough of
/// it is read, lets go and falls, faster and faster, fading as it goes. Below the reading line the
/// window is still itself.
const DEREZ_MAIN: &str = r#"
uniform float window_h;
uniform float gravity;

const float READ = 0.42;

// The window and its glyphs at `p`.
vec4 scene(vec2 p) {
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
    // The picture tears round the reading line until just after the reading is done, and the
    // line is lit across the window while it reads.
    float line = window_h * clamp(t / READ, 0.0, 1.0);
    float beam = step(t, READ) * step(px.y, window_h);
    finish(crt(px, line, step(t, READ + 0.05), beam, 1.0 - smoothstep(0.35, 0.8, t)));
}
"#;

/// Wake. The window holds still for a moment, then drops towards the bottom of the screen as one
/// rigid pane, speeding up and drawing back a little into the distance as it goes. Its slipstream
/// pours down through it faster than it falls, drawn as the streams draw their light, half
/// seen-through, and eats it from the top, ragged and glowing where it bites, until nothing is
/// left: streaks at different depths and paces, near ones wider, brighter and quicker, their
/// glowing heads on the bite, far ones fine, dim and slow. Each takes its colour from the part of
/// the window it came from, starting in the window's own colours and turning to the rain's green,
/// and all of them fade once the pane has gone. The failing CRT it is shown on tears worst where
/// the shower bites.
const WAKE_MAIN: &str = r#"
uniform float window_h;
uniform float window_w;
// How fast the shower falls, in screen pixels a second a second.
uniform float gravity;
// One logical pixel, in screen pixels.
uniform float unit;

// The same timings as `WAKE_GO`, `WAKE_DROP` and `WAKE` in `fx.rs`.
const float LEAN = 0.06;
const float GO = 0.42;
const float DROP = 0.5;
const float WAKE = 0.4;
// How far into the distance the pane draws back as it drops.
const float RECEDE = 0.08;
// The bite's ragged edge, up to this many logical pixels behind the shower, in teeth this wide.
const float RAG = 26.0;
const float TOOTH = 14.0;
// How far below the bite the pane glows, in logical pixels.
const float SEAR = 10.0;
// Three layers of streaks, each in cells this many logical pixels wide, some left empty.
const float CELL = 22.0;
const float EMPTY = 0.3;
// How much light the shower gives: it is seen through, over whatever the window uncovers.
const float SHOWER = 0.55;

// How far the shower has fallen from the pane's top edge, `u` seconds in.
float shower_y(float u) {
    float a = max(u - LEAN, 0.0);
    return 0.5 * gravity * a * a;
}

// How far the pane has dropped: the same way as the shower, more slowly.
float pane_y(float u) {
    float a = max(u - LEAN, 0.0) / GO;
    return DROP * content.y * a * a;
}

float pane_scale(float u) {
    return 1.0 - RECEDE * clamp(pane_y(u) / (DROP * content.y), 0.0, 1.0);
}

// How far behind the shower the bite is at `x` across the pane: broad teeth, with a finer edge.
float rag(float x) {
    float c = x / (TOOTH * unit);
    float i = floor(c);
    float f = fract(c);
    float broad = mix(hash(vec2(i, 9.0)), hash(vec2(i + 1.0, 9.0)), f * f * (3.0 - 2.0 * f));
    float fine = hash(vec2(floor(x / (4.0 * unit)), 10.0));
    return (0.75 * broad + 0.25 * fine) * RAG * unit;
}

// The pane's picture at point `p`, `u` seconds in: nothing outside it, nor above the bite, and a
// glow just below it.
vec4 pane_at(vec2 p, float u) {
    vec2 middle = vec2(window_w, window_h) * 0.5;
    float scale = pane_scale(u);
    vec2 local = (p - vec2(0.0, pane_y(u)) - middle) / scale + middle;
    if (local.x < 0.0 || local.y < 0.0 || local.x >= window_w || local.y >= window_h) {
        return vec4(0.0);
    }
    float bite = (shower_y(u) - pane_y(u) - middle.y) / scale + middle.y - rag(local.x);
    float below = local.y - bite;
    if (below <= 0.0) {
        return vec4(0.0);
    }
    vec4 c = picture(local) * clamp(below / (1.5 * unit), 0.0, 1.0);
    float sear = pow(1.0 - clamp(below / (SEAR * unit), 0.0, 1.0), 2.0) * step(LEAN, u);
    vec3 own = c.rgb * 1.2 + ink * 0.2;
    vec3 hot = mix(mix(own, ink, clamp((u - LEAN) / 0.18, 0.0, 1.0)), vec3(1.0), 0.5);
    c.rgb += hot * sear * 0.8;
    return c;
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
    // Far streaks fall behind the bite; near ones keep close to it.
    float lag = 0.01 + 0.16 * (1.0 - near) * hash(vec2(cell, seed + 4.0));
    float u = t - lag;
    if (u <= LEAN) {
        return vec3(0.0);
    }
    // Each streak keeps its own pace, from seven tenths of the shower's to all of it, near ones
    // quicker, so the nearest bite into the pane.
    float pace = 0.7 + 0.3 * mix(hash(vec2(cell, seed + 6.0)), 1.0, near);
    float head = shower_y(u) * pace + 5.0 * unit;
    float speed = gravity * (u - LEAN) * pace;
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
    return (colour * body + hot * glow) * bright * SHOWER;
}

// The pane and its wake at `p`.
vec4 scene(vec2 p) {
    if (p.x < 0.0 || p.x >= content.x) {
        return vec4(0.0);
    }
    // What's left of the pane.
    vec4 color = pane_at(p, t);
    // Its wake, fading once the pane is gone.
    float fade = 1.0 - clamp((t - LEAN - GO) / WAKE, 0.0, 1.0);
    vec3 shower = streak(p, 0.0) + streak(p, 1.0) + streak(p, 2.0);
    color.rgb += shower * fade;
    return color;
}

void main() {
    vec2 px = v_coords * texpx;
    // The picture tears round the shower's front from when the pane lets go until just after it
    // is eaten through. No row is lit: the bite has a glow of its own.
    float eating = step(LEAN, t) * step(t, LEAN + GO + 0.05);
    finish(crt(px, shower_y(t), eating, 0.0, 1.0 - smoothstep(0.25, 0.6, t)));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_closing_effects_go_through_the_failing_crt() {
        assert!(HEAD.contains("vec4 crt(vec2 px"));
        for main in [DEREZ_MAIN, WAKE_MAIN] {
            // Each draws its own scene, and shows it only through the tube.
            assert!(main.contains("vec4 scene(vec2 p) {"));
            assert!(main.contains("finish(crt(px, "));
            assert_eq!(main.matches("finish(").count(), 1);
        }
    }
}
