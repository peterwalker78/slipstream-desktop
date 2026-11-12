//! The rave, an easter egg of the wind tunnel bar: knock on the bar's empty glass ten times
//! quickly and the screen zooms in on the bar's middle, as the screen zoom does, while the bar's
//! own text fades and its glass comes towards you faster still, until it fills the screen. Then the
//! music drops, the loud section of "The Mundane Insane" by REKLAW, looped, and a laser show plays
//! in the club inside the wind tunnel. Any key, or a click that isn't more of the knocking, ends
//! it: the music stops at once and the show zooms back out into the bar.
//!
//! The club is drawn in 3D: a camera in a room full of drifting haze, three projectors on a stage,
//! and each beam a line through the room, seen by the haze it lights, brighter where the haze is
//! thick and blazing where a beam points at you, as light scattered forwards does. A projector can
//! scan a circle into a cone, a tunnel you stand inside, or a line into a sheet of light overhead,
//! a "liquid sky" rippling with the haze.
//!
//! The show is cued to the loop the way a laser show is programmed to a track: the kicks, snares
//! and hi-hats of each sixteenth note were measured from the music and are kept here, so the
//! kicks pulse the beams and widen the fans, the snares flash them in alternate halves, the
//! hi-hats drive the chases and the off-beat stabs snap every beam on. The look changes on each
//! bar's downbeat, the sections change with the crashes, the last bar closes every beam in on you,
//! and the loop coming round bursts the tunnel open. Big changes of brightness come no faster than
//! the beat, under the three a second that can bring on a seizure; only thin beams chase faster.
//! It is drawn in the theme's three colours.
//!
//! The music plays through PipeWire (`pw-play`), as the volume click does: sixteen bars repeated
//! four times without a break, so it runs a couple of minutes before it starts again, and the
//! show's beat follows it from when it starts. It is started so the drop lands as the zoom ends.
//! If Slipstream goes, the music stops with it. Nested runs, whose sound would come out of the
//! desktop around them, play the show without it.
//!
//! From the third quick knock it hints at what's coming: the bar shakes a little and the first
//! second of the drop plays, very quietly, each knock after shaking it harder and playing louder.
//!
//! With reduced motion there is no zoom and the show stands still, one moment of it held, with the
//! music.

use std::{
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use smithay::{
    backend::renderer::gles::{GlesPixelProgram, GlesRenderer},
    utils::{Logical, Point, Rectangle, Scale, Size},
};

use crate::effects::shader::{self, Shaded, Values};

/// How many knocks on the bar start the rave, the knock its hints start from, and the longest pause
/// between two knocks, seconds.
const KNOCKS: u32 = 10;
const HINT_FROM: u32 = 3;
const KNOCK_GAP: f64 = 0.45;
/// How long the zoom into the bar takes, and back out, seconds.
const ZOOM_IN: f64 = 1.8;
const ZOOM_OUT: f64 = 0.6;
/// How much of the zoom in the screen zoom does, as a share of its magnification's logarithm: the
/// rest is the bar's glass coming nearer. Together they are worked out from the screen, to just
/// more than fill it, whatever its size.
const SCREEN_SHARE: f64 = 0.7;
const FILL_MORE: f64 = 1.1;
/// How far back the club is at the start of the zoom, as a share of its size on screen.
const CLUB_FAR: f64 = 0.12;
/// The music's tempo, and how many beats there are in the loop.
const BPM: f64 = 118.0;
const LOOP_BEATS: f64 = 64.0;
/// How long the music takes to be heard after `pw-play` is started, seconds.
const LATENCY: f64 = 0.12;
/// The beat the show is held at with reduced motion: inside the tunnel, a bar in.
const STILL_BEAT: f64 = 5.0;
/// How quickly each kind of hit dies away, seconds: kicks, snares, hi-hats.
const KICK_DECAY: f64 = 0.14;
const SNARE_DECAY: f64 = 0.11;
const HAT_DECAY: f64 = 0.06;

/// Sixteen bars of "The Mundane Insane" by REKLAW, four times over (CC BY-NC-ND 4.0; see
/// `assets/music/LICENSE.txt`).
static MUSIC: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-loop.opus");
/// Its first second, faded out: the hint.
static HINT: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-hint.opus");
/// How loud the hint plays at its first knock and at the knock before the rave, 0 to 1.
const HINT_QUIET: f64 = 0.06;
const HINT_LOUD: f64 = 0.45;

/// How loud each sixteenth note of the loop is at its start, 0 to 9, measured from the music:
/// the bass and kick drum (below 120 Hz), the snare (200 Hz to 2.5 kHz) and the hi-hats (above
/// 6 kHz). Two hundred and fifty-six of each, sixteen to a bar.
const KICKS: &[u8; 256] = b"4543301100003011674330110068411179643011011030117978441101113011685330110110301167433011016740016753301101003011677743110100322355433111111131115643301101563001564331111111311157674311011030015643311111113111564330110056300156433111111131115666431101103001";
const SNARES: &[u8; 256] = b"5332822222218322632283322264832374328332222293227464733222229322632293222222932263228322225383236322832222219322635372222221855663338322222283225322822322538322532273222222832253537222112183125322732222217222532272222253732253227322122172225353722211117212";
const HATS: &[u8; 256] = b"7675865465549664535486657564866663658775867587745354766575659664535586657565966453548654655485555354865475658664535475547564877777868654644485434445756665447665445575445444744343336444544474434445764454447443445574556454755443447544544474434333644454447443";

/// What a knock on the bar's glass does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Knock {
    Nothing,
    /// A hint at what's coming, `0` at the first and `1` at the knock before the rave.
    Hint(f64),
    Rave,
}

