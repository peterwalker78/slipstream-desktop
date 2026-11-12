//! The lit edge's bar: a pane of frosted glass with air moving through it. The bar's own picture
//! is drawn through one shader that bends it where the air does and lights what the air carries,
//! so the effect is the bar itself rather than a layer laid over it.
//!
//! Slow air drifts along the bar from left to right, and through the glass it shows the way a
//! wind tunnel's schlieren photographs do: as soft light and shade where it gathers and thins,
//! with rims in the logo's colours, amber through mint to cyan. It drifts in patches of fog that
//! gather, swell, thin and clear over tens of seconds, at any height in the glass. The workspace
//! marker stands in the stream and sheds a curl of smoke every few seconds, alternately above and
//! below it; each drifts downwind, rises, turns and breathes, and fades on its own clock. Three
//! thin laser sheets cross the bar at shallow angles, seen only where they pass through smoke, and
//! tip by a few degrees over most of a minute. A broad sheen slides slowly along the frosted
//! glass, and in it single facets of the frosting flash as pinpoints as the light lines up with
//! them. The air bends the bar's picture behind it, more in the lower glass than over the text,
//! with a hint of colour split. The foot is a polished edge with an inner reflection, lit by the
//! light running along it, brighter round the marker and where fog sits against it.
//!
//! Nothing moves faster than a few pixels a second, so it is drawn about fifteen times a second.
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
            ImportMem,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    utils::{Logical, Rectangle},
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

/// The shader, in the bar's units (forty to its height) unless it says otherwise. The picture's
/// colours are premultiplied, and the light is added to them without changing their alpha.
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

// The picture's width and height in screen pixels, and screen pixels to one of the bar's units.
uniform vec3 frame;
// The air's clock in seconds, the newest curl's number, the marker's middle (far off the bar for
// none), and how strongly the marker glows.
uniform vec4 air;
// Where each curl left from, the newest first: across the bar, in its units.
uniform vec4 origins;
uniform vec3 origins_more;
uniform vec3 ring;
// Where a docked pane's frame carries the bar on down, so its foot has no edge: left and right.
uniform vec2 gap;

const float PI = 3.14159265;
const vec3 AMBER = vec3(0.96, 0.66, 0.30);
const vec3 MINT = vec3(0.42, 0.90, 0.70);
const vec3 PALE = vec3(0.86, 0.97, 1.0);
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

// The fog at `p`: smoky, carried downwind, gathering here and clearing there over tens of
// seconds, at any height in the glass.
float fog(vec2 p, float t) {
    vec2 q = vec2((p.x - AIR * t) / 150.0, p.y / 16.0);
    vec2 w = vec2(snoise(vec3(q * 0.7, t * 0.025)), snoise(vec3(q * 0.7 + 5.2, t * 0.025 + 3.0)));
    float f = 0.5 + 0.5 * fbm(vec3(q + 0.8 * w, t * 0.03));
    float life = snoise(vec3((p.x - 4.0 * AIR / 5.0 * t) / 380.0, 1.7, t * 0.033));
    float d = smoothstep(0.33, 0.72, f + 0.55 * life);
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
        acc += fade * exp(-r * r / (1.8 * r0 * r0)) * (0.35 + 0.65 * arm);
    }
    return acc;
}

float smoke(vec2 p, float t) {
    return fog(p, t) + 1.2 * curls(p, t);
}

// The laser sheets: thin beams seen only where they cross the smoke. Two cross between the
// workspaces and the clock, one rising from the foot and one falling from the top; the third
// rises leftwards from the foot past the tray. Each tips slowly, and its pivot drifts along the
// bar.
vec3 lasers(vec2 p, float t, float wide, float f, float through) {
    vec3 acc = vec3(0.0);
    for (int i = 0; i < 3; i++) {
        float fi = float(i);
        float across = i == 0 ? 0.30 : (i == 1 ? 0.38 : 0.76);
        float y = i == 1 ? 0.0 : 40.0;
        float tip = i == 0 ? 0.05 : (i == 1 ? -0.045 : -0.055);
        vec2 pivot = vec2(wide * across + 70.0 * sin(2.0 * PI * t / (130.0 + 40.0 * fi) + fi * 1.9), y);
        float angle = tip + 0.016 * sin(2.0 * PI * t / (48.0 + 17.0 * fi) + fi * 2.1);
        vec2 dir = vec2(cos(angle), -sin(angle));
        vec2 rel = p - pivot;
        float along = dot(rel, dir);
        float off = (rel.x * dir.y - rel.y * dir.x) * f;
        float beam = exp(-off * off / 0.35) + 0.18 * exp(-off * off / 9.0);
        float reach = smoothstep(0.0, 60.0, abs(along)) * exp(-abs(along) / 900.0);
        vec3 hue = i == 0 ? ring : (i == 1 ? MINT : AMBER);
        acc += hue * beam * reach * (0.04 + 1.2 * through);
    }
    return acc;
}

