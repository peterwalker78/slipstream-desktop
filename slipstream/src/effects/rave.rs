//! The rave, an easter egg: knock on the bar's empty glass ten times quickly and the bar zooms out
//! to fill the screen, covering everything on it, and a laser show plays to the loud section of
//! "The Mundane Insane", looped. Any key or click ends it: the music stops at once and the show
//! zooms back into the bar.
//!
//! The show keeps to the beat of the music, 118 to the minute, and changes every four bars, in
//! the theme's three colours: a tunnel of beams turning about the middle with the smoke rushing
//! at you, as in the bar's own wind tunnel; three fans that sweep and open from below; a rippling
//! sheet of light overhead with beams crossing under it; and a chase across two fans that closes
//! in on the middle and bursts as the loop comes round. Big changes of brightness come no faster
//! than the beat, under the three a second that can bring on a seizure; only thin beams chase
//! faster.
//!
//! The music plays through PipeWire (`pw-play`), as the volume click does. It is sixteen bars,
//! repeated four times without a break, so it runs a couple of minutes before it starts again; the
//! show's beat follows it from when it started. If Slipstream goes, the music stops with it. Nested
//! runs, whose sound would come out of the desktop around them, play the show without it.
//!
//! With reduced motion there is no zoom and the show stands still, one moment of it held, with the
//! music.

use std::{
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Stdio},
};

use smithay::{
    backend::renderer::gles::{GlesPixelProgram, GlesRenderer},
    utils::{Logical, Rectangle, Scale},
};

use crate::effects::shader::{self, Shaded, Values};

/// How many knocks on the bar start the rave, and the longest pause between two of them, seconds.
const KNOCKS: u32 = 10;
const KNOCK_GAP: f64 = 0.45;
/// How long the bar takes to zoom out to fill the screen, and back in, seconds.
const ZOOM_OUT: f64 = 0.9;
const ZOOM_BACK: f64 = 0.45;
/// How long the projectors take to come up as the show starts, seconds.
const POWER_UP: f64 = 1.2;
/// The music's tempo, and how many beats there are in the loop.
const BPM: f64 = 118.0;
const LOOP_BEATS: f64 = 64.0;
/// How long the music takes to be heard after `pw-play` is started, seconds.
const LATENCY: f64 = 0.12;
/// The beat the show is held at with reduced motion: a turning tunnel, mid-phrase.
const STILL_BEAT: f64 = 6.25;

/// Sixteen bars of "The Mundane Insane" by REKLAW, four times over (CC BY-NC-ND 4.0; see
/// `assets/music/LICENSE.txt`).
static MUSIC: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-loop.opus");

/// Knocks on the bar's glass: counts quick clicks in a row.
#[derive(Debug, Default)]
pub struct Knocks {
    last: Option<f64>,
    count: u32,
}

impl Knocks {
    /// A knock at `now`, seconds. Whether it is the last of enough quick ones in a row to start
    /// the rave; if so the count starts again.
    pub fn knock(&mut self, now: f64) -> bool {
        let quick = self.last.is_some_and(|last| now - last <= KNOCK_GAP);
        self.count = if quick { self.count + 1 } else { 1 };
        self.last = Some(now);
        if self.count >= KNOCKS {
            *self = Self::default();
            return true;
        }
        false
    }
}

/// The music while it plays: `pw-play`, and when it was started.
struct Music {
    child: Child,
    started: f64,
}

impl Drop for Music {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the music at `now`, unless this is a nested run or it can't be written out or played.
fn play(now: f64) -> Option<Music> {
    if crate::launch::machine_commands_held() {
        return None;
    }
    let path = music_file()?;
    let mut command = crate::launch::command("pw-play");
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `prctl` is async-signal-safe, which is all `pre_exec` asks. It makes the music stop
    // when Slipstream does, however it goes.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .map_err(|err| tracing::warn!("couldn't play the rave's music: {err}"))
        .ok()
        .map(|child| Music {
            child,
            started: now,
        })
}

/// The music written out where `pw-play` can read it: the session's runtime folder, which is
/// in memory and cleared at logout.
fn music_file() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("slipstream-rave.opus");
    let written = std::fs::metadata(&path).is_ok_and(|meta| meta.len() == MUSIC.len() as u64);
    if !written {
        std::fs::write(&path, MUSIC)
            .map_err(|err| tracing::warn!("couldn't write out the rave's music: {err}"))
            .ok()?;
    }
    Some(path)
}