/// Knocks on the bar's glass: counts quick clicks in a row, and plays the hint.
#[derive(Debug, Default)]
pub struct Knocks {
    last: Option<f64>,
    count: u32,
    hint: Option<Child>,
}

impl Knocks {
    /// A knock at `now`, seconds: nothing yet, a hint, or, at enough quick ones in a row, the
    /// rave, after which the count starts again.
    pub fn knock(&mut self, now: f64) -> Knock {
        let quick = self.last.is_some_and(|last| now - last <= KNOCK_GAP);
        self.count = if quick { self.count + 1 } else { 1 };
        self.last = Some(now);
        if self.count >= KNOCKS {
            self.count = 0;
            self.hush();
            return Knock::Rave;
        }
        if self.count >= HINT_FROM {
            let share = f64::from(self.count - HINT_FROM) / f64::from(KNOCKS - 1 - HINT_FROM);
            return Knock::Hint(share);
        }
        Knock::Nothing
    }

    /// Plays the hint, `share` of the way from very quiet to louder, cutting off the last one.
    pub fn hint(&mut self, share: f64) {
        self.hush();
        let volume = HINT_QUIET + (HINT_LOUD - HINT_QUIET) * share;
        self.hint = written("slipstream-rave-hint.opus", HINT)
            .and_then(|path| spawn(player(&path).arg(format!("--volume={volume:.3}")), "hint"));
    }

    /// Cuts the hint off, if one is playing.
    fn hush(&mut self) {
        if let Some(mut hint) = self.hint.take() {
            let _ = hint.kill();
            let _ = hint.wait();
        }
    }
}

