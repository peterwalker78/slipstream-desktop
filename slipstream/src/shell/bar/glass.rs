//! The lit edge's bar: a pane of frosted glass with air moving through it. The bar's own picture
//! is drawn through one shader that bends it where the air does and lights what the air carries,
//! so the effect is the bar itself rather than a layer laid over it.
//!
//! Slow air drifts along the bar from left to right, and through the glass it shows the way a
//! wind tunnel's schlieren photographs do: as soft light and shade where it gathers and thins,
//! with faint rims in the logo's colours, amber through mint to cyan. It drifts as soft billows of
//! smoke with a fine, fluffy grain, which gather, swell, thin and clear over tens of seconds at
//! any height in the glass. The workspace marker stands in the stream and sheds a curl of smoke
//! every few seconds, alternately above and below it; each drifts downwind, rises, turns and
//! breathes, and fades on its own clock. Two laser projectors at the bar's middle, just out of
//! sight below and above it, fan thin, straight beams out both ways across it, as at a show;
//! each beam is widest at its projector and thins with distance, and brightens where it crosses
//! smoke. A broad sheen slides slowly along the frosted glass, and where it meets the polished
//! foot and the top edge the light catches the glass at a point that glides with it, and a few
//! nicks in the foot flare as it passes. The air bends the bar's picture behind it, more in the
//! lower glass than over the text, with a hint of colour split, and round the text and icons its
//! light is held back so they stay easy to read. The foot is a polished edge with an inner
//! reflection, lit by the light running along it, brighter round the marker and where smoke sits
//! against it.
//!
//! It is drawn in two passes. The smoke, which takes the noise, is worked out into a small field
//! once a frame, a cell for every eight pixels, in rows of the same texture under the bar's
//! picture; the bar's own shader then reads the smoke and the air's gradient from that field, and
//! adds the lasers, the sheen and its glints, and the edge, each of which gives up early where it
//! has nothing to draw. Nothing moves faster than a few pixels a second, so it is drawn about
//! fifteen times a second.
//! When the workspace changes the air rushes: for about a second and a half it runs up to fifteen
//! times as fast, at the display's own rate, and settles back. It holds still while the desktop
//! is fading, with reduced motion, and while the power mode is set to save power, so then it
//! never wakes the screen. Every screen has its own air.

use std::sync::atomic::{AtomicU64, Ordering};

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, ImportMem, Offscreen, Renderer,
            gles::{GlesError, GlesPixelProgram, GlesRenderer, GlesTexture},
        },
    },
    utils::{Logical, Physical, Rectangle, Size, Transform},
};

use crate::effects::shader::{self, Shaded, Values};

/// How often the air is drawn, a second, outside a rush.
pub const STEPS: f64 = 15.0;
/// Seconds between one curl leaving the marker and the next.
const SHED: f64 = 12.0;
/// How many curls are drawn: the newest, and those before it still alive.
const CURLS: usize = 7;
/// The air's own clock starts again from zero after this many seconds: a whole number of curls,
/// so they carry on across it, and short enough that the shader's numbers stay exact.
const WRAP: f64 = 1200.0 * SHED;
/// The rush on a change of workspace: how many times as fast the air runs at its height, how
/// long it takes to get there, how long it holds, and how long it takes to settle back.
const RUSH_PEAK: f64 = 15.0;
const RUSH_UP: f64 = 0.15;
const RUSH_HOLD: f64 = 0.375;
const RUSH_DOWN: f64 = 1.0;
/// The longest step the air takes between two frames, so a stall doesn't throw it forward.
const STEP_MOST: f64 = 0.25;
/// Where a curl is said to have left from when the marker is nowhere: off the bar, unseen.
const NOWHERE: f32 = -1e4;

/// The smoke's field: the smoke's density, worked out once a frame at a quarter of the picture's
/// width and half its height into rows of the same texture under the picture, so the noise is
/// worked out once for every eight pixels and the bar's own shader only reads it, taking the
/// air's gradient from neighbouring cells. Its numbers are in the bar's units unless they say
/// otherwise.
const FIELD: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The picture's width and height in screen pixels, and screen pixels to one of the bar's units.
uniform vec3 frame;
// The air's clock in seconds, the newest curl's number, the marker's middle (far off the bar for
// none), and how strongly the marker glows.
uniform vec4 air;
// Where each curl left from, the newest first: across the bar, in its units.
uniform vec4 origins;
uniform vec3 origins_more;

const float PI = 3.14159265;
// How fast the air drifts and the curls with it, units a second.
const float AIR = 5.0;
const float DRIFT = 6.0;
// The height of the marker's line, which the curls leave from.
const float WAKE_Y = 31.5;