/// The rave on one screen.
pub struct Rave {
    /// The screen it's on, by its output's name.
    pub screen: String,
    /// Where the bar was when it started, in the screen's own logical pixels.
    bar: Rectangle<f64, Logical>,
    began: f64,
    /// When it was asked to end.
    ending: Option<f64>,
    music: Option<Music>,
    /// Reduced motion: no zoom, and the show held still.
    still: bool,
    kept: shader::Kept,
}

impl Rave {
    /// The rave starting at `now` on `screen`, whose bar is at `bar`.
    pub fn begin(screen: String, bar: Rectangle<f64, Logical>, now: f64, still: bool) -> Self {
        Self {
            screen,
            bar,
            began: now,
            ending: None,
            music: play(now),
            still,
            kept: shader::Kept::default(),
        }
    }

    /// Ends it: the music stops now and the show zooms back into the bar.
    pub fn end(&mut self, now: f64) {
        if self.ending.is_none() {
            self.ending = Some(now);
            self.music = None;
        }
    }

    /// Whether it has been asked to end.
    pub fn ending(&self) -> bool {
        self.ending.is_some()
    }

    /// Moves on to `now`: starts the music again when it has run out. Whether it is still on
    /// screen.
    pub fn tick(&mut self, now: f64) -> bool {
        if let Some(music) = self.music.as_mut()
            && !matches!(music.child.try_wait(), Ok(None))
        {
            self.music = play(now);
        }
        self.shown(now) > 0.0
    }

    /// How far it has zoomed out at `now`, 0 the bar and 1 the whole screen.
    fn shown(&self, now: f64) -> f64 {
        if self.still {
            return if self.ending.is_some() { 0.0 } else { 1.0 };
        }
        let out = ease(((now - self.began) / ZOOM_OUT).clamp(0.0, 1.0));
        match self.ending {
            Some(from) => out * (1.0 - ease(((now - from) / ZOOM_BACK).clamp(0.0, 1.0))),
            None => out,
        }
    }

    /// The beat at `now`, counted from the start of the music's loop: from when the music
    /// started, or, without it, from when the rave did.
    fn beat(&self, now: f64) -> f64 {
        if self.still {
            return STILL_BEAT;
        }
        let from = self
            .music
            .as_ref()
            .map_or(self.began, |music| music.started + LATENCY);
        ((now - from).max(0.0) * BPM / 60.0) % LOOP_BEATS
    }

    /// How far the projectors have come up at `now`.
    fn power(&self, now: f64) -> f64 {
        if self.still {
            return 1.0;
        }
        ease(((now - self.began) / POWER_UP).clamp(0.0, 1.0))
    }

    /// The show over the screen as `look` says at `now`. `None` if the shader didn't compile.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        look: &Look,
        now: f64,
    ) -> Option<Shaded> {
        let program = program(renderer)?;
        let (area, uniforms) = self.uniforms(look, now);
        Some(self.kept.pixel(program, area, &uniforms))
    }

    /// Where the show is drawn and its shader's numbers, as `look` says at `now`.
    fn uniforms(&self, look: &Look, now: f64) -> (Rectangle<i32, Logical>, Uniforms) {
        let k = self.shown(now);
        let lerp = |a: f64, b: f64| a + (b - a) * k;
        let screen = look.screen;
        let area = Rectangle::<i32, Logical>::new(
            (
                lerp(self.bar.loc.x, 0.0).floor() as i32,
                lerp(self.bar.loc.y, 0.0).floor() as i32,
            )
                .into(),
            (
                lerp(self.bar.size.w, screen.w).ceil() as i32,
                lerp(self.bar.size.h, screen.h).ceil() as i32,
            )
                .into(),
        );
        let s = look.scale.x;
        let uniforms = Uniforms {
            place: [
                (area.loc.x as f64 * s) as f32,
                (area.loc.y as f64 * s) as f32,
                (area.size.w as f64 * s) as f32,
                (area.size.h as f64 * s) as f32,
            ],
            screen: [(screen.w * s) as f32, (screen.h * s) as f32],
            beat: [
                self.beat(now) as f32,
                (self.power(now) * k) as f32,
                k as f32,
                0.0,
            ],
            ring: look.ring,
            secondary: look.secondary,
            tertiary: look.tertiary,
        };
        (area, uniforms)
    }
}