/// `pw-play` for the sound at `path`, unless this is a nested run, whose sound would come out of
/// the desktop around it. It stops when Slipstream does, however it goes.
fn player(path: &std::path::Path) -> Command {
    let mut command = crate::launch::command("pw-play");
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `prctl` is async-signal-safe, which is all `pre_exec` asks.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

/// Starts `command`, the rave's `what`, unless this is a nested run.
fn spawn(command: &mut Command, what: &str) -> Option<Child> {
    if crate::launch::machine_commands_held() {
        return None;
    }
    command
        .spawn()
        .map_err(|err| tracing::warn!("couldn't play the rave's {what}: {err}"))
        .ok()
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
    let path = written("slipstream-rave.opus", MUSIC)?;
    spawn(&mut player(&path), "music").map(|child| Music {
        child,
        started: now,
    })
}

/// `bytes` written out as `name` where `pw-play` can read it: the session's runtime folder, which
/// is in memory and cleared at logout.
fn written(name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(name);
    let already = std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64);
    if !already {
        std::fs::write(&path, bytes)
            .map_err(|err| tracing::warn!("couldn't write out the rave's sound: {err}"))
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
    /// How far the bar has to be magnified, all told, about its middle to fill the screen.
    fill: f64,
    began: f64,
    /// The last knock, which more quick ones carry on from rather than ending the show.
    last_knock: f64,
    /// When it was asked to end, and how far it had zoomed in then.
    ending: Option<(f64, f64)>,
    music: Option<Music>,
    /// Whether the music has yet to be started: it waits so the drop lands as the zoom ends.
    music_due: bool,
    /// Reduced motion: no zoom, and the show held still.
    still: bool,
    kept: shader::Kept,
}

impl Rave {
    /// The rave starting at `now` on `screen`, `size` logical pixels, whose bar is at `bar`.
    pub fn begin(
        screen: String,
        size: Size<f64, Logical>,
        bar: Rectangle<f64, Logical>,
        now: f64,
        still: bool,
    ) -> Self {
        let mut rave = Self {
            screen,
            fill: fill(size, bar),
            bar,
            began: now,
            last_knock: now,
            ending: None,
            music: None,
            music_due: true,
            still,
            kept: shader::Kept::default(),
        };
        rave.tick(now);
        rave
    }

    /// A click at `now`. One that carries on the knocking rhythm, or comes while the zoom is still
    /// going in, is part of starting it; any other ends it. Whether it ended it.
    pub fn press(&mut self, now: f64) -> bool {
        if now - self.last_knock <= KNOCK_GAP || now - self.began < ZOOM_IN {
            self.last_knock = now;
            return false;
        }
        self.end(now);
        true
    }

    /// Ends it: the music stops now and the show zooms back out into the bar.
    pub fn end(&mut self, now: f64) {
        if self.ending.is_none() {
            self.ending = Some((now, self.zoomed(now)));
            self.music = None;
            self.music_due = false;
        }
    }

    /// Moves on to `now`: starts the music when it is due, and again when it has run out.
    /// Whether it is still on screen.
    pub fn tick(&mut self, now: f64) -> bool {
        let due = if self.still {
            self.began
        } else {
            self.began + ZOOM_IN - LATENCY
        };
        if self.music_due && now >= due {
            self.music_due = false;
            self.music = play(now);
        }
        if let Some(music) = self.music.as_mut()
            && !matches!(music.child.try_wait(), Ok(None))
        {
            self.music = play(now);
        }
        self.zoomed(now) > 0.0
    }

    /// How far it has zoomed in at `now`, 0 the bar as it was and 1 the club filling the screen.
    fn zoomed(&self, now: f64) -> f64 {
        if self.still {
            return if self.ending.is_some() { 0.0 } else { 1.0 };
        }
        match self.ending {
            Some((from, was)) => was * (1.0 - ((now - from) / ZOOM_OUT).clamp(0.0, 1.0)),
            None => ((now - self.began) / ZOOM_IN).clamp(0.0, 1.0),
        }
    }

    /// The screen zoom while it zooms in or out: centred on the bar's middle, from 1 up to its
    /// share of filling the screen. `None` once the club fills it, or with reduced motion.
    pub fn zoom(&self, now: f64, scale: f64) -> Option<crate::zoom::View> {
        let k = self.zoomed(now);
        if self.still || k >= 1.0 || k <= 0.0 {
            return None;
        }
        let centre = self.bar_middle().to_physical(scale).to_i32_round();
        Some(crate::zoom::View {
            centre,
            level: self.fill.powf(SCREEN_SHARE * eased(k)),
        })
    }

    fn bar_middle(&self) -> Point<f64, Logical> {
        let bar = self.bar;
        (bar.loc.x + bar.size.w / 2.0, bar.loc.y + bar.size.h / 2.0).into()
    }

    /// The beat at `now`, counted from the start of the music's loop: from when the music
    /// started, or, without it, from when it was due. Before the drop it is below zero.
    fn beat(&self, now: f64) -> f64 {
        if self.still {
            return STILL_BEAT;
        }
        let from = self
            .music
            .as_ref()
            .map_or(self.began + ZOOM_IN, |music| music.started + LATENCY);
        let beats = (now - from) * BPM / 60.0;
        if beats < 0.0 {
            beats
        } else {
            beats % LOOP_BEATS
        }
    }

    /// The show as `look` says at `now`. `None` if the shader didn't compile.
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
        let k = self.zoomed(now);
        let e = eased(k);
        let s = look.scale.x;
        let screen = look.screen;
        let middle = self.bar_middle();
        // While zooming, the bar's glass, nearer about its middle, under the screen zoom; once the
        // club fills the screen, the screen itself.
        let zooming = self.zoom(now, s).is_some();
        let (area, on_screen) = if zooming {
            let glass = self.fill.powf((1.0 - SCREEN_SHARE) * e);
            let level = self.fill.powf(SCREEN_SHARE * e);
            let w = self.bar.size.w * glass;
            let h = self.bar.size.h * glass;
            let area = Rectangle::<f64, Logical>::new(
                (middle.x - w / 2.0, middle.y - h / 2.0).into(),
                (w, h).into(),
            );
            // Where the screen zoom shows it.
            let shown = Rectangle::<f64, Logical>::new(
                (
                    middle.x + (area.loc.x - middle.x) * level,
                    middle.y + (area.loc.y - middle.y) * level,
                )
                    .into(),
                (w * level, h * level).into(),
            );
            (area, shown)
        } else {
            let whole = Rectangle::<f64, Logical>::from_size(screen);
            (whole, whole)
        };
        let area = Rectangle::<i32, Logical>::new(
            (area.loc.x.floor() as i32, area.loc.y.floor() as i32).into(),
            (area.size.w.ceil() as i32, area.size.h.ceil() as i32).into(),
        );
        let beat = self.beat(now);
        let uniforms = Uniforms {
            place: [
                (on_screen.loc.x * s) as f32,
                (on_screen.loc.y * s) as f32,
                (on_screen.size.w * s) as f32,
                (on_screen.size.h * s) as f32,
            ],
            screen: [(screen.w * s) as f32, (screen.h * s) as f32],
            beat: [
                beat as f32,
                // The bar's text fades as the glass comes in; the club comes from far off.
                smooth(0.05, 0.45, e) as f32,
                (CLUB_FAR + (1.0 - CLUB_FAR) * e) as f32,
                (middle.y * s) as f32,
            ],
            hits: hits(beat),
            cue: cue(beat),
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
    pub screen: Size<f64, Logical>,
    pub scale: Scale<f64>,
    /// The theme's three colours.
    pub ring: [f32; 3],
    pub secondary: [f32; 3],
    pub tertiary: [f32; 3],
}

/// How far `bar` has to be magnified about its middle to just more than fill a screen `size`
/// logical pixels, whatever its size or shape.
fn fill(size: Size<f64, Logical>, bar: Rectangle<f64, Logical>) -> f64 {
    let (x, y) = (bar.loc.x + bar.size.w / 2.0, bar.loc.y + bar.size.h / 2.0);
    let across = x.max(size.w - x) / (bar.size.w / 2.0).max(1.0);
    let down = y.max(size.h - y) / (bar.size.h / 2.0).max(1.0);
    across.max(down).max(1.0) * FILL_MORE
}

fn eased(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn smooth(low: f64, high: f64, x: f64) -> f64 {
    eased(((x - low) / (high - low)).clamp(0.0, 1.0))
}

/// How hard sixteenth `step` of `band` hits: how much louder it is than the one before, 0 to 1.
fn onset(band: &[u8; 256], step: usize) -> f64 {
    let level = |n: usize| f64::from(band[n % 256] - b'0');
    ((level(step) - level(step + 255)) / 9.0).max(0.0)
}

/// How much of `band`'s hits is still ringing at `beat`, each dying away over `decay` seconds.
fn ringing(band: &[u8; 256], beat: f64, decay: f64) -> f64 {
    if beat < 0.0 {
        return 0.0;
    }
    let sixteenths = beat * 4.0;
    let now = sixteenths.floor() as usize;
    let step_secs = 60.0 / BPM / 4.0;
    (0..4usize)
        .filter(|&back| back <= now)
        .map(|back| {
            let step = now - back;
            let ago = (sixteenths - step as f64) * step_secs;
            onset(band, step) * (-ago / decay).exp()
        })
        .fold(0.0, f64::max)
}

/// The kicks, snares and hi-hats ringing at `beat`, and how far into the last bar's build it is.
fn hits(beat: f64) -> [f32; 4] {
    let bar = (beat / 4.0).floor();
    let build = if bar == 15.0 { beat / 4.0 - 15.0 } else { 0.0 };
    [
        ringing(KICKS, beat, KICK_DECAY) as f32,
        ringing(SNARES, beat, SNARE_DECAY) as f32,
        ringing(HATS, beat, HAT_DECAY) as f32,
        build.clamp(0.0, 1.0) as f32,
    ]
}

/// How much of each part of the show is on at `beat`: the tunnel, the fans, the liquid sky and
/// the chase. The cue changes on a bar's downbeat, snapping in over a sixteenth as a show's does.
/// The first half, with its crash, opens in the tunnel and goes to the fans, chasing through bar
/// eight's fill; the second half's crash drops the sky, the hi-hats chase under it, and the last
/// bars bring the tunnel back with the fans as everything closes in.
fn cue(beat: f64) -> [f32; 4] {
    if beat < 0.0 {
        return [0.0; 4];
    }
    let bar = (beat / 4.0).floor() as usize % 16;
    const PARTS: [[f32; 4]; 16] = [
        [1.0, 0.4, 0.0, 0.0],
        [1.0, 0.4, 0.0, 0.0],
        [1.0, 0.6, 0.0, 0.0],
        [1.0, 0.8, 0.0, 0.3],
        [0.3, 1.0, 0.0, 0.0],
        [0.3, 1.0, 0.0, 0.0],
        [0.3, 1.0, 0.0, 0.3],
        [0.3, 1.0, 0.0, 1.0],
        [0.0, 0.3, 1.0, 0.6],
        [0.0, 0.3, 1.0, 0.6],
        [0.0, 0.5, 1.0, 0.8],
        [0.0, 0.5, 1.0, 1.0],
        [0.8, 0.8, 0.5, 0.3],
        [0.8, 0.8, 0.5, 0.3],
        [1.0, 1.0, 0.3, 0.6],
        [1.0, 1.0, 0.0, 1.0],
    ];
    let into_bar = beat / 4.0 - (beat / 4.0).floor();
    let snap = (into_bar * 16.0).clamp(0.0, 1.0) as f32;
    let (now, before) = (PARTS[bar], PARTS[(bar + 15) % 16]);
    std::array::from_fn(|n| before[n] + (now[n] - before[n]) * snap)
}

/// The shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    place: [f32; 4],
    screen: [f32; 2],
    beat: [f32; 4],
    hits: [f32; 4],
    cue: [f32; 4],
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
            .with("hits", self.hits)
            .with("cue", self.cue)
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

/// The club, in metres in the room unless it says otherwise: x across, y up, z away from you.
const SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Where the show is shown on the screen, in screen pixels: its corner and its size.
uniform vec4 place;
// The screen's size in screen pixels.
uniform vec2 screen;
// The beat in the music's loop (below zero before the drop), how far the club has faded in over
// the bar, how near the club has come (it starts far off), and the bar's middle, how far down the
// screen it is.
uniform vec4 beat;
// The kicks, snares and hi-hats ringing now, and how far into the last bar's build it is.
uniform vec4 hits;
// How much of the tunnel, the fans, the liquid sky and the chase is on.
uniform vec4 cue;
uniform vec3 ring;
uniform vec3 secondary;
uniform vec3 tertiary;

const float PI = 3.14159265;
const vec3 EYE = vec3(0.0, 1.7, 0.0);
// Half the view's height, as the tangent of its angle.
const float VIEW = 0.62;

// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash(vec3 p) {
    p = fract(p * 0.1031);
    p += dot(p, p.zyx + 31.32);
    return fract((p.x + p.y) * p.z);
}

float noise(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    vec3 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(mix(hash(i), hash(i + vec3(1.0, 0.0, 0.0)), u.x),
                   mix(hash(i + vec3(0.0, 1.0, 0.0)), hash(i + vec3(1.0, 1.0, 0.0)), u.x), u.y),
               mix(mix(hash(i + vec3(0.0, 0.0, 1.0)), hash(i + vec3(1.0, 0.0, 1.0)), u.x),
                   mix(hash(i + vec3(0.0, 1.0, 1.0)), hash(i + vec3(1.0, 1.0, 1.0)), u.x), u.y), u.z);
}

// The haze at `p`: billows drifting towards you down the room, as the wind tunnel's smoke does,
// thicker lower down.
float haze(vec3 p, float t) {
    vec3 q = p * 0.32 + vec3(0.0, 0.0, t * 0.55);
    float n = 0.62 * noise(q) + 0.38 * noise(q * 2.3 + 5.1);
    return (0.3 + 1.1 * n * n) * exp(-max(p.y, 0.0) / 7.0);
}

vec3 vivid(vec3 c) {
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    return clamp(mix(vec3(grey), c, 1.6), 0.0, 1.0);
}

// The way a beam aimed `across` to the side and `up` goes, from the stage towards you.
vec3 aimed(float across, float up) {
    return normalize(vec3(sin(across) * cos(up), sin(up), -cos(across) * cos(up)));
}

// A beam from `from` along `dir`, `lit` bright, seen along the ray from the eye `ray`, `pix` the
// angle of a pixel: the light the haze scatters at the beam's nearest point to the ray. A beam is
// a few centimetres across; drawn wider as its distance makes it smaller than a pixel, and dimmer
// in step, so it never steps or breaks up. Seen along its length it is brighter, as there is more
// of it lit in the line of sight, and pointed at you it blazes, as haze scatters light forwards.
vec3 beam(vec3 ray, vec3 from, vec3 dir, vec3 hue, float lit, float t, float pix) {
    if (lit <= 0.002) return vec3(0.0);
    vec3 w = EYE - from;
    float b = dot(ray, dir);
    float d = dot(ray, w);
    float e = dot(dir, w);
    float den = max(1.0 - b * b, 1e-4);
    float along_ray = (b * e - d) / den;
    float along_beam = (e - b * d) / den;
    if (along_ray < 0.2 || along_beam < 0.0 || along_beam > 60.0) return vec3(0.0);
    vec3 off = (EYE + ray * along_ray) - (from + dir * along_beam);
    float sigma = 0.012 + pix * along_ray * 0.8;
    float gap2 = dot(off, off) / (sigma * sigma);
    if (gap2 > 16.0) return vec3(0.0);
    float core = exp(-gap2) * 0.012 / sigma;
    float glow = exp(-gap2 * 0.15) * 0.012 / sigma * 0.15;
    float towards = 1.0 + 4.0 * pow(max(-b, 0.0), 12.0);
    float lens = 1.0 / max(sqrt(den), 0.07);
    float seen = haze(from + dir * along_beam, t) * towards * lens * exp(-along_ray * 0.02) * lit;
    return (vivid(hue) * (core + glow) + vec3(0.7) * core * core) * seen;
}

// The tunnel: a cone the centre projector scans, opening `wide` (the cosine of its half-angle)
// towards you, so you stand inside it. Where the eye's ray crosses its wall the haze there lights
// up, more where the ray grazes it; gaps blanked into the scan turn about the axis.
vec3 tunnel(vec3 ray, vec3 apex, vec3 axis, float wide, vec3 hue, float turn, float t) {
    vec3 co = EYE - apex;
    float c2 = wide * wide;
    float ra = dot(ray, axis);
    float oa = dot(co, axis);
    float qa = ra * ra - c2;
    float qb = 2.0 * (ra * oa - dot(ray, co) * c2);
    float qc = oa * oa - dot(co, co) * c2;
    float disc = qb * qb - 4.0 * qa * qc;
    if (disc < 0.0) return vec3(0.0);
    float root = sqrt(disc);
    vec3 light = vec3(0.0);
    vec3 side = normalize(cross(axis, vec3(0.0, 1.0, 0.0)));
    vec3 lift = cross(side, axis);
    for (int n = 0; n < 2; n++) {
        float hit = (-qb + (n == 0 ? -root : root)) / (2.0 * qa);
        if (hit < 0.2) continue;
        vec3 at = EYE + ray * hit;
        vec3 from_apex = at - apex;
        float depth = dot(from_apex, axis);
        if (depth < 0.0 || depth > 40.0) continue;
        float round_about = atan(dot(from_apex, lift), dot(from_apex, side));
        float blank = smoothstep(0.35, 0.45, fract(round_about / (2.0 * PI) * 8.0 + turn));
        vec3 normal = normalize(from_apex - axis * depth / wide);
        float graze = 1.0 / max(abs(dot(ray, normal)), 0.12);
        light += vivid(hue) * haze(at, t) * graze * blank * 0.05 * exp(-hit * 0.03);
    }
    return light;
}

// The liquid sky: a sheet of light overhead at `height`, scanned from the stage, lit where the
// haze in it is thick, so it ripples and streams as the haze moves through it.
vec3 sky(vec3 ray, float height, vec3 hue, float t) {
    if (ray.y < 0.01) return vec3(0.0);
    float hit = (height - EYE.y) / ray.y;
    vec3 at = EYE + ray * hit;
    if (at.z < 1.0 || at.z > 26.0 || abs(at.x) > 16.0) return vec3(0.0);
    float streams = noise(vec3(at.x * 0.45, at.z * 0.35 - t * 0.9, t * 0.3));
    float liquid = smoothstep(0.35, 0.85, streams) * haze(at, t);
    float graze = 1.0 / max(ray.y, 0.06);
    float edge = smoothstep(26.0, 20.0, at.z) * smoothstep(16.0, 11.0, abs(at.x));
    return vivid(hue) * liquid * graze * edge * 0.1;
}

void main() {
    // This pixel on the screen, and where it is in the club, which comes from far off about the
    // bar's middle as the zoom goes in.
    vec2 q = place.xy + v_coords * place.zw;
    vec2 focus = vec2(0.5 * screen.x, beat.w);
    vec2 club = focus + (q - focus) / beat.z;
    club.y += (1.0 - beat.z) * (0.5 * screen.y - beat.w);
    vec2 ndc = vec2(club.x - 0.5 * screen.x, 0.5 * screen.y - club.y) / (0.5 * screen.y);
    vec3 forward = normalize(vec3(0.0, 0.14, 1.0));
    vec3 right = vec3(1.0, 0.0, 0.0);
    vec3 up = cross(forward, right);
    vec3 ray = normalize(forward + ndc.x * VIEW * right + ndc.y * VIEW * up);
    float pix = 2.0 * VIEW / screen.y / beat.z;

    float b = beat.x;
    float t = max(b, 0.0) * 60.0 / 118.0;
    float kick = hits.x;
    float snare = hits.y;
    float hat = hits.z;
    float build = hits.w;
    float bar = floor(max(b, 0.0) / 4.0);
    float sixteenth = floor(max(b, 0.0) * 4.0);
    // Before the drop the projectors are dark; the drop brings them up at once.
    float on = step(0.0, b);

    vec3 c1 = ring;
    vec3 c2 = secondary;
    vec3 c3 = tertiary;
    vec3 light = vec3(0.0);

    // The tunnel, from the centre projector, opening and turning with the kicks, and bursting
    // open as the loop comes round.
    if (cue.x > 0.0) {
        float burst = exp(-max(b, 0.0) * 1.5) * step(b, 4.0);
        float open = 0.988 - 0.012 * kick - 0.03 * burst;
        vec3 axis = normalize(vec3(0.15 * sin(t * 0.4), -0.06, -1.0));
        light += tunnel(ray, vec3(0.0, 3.2, 26.0), axis, open, c1, b * 0.125, t) * cue.x * (0.7 + 0.6 * kick);
    }

    // The fans: three projectors, eight beams each, the outer two mirror images. Each bar's
    // downbeat brings a new look: where they aim and how far they spread. The kicks widen them,
    // the snares flash alternate halves, the hi-hats chase across them, and over the last bar
    // every beam closes in on you.
    if (cue.y > 0.0 || cue.w > 0.0) {
        float look = mod(bar, 4.0);
        float spread = 0.05 + 0.025 * look + 0.03 * kick;
        float tilt = 0.06 + 0.05 * mod(bar, 3.0) + 0.04 * sin(t * 0.8);
        for (int p = 0; p < 3; p++) {
            float side = float(p) - 1.0;
            vec3 from = vec3(side * 7.0, 2.8 + 0.3 * abs(side), 25.0 - abs(side));
            vec3 hue = p == 1 ? c1 : c2;
            for (int k = 0; k < 8; k++) {
                float fk = float(k);
                float across = side * (0.18 + 0.06 * look) + (fk - 3.5) * spread * (side == 0.0 ? 1.0 : side);
                float rise = tilt + (mod(fk, 2.0) - 0.5) * 0.02 * look;
                vec3 dir = aimed(across, rise);
                // Over the last bar, every beam turns to you.
                dir = normalize(mix(dir, normalize(EYE + vec3(side * 0.6, 0.4, 0.0) - from), build));
                float half_flash = mod(fk + bar, 2.0) < 0.5 ? snare : 0.0;
                float chased = mod(fk + (side < 0.0 ? 0.0 : 4.0), 8.0) == mod(sixteenth, 8.0) ? 1.0 : 0.12;
                float lit = cue.y * (0.45 + 0.5 * kick + 0.6 * half_flash) + cue.w * chased * (0.4 + hat);
                lit = mix(lit, 1.2, build);
                light += beam(ray, from, dir, hue, lit * on, t, pix);
            }
        }
    }

    // The liquid sky, dropping in with the second half's crash and rising and falling with the
    // bass.
    if (cue.z > 0.0) {
        float height = 5.5 - 0.6 * kick + 2.0 * exp(-max(b - 32.0, 0.0) * 2.0) * step(32.0, b);
        light += sky(ray, height, c3, t) * cue.z * (0.8 + 0.6 * hat);
    }

    // The projectors themselves, bright points on the stage while their beams are out.
    for (int p = 0; p < 3; p++) {
        float side = float(p) - 1.0;
        vec3 at = vec3(side * 7.0, 2.8 + 0.3 * abs(side), 25.0 - abs(side));
        vec3 to = normalize(at - EYE);
        float near = 1.0 - dot(ray, to);
        float glare = exp(-near * 6.0e4) + 0.08 * exp(-near * 2.0e3);
        light += mix(c1, vec3(1.0), 0.6) * glare * (cue.y + cue.w + 0.5 * cue.x) * (0.6 + 0.6 * kick) * on;
    }

    // The room: the haze lit faintly by everything that's on, so the dark has depth.
    float room = haze(EYE + ray * 12.0, t) * 0.02 * (cue.x + cue.y + cue.z + cue.w) * on;
    vec3 colour = vec3(0.004, 0.005, 0.009) + mix(c1, c3, 0.5 + 0.5 * ray.x) * room + light;

    // While the zoom is still going in, the bar's own glass and line show round the club.
    vec2 local = v_coords * place.zw;
    float rim = min(min(local.x, place.z - local.x), min(local.y, place.w - local.y));
    colour += vivid(c1) * exp(-rim * rim / 6.0) * (1.0 - beat.y) * 0.6;

    colour = 1.0 - exp(-colour * 1.4);
    vec4 out_colour = vec4(colour, 1.0) * beat.y * alpha;
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
    fn ten_quick_knocks_start_it_hinting_from_the_third_and_a_pause_starts_the_count_again() {
        let mut knocks = Knocks::default();
        let seen: Vec<Knock> = (0..10).map(|n| knocks.knock(n as f64 * 0.3)).collect();
        assert_eq!(seen[..2], [Knock::Nothing, Knock::Nothing]);
        assert_eq!(seen[2], Knock::Hint(0.0), "the first hint is the quietest");
        assert_eq!(
            seen[8],
            Knock::Hint(1.0),
            "the last before the rave the loudest"
        );
        assert_eq!(seen[9], Knock::Rave, "the tenth");
        // A pause between knocks starts the count again.
        let mut knocks = Knocks::default();
        for n in 0..5 {
            knocks.knock(n as f64 * 0.3);
        }
        assert_eq!(knocks.knock(10.0), Knock::Nothing);
    }

    fn rave(still: bool) -> Rave {
        Rave {
            screen: "eDP-1".into(),
            bar: Rectangle::new((0.0, 0.0).into(), (1536.0, 32.0).into()),
            fill: fill(
                (1536.0, 960.0).into(),
                Rectangle::new((0.0, 0.0).into(), (1536.0, 32.0).into()),
            ),
            began: 10.0,
            last_knock: 10.0,
            ending: None,
            music: None,
            music_due: false,
            still,
            kept: shader::Kept::default(),
        }
    }

    #[test]
    fn more_knocks_than_ten_carry_on_rather_than_ending_it() {
        let mut rave = rave(false);
        // Three more quick knocks, then a pause, all during the zoom in.
        for n in 1..=3 {
            assert!(!rave.press(10.0 + n as f64 * 0.3));
        }
        assert!(!rave.press(11.5), "still zooming in");
        // Well after, a click on its own ends it.
        assert!(rave.press(20.0));
        assert!(rave.ending.is_some());
    }

    #[test]
    fn the_screen_zooms_in_on_the_bars_middle_then_hands_over_to_the_club() {
        let mut rave = rave(false);
        let start = rave.zoom(10.0 + 0.01, 1.0).expect("zooming");
        assert_eq!(start.centre, (768, 16).into());
        let later = rave.zoom(10.0 + ZOOM_IN * 0.9, 1.0).expect("zooming");
        assert!(later.level > 10.0, "{}", later.level);
        assert!(
            rave.zoom(10.0 + ZOOM_IN, 1.0).is_none(),
            "the club fills the screen"
        );
        // Ending zooms back out from where it was.
        rave.end(20.0);
        assert!(rave.zoom(20.0 + ZOOM_OUT / 2.0, 1.0).is_some());
        assert!(!rave.tick(20.0 + ZOOM_OUT), "gone");
    }

    #[test]
    fn the_bar_and_its_glass_fill_any_screen_by_the_end_of_the_zoom() {
        for (w, h) in [
            (1536.0, 960.0),
            (1280.0, 2560.0),
            (7680.0, 2160.0),
            (800.0, 600.0),
        ] {
            let bar = Rectangle::new((0.0, 0.0).into(), (w, 32.0).into());
            let all = fill((w, h).into(), bar);
            // Half the bar's height reaches past the screen's foot, and half its width past its
            // sides.
            assert!(16.0 * all > h - 16.0, "{w}x{h}");
            assert!(w / 2.0 * all > w / 2.0, "{w}x{h}");
        }
    }

    #[test]
    fn the_drop_lands_as_the_zoom_ends() {
        let rave = rave(false);
        assert!(
            rave.beat(10.0 + ZOOM_IN - 0.1) < 0.0,
            "silent while zooming"
        );
        assert!(rave.beat(10.0 + ZOOM_IN + 0.01) >= 0.0);
    }

    #[test]
    fn the_cues_follow_the_music() {
        // The kick on the first downbeat, the snare on beat two, the off-beat stab in bar two.
        assert!(hits(0.02)[0] > 0.3);
        assert!(hits(1.02)[1] > 0.5);
        assert!(onset(KICKS, 16 + 10) > 0.3, "bar two's stab");
        // The tunnel opens the loop, the sky drops with the second half's crash, and the last
        // bar builds.
        assert_eq!(cue(0.5)[0], 1.0);
        assert_eq!(cue(32.5)[2], 1.0);
        assert!(hits(62.0)[3] > 0.4);
    }

    #[test]
    fn with_reduced_motion_there_is_no_zoom_and_the_show_holds_still() {
        let mut rave = rave(true);
        assert!(rave.zoom(10.0, 1.0).is_none());
        assert_eq!(rave.beat(10.0), rave.beat(99.0));
        rave.end(20.0);
        assert!(!rave.tick(20.0), "gone at once");
    }

    #[test]
    fn its_brightness_changes_no_faster_than_the_beat() {
        // Under three a second, where flashing can bring on a seizure: the kicks that pulse the
        // whole show come at most once a beat.
        const { assert!(BPM / 60.0 < 3.0) };
        let kicks_a_bar = (0..16).filter(|&n| onset(KICKS, n) > 0.3).count();
        assert!(kicks_a_bar <= 4, "{kicks_a_bar}");
    }

    #[test]
    fn the_music_ships_with_its_licence() {
        assert!(MUSIC.starts_with(b"OggS") && HINT.starts_with(b"OggS"));
        let licence = include_str!("../../../assets/music/LICENSE.txt");
        assert!(licence.contains("REKLAW") && licence.contains("CC BY-NC-ND 4.0"));
    }
}