// Frosted glass is countless tiny facets, each tilted its own way, so the sheen is a soft patch
// rather than a mirror's spot. A facet tilted just right to send the light to the eye flashes as
// a pinpoint. Most are tilted only a little, so the flashes gather where the sheen is; as the
// light moves each lights for a fraction of a second and goes out while others light, and when
// the light holds still, so do they.
vec3 sparkle(vec2 p, float t, float from_sheen, float f) {
    vec2 cell = floor(p / 2.0);
    float h1 = hash2(cell);
    float h2 = hash2(cell + 17.3);
    float h3 = hash2(cell + 41.9);
    float h4 = hash2(cell + 77.1);
    float tilt = sqrt(-2.0 * log(max(h1, 1e-4))) * cos(2.0 * PI * h4);
    float want = from_sheen + 0.05 * sin(2.0 * PI * t / 23.0) + 0.04 * snoise(vec3(t / 9.0, 0.0, 3.0));
    float lit = exp(-pow((tilt - want) / 0.008, 2.0));
    vec2 d = (p - (cell + 0.25 + 0.5 * vec2(h2, h3)) * 2.0) * f;
    float point = exp(-dot(d, d) / 0.32) + 0.12 * exp(-dot(d, d) / 2.5);
    vec3 hue = mix(vec3(0.92, 0.98, 1.0), h3 > 0.82 ? AMBER : ring, 0.22);
    return hue * lit * (0.25 + 0.75 * h2 * h2 * h2) * point;
}

