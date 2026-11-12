//! The lit edge's bar: a pane of frosted glass with air moving through it. The bar's own picture
//! is drawn through one shader that bends it where the air does and lights what the air carries,
//! so the effect is the bar itself rather than a layer laid over it.
//!
//! The wind comes down the tunnel towards you from the bar's middle, and through the glass it shows
//! the way a wind tunnel's schlieren photographs do: as soft light and shade where it gathers and
//! thins, with faint rims in the theme's three colours. It carries soft billows of smoke with a
//! fine, fluffy grain, which come from far off at the middle, small and faint, and grow and speed
//! up as they stream out to either end towards you, gathering, swelling, thinning and clearing over
//! tens of seconds at any height in the glass. The workspace marker stands in the stream and sheds
//! a curl of smoke every few seconds, alternately above and below it; each comes towards you,
//! swelling, drifts outwards, rises, turns and breathes, and fades on its own clock. A laser
//! projector at the bar's very middle, behind the clock, throws sharp, vivid beams in the primary
//! colour out both ways, as at a show, in a pattern symmetrical left to right and top to bottom:
//! two pairs each way that open and close against each other like scissors, never turning faster
//! than ten degrees a minute, and swell and fade in turn. Each beam is widest at the projector and
//! thins with distance, and where it meets the glass's top edge or its foot it catches the glass in
//! a glint. A broad sheen slides slowly along the frosted glass, kept faint behind the clock as the
//! smoke is. The air bends the bar's picture behind it, more in the lower glass than over the text,
//! with a hint of colour split, and round the text and icons its light is held back so they stay
//! easy to read: a beam nearing text fades gradually. The foot is a polished edge with an inner
//! reflection, lit by the light running along it, brighter round the marker and where smoke sits
//! against it.
//!
//! It is drawn in two passes. The smoke, which takes the noise, is worked out into a small field
//! once a frame, a cell for every eight pixels, in rows of the same texture under the bar's
//! picture, beside an ink map of how near each spot is to text, worked out only when the picture
//! is painted; the bar's own shader then reads the smoke and the air's gradient from that field, and
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
            gles::{GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture},
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
/// The breath in before a rush, seconds: the air keeps its pace while the lasers dip and close a
/// hair, then the rush blows through.
const INTAKE: f64 = 0.12;
/// How long the whole rush lasts, breath and all.
const RUSH_LONG: f64 = INTAKE + RUSH_UP + RUSH_HOLD + RUSH_DOWN;
/// How long the projector takes to power up as the desktop comes back, and to go down as it
/// fades, seconds.
const POWER_UP: f64 = 0.5;
const POWER_DOWN: f64 = 0.3;
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
// The thin haze everywhere, as a share of the thickest smoke.
const float HAZE = 0.12;
// How fast the smoke comes down the tunnel towards you, in its depth a second, and how fast the
// curls drift outwards, units a second.
const float TOWARDS = 0.22;
const float OUTWARDS = 1.5;
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

// Where `p` lies in the tunnel, the wind coming down it towards you from the bar's middle: how
// deep it is (far at the middle, near at the ends, as in perspective), how high, and which side of
// the middle. The smoke is drawn in these, so it comes from far off at the middle, small, and grows
// and speeds up as it streams out to either end towards you.
vec3 tunnel(vec2 p) {
    float centre = 0.5 * frame.x / frame.z;
    float out_from = max(abs(p.x - centre), 40.0);
    float depth = 1000.0 / out_from;
    return vec3(depth, (p.y - 20.0) * depth / 40.0, sign(p.x - centre));
}

// The fine grain smoke is made of: small soft lumps that come down the tunnel with it, from 0 to
// 1.
float grain(vec3 at, float t) {
    vec3 q = vec3((at.x * 5.4 + TOWARDS * t) * 2.2, at.y * 6.0, at.z * 11.0 + t * 0.05);
    return 0.5 + 0.5 * (0.65 * snoise(q) + 0.35 * snoise(q * 2.3 + 4.1));
}