vec3 mod289(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec4 mod289(vec4 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec4 permute(vec4 x) { return mod289(((x * 34.0) + 1.0) * x); }
vec4 taylorInvSqrt(vec4 r) { return 1.79284291400159 - 0.85373472095314 * r; }

// Simplex noise in three dimensions, from webgl-noise by Ian McEwan and Stefan Gustavson.
// Copyright (C) 2011 Ashima Arts, (C) 2011-2016 Stefan Gustavson. Distributed under the MIT
// License.
float snoise(vec3 v) {
    const vec2 C = vec2(1.0 / 6.0, 1.0 / 3.0);
    const vec4 D = vec4(0.0, 0.5, 1.0, 2.0);
    vec3 i = floor(v + dot(v, C.yyy));
    vec3 x0 = v - i + dot(i, C.xxx);
    vec3 g = step(x0.yzx, x0.xyz);
    vec3 l = 1.0 - g;
    vec3 i1 = min(g.xyz, l.zxy);
    vec3 i2 = max(g.xyz, l.zxy);
    vec3 x1 = x0 - i1 + C.xxx;
    vec3 x2 = x0 - i2 + C.yyy;
    vec3 x3 = x0 - D.yyy;
    i = mod289(i);
    vec4 p = permute(permute(permute(i.z + vec4(0.0, i1.z, i2.z, 1.0))
        + i.y + vec4(0.0, i1.y, i2.y, 1.0)) + i.x + vec4(0.0, i1.x, i2.x, 1.0));
    float n_ = 0.142857142857;
    vec3 ns = n_ * D.wyz - D.xzx;
    vec4 j = p - 49.0 * floor(p * ns.z * ns.z);
    vec4 x_ = floor(j * ns.z);
    vec4 y_ = floor(j - 7.0 * x_);
    vec4 x = x_ * ns.x + ns.yyyy;
    vec4 y = y_ * ns.x + ns.yyyy;
    vec4 h = 1.0 - abs(x) - abs(y);
    vec4 b0 = vec4(x.xy, y.xy);
    vec4 b1 = vec4(x.zw, y.zw);
    vec4 s0 = floor(b0) * 2.0 + 1.0;
    vec4 s1 = floor(b1) * 2.0 + 1.0;
    vec4 sh = -step(h, vec4(0.0));
    vec4 a0 = b0.xzyw + s0.xzyw * sh.xxyy;
    vec4 a1 = b1.xzyw + s1.xzyw * sh.zzww;
    vec3 p0 = vec3(a0.xy, h.x);
    vec3 p1 = vec3(a0.zw, h.y);
    vec3 p2 = vec3(a1.xy, h.z);
    vec3 p3 = vec3(a1.zw, h.w);
    vec4 norm = taylorInvSqrt(vec4(dot(p0, p0), dot(p1, p1), dot(p2, p2), dot(p3, p3)));
    p0 *= norm.x;
    p1 *= norm.y;
    p2 *= norm.z;
    p3 *= norm.w;
    vec4 m = max(0.6 - vec4(dot(x0, x0), dot(x1, x1), dot(x2, x2), dot(x3, x3)), 0.0);
    m = m * m;
    return 42.0 * dot(m * m, vec4(dot(p0, x0), dot(p1, x1), dot(p2, x2), dot(p3, x3)));
}

float fbm(vec3 q) {
    return 0.6 * snoise(q) + 0.28 * snoise(q * 2.03 + 7.1) + 0.12 * snoise(q * 4.1 + 3.3);
}

float hash(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}


// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash2(vec2 c) {
    vec3 p3 = fract(vec3(c.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// The fine grain smoke is made of: small soft lumps, the same size every way, carried a little
// faster than the billows they sit in, from 0 to 1.
float grain(vec2 p, float t) {
    vec2 q = vec2(p.x - 1.1 * AIR * t, p.y) / 9.0;
    return 0.5 + 0.5 * (0.65 * snoise(vec3(q, t * 0.08)) + 0.35 * snoise(vec3(q * 2.3 + 4.1, t * 0.11)));
}

// The fog at `p`, given its grain: soft billows carried downwind and curled by the air, gathering
// here and clearing there over tens of seconds, at any height in the glass. It thins softly at
// its edges, and the grain gives it the fluffy texture of real smoke.
float fog(vec2 p, float t, float grained) {
    vec2 q = vec2((p.x - AIR * t) / 150.0, p.y / 18.0);
    vec2 w = vec2(snoise(vec3(q * 0.6, t * 0.025)), snoise(vec3(q * 0.6 + 5.2, t * 0.025 + 3.0)));
    float body = 0.5 + 0.5 * fbm(vec3(q + 1.2 * w, t * 0.03));
    float life = snoise(vec3((p.x - 0.8 * AIR * t) / 380.0, 1.7, t * 0.033));
    float d = smoothstep(0.25, 0.9, body + 0.55 * life + 0.25 * (grained - 0.5));
    d *= 0.6 + 0.8 * grained;
    return d * smoothstep(0.0, 7.0, p.y) * (0.75 + 0.25 * smoothstep(10.0, 39.0, p.y));
}

float origin(int k) {
    if (k == 0) return origins.x;
    if (k == 1) return origins.y;
    if (k == 2) return origins.z;
    if (k == 3) return origins.w;
    if (k == 4) return origins_more.x;
    if (k == 5) return origins_more.y;
    return origins_more.z;
}

// The marker's curls as smoke: each one drifts downwind and rises from where it left, swells,
// breathes, turns and fades on its own clock.
float curls(vec2 p, float t) {
    float acc = 0.0;
    for (int k = 0; k < 7; k++) {
        float from = origin(k);
        if (from < -1000.0) continue;
        float n = air.y - float(k);
        float a = t - n * SHED;
        float life = 45.0 + 30.0 * hash(n);
        float side = mod(n, 2.0) < 0.5 ? 1.0 : -1.0;
        float across = from + 22.0 + DRIFT * a;
        // Too far along the bar to reach here at its widest breath: skip its noise.
        if (abs(p.x - across) > 5.2 * (4.0 + 0.16 * a)) continue;
        float lift = -0.22 * a + 10.0 * snoise(vec3(n * 1.7, a * 0.03, 5.0));
        vec2 c = vec2(across, clamp(WAKE_Y + side * 3.0 + lift, 7.0, 35.0));
        float breathe = 1.0 + 0.3 * snoise(vec3(n * 3.1, a * 0.12, 0.0));
        float r0 = (4.0 + 0.16 * a) * breathe;
        float fade = smoothstep(0.0, 6.0, a) * smoothstep(life, life * 0.45, a);
        vec2 d = p - c;
        float r = length(d);
        float arm = 0.5 + 0.5 * sin(atan(d.y, d.x) + side * 1.6 * r / r0 - side * 0.3 * a);
        acc += fade * exp(-r * r / (1.8 * r0 * r0)) * (0.55 + 0.45 * arm);
    }
    return acc;
}

// All the smoke at `p`: the fog, and the curls, with the same grain.
float smoke(vec2 p, float t) {
    float grained = grain(p, t);
    return fog(p, t, grained) + 1.2 * curls(p, t) * (0.6 + 0.4 * grained);
}

// The smoke here, for the picture's pixels under this one of the field's.
void main() {
    float here = clamp(smoke(v_coords * frame.xy / frame.z, air.x), 0.0, 1.0);
    gl_FragColor = vec4(here, here, here, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        gl_FragColor = vec4(0.0, 0.2, 0.0, 0.2) + gl_FragColor * 0.8;
#endif
}
"#;

/// The bar's shader: the picture bent by the air, with the light the smoke, the lasers, the sheen
/// and the edge add to it. Its colours are premultiplied, and the light is added to them without
/// changing their alpha.
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

// The picture's width and height in screen pixels, screen pixels to one of the bar's units, and
// the height of the texture holding the picture with the smoke's field under it.
uniform vec4 frame;
// The air's clock in seconds, the newest curl's number, the marker's middle (far off the bar for
// none), and how strongly the marker glows.
uniform vec4 air;
uniform vec3 ring;
// Where a docked pane's frame carries the bar on down, so its foot has no edge: left and right.
uniform vec2 gap;

const float PI = 3.14159265;
const vec3 AMBER = vec3(0.96, 0.66, 0.30);
const vec3 MINT = vec3(0.42, 0.90, 0.70);
const vec3 PALE = vec3(0.86, 0.97, 1.0);

// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash2(vec2 c) {
    vec3 p3 = fract(vec3(c.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// The smoke at the picture's pixel `px`, from the field in the texture's rows under the picture.
float field(vec2 px) {
    vec2 cells = ceil(frame.xy / vec2(FIELD_X, FIELD_Y));
    vec2 at = clamp(px / vec2(FIELD_X, FIELD_Y), vec2(0.5), cells - 0.5);
    return texture2D(tex, vec2(at.x, frame.y + at.y) / frame.xw).r;
}

// One laser beam from `from` along `dir`, of colour `hue`, through smoke `through`: a thin, straight
// line, the same brightness all along, with a whiter core and a faint glow, as the air it lights
// is seen from the side. In perspective it is widest at its projector and thins as it goes away.
// Smoke it crosses lights up more brightly.
vec3 beam(vec2 p, vec2 from, vec2 dir, vec3 hue, float f, float through) {
    vec2 rel = p - from;
    float along = dot(rel, dir);
    float width = max(1.6 * f * (1.0 - along / 2600.0), 0.5);
    float off = (rel.x * dir.y - rel.y * dir.x) * f;
    if (along < 0.0 || abs(off) > 10.0 * width) return vec3(0.0);
    float core = exp(-off * off / (0.5 * width * width));
    float glow = exp(-off * off / (9.0 * width * width));
    return (mix(hue, vec3(1.0), 0.5 * core) * core + 0.25 * hue * glow)
        * exp(-along / 1800.0) * (0.35 + 0.9 * through);
}

// Two laser projectors at the bar's middle, as at a show: one just below its foot fans four beams
// out and up both ways, one just above its top fans two out and down. Both are out of sight, so
// the beams cross the bar clear of the clock. The fans scan slowly and drift a little.
vec3 lasers(vec2 p, float t, float wide, float f, float through) {
    vec3 acc = vec3(0.0);
    vec2 low = vec2(wide * 0.5 + 30.0 * sin(2.0 * PI * t / 190.0), 58.0);
    for (int i = 0; i < 4; i++) {
        float fi = float(i);
        float side = i < 2 ? -1.0 : 1.0;
        float k = mod(fi, 2.0);
        float angle = 0.034 + 0.02 * k + 0.008 * sin(2.0 * PI * t / (50.0 + 9.0 * fi) + fi * 1.7);
        vec3 hue = k < 0.5 ? ring : MINT;
        acc += beam(p, low, vec2(side * cos(angle), -sin(angle)), hue, f, through);
    }
    vec2 high = vec2(wide * 0.5 - 30.0 * sin(2.0 * PI * t / 230.0 + 1.0), -18.0);
    for (int i = 0; i < 2; i++) {
        float fi = float(i);
        float side = i == 0 ? -1.0 : 1.0;
        float angle = 0.03 + 0.008 * sin(2.0 * PI * t / (61.0 + 13.0 * fi) + fi * 2.3);
        acc += beam(p, high, vec2(side * cos(angle), sin(angle)), AMBER, f, through);
    }
    return acc;
}

// Light catching the glass at points, as it does on a real pane: where the sheen meets the
// polished foot, and its top edge, the glass flares at one point that glides along with it, a
// bright point drawn out a little along the edge with a soft bloom, swelling and fading as the
// pane trims; and a few fixed nicks in the foot flare as the sheen passes over them. `crossing`
// is where the sheen meets the foot, in the bar's units.
vec3 glints(vec2 px, vec2 p, float t, float f, float wide, float crossing) {
    float swell = 0.55 + 0.45 * sin(2.0 * PI * t / 37.0);
    vec3 acc = vec3(0.0);
    for (int e = 0; e < 2; e++) {
        // The foot, and the top edge, where the sheen meets it 37 units further on.
        float y = e == 0 ? frame.y - 0.5 * f : 0.5;
        float x = e == 0 ? crossing : crossing + 37.0;
        vec2 d = vec2((p.x - x) * f, px.y - y);
        if (abs(d.x) > 120.0 * f || abs(d.y) > 10.0 * f) continue;
        float point = exp(-dot(d, d) / (1.2 * f * f));
        float drawn = exp(-abs(d.x) / (18.0 * f)) * exp(-d.y * d.y / (0.35 * f * f));
        float bloom = exp(-dot(d, d) / (36.0 * f * f));
        acc += PALE * swell * (e == 0 ? 1.0 : 0.6) * (0.9 * point + 0.35 * drawn + 0.08 * bloom);
    }
    if (p.y > 33.0) {
        for (int k = 0; k < 5; k++) {
            float x = wide * (0.08 + 0.84 * hash2(vec2(float(k) * 7.31, 1.7)));
            float flare = exp(-pow((x - crossing) / 70.0, 2.0));
            vec2 d = vec2((p.x - x) * f, px.y - (frame.y - 0.5 * f));
            acc += PALE * flare * (0.7 * exp(-dot(d, d) / (0.8 * f * f))
                + 0.06 * exp(-dot(d, d) / (25.0 * f * f)));
        }
    }
    return acc;
}

void main() {
    float f = frame.z;
    vec2 px = v_coords * frame.xw;
    vec2 p = px / f;
    float wide = frame.x / f;
    float t = air.x;

    // The smoke, worked out at a lower resolution once a frame, and the air's gradient across
    // the field's neighbouring cells, as a schlieren photograph reads it: its strength and which
    // way it runs, the way giving the colour.
    float here = field(px);
    vec2 step_px = vec2(FIELD_X, FIELD_Y);
    vec2 g = vec2(field(px + vec2(step_px.x, 0.0)) - field(px - vec2(step_px.x, 0.0)),
                  field(px + vec2(0.0, step_px.y)) - field(px - vec2(0.0, step_px.y)))
        * f / (2.0 * step_px);
    g = clamp(g * vec2(26.0, 7.0), -GRADIENT, GRADIENT);
    float strength = length(g);
    float way = 0.5 + 0.5 * sin(atan(g.y, g.x) + 0.6);
    vec3 col = way < 0.5 ? mix(AMBER, MINT, way * 2.0) : mix(MINT, ring, way * 2.0 - 1.0);
    float glass = step(p.y, 39.0);

    // The picture behind, bent by the air: a little over the text, more in the lower glass, with
    // a hint of colour split.
    float bending = glass * smoothstep(8.0, 30.0, p.y) * mix(0.45, 1.0, smoothstep(24.0, 32.0, p.y));
    // Clear of the text it isn't bent at all, so takes a single read.
    vec4 behind = texture2D(tex, v_coords);
    if (bending > 0.0) {
        vec2 bend = clamp(g, -1.5, 1.5) * bending * f / frame.xw;
        // Never into the field's rows under the picture.
        float floor_y = (frame.y - 0.5) / frame.w;
        behind = texture2D(tex, vec2(v_coords.x + bend.x, min(v_coords.y + bend.y, floor_y)));
        behind.r = texture2D(tex, vec2(v_coords.x + 1.45 * bend.x, min(v_coords.y + 1.45 * bend.y, floor_y))).r;
        behind.b = texture2D(tex, vec2(v_coords.x + 0.55 * bend.x, min(v_coords.y + 0.55 * bend.y, floor_y))).b;
    }
#if defined(NO_ALPHA)
    behind = vec4(behind.rgb, 1.0);
#endif

    // The pane: a tint deepening towards the foot, and the sheen sliding slowly along it, faintly
    // grainy as frosting is.
    float body = smoothstep(14.0, 39.0, p.y);
    float sheen_at = 0.5 + 0.34 * sin(2.0 * PI * t / 960.0 + 2.2) + 0.06 * sin(2.0 * PI * t / 310.0);
    float from_sheen = (p.x - sheen_at * wide + (p.y - 20.0) * 1.9) / 260.0;
    float sheen = exp(-from_sheen * from_sheen);
    float frosting = 0.75 + 0.5 * hash2(floor(px) + 5.5);
    vec3 light = (ring * 0.016 * body + vec3(0.8, 0.92, 1.0) * 0.05 * sheen * frosting * (0.4 + 0.6 * body)) * glass;

    // The smoke catches the light, as edge-lit glass does, nearly white with a touch of the ring,
    // and its rims show faintly in the logo's colours; the lasers cross it.
    light += mix(PALE, ring, 0.25) * smoothstep(0.0, 1.0, here) * (0.08 + 0.06 * body) * glass;
    light += mix(PALE, col, 0.4) * smoothstep(0.08, 0.8, strength) * 0.05 * glass * smoothstep(16.0, 39.0, p.y);
    light += lasers(p, t, wide, f, smoothstep(0.05, 0.9, here)) * 0.4 * glass;

    // Text and icons stay easy to read: round anything bright in the picture, the air's light
    // is held back. A few pixels either way are looked at, so a glyph keeps a dark margin.
    if (dot(light, vec3(1.0)) > 0.01) {
        vec2 reach = 3.0 * f / frame.xw;
        float ink = 0.0;
        ink = max(ink, dot(texture2D(tex, v_coords).rgb, vec3(0.33)));
        ink = max(ink, dot(texture2D(tex, v_coords + vec2(reach.x, 0.0)).rgb, vec3(0.33)));
        ink = max(ink, dot(texture2D(tex, v_coords - vec2(reach.x, 0.0)).rgb, vec3(0.33)));
        ink = max(ink, dot(texture2D(tex, vec2(v_coords.x, min(v_coords.y + reach.y, (frame.y - 0.5) / frame.w))).rgb, vec3(0.33)));
        ink = max(ink, dot(texture2D(tex, v_coords - vec2(0.0, reach.y)).rgb, vec3(0.33)));
        light *= 1.0 - 0.8 * smoothstep(0.15, 0.4, ink);
    }

    // Where the light catches the glass, after that: it never sits on the text.
    float crossing = sheen_at * wide - (39.5 - 20.0) * 1.9;
    light += glints(px, p, t, f, wide, crossing);

    // The polished edge along the foot, wavering with the air: a bright outer face and an inner
    // reflection, brighter round the marker and where fog sits against it. None where a docked
    // pane's frame carries the bar on down.
    float foot = frame.y - 0.5 * f;
    float waver = clamp(g.y * 5.0 / 7.0, -0.8, 0.8) * f;
    float d1 = (px.y - (foot + waver)) / f;
    float d2 = (px.y - (foot - 2.2 * f + 0.6 * waver)) / f;
    float near = exp(-pow((p.x - air.z) / 300.0, 2.0)) * air.w;
    // Only the rows round the foot have an edge to light.
    float lit = 0.0;
    if (p.y > 35.0) {
        lit = 0.26 + 0.24 * near + 0.45 * smoothstep(0.0, 0.8, field(vec2(px.x, 38.0 * f)));
    }
    float in_gap = clamp(min(px.x + 0.5, gap.y * f) - max(px.x - 0.5, gap.x * f), 0.0, 1.0);
    float edge = (exp(-d1 * d1 / 0.38) + 0.34 * glass * exp(-d2 * d2 / 0.5)) * lit * (1.0 - in_gap);
    light += mix(ring, PALE, 0.18 + 0.2 * near) * edge;

    vec4 color = vec4(behind.rgb + min(light, vec3(0.7, 0.9, 1.0)) * behind.a, behind.a) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// Mark the glass's programs in a renderer's keeping.
struct Program;
struct FieldProgram;

/// The smoke's field has one cell for this many screen pixels across, and this many down.
const FIELD_X: f32 = 4.0;
const FIELD_Y: f32 = 2.0;
/// The largest gradient of the air the bar's shader takes, either way.
const GRADIENT: f32 = 2.0;

/// The shaders with the numbers they share with the clock and the field here.
fn source() -> String {
    shader::with_constants(
        SHADER,
        &[
            ("FIELD_X", &FIELD_X),
            ("FIELD_Y", &FIELD_Y),
            ("GRADIENT", &GRADIENT),
        ],
    )
}

fn field_source() -> String {
    shader::with_constants(FIELD, &[("SHED", &SHED)])
}

/// The bar's shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    frame: [f32; 4],
    air: [f32; 4],
    ring: [f32; 3],
    gap: [f32; 2],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("air", self.air)
            .with("ring", self.ring)
            .with("gap", self.gap)
    }
}

/// The field's shader's numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FieldUniforms {
    frame: [f32; 3],
    air: [f32; 4],
    origins: [f32; 4],
    origins_more: [f32; 3],
}

impl shader::Uniforms for FieldUniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("air", self.air)
            .with("origins", self.origins)
            .with("origins_more", self.origins_more)
    }
}

/// How many times as fast the air runs `since` seconds after a rush began.
fn rush(since: f64) -> f64 {
    if !(0.0..=RUSH_UP + RUSH_HOLD + RUSH_DOWN).contains(&since) {
        return 1.0;
    }
    let up = (since / RUSH_UP).min(1.0);
    let down = 1.0 - ((since - RUSH_UP - RUSH_HOLD) / RUSH_DOWN).max(0.0);
    let e = up.min(down);
    1.0 + (RUSH_PEAK - 1.0) * e * e * (3.0 - 2.0 * e)
}

/// How many screens' air has been started, so each one's starts somewhere else.
static AIRS: AtomicU64 = AtomicU64::new(0);

/// One screen's air: its own clock, which a rush runs faster, and where its curls left from.
#[derive(Debug)]
pub struct Air {
    /// The air's clock, in seconds.
    t: f64,
    /// When it was last moved on, by the wall clock.
    last: Option<f64>,
    /// When a rush began, by the wall clock, or that one is to begin on the next frame.
    rush: Rush,
    /// The step of a fifteenth of a second last drawn, and the clock as it was drawn then.
    held: (i64, f64),
    /// Each curl's number and where it left from, by its number's place in the ring.
    origins: [(i64, f32); CURLS + 1],
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rush {
    None,
    Asked,
    Since(f64),
}

impl Default for Air {
    fn default() -> Self {
        let n = AIRS.fetch_add(1, Ordering::Relaxed);
        Self::starting_at((n as f64 * 1777.0) % WRAP)
    }
}

impl Air {
    fn starting_at(t: f64) -> Self {
        Self {
            t,
            last: None,
            rush: Rush::None,
            held: (i64::MIN, t),
            origins: [(i64::MIN, NOWHERE); CURLS + 1],
        }
    }

    /// The air rushes from the next frame it moves on.
    pub fn rush_now(&mut self) {
        self.rush = Rush::Asked;
    }

    /// Whether a rush waits for the next frame.
    #[cfg(test)]
    pub fn asked_to_rush(&self) -> bool {
        self.rush == Rush::Asked
    }

    /// Moves the clock on to `now`, if it `moves`, with the marker's middle at `marker` in the
    /// bar's units. Says the time to draw at: `now`'s own during a rush, so it runs at the
    /// display's rate, and otherwise as it was at the last fifteenth of a second, so the screen
    /// is woken only that often. Standing still, the time holds.
    fn advance(&mut self, now: f64, moves: bool, marker: Option<f32>) -> f64 {
        if !moves {
            self.rush = Rush::None;
        } else if self.rush == Rush::Asked {
            self.rush = Rush::Since(now);
        }
        let rushing = match self.rush {
            Rush::Since(from) if now - from <= RUSH_UP + RUSH_HOLD + RUSH_DOWN => Some(from),
            _ => None,
        };
        if moves && let Some(last) = self.last {
            let speed = rushing.map_or(1.0, |from| rush(now - from));
            self.t = (self.t + (now - last).clamp(0.0, STEP_MOST) * speed) % WRAP;
        }
        self.last = Some(now);
        let step = (now * STEPS).floor() as i64;
        if !moves || rushing.is_some() || step != self.held.0 {
            self.held = (step, self.t);
        }
        let at = self.held.1;
        self.note_curls(at, marker.unwrap_or(NOWHERE));
        at
    }

    /// The newest curl's number at `at`.
    fn newest(at: f64) -> i64 {
        (at / SHED).floor() as i64
    }

    /// Notes where the newest curl at `at` left from, the marker's middle at `marker`, if it is
    /// new.
    fn note_curls(&mut self, at: f64, marker: f32) {
        let n = Self::newest(at);
        let place = &mut self.origins[n.rem_euclid(CURLS as i64 + 1) as usize];
        if place.0 != n {
            *place = (n, marker);
        }
    }

    /// Where each of the curls alive at `at` left from, the newest first. One left before this
    /// screen's air began is taken to have left from the marker where it is now.
    fn curl_origins(&self, at: f64, marker: f32) -> [f32; CURLS] {
        let newest = Self::newest(at);
        std::array::from_fn(|k| {
            let n = newest - k as i64;
            let (noted, from) = self.origins[n.rem_euclid(CURLS as i64 + 1) as usize];
            if noted == n { from } else { marker }
        })
    }
}

/// The bar's picture, uploaded, and its size in screen pixels.
struct Picture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
}

/// The texture the bar is drawn from: the picture, and the smoke's field in the rows under it.
struct Atlas {
    texture: GlesTexture,
    /// The picture's size, which it was made for.
    size: Size<i32, Physical>,
    /// The picture is in it as last painted.
    holds_picture: bool,
    /// The air's time its field was last worked out for.
    field_at: Option<f64>,
}

impl Atlas {
    /// The field's size in cells for a picture `size` screen pixels.
    fn cells(size: Size<i32, Physical>) -> Size<i32, Physical> {
        (
            (size.w as f32 / FIELD_X).ceil() as i32,
            (size.h as f32 / FIELD_Y).ceil() as i32,
        )
            .into()
    }

    /// The texture's whole size for a picture `size` screen pixels: the field's rows under it.
    fn whole(size: Size<i32, Physical>) -> Size<i32, Physical> {
        (size.w, size.h + Self::cells(size).h).into()
    }
}

/// One screen's glass: the picture it bends, its air, and its element kept from frame to frame.
#[derive(Default)]
pub struct Glass {
    picture: Option<Picture>,
    atlas: Option<Atlas>,
    pub air: Air,
    kept: shader::Kept,
}

/// What the glass is drawn with, besides its picture.
pub struct Look {
    /// The bar's place, in logical pixels.
    pub area: Rectangle<i32, Logical>,
    /// Screen pixels to one of the bar's units.
    pub f: f32,
    /// The marker's middle, in the bar's units, and how strongly it glows.
    pub marker: Option<f32>,
    pub strength: f32,
    pub ring: u32,
    /// Where a docked pane's frame carries the bar on down, in the bar's units.
    pub gap: Option<(f32, f32)>,
    pub alpha: f32,
    /// Whether the air moves.
    pub moves: bool,
    pub now: f64,
}

impl Glass {
    /// Takes the bar's picture just painted, to bend.
    pub fn repainted(&mut self, renderer: &mut GlesRenderer, pixmap: &Pixmap) {
        let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
        self.picture = renderer
            .import_memory(pixmap.data(), Fourcc::Abgr8888, (w, h).into(), false)
            .map_err(|err| tracing::warn!("couldn't upload the bar's picture: {err}"))
            .ok()
            .map(|texture| Picture {
                texture,
                size: (w, h).into(),
            });
        if let Some(atlas) = self.atlas.as_mut() {
            atlas.holds_picture = false;
        }
    }

    /// Lets the picture go, in a style that doesn't need it.
    pub fn forget(&mut self) {
        self.picture = None;
        self.atlas = None;
    }

    /// The bar as `look` says. `None` without a picture, or if either shader didn't compile or
    /// the texture couldn't be made or drawn, so the bar is drawn as painted instead.
    pub fn element(&mut self, renderer: &mut GlesRenderer, look: &Look) -> Option<Shaded> {
        let picture = self.picture.as_ref()?;
        let program = shader::program::<Program, _>(
            renderer,
            |renderer| {
                renderer.compile_custom_texture_shader(source(), &shader::names::<Uniforms>())
            },
            "the lit edge's glass",
        )?;
        let field = shader::program::<FieldProgram, _>(
            renderer,
            |renderer| {
                renderer
                    .compile_custom_pixel_shader(field_source(), &shader::names::<FieldUniforms>())
            },
            "the lit edge's smoke",
        )?;
        let at = self.air.advance(look.now, look.moves, look.marker);
        if self
            .atlas
            .as_ref()
            .is_none_or(|atlas| atlas.size != picture.size)
        {
            let whole = Atlas::whole(picture.size);
            let texture = renderer
                .create_buffer(Fourcc::Abgr8888, (whole.w, whole.h).into())
                .map_err(|err| tracing::warn!("no texture for the lit edge's glass: {err}"))
                .ok()?;
            self.atlas = Some(Atlas {
                texture,
                size: picture.size,
                holds_picture: false,
                field_at: None,
            });
        }
        let atlas = self.atlas.as_mut()?;
        if !atlas.holds_picture || atlas.field_at != Some(at) {
            let numbers = field_uniforms(&self.air, at, picture.size, look);
            if let Err(err) = fill(renderer, atlas, picture, &field, &numbers) {
                tracing::warn!("couldn't draw the lit edge's smoke: {err}");
                self.atlas = None;
                return None;
            }
            atlas.holds_picture = true;
            atlas.field_at = Some(at);
            self.kept.changed();
        }
        let whole = Atlas::whole(atlas.size);
        let uniforms = Uniforms {
            frame: [
                atlas.size.w as f32,
                atlas.size.h as f32,
                look.f,
                whole.h as f32,
            ],
            air: [
                at as f32,
                Air::newest(at) as f32,
                marker(look),
                strength(look),
            ],
            ring: rgb(look.ring),
            gap: look.gap.unwrap_or((-1.0, -1.0)).into(),
        };
        let part = Rectangle::from_size((atlas.size.w as f64, atlas.size.h as f64).into());
        Some(self.kept.texture_part(
            (program, atlas.texture.clone(), part),
            look.area,
            look.alpha,
            &uniforms,
        ))
    }
}

/// Draws into `atlas` the picture, if it doesn't hold it yet, and the smoke's field with
/// `numbers`.
fn fill(
    renderer: &mut GlesRenderer,
    atlas: &mut Atlas,
    picture: &Picture,
    field: &GlesPixelProgram,
    numbers: &FieldUniforms,
) -> Result<(), GlesError> {
    let size = atlas.size;
    let cells = Atlas::cells(size);
    let mut target = renderer.bind(&mut atlas.texture)?;
    let mut frame = renderer.render(&mut target, Atlas::whole(size), Transform::Normal)?;
    if !atlas.holds_picture {
        let rect = Rectangle::<i32, Physical>::from_size(size);
        frame.clear(Color32F::TRANSPARENT, &[rect])?;
        frame.render_texture_from_to(
            &picture.texture,
            Rectangle::from_size((size.w as f64, size.h as f64).into()),
            rect,
            &[rect],
            &[],
            Transform::Normal,
            1.0,
            None,
            &[],
        )?;
    }
    let (list, len) = shader::Uniforms::values(numbers).listed();
    frame.render_pixel_shader_to(
        field,
        Rectangle::from_size((cells.w as f64, cells.h as f64).into()),
        Rectangle::new((0, size.h).into(), cells),
        (cells.w, cells.h).into(),
        Some(&[Rectangle::from_size(cells)]),
        1.0,
        &list[..len],
    )?;
    // The bar is drawn from it later on the same context, which keeps the order of its
    // commands, so there is nothing to wait for.
    let _ordered = frame.finish()?;
    Ok(())
}

/// The field's numbers at the air's time `at`, for a picture `size` screen pixels.
fn field_uniforms(air: &Air, at: f64, size: Size<i32, Physical>, look: &Look) -> FieldUniforms {
    let origins = air.curl_origins(at, marker(look));
    FieldUniforms {
        frame: [size.w as f32, size.h as f32, look.f],
        air: [
            at as f32,
            Air::newest(at) as f32,
            marker(look),
            strength(look),
        ],
        origins: [origins[0], origins[1], origins[2], origins[3]],
        origins_more: [origins[4], origins[5], origins[6]],
    }
}

/// The marker's middle in the bar's units, or far off the bar without one.
fn marker(look: &Look) -> f32 {
    look.marker.unwrap_or(NOWHERE)
}

/// How strongly the marker glows: not at all without one.
fn strength(look: &Look) -> f32 {
    if look.marker.is_some() {
        look.strength
    } else {
        0.0
    }
}

fn rgb(c: u32) -> [f32; 3] {
    [
        (c >> 24) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(air: &mut Air, from: f64, secs: f64, fps: f64) -> Vec<f64> {
        let frames = (secs * fps) as usize;
        (0..=frames)
            .map(|n| air.advance(from + n as f64 / fps, true, Some(60.0)))
            .collect()
    }

    #[test]
    fn the_air_is_drawn_fifteen_times_a_second_and_runs_at_the_wall_clocks_pace() {
        let mut air = Air::starting_at(100.0);
        let seen = moved(&mut air, 10.0, 4.0, 60.0);
        let mut changes = seen.clone();
        changes.dedup();
        assert!(
            (59..=61).contains(&changes.len()),
            "{} changes",
            changes.len()
        );
        let went = seen.last().unwrap() - seen.first().unwrap();
        assert!((went - 4.0).abs() < 0.1, "went {went}");
    }

    #[test]
    fn a_rush_runs_up_to_fifteen_times_as_fast_for_about_a_second_and_a_half_at_every_frame() {
        let mut air = Air::starting_at(100.0);
        moved(&mut air, 10.0, 1.0, 60.0);
        air.rush_now();
        let seen = moved(&mut air, 11.0, 1.525, 60.0);
        let mut changes = seen.clone();
        changes.dedup();
        assert_eq!(
            changes.len(),
            seen.len(),
            "every frame moves while it rushes"
        );
        let went = seen.last().unwrap() - seen.first().unwrap();
        // Up, held and down again: about 1 + 14 × (0.075 + 0.375 + 0.5) seconds of air.
        assert!((13.0..16.0).contains(&went), "went {went}");
        assert_eq!(rush(0.3), RUSH_PEAK);
        assert_eq!(rush(2.0), 1.0);
        // And it settles back to fifteen steps a second.
        let after = moved(&mut air, 12.6, 2.0, 60.0);
        let mut changes = after.clone();
        changes.dedup();
        assert!(changes.len() <= 32, "{} changes", changes.len());
    }

    #[test]
    fn standing_still_the_air_holds_and_takes_up_again_without_a_jump() {
        let mut air = Air::starting_at(100.0);
        let before = *moved(&mut air, 10.0, 1.0, 60.0).last().unwrap();
        air.rush_now();
        for n in 0..120 {
            assert_eq!(
                air.advance(11.0 + n as f64 / 60.0, false, Some(60.0)),
                before
            );
        }
        let again = air.advance(13.0, true, Some(60.0));
        assert!(again - before < 0.1, "no jump: {}", again - before);
        assert_eq!(
            air.rush,
            Rush::None,
            "a rush asked for while still is let go"
        );
    }

    #[test]
    fn a_curl_keeps_where_it_left_from_when_the_marker_moves_on() {
        let mut air = Air::starting_at(SHED * 10.0 + 1.0);
        air.advance(0.0, true, Some(60.0));
        // Past the next curl, with the marker moved to workspace 3.
        let mut at = 0.0;
        for n in 1..=(SHED as usize * 60) {
            at = air.advance(n as f64 / 60.0, true, Some(140.0));
        }
        let origins = air.curl_origins(at, 140.0);
        assert_eq!(
            origins[0], 140.0,
            "the newest left from where the marker is now"
        );
        assert_eq!(origins[1], 60.0, "the one before from where it was");
    }

    #[test]
    fn the_clock_wraps_on_a_whole_curl() {
        assert_eq!(WRAP % SHED, 0.0);
        let mut air = Air::starting_at(WRAP - 0.5);
        let seen = moved(&mut air, 0.0, 1.0, 60.0);
        assert!(seen.last().unwrap() < &1.0);
    }

    #[test]
    fn the_shared_numbers_come_after_the_shaders_precision() {
        let source = field_source();
        let precision = source.find("precision highp float;").unwrap();
        assert!(source.find("const float SHED = 12.0;").unwrap() > precision);
    }

    #[test]
    fn the_field_sits_under_the_picture_with_a_cell_for_every_four_by_two_pixels() {
        let size = Size::<i32, Physical>::from((1920, 40));
        assert_eq!(Atlas::cells(size), (480, 20).into());
        assert_eq!(Atlas::whole(size), (1920, 60).into());
        // A picture that doesn't divide evenly still has every pixel covered.
        let odd = Size::<i32, Physical>::from((1537, 41));
        assert_eq!(Atlas::cells(odd), (385, 21).into());
    }

    #[test]
    fn the_shared_numbers_are_declared_in_both_shaders() {
        assert!(source().contains("const float FIELD_X = 4.0;"));
        assert!(source().contains("const float GRADIENT = 2.0;"));
        assert!(field_source().contains("const float SHED = 12.0;"));
    }
}