void main() {
    float f = frame.z;
    vec2 px = v_coords * frame.xy;
    vec2 p = px / f;
    float wide = frame.x / f;
    float t = air.x;

    // The air's gradient, read as a schlieren photograph reads it: its strength and which way it
    // runs, the way giving the colour.
    float e = 1.25;
    float here = smoke(p, t);
    float gx = (smoke(p + vec2(e, 0.0), t) - smoke(p - vec2(e, 0.0), t)) / (2.0 * e);
    float gy = (smoke(p + vec2(0.0, e), t) - smoke(p - vec2(0.0, e), t)) / (2.0 * e);
    vec2 g = vec2(gx * 26.0, gy * 7.0);
    float strength = length(g);
    float way = 0.5 + 0.5 * sin(atan(g.y, g.x) + 0.6);
    vec3 col = way < 0.5 ? mix(AMBER, MINT, way * 2.0) : mix(MINT, ring, way * 2.0 - 1.0);
    float glass = step(p.y, 39.0);

    // The picture behind, bent by the air: a little over the text, more in the lower glass, with
    // a hint of colour split.
    float bending = glass * smoothstep(8.0, 30.0, p.y) * mix(0.45, 1.0, smoothstep(24.0, 32.0, p.y));
    vec2 bend = clamp(g, -1.5, 1.5) * bending * f / frame.xy;
    vec4 behind = texture2D(tex, v_coords + bend);
    behind.r = texture2D(tex, v_coords + bend * 1.45).r;
    behind.b = texture2D(tex, v_coords + bend * 0.55).b;
#if defined(NO_ALPHA)
    behind = vec4(behind.rgb, 1.0);
#endif

    // The pane: a tint deepening towards the foot, and the sheen sliding slowly along it, faintly
    // grainy as frosting is.
    float body = smoothstep(14.0, 39.0, p.y);
    float sheen_at = 0.5 + 0.34 * sin(2.0 * PI * t / 960.0 + 2.2) + 0.06 * sin(2.0 * PI * t / 310.0);
    float from_sheen = (p.x - sheen_at * wide + (p.y - 20.0) * 1.9) / 260.0;
    float sheen = exp(-from_sheen * from_sheen);
    float grain = 0.75 + 0.5 * hash2(floor(px) + 5.5);
    vec3 light = (ring * 0.016 * body + vec3(0.8, 0.92, 1.0) * 0.05 * sheen * grain * (0.4 + 0.6 * body)) * glass;

    // The smoke catches the light, as edge-lit glass does, and its rims show in the logo's
    // colours; the lasers cross it, and the frosting glints.
    light += mix(ring, col, 0.55) * smoothstep(0.0, 1.0, here) * 0.15 * glass;
    light += col * smoothstep(0.08, 0.8, strength) * 0.15 * glass * smoothstep(16.0, 39.0, p.y);
    light += lasers(p, t, wide, f, smoothstep(0.05, 0.9, here)) * 0.3 * glass;
    light += sparkle(p, t, from_sheen, f) * 1.1 * glass;

    // The polished edge along the foot, wavering with the air: a bright outer face and an inner
    // reflection, brighter round the marker and where fog sits against it. None where a docked
    // pane's frame carries the bar on down.
    float foot = frame.y - 0.5 * f;
    float waver = clamp(gy * 5.0, -0.8, 0.8) * f;
    float d1 = (px.y - (foot + waver)) / f;
    float d2 = (px.y - (foot - 2.2 * f + 0.6 * waver)) / f;
    float near = exp(-pow((p.x - air.z) / 300.0, 2.0)) * air.w;
    float lit = 0.26 + 0.24 * near + 0.45 * smoothstep(0.0, 0.8, fog(vec2(p.x, 38.0), t));
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

/// Marks the glass's program in a renderer's keeping.
struct Program;

/// The shader with the numbers it shares with the clock here.
fn source() -> String {
    shader::with_constants(SHADER, &[("SHED", &SHED)])
}

/// The shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    frame: [f32; 3],
    air: [f32; 4],
    origins: [f32; 4],
    origins_more: [f32; 3],
    ring: [f32; 3],
    gap: [f32; 2],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("frame", self.frame)
            .with("air", self.air)
            .with("origins", self.origins)
            .with("origins_more", self.origins_more)
            .with("ring", self.ring)
            .with("gap", self.gap)
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

/// The bar's picture, uploaded for the shader.
struct Picture {
    texture: GlesTexture,
    size: [f32; 2],
}

/// One screen's glass: the picture it bends, its air, and its element kept from frame to frame.
#[derive(Default)]
pub struct Glass {
    picture: Option<Picture>,
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
                size: [w as f32, h as f32],
            });
        self.kept.changed();
    }

    /// Lets the picture go, in a style that doesn't need it.
    pub fn forget(&mut self) {
        self.picture = None;
    }

    /// The bar as `look` says. `None` without a picture, or if the shader didn't compile.
    pub fn element(&mut self, renderer: &mut GlesRenderer, look: &Look) -> Option<Shaded> {
        let picture = self.picture.as_ref()?;
        let program = shader::program::<Program, _>(
            renderer,
            |renderer| {
                renderer.compile_custom_texture_shader(source(), &shader::names::<Uniforms>())
            },
            "the lit edge's glass",
        )?;
        let uniforms = uniforms(&mut self.air, picture.size, look);
        Some(self.kept.texture(
            (program, picture.texture.clone()),
            look.area,
            look.alpha,
            &uniforms,
        ))
    }
}

/// The shader's numbers for a picture `size` screen pixels as `look` says, moving `air` on.
fn uniforms(air: &mut Air, size: [f32; 2], look: &Look) -> Uniforms {
    let at = air.advance(look.now, look.moves, look.marker);
    let marker = look.marker.unwrap_or(NOWHERE);
    let origins = air.curl_origins(at, marker);
    let rgb = |c: u32| {
        [
            (c >> 24) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
        ]
    };
    Uniforms {
        frame: [size[0], size[1], look.f],
        air: [
            at as f32,
            Air::newest(at) as f32,
            marker,
            if look.marker.is_some() {
                look.strength
            } else {
                0.0
            },
        ],
        origins: [origins[0], origins[1], origins[2], origins[3]],
        origins_more: [origins[4], origins[5], origins[6]],
        ring: rgb(look.ring),
        gap: look.gap.unwrap_or((-1.0, -1.0)).into(),
    }
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
        let source = source();
        let precision = source.find("precision highp float;").unwrap();
        assert!(source.find("const float SHED = 12.0;").unwrap() > precision);
    }

    #[test]
    fn its_numbers_fit_the_shader_plumbing() {
        let names = shader::names::<Uniforms>();
        assert_eq!(names.len(), 6);
    }
}