// The fog at `p`, given where it lies in the tunnel and its grain: soft billows coming towards
// you, curled by the air, gathering here and clearing there over tens of seconds, at any height in
// the glass. It thins softly at its edges, the grain gives it the fluffy texture of real smoke, and
// far off down the tunnel it is fainter.
float fog(vec2 p, vec3 at, float t, float grained) {
    vec3 q = vec3((at.x * 5.4 + TOWARDS * t) * 0.5, at.y * 1.6, at.z * 5.0);
    // The billows change only slowly as they come, so the eye can follow one past.
    vec2 w = vec2(snoise(q * 0.6 + vec3(0.0, 0.0, t * 0.007)),
                  snoise(q * 0.6 + vec3(5.2, 0.0, t * 0.007 + 3.0)));
    float body = 0.5 + 0.5 * fbm(q + vec3(1.2 * w, t * 0.01));
    float life = snoise(vec3((at.x * 5.4 + TOWARDS * t) * 0.12, at.z * 3.0, t * 0.012));
    float d = smoothstep(0.25, 0.9, body + 0.55 * life + 0.25 * (grained - 0.5));
    d *= 0.6 + 0.8 * grained;
    // Nearer smoke, on the near side of the tunnel, streaks past twice as fast towards the ends,
    // fainter, so you feel yourself moving through it.
    float out_from = 1000.0 / at.x;
    float near_n = (at.x * 2.7 + TOWARDS * t) * 1.2;
    float wisps = 0.5 + 0.5 * fbm(vec3(near_n, at.y * 0.4, at.z * 9.0 + t * 0.004));
    d = max(d, 0.6 * smoothstep(0.55, 0.9, wisps) * smoothstep(150.0, 600.0, out_from));
    // A thin haze fills the whole glass between the billows, as in a club, so the beams always
    // have something to light.
    d = HAZE + (1.0 - HAZE) * d;
    float far = mix(0.5, 1.0, smoothstep(1.5, 8.0, 1000.0 / max(at.x, 0.1) / 40.0));
    return d * far * smoothstep(0.0, 7.0, p.y) * (0.75 + 0.25 * smoothstep(10.0, 39.0, p.y));
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

// The marker's curls as smoke: each one comes towards you, swelling as it does, drifts outwards
// from the middle and rises from where it left, breathes, turns and fades on its own clock.
float curls(vec2 p, float t) {
    float acc = 0.0;
    for (int k = 0; k < 7; k++) {
        float from = origin(k);
        if (from < -1000.0) continue;
        float n = air.y - float(k);
        float a = t - n * SHED;
        float life = 45.0 + 30.0 * hash(n);
        float side = mod(n, 2.0) < 0.5 ? 1.0 : -1.0;
        float away = from < 0.5 * frame.x / frame.z ? -1.0 : 1.0;
        float across = from + away * (12.0 + OUTWARDS * a);
        // Too far along the bar to reach here at its widest breath: skip its noise.
        if (abs(p.x - across) > 5.2 * (4.0 + 0.22 * a)) continue;
        float lift = -0.22 * a + 10.0 * snoise(vec3(n * 1.7, a * 0.03, 5.0));
        vec2 c = vec2(across, clamp(WAKE_Y + side * 3.0 + lift, 7.0, 35.0));
        float breathe = 1.0 + 0.3 * snoise(vec3(n * 3.1, a * 0.12, 0.0));
        float r0 = (4.0 + 0.22 * a) * breathe;
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
    vec3 at = tunnel(p);
    float grained = grain(at, t);
    return fog(p, at, t, grained) + 1.2 * curls(p, t) * (0.6 + 0.4 * grained);
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

/// The ink map: how near each spot of the bar is to its text and icons, worked out from the
/// picture once each time it is painted, at the field's resolution, into rows of the same texture
/// under the field. Red is how near a bright pixel is, a few pixels round, and green how near one is
/// along a stretch of its line, which is what a beam has to stay clear of not to strike text
/// through; both fade smoothly with distance. Its numbers
/// are in the bar's units unless they say otherwise.
const INK: &str = r#"#version 100

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

// The picture's width and height in screen pixels, and screen pixels to one of the bar's units.
uniform vec3 frame;

void main() {
    vec2 px = v_coords * frame.xy;
    float near = 0.0;
    float along = 0.0;
    // Each bright pixel counts for less the further off it is, so both fade smoothly with
    // distance rather than stopping at an edge.
    for (int j = -3; j <= 3; j++) {
        for (int i = -14; i <= 14; i++) {
            vec2 off = vec2(float(i) * 2.0, float(j) * 1.5);
            float ink = dot(texture2D(tex, (px + off * frame.z) / frame.xy).rgb, vec3(0.33));
            along = max(along, ink * exp(-off.x * off.x / 256.0 - off.y * off.y / 16.0));
            near = max(near, ink * exp(-dot(off, off) / 9.0));
        }
    }
    gl_FragColor = vec4(near, along, 0.0, 1.0) * alpha;
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
// The theme's secondary and tertiary colours, which with the ring's colour the smoke's rims.
uniform vec3 secondary;
uniform vec3 tertiary;
// Where a docked pane's frame carries the bar on down, so its foot has no edge: left and right.
uniform vec2 gap;
// The clock, the date and the chip after them, which the lasers stay clear of: left and right.
uniform vec2 clock;
// How far the projector has powered up, 0 to 1, and how deep the breath in before a rush is.
uniform vec2 stage;

// How far a beam reaches once the projector has fully powered up, units: past either end.
const float POWER_REACH = 1400.0;

const float PI = 3.14159265;
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

// How near the picture's pixel `px` is to text and icons, from the ink map under the field: a few
// pixels round, and along its line.
vec2 inked(vec2 px) {
    vec2 cells = ceil(frame.xy / vec2(FIELD_X, FIELD_Y));
    vec2 at = clamp(px / vec2(FIELD_X, FIELD_Y), vec2(0.5), cells - 0.5);
    return texture2D(tex, vec2(at.x, frame.y + cells.y + at.y) / frame.xw).rg;
}

// `c` with its colour pushed further from grey, as laser light is.
vec3 vivid(vec3 c) {
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    return clamp(mix(vec3(grey), c, 1.6), 0.0, 1.0);
}

// One laser beam from `from` along `dir`, of colour `hue`, through smoke `through`: a straight
// line with a sharp edge, in a vivid colour, with a thin white-hot line down its middle and only
// a breath of glow round it, the same brightness all along. In perspective it is widest at its
// projector and thins as it goes away. Smoke it crosses lights up a little more.
vec3 beam(vec2 p, vec2 from, vec2 dir, vec3 hue, float f, float through) {
    vec2 rel = p - from;
    float along = dot(rel, dir);
    float width = max(1.6 * f * (1.0 - along / 2600.0), 0.8);
    float off = (rel.x * dir.y - rel.y * dir.x) * f;
    if (along < 0.0 || abs(off) > 8.0 * width) return vec3(0.0);
    // The edge falls off over well under a pixel, so it is sharp without stepping.
    float body = 1.0 - smoothstep(0.5 * width - 0.35, 0.5 * width + 0.35, abs(off));
    float hot = exp(-off * off / (0.06 * width * width + 0.04));
    float glow = exp(-off * off / (6.0 * width * width));
    // In thick smoke the beam's light spreads a little into the haze round it.
    float bloom = exp(-off * off / (40.0 * width * width)) * through;
    // Seen mostly for its own light, smoke or not, and a little more where smoke is thick.
    float seen = 0.6 + 0.5 * through;
    // The projector sits behind the clock, so a beam is fainter behind the clock, the date and
    // the chip after them, and comes into its own once it is clear of them.
    float reach = (dir.x < 0.0 ? from.x - clock.x : clock.y - from.x) + 12.0;
    float clear = mix(0.56, 1.0, smoothstep(reach - 10.0, reach + 30.0, along));
    // Powering up, the beam draws out from the projector.
    float front = stage.x * POWER_REACH;
    float drawn_out = 1.0 - smoothstep(front - 40.0, front, along);
    vec3 colour = vivid(hue);
    return (colour * body + vec3(0.55) * hot * body + 0.08 * colour * glow + 0.04 * colour * bloom)
        * exp(-along / 1800.0) * seen * clear * drawn_out;
}

// The laser projector at the bar's very middle, half-way up, behind the clock, as at a show. Its
// pattern is symmetrical, left to right and top to bottom: four beams each way in the primary
// colour, an inner pair and an outer pair, which open and close against each other like scissors, crossing in a diamond behind the clock. The pairs swell and
// fade in turn, and the whole show breathes. No beam turns faster than ten degrees a minute.
const vec2 PROJECTOR_Y = vec2(0.5, 20.0);

// Beam `i` of the eight at time `t`: which way it goes, how brightly it is lit, and its colour.
void show(int i, float t, out vec2 dir, out float lit, out vec3 hue) {
    float fi = float(i);
    float side = i < 4 ? -1.0 : 1.0;
    float up = mod(fi, 2.0) < 0.5 ? 1.0 : -1.0;
    bool inner = mod(floor(fi / 2.0), 2.0) < 0.5;
    float scissor = SCISSOR * sin(2.0 * PI * t / SCISSOR_PERIOD);
    float angle = up * (inner ? 0.022 + scissor : 0.05 - scissor);
    // Breathing in before a rush, the scissors close a hair and the beams dip.
    angle = mix(angle, up * 0.036, 0.3 * stage.y);
    float swell = sin(2.0 * PI * t / 23.0);
    lit = (0.85 + 0.15 * sin(2.0 * PI * t / 41.0))
        * (0.3 + 0.7 * smoothstep(-0.4, 0.7, inner ? swell : -swell)) * (1.0 - 0.5 * stage.y);
    hue = ring;
    dir = vec2(side * cos(angle), -sin(angle));
}

vec3 lasers(vec2 p, float t, float wide, float f, float through) {
    vec3 acc = vec3(0.0);
    vec2 from = vec2(wide * PROJECTOR_Y.x, PROJECTOR_Y.y);
    for (int i = 0; i < 8; i++) {
        vec2 dir;
        float lit;
        vec3 hue;
        show(i, t, dir, lit, hue);
        acc += beam(p, from, dir, hue, f, through) * lit;
    }
    return acc;
}

// Light catching the glass where a beam meets it: at the top edge or the polished foot, each
// beam lights a point in its colour, white at the middle, drawn out a little along the edge
// with a soft bloom, as bright as the beam is.
vec3 glints(vec2 px, vec2 p, float t, float f, float wide) {
    vec3 acc = vec3(0.0);
    vec2 from = vec2(wide * PROJECTOR_Y.x, PROJECTOR_Y.y);
    for (int i = 0; i < 8; i++) {
        vec2 dir;
        float lit;
        vec3 hue;
        show(i, t, dir, lit, hue);
        // Up to the top edge, or down to the foot's middle.
        float to = dir.y < 0.0 ? 0.5 * f : frame.y - 0.5 * f;
        float along = abs((to / f - from.y) / dir.y);
        vec2 hit = from + dir * along;
        if (hit.x < 0.0 || hit.x > wide) continue;
        // Powering up, a beam catches the glass only once it has reached it, and flashes as it
        // arrives.
        float front = stage.x * POWER_REACH;
        if (along > front) continue;
        float arrives = 1.0 + 1.5 * exp(-pow((front - along) / 50.0, 2.0)) * (1.0 - step(0.999, stage.x));
        // Where a docked pane's frame carries the bar on down, the foot has no edge to catch.
        if (dir.y > 0.0 && hit.x > gap.x && hit.x < gap.y) continue;
        vec2 d = vec2((p.x - hit.x) * f, px.y - to);
        if (abs(d.x) > 60.0 * f || abs(d.y) > 8.0 * f) continue;
        float point = exp(-dot(d, d) / (1.2 * f * f));
        float drawn = exp(-abs(d.x) / (14.0 * f)) * exp(-d.y * d.y / (0.35 * f * f));
        float bloom = exp(-dot(d, d) / (30.0 * f * f));
        vec3 colour = vivid(hue);
        acc += (mix(colour, vec3(1.0), 0.6) * point + colour * (0.45 * drawn + 0.12 * bloom))
            * lit * exp(-along / 1800.0) * arrives;
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
    vec3 col = way < 0.5 ? mix(secondary, tertiary, way * 2.0) : mix(tertiary, ring, way * 2.0 - 1.0);
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

    // The lasers cross the smoke, which catches the light, as edge-lit glass does, nearly white
    // with a touch of the ring, and its rims show faintly in the logo's colours. Right beside a
    // beam the smoke's own glow is a little darker, so the beam stands out.
    vec3 beams = lasers(p, t, wide, f, smoothstep(0.05, 0.9, here)) * 0.47 * glass;
    float beside = smoothstep(0.0, 0.1, dot(beams, vec3(0.33)));
    light += mix(PALE, ring, 0.25) * smoothstep(0.0, 1.0, here) * (0.095 + 0.07 * body) * glass
        * (1.0 - 0.5 * beside);
    light += mix(PALE, col, 0.4) * smoothstep(0.08, 0.8, strength) * 0.05 * glass * smoothstep(16.0, 39.0, p.y);

    // Text and icons stay easy to read: round anything bright in the picture, the smoke's and
    // the sheen's light is held back, so a glyph keeps a dark margin; and a beam through a line
    // of text would strike it through, so beams are held back along the whole line, passing over
    // or under the numbers and icons, never through them.
    vec2 ink = inked(px);
    light *= 1.0 - 0.8 * smoothstep(0.15, 0.4, ink.x);
    // Behind the clock and the chip, where the projector is, the beams show through rather more,
    // and the smoke and the sheen less, so the clock never sits on a bright patch.
    float behind_clock = smoothstep(clock.x - 12.0, clock.x + 12.0, p.x)
        * (1.0 - smoothstep(clock.y - 12.0, clock.y + 12.0, p.x));
    light *= 1.0 - 0.55 * behind_clock;
    beams *= 1.0 - mix(0.92, 0.54, behind_clock) * smoothstep(0.05, 0.5, ink.y);

    // Where the scissor pairs meet, about every half a minute, the projector flares softly
    // behind the clock, held back round its text, and the beams brighten near it.
    float s = SCISSOR * sin(2.0 * PI * t / SCISSOR_PERIOD);
    float meet = exp(-pow((2.0 * SCISSOR - 2.0 * s) / 0.0025, 2.0)) * stage.x;
    vec2 to_projector = (p - vec2(wide * PROJECTOR_Y.x, PROJECTOR_Y.y)) * vec2(0.35, 1.0);
    light += ring * meet * 0.12 * exp(-dot(to_projector, to_projector) / 300.0) * glass
        * (1.0 - 0.8 * smoothstep(0.15, 0.4, ink.x));
    beams *= 1.0 + 0.5 * meet * exp(-abs(p.x - wide * PROJECTOR_Y.x) / 250.0);

    // Where the beams catch the glass, after that: on its edges, never on the text.
    beams += glints(px, p, t, f, wide);

    // The polished edge along the foot, wavering with the air: a bright outer face and an inner
    // reflection, lit only where fog sits against it, so it comes and goes with the smoke. None
    // where a docked pane's frame carries the bar on down.
    float foot = frame.y - 0.5 * f;
    float waver = clamp(g.y * 5.0 / 7.0, -0.8, 0.8) * f;
    float d1 = (px.y - (foot + waver)) / f;
    float d2 = (px.y - (foot - 2.2 * f + 0.6 * waver)) / f;
    float near = exp(-pow((p.x - air.z) / 300.0, 2.0)) * air.w;
    // Only the rows round the foot have an edge to light.
    float lit = 0.0;
    if (p.y > 35.0) {
        lit = 0.45 * smoothstep(0.0, 0.8, field(vec2(px.x, 38.0 * f)));
    }
    float in_gap = clamp(min(px.x + 0.5, gap.y * f) - max(px.x - 0.5, gap.x * f), 0.0, 1.0);
    float edge = (exp(-d1 * d1 / 0.38) + 0.34 * glass * exp(-d2 * d2 / 0.5)) * lit * (1.0 - in_gap);
    light += mix(ring, PALE, 0.18 + 0.2 * near) * edge;

    // The beams go over the ceiling the rest of the light keeps to, so their colours stay vivid.
    // The glass's own light is kept low, so the bar reads dark.
    vec3 added = min(min(0.8 * light, vec3(0.7, 0.9, 1.0)) + beams, vec3(1.0));
    vec4 color = vec4(behind.rgb + added * behind.a, behind.a) * alpha;
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
struct InkProgram;

/// The smoke's field has one cell for this many screen pixels across, and this many down.
const FIELD_X: f32 = 4.0;
const FIELD_Y: f32 = 2.0;
/// The largest gradient of the air the bar's shader takes, either way.
const GRADIENT: f32 = 2.0;
/// The lasers' scissors: how far the pairs open and close either way, in radians, and how long
/// they take to open and close again, in seconds. It keeps the fastest beam under ten degrees a
/// minute: a beam's far end moves a few pixels a second, which shows if you watch it for a few
/// seconds and stays out of the way if you don't.
const SCISSOR: f32 = 0.014;
const SCISSOR_PERIOD: f32 = 31.0;

/// The shaders with the numbers they share with the clock and the field here.
fn source() -> String {
    shader::with_constants(
        SHADER,
        &[
            ("FIELD_X", &FIELD_X),
            ("FIELD_Y", &FIELD_Y),
            ("GRADIENT", &GRADIENT),
            ("SCISSOR", &SCISSOR),
            ("SCISSOR_PERIOD", &SCISSOR_PERIOD),
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
    secondary: [f32; 3],
    tertiary: [f32; 3],
    gap: [f32; 2],
    clock: [f32; 2],
    stage: [f32; 2],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("air", self.air)
            .with("ring", self.ring)
            .with("secondary", self.secondary)
            .with("tertiary", self.tertiary)
            .with("gap", self.gap)
            .with("clock", self.clock)
            .with("stage", self.stage)
    }
}

/// The ink map's shader's numbers: the picture's size and screen pixels to a unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct InkUniforms {
    frame: [f32; 3],
}

impl shader::Uniforms for InkUniforms {
    fn values(&self) -> Values {
        Values::default().with("frame", self.frame)
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
    let since = since - INTAKE;
    if !(0.0..=RUSH_UP + RUSH_HOLD + RUSH_DOWN).contains(&since) {
        return 1.0;
    }
    let up = (since / RUSH_UP).min(1.0);
    let down = 1.0 - ((since - RUSH_UP - RUSH_HOLD) / RUSH_DOWN).max(0.0);
    let e = up.min(down);
    1.0 + (RUSH_PEAK - 1.0) * e * e * (3.0 - 2.0 * e)
}

/// How deep the breath in before a rush is `since` seconds after the rush was asked for: it swells
/// over the intake and lets go as the rush takes hold.
fn intake(since: f64) -> f64 {
    let long = INTAKE * 1.8;
    if !(0.0..long).contains(&since) {
        return 0.0;
    }
    (std::f64::consts::PI * since / long).sin().powi(2)
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
    /// How far the projector has powered up, 0 to 1, and when that was last moved on.
    power: f64,
    power_last: Option<f64>,
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
            power: 0.0,
            power_last: None,
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
            Rush::Since(from) if now - from <= RUSH_LONG => Some(from),
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

    /// The projector's power and the breath before a rush at `now`, for the shader. The
    /// projector powers up while the desktop is `shown` and goes down while it fades; without
    /// motion (`animated` false) it is simply on.
    fn stage(&mut self, now: f64, shown: bool, animated: bool) -> [f32; 2] {
        let step = self
            .power_last
            .map_or(0.0, |last| (now - last).clamp(0.0, STEP_MOST));
        self.power_last = Some(now);
        self.power = if !animated {
            1.0
        } else if shown {
            (self.power + step / POWER_UP).min(1.0)
        } else {
            (self.power - step / POWER_DOWN).max(0.0)
        };
        let eased = self.power * self.power * (3.0 - 2.0 * self.power);
        let breath = match self.rush {
            Rush::Since(from) => intake(now - from),
            _ => 0.0,
        };
        [eased as f32, breath as f32]
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

    /// The texture's whole size for a picture `size` screen pixels: the field's rows under it,
    /// and the ink map's under those.
    fn whole(size: Size<i32, Physical>) -> Size<i32, Physical> {
        (size.w, size.h + 2 * Self::cells(size).h).into()
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
    /// The theme's secondary and tertiary colours.
    pub secondary: u32,
    pub tertiary: u32,
    /// Where a docked pane's frame carries the bar on down, in the bar's units.
    pub gap: Option<(f32, f32)>,
    /// The clock's block, its ends in the bar's units.
    pub clock: (f32, f32),
    pub alpha: f32,
    /// Whether the air moves; whether the desktop is shown rather than fading; and whether the
    /// glass animates at all, which it doesn't with reduced motion or in the power saver mode.
    pub moves: bool,
    pub shown: bool,
    pub animated: bool,
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
        let ink = shader::program::<InkProgram, _>(
            renderer,
            |renderer| renderer.compile_custom_texture_shader(INK, &shader::names::<InkUniforms>()),
            "the lit edge's ink map",
        )?;
        let at = self.air.advance(look.now, look.moves, look.marker);
        let stage = self.air.stage(look.now, look.shown, look.animated);
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
            let programs = Programs {
                field: &field,
                ink: &ink,
                f: look.f,
            };
            if let Err(err) = fill(renderer, atlas, picture, &programs, &numbers) {
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
            secondary: rgb(look.secondary),
            tertiary: rgb(look.tertiary),
            gap: look.gap.unwrap_or((-1.0, -1.0)).into(),
            clock: look.clock.into(),
            stage,
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

/// What the atlas is drawn with besides the picture: the field's and the ink map's programs, and
/// screen pixels to one of the bar's units.
struct Programs<'a> {
    field: &'a GlesPixelProgram,
    ink: &'a GlesTexProgram,
    f: f32,
}

/// Draws into `atlas` the picture and its ink map, if it doesn't hold them yet, and the smoke's
/// field with `numbers`.
fn fill(
    renderer: &mut GlesRenderer,
    atlas: &mut Atlas,
    picture: &Picture,
    programs: &Programs<'_>,
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
        let map = Rectangle::new((0, size.h + cells.h).into(), cells);
        let (list, len) = shader::Uniforms::values(&InkUniforms {
            frame: [size.w as f32, size.h as f32, programs.f],
        })
        .listed();
        frame.render_texture_from_to(
            &picture.texture,
            Rectangle::from_size((size.w as f64, size.h as f64).into()),
            map,
            &[Rectangle::from_size(cells)],
            &[],
            Transform::Normal,
            1.0,
            Some(programs.ink),
            &list[..len],
        )?;
    }
    let (list, len) = shader::Uniforms::values(numbers).listed();
    frame.render_pixel_shader_to(
        programs.field,
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
        let seen = moved(&mut air, 11.0, RUSH_LONG, 60.0);
        let mut changes = seen.clone();
        changes.dedup();
        assert_eq!(
            changes.len(),
            seen.len(),
            "every frame moves while it rushes"
        );
        let went = seen.last().unwrap() - seen.first().unwrap();
        // A breath in, then up, held and down again: about 1.1 + 14 × (0.075 + 0.375 + 0.5)
        // seconds of air.
        assert!((13.0..16.0).contains(&went), "went {went}");
        assert_eq!(rush(0.3), RUSH_PEAK);
        assert_eq!(rush(2.0), 1.0);
        // And it settles back to fifteen steps a second.
        let after = moved(&mut air, 11.0 + RUSH_LONG + 0.05, 2.0, 60.0);
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
    fn a_rush_breathes_in_first_then_blows_through() {
        assert_eq!(
            rush(0.05),
            1.0,
            "the air keeps its pace while it breathes in"
        );
        assert!(intake(INTAKE * 0.9) > 0.9);
        assert!(rush(INTAKE + RUSH_UP + 0.1) > 10.0);
        assert_eq!(
            intake(INTAKE * 2.0),
            0.0,
            "let go by the time the rush takes hold"
        );
    }

    #[test]
    fn the_projector_powers_up_as_the_desktop_comes_back_and_is_simply_on_without_motion() {
        let mut air = Air::starting_at(100.0);
        assert_eq!(air.stage(0.0, true, true)[0], 0.0, "off at first");
        let mut power = 0.0;
        for n in 1..=30 {
            power = air.stage(n as f64 / 60.0, true, true)[0];
        }
        assert_eq!(power, 1.0, "fully on after half a second");
        for n in 31..=50 {
            power = air.stage(n as f64 / 60.0, false, true)[0];
        }
        assert_eq!(power, 0.0, "down again as the desktop fades");
        assert_eq!(
            air.stage(1.0, false, false)[0],
            1.0,
            "simply on without motion"
        );
    }

    #[test]
    fn no_laser_turns_faster_than_ten_degrees_a_minute() {
        use std::f32::consts::TAU;
        let fastest = SCISSOR * TAU / SCISSOR_PERIOD;
        let per_minute = fastest.to_degrees() * 60.0;
        assert!(per_minute <= 10.0, "{per_minute} degrees a minute");
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
    fn the_field_and_the_ink_map_sit_under_the_picture_a_cell_for_every_four_by_two_pixels() {
        let size = Size::<i32, Physical>::from((1920, 40));
        assert_eq!(Atlas::cells(size), (480, 20).into());
        assert_eq!(Atlas::whole(size), (1920, 80).into());
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