/// What the show is drawn with.
pub struct Look {
    /// The screen's size in logical pixels, and its scale.
    pub screen: smithay::utils::Size<f64, Logical>,
    pub scale: Scale<f64>,
    /// The theme's three colours.
    pub ring: [f32; 3],
    pub secondary: [f32; 3],
    pub tertiary: [f32; 3],
}

fn ease(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

/// The shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    place: [f32; 4],
    screen: [f32; 2],
    beat: [f32; 4],
    ring: [f32; 3],
    secondary: [f32; 3],
    tertiary: [f32; 3],
}

impl shader::Uniforms for Uniforms {
    fn values(&self) -> Values {
        Values::default()
            .with("place", self.place)
            .with("screen", self.screen)
            .with("beat", self.beat)
            .with("ring", self.ring)
            .with("secondary", self.secondary)
            .with("tertiary", self.tertiary)
    }
}

/// Marks the show's program in a renderer's keeping.
struct Program;

fn program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    shader::program::<Program, _>(
        renderer,
        |renderer| renderer.compile_custom_pixel_shader(SHADER, &shader::names::<Uniforms>()),
        "the rave's",
    )
}

/// The show, in screen pixels unless it says otherwise.
const SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Where the show is drawn: its corner from the screen's and its size.
uniform vec4 place;
// The screen's size.
uniform vec2 screen;
// The beat in the music's loop, how far the projectors are up, and how far the bar has zoomed out.
uniform vec4 beat;
uniform vec3 ring;
uniform vec3 secondary;
uniform vec3 tertiary;

const float PI = 3.14159265;

// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
               mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

float fbm(vec2 p) {
    return 0.55 * noise(p) + 0.3 * noise(p * 2.1 + 7.3) + 0.15 * noise(p * 4.3 + 1.7);
}

// `c` pushed further from grey, as laser light is.
vec3 vivid(vec3 c) {
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    return clamp(mix(vec3(grey), c, 1.6), 0.0, 1.0);
}

// The way a beam at `angle` goes: 0 straight up, turning clockwise.
vec2 way(float angle) {
    return vec2(sin(angle), -cos(angle));
}

// A beam from `from` along `dir`, `lit` bright: a sharp line with a white-hot middle and a little
// glow, widening as it comes towards you, drawn out as far as the projectors have come up.
vec3 beam(vec2 q, vec2 from, vec2 dir, vec3 hue, float lit, float reach) {
    if (lit <= 0.001) return vec3(0.0);
    vec2 rel = q - from;
    float along = dot(rel, dir);
    if (along < 0.0 || along > reach) return vec3(0.0);
    float width = 1.6 + along * 0.004;
    float off = abs(rel.x * dir.y - rel.y * dir.x);
    if (off > 14.0 * width) return vec3(0.0);
    float body = 1.0 - smoothstep(0.5 * width - 0.5, 0.5 * width + 0.5, off);
    float hot = exp(-off * off / (0.08 * width * width + 0.1));
    float glow = exp(-off * off / (10.0 * width * width));
    float end = 1.0 - smoothstep(reach - 60.0, reach, along);
    return (hue * body + vec3(0.6) * hot * body + 0.15 * hue * glow) * lit * end;
}

void main() {
    vec2 q = place.xy + v_coords * place.zw;
    vec2 middle = 0.5 * screen;
    float b = beat.x;
    float power = beat.y;
    float phrase = mod(floor(b / 16.0), 4.0);
    float into = mod(b, 16.0);
    // Each beat's accent, gone within the beat: the whole picture brightens at most once a beat.
    float kick = exp(-fract(b) * 5.0);
    float pulse = 0.8 + 0.3 * kick;
    // Each phrase comes in over its first beat and goes over its last half beat.
    float phrase_in = smoothstep(0.0, 1.0, into) * (1.0 - smoothstep(15.5, 16.0, into));
    vec3 c1 = vivid(phrase < 0.5 ? ring : (phrase < 1.5 ? secondary : (phrase < 2.5 ? tertiary : ring)));
    vec3 c2 = vivid(phrase < 0.5 ? secondary : (phrase < 1.5 ? tertiary : (phrase < 2.5 ? ring : secondary)));
    float reach = power * 2.0 * length(screen);

    // The haze: the wind tunnel's smoke, rushing at you from the middle.
    vec2 d = (q - middle) / screen.y;
    float r = max(length(d * vec2(1.0, 1.4)), 0.02);
    float depth = 0.18 / r;
    // Read round the middle by direction rather than by angle, so it has no seam where the
    // angle wraps round.
    float smoke = fbm(d / r * 1.6 + vec2(depth * 3.0 + b * 0.6, depth * 1.7 - b * 0.4));
    smoke = smoothstep(0.35, 0.85, smoke) * smoothstep(0.03, 0.25, r);
    float haze = 0.05 + 0.25 * smoke;
    vec3 colour = vec3(0.012, 0.014, 0.025) + mix(vec3(0.6, 0.7, 0.8), c1, 0.4) * haze * 0.25 * power;

    vec3 lasers = vec3(0.0);
    if (phrase < 0.5) {
        // The tunnel: sixteen beams turning about the middle, and the burst as the loop comes
        // round, one swell of light from the middle.
        for (int i = 0; i < 16; i++) {
            float fi = float(i);
            float even = mod(fi, 2.0);
            float lit = pulse * (even < 0.5 ? 1.0 : 0.6 + 0.4 * sin(b * PI * 0.5));
            lasers += beam(q, middle, way(2.0 * PI * fi / 16.0 + b * 0.12), even < 0.5 ? c1 : c2, lit, reach);
        }
        float burst = exp(-into * 2.0) * exp(-dot(d, d) * 6.0);
        colour += c1 * burst * 0.35 * power;
    } else if (phrase < 1.5) {
        // Three fans from below, the outer two sweeping as mirror images, all opening and closing
        // over two bars.
        float spread = 0.09 + 0.05 * sin(b * PI / 4.0);
        float sweep = 0.35 * sin(b * PI / 8.0);
        for (int k = 0; k < 3; k++) {
            float fk = float(k) - 1.0;
            vec2 from = vec2(middle.x + fk * 0.33 * screen.x, screen.y + 10.0);
            for (int j = 0; j < 7; j++) {
                float angle = (float(j) - 3.0) * spread + fk * sweep;
                lasers += beam(q, from, way(angle), fk == 0.0 ? c1 : c2, pulse, reach);
            }
        }
    } else if (phrase < 2.5) {
        // A sheet of light overhead, rippling on the beat, with the haze under it lit, and beams
        // crossing beneath from the corners.
        float x = q.x / screen.x;
        float sheet_y = screen.y * 0.3 + 0.03 * screen.y * sin(x * 12.0 + b * PI * 0.5)
            + 0.02 * screen.y * sin(x * 5.0 - b * PI * 0.25);
        float below = q.y - sheet_y;
        float line = exp(-below * below / 4.0);
        float lit_under = exp(-max(below, 0.0) / (0.06 * screen.y)) * step(0.0, below) * (0.3 + haze);
        lasers += c1 * (line * 1.2 + lit_under * 0.35) * pulse * power;
        for (int k = 0; k < 2; k++) {
            float side = k == 0 ? 1.0 : -1.0;
            vec2 from = vec2(k == 0 ? 0.0 : screen.x, screen.y);
            for (int j = 0; j < 2; j++) {
                float angle = side * (0.6 + 0.12 * float(j) + 0.15 * sin(b * PI / 8.0));
                lasers += beam(q, from, way(angle), c2, pulse, reach);
            }
        }
    } else {
        // A chase across two fans, mirror images, each beam in turn on the eighth notes, then over
        // the last bar all of them closing in on the middle.
        float closing = smoothstep(12.0, 16.0, into);
        float step8 = mod(floor(b * 2.0), 8.0);
        for (int k = 0; k < 2; k++) {
            float side = k == 0 ? -1.0 : 1.0;
            vec2 from = vec2(middle.x + side * 0.35 * screen.x, screen.y + 10.0);
            vec2 to_middle = normalize(middle - from);
            float aim = atan(to_middle.x, -to_middle.y);
            for (int j = 0; j < 8; j++) {
                float fj = float(j);
                float angle = mix(side * (fj - 3.5) * 0.15, aim, closing);
                float which = k == 0 ? fj : 7.0 - fj;
                float lit = mix(which == step8 ? 1.0 : 0.15, 1.0, closing) * pulse;
                lasers += beam(q, from, way(angle), mod(fj, 2.0) < 0.5 ? c1 : c2, lit, reach);
            }
        }
    }
    colour += lasers * phrase_in * (0.8 + 1.5 * smoke) * power;

    // While the bar zooms out, its edge is still to be seen, in the ring's colour.
    vec2 local = v_coords * place.zw;
    float rim = min(min(local.x, place.z - local.x), min(local.y, place.w - local.y));
    colour += vivid(ring) * exp(-rim * rim / 4.0) * (1.0 - beat.z) * 0.8;

    colour = 1.0 - exp(-colour * 1.3);
    vec4 out_colour = vec4(colour, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        out_colour = vec4(0.0, 0.2, 0.0, 0.2) + out_colour * 0.8;
#endif
    gl_FragColor = out_colour;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_quick_knocks_start_it_and_a_pause_starts_the_count_again() {
        let mut knocks = Knocks::default();
        for n in 0..9 {
            assert!(!knocks.knock(n as f64 * 0.3), "knock {}", n + 1);
        }
        assert!(knocks.knock(2.7), "the tenth");
        // A pause between knocks starts the count again.
        let mut knocks = Knocks::default();
        for n in 0..5 {
            knocks.knock(n as f64 * 0.3);
        }
        for n in 0..9 {
            assert!(!knocks.knock(10.0 + n as f64 * 0.3));
        }
        assert!(knocks.knock(10.0 + 9.0 * 0.3));
    }

    fn rave(still: bool) -> Rave {
        Rave {
            screen: "eDP-1".into(),
            bar: Rectangle::new((0.0, 0.0).into(), (1536.0, 32.0).into()),
            began: 10.0,
            ending: None,
            music: None,
            still,
            kept: shader::Kept::default(),
        }
    }

    #[test]
    fn the_bar_zooms_out_to_fill_the_screen_and_back_in_when_it_ends() {
        let mut rave = rave(false);
        assert_eq!(rave.shown(10.0), 0.0);
        assert_eq!(rave.shown(10.0 + ZOOM_OUT), 1.0);
        rave.end(20.0);
        assert!(rave.ending());
        assert!(rave.tick(20.1), "still zooming back in");
        assert!(!rave.tick(20.0 + ZOOM_BACK), "gone");
    }

    #[test]
    fn the_show_keeps_to_the_beat_and_loops_with_the_music() {
        let rave = rave(false);
        let beat = 60.0 / BPM;
        assert!((rave.beat(10.0 + beat) - 1.0).abs() < 1e-9);
        assert!((rave.beat(10.0 + (LOOP_BEATS + 2.0) * beat) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn with_reduced_motion_there_is_no_zoom_and_the_show_holds_still() {
        let mut rave = rave(true);
        assert_eq!(rave.shown(10.0), 1.0);
        assert_eq!(rave.beat(10.0), rave.beat(99.0));
        rave.end(20.0);
        assert!(!rave.tick(20.0), "gone at once");
    }

    #[test]
    fn its_brightness_changes_no_faster_than_the_beat() {
        // Under three a second, where flashing can bring on a seizure.
        const { assert!(BPM / 60.0 < 3.0) };
        assert!(SHADER.contains("exp(-fract(b) * 5.0)"));
    }

    #[test]
    fn the_music_ships_with_its_licence() {
        assert!(MUSIC.starts_with(b"OggS"));
        let licence = include_str!("../../../assets/music/LICENSE.txt");
        assert!(licence.contains("REKLAW") && licence.contains("CC BY-NC-ND 4.0"));
    }
}
