//! The rave, an easter egg of the wind tunnel bar: knock on the bar's empty glass ten times
//! quickly and the camera dives into the bar. It eases back a touch, then rockets in on the bar's
//! middle, the world streaking past in a zoom blur, its colours fringing and the screen sinking
//! into the dark, while the bar's own glass, kept sharp, comes at you faster still; a flare sweeps
//! across as you pass through the glass, and the music drops as you land: the loud section of "The
//! Mundane Insane" by REKLAW and the breakdown after it, looped, with a laser show in the club
//! inside the wind tunnel. Any key, or a click that isn't more of the knocking, ends it: the music
//! stops at once and the camera pulls back out into the bar. The keys a machine has for itself,
//! volume, brightness and media, go on doing their jobs while it plays.
//!
//! The club is drawn in 3D: a camera in a room full of drifting haze, a crowd between you and the
//! stage, five projectors along the stage and two on towers, and each beam a line through the
//! room, seen by the haze it lights, brighter where the haze is thick and blazing where a beam
//! points at you, as light scattered forwards does. A projector can also scan a circle into a
//! cone, a tunnel you stand inside, or a line into a sheet of light overhead, a "liquid sky"
//! rippling with the haze. The projectors flare across the lens as a film camera's would.
//!
//! The show is cued to the loop the way a laser show is programmed to a track: the kicks, snares
//! and hi-hats of each sixteenth note were measured from the music and are kept here. The kicks
//! snap the fans open and bob the crowd, the snares stab alternate beams, the hi-hats drive the
//! chases, and waves roll across the fans a bar at a time while they sweep as mirror images. The
//! look changes on each bar's downbeat and the sections with the crashes: the tunnel opens it,
//! the fans take over, the sky drops with the second crash, everything goes at once for the third,
//! and the breakdown slows to a few beams turning in the haze before its last two bars build,
//! every beam closing in on you, into the drop as the loop comes round. Big changes of brightness
//! come no faster than the beat, under the three a second that can bring on a seizure; only thin
//! beams chase faster. It is drawn in the theme's three colours.
//!
//! The music plays through PipeWire (`pw-play`), as the volume click does: thirty-two bars played
//! twice without a break, so it runs a couple of minutes before it starts again, and the show's
//! beat follows it from when it starts. It is started so the drop lands as the camera does. If
//! Slipstream goes, the music stops with it. Nested runs, whose sound would come out of the
//! desktop around them, play the show without it.
//!
//! From the third quick knock it hints at what's coming: the bar shakes a little and the first
//! moment of the drop plays, very quietly, each knock after shaking it harder and playing louder.
//!
//! With reduced motion there is no camera move and the show stands still, one moment of it held,
//! with the music.

use std::{
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use smithay::{
    backend::renderer::{
        Renderer,
        element::{Id, Kind, texture::TextureRenderElement},
        gles::{
            GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
            element::TextureShaderElement,
        },
    },
    utils::{Logical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{
    effects::{
        shader::{self, Shaded, Values},
        tilt,
    },
    render::OutputElement,
};

/// How many knocks on the bar start the rave, the knock its hints start from, and the longest pause
/// between two knocks, seconds.
const KNOCKS: u32 = 10;
const HINT_FROM: u32 = 3;
const KNOCK_GAP: f64 = 0.45;
/// How long the dive into the bar takes, and the pull back out, seconds.
const DIVE: f64 = 2.0;
const PULL_OUT: f64 = 0.7;
/// The dive's first share, in which the camera eases back a touch before it rockets in, and how far
/// back it goes.
const WIND_UP: f64 = 0.12;
const WIND_BACK: f64 = 0.035;
/// How much of the dive's magnification the camera does, as a share of its logarithm: the rest is
/// the bar's glass coming nearer. Together they are worked out from the screen, to just more than
/// fill it, whatever its size.
const CAMERA_SHARE: f64 = 0.7;
const FILL_MORE: f64 = 1.1;
/// How far back the club is as the glass comes in, as a share of its size on screen.
const CLUB_FAR: f64 = 0.15;
/// The music's tempo, and how many beats and sixteenths there are in the loop.
const BPM: f64 = 118.0;
const LOOP_BEATS: f64 = 128.0;
const STEPS: usize = 512;
/// How long the music takes to be heard after `pw-play` is started, seconds.
const LATENCY: f64 = 0.12;
/// The beat the show is held at with reduced motion: the fans, a bar into the second.
const STILL_BEAT: f64 = 21.0;
/// How quickly each kind of hit dies away, seconds: kicks, snares, hi-hats.
const KICK_DECAY: f64 = 0.14;
const SNARE_DECAY: f64 = 0.11;
const HAT_DECAY: f64 = 0.06;

/// Thirty-two bars of "The Mundane Insane" by REKLAW, twice over (CC BY-NC-ND 4.0; see
/// `assets/music/LICENSE.txt`).
static MUSIC: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-loop.opus");
/// Its first 0.6 seconds, faded out: the hint.
static HINT: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-hint.opus");
/// How loud the hint plays at its first knock and at the knock before the rave, 0 to 1.
const HINT_QUIET: f64 = 0.06;
const HINT_LOUD: f64 = 0.45;

/// How loud each sixteenth note of the loop is at its start, 0 to 9, measured from the music:
/// the bass and kick drum (below 120 Hz), the snare (200 Hz to 2.5 kHz) and the hi-hats (above
/// 6 kHz). Five hundred and twelve of each, sixteen to a bar.
const KICKS: &[u8; STEPS] = b"45433011000030116743301100684111796430110110301179784411011130116853301101103011674330110167400167533011010030116777431101003223554331111111311156433011015630015643311111113111576743110110300156433111111131115643301100563001564331111111311156664311011030014543311111113111564230110156300156433111111031115666331101103001564331111111311155422111115631115643311111113111565633211111311222211111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111";
const SNARES: &[u8; STEPS] = b"53328222222183226322833222648323743283322222932274647332222293226322932222229322632283222253832363228322222193226353722222218556633383222222832253228223225383225322732222228322535372221121831253227322222172225322722222537322532273221221722253537222111172125333732212217222532272232253732253227322222172225353622211117212532273222222722253227222225373225322732222227322534363333332733353335333233253423222532333225332433252232332544232325323343252223232532323334342332252232322533242325323333253323232532323325322";
const HATS: &[u8; STEPS] = b"76758654655496645354866575648666636587758675877453547665756596645355866575659664535486546554855553548654756586645354755475648777778686546444854344457566654476654455754454447443433364445444744344457644544474434455745564547554434475445444744343336444544474436776764454447443444464556543755543447544544374434333644454447443444475445444744344456454644475434344654454436443434365556545755587758544343373423232833333337332423273333442743242327334343273224132843333337442333283333332732242327433344274323232843433327332";

/// Whether a key passes through the rave rather than ending it: the keys a machine has for
/// itself, volume, mute, brightness and media, which go on doing their jobs while it plays. Raw
/// key codes, as the kernel numbers them.
pub fn passes_through(raw: u32) -> bool {
    const MUTE: u32 = 113;
    const VOLUME_DOWN: u32 = 114;
    const VOLUME_UP: u32 = 115;
    const NEXT: u32 = 163;
    const PLAY_PAUSE: u32 = 164;
    const PREVIOUS: u32 = 165;
    const BRIGHTNESS_DOWN: u32 = 224;
    const BRIGHTNESS_UP: u32 = 225;
    const MIC_MUTE: u32 = 248;
    [
        MUTE,
        VOLUME_DOWN,
        VOLUME_UP,
        NEXT,
        PLAY_PAUSE,
        PREVIOUS,
        BRIGHTNESS_DOWN,
        BRIGHTNESS_UP,
        MIC_MUTE,
    ]
    .contains(&raw)
}

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
    /// When it was asked to end, and how far into the dive it was then.
    ending: Option<(f64, f64)>,
    music: Option<Music>,
    /// Whether the music has yet to be started: it waits so the drop lands as the camera does.
    music_due: bool,
    /// Reduced motion: no camera move, and the show held still.
    still: bool,
    kept: shader::Kept,
    camera: Camera,
}

/// The camera's move at one moment of the dive: how far it magnifies about the bar's middle, how
/// much the world streaks with the speed, and how far it has sunk into the dark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub level: f64,
    pub blur: f64,
    pub dark: f64,
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
            camera: Camera::default(),
        };
        rave.tick(now);
        rave
    }

    /// A click at `now`. One that carries on the knocking rhythm, or comes during the dive, is
    /// part of starting it; any other ends it. Whether it ended it.
    pub fn press(&mut self, now: f64) -> bool {
        if now - self.last_knock <= KNOCK_GAP || now - self.began < DIVE {
            self.last_knock = now;
            return false;
        }
        self.end(now);
        true
    }

    /// Ends it: the music stops now and the camera pulls back out into the bar.
    pub fn end(&mut self, now: f64) {
        if self.ending.is_none() {
            self.ending = Some((now, self.dived(now)));
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
            self.began + DIVE - LATENCY
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
        self.dived(now) > 0.0
    }

    /// How far into the dive it is at `now`, 0 the bar as it was and 1 landed in the club.
    fn dived(&self, now: f64) -> f64 {
        if self.still {
            return if self.ending.is_some() { 0.0 } else { 1.0 };
        }
        match self.ending {
            Some((from, was)) => was * (1.0 - eased(((now - from) / PULL_OUT).clamp(0.0, 1.0))),
            None => ((now - self.began) / DIVE).clamp(0.0, 1.0),
        }
    }

    /// The camera's move at `now`, while it dives in or pulls out. `None` once it has landed in
    /// the club, or with reduced motion.
    pub fn shot(&self, now: f64) -> Option<Shot> {
        let u = self.dived(now);
        if self.still || u >= 1.0 || u <= 0.0 {
            return None;
        }
        let w = rocket(u);
        Some(Shot {
            level: self.fill.powf(CAMERA_SHARE * depth(u, self.fill)),
            blur: 0.3 * w.powf(1.4),
            dark: smooth(0.35, 0.9, w),
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
            .map_or(self.began + DIVE, |music| music.started + LATENCY);
        let beats = (now - from) * BPM / 60.0;
        if beats < 0.0 {
            beats
        } else {
            beats % LOOP_BEATS
        }
    }

    /// The club as `look` says at `now`, over the screen or, during the dive, over the bar's glass
    /// coming nearer. `None` if the shader didn't compile.
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

    /// `elements`, everything else on the screen, as the camera shows them diving into the bar at
    /// `now`: `None` when it isn't diving, or if it can't be drawn, when they are shown as they
    /// are.
    pub fn camera(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
        sizes: tilt::Sizes,
        now: f64,
    ) -> Option<TextureShaderElement> {
        let shot = self.shot(now)?;
        let centre = self.bar_middle().to_physical(sizes.scale);
        self.camera.shot(renderer, elements, sizes, centre, shot)
    }

    /// Where the club is drawn and its shader's numbers, as `look` says at `now`.
    fn uniforms(&self, look: &Look, now: f64) -> (Rectangle<i32, Logical>, Uniforms) {
        let u = self.dived(now);
        let w = if self.still { 1.0 } else { rocket(u) };
        let s = look.scale.x;
        let screen = look.screen;
        let middle = self.bar_middle();
        // During the dive, the bar's glass, nearer about its middle, magnified with the camera;
        // once landed, the screen itself.
        let area = match self.shot(now) {
            Some(shot) => {
                let glass = self.fill.powf((1.0 - CAMERA_SHARE) * depth(u, self.fill));
                let k = glass * shot.level;
                let (w, h) = (self.bar.size.w * k, self.bar.size.h * k);
                Rectangle::<f64, Logical>::new(
                    (middle.x - w / 2.0, middle.y - h / 2.0).into(),
                    (w, h).into(),
                )
            }
            None => Rectangle::<f64, Logical>::from_size(screen),
        };
        let shown = Rectangle::<i32, Logical>::new(
            (area.loc.x.floor() as i32, area.loc.y.floor() as i32).into(),
            (area.size.w.ceil() as i32, area.size.h.ceil() as i32).into(),
        );
        let beat = self.beat(now);
        let bar = (beat.max(0.0) / 4.0).floor() as usize % 32;
        let uniforms = Uniforms {
            place: [
                (shown.loc.x as f64 * s) as f32,
                (shown.loc.y as f64 * s) as f32,
                (shown.size.w as f64 * s) as f32,
                (shown.size.h as f64 * s) as f32,
            ],
            screen: [(screen.w * s) as f32, (screen.h * s) as f32],
            beat: [
                beat as f32,
                // The bar's text fades as its glass comes in, and the club comes from far off.
                smooth(0.1, 0.5, w) as f32,
                (CLUB_FAR + (1.0 - CLUB_FAR) * w * w) as f32,
                (middle.y * s) as f32,
            ],
            hits: hits(beat),
            cue: cue(beat),
            mood: [
                breakdown(bar) as f32,
                // The drop, landing each time the loop comes round.
                if (0.0..2.0).contains(&beat) {
                    (-beat * 3.0).exp() as f32
                } else {
                    0.0
                },
                // The flare as the camera passes through the glass.
                (-((w - 0.9) / 0.05).powi(2)).exp() as f32 * f32::from(u8::from(!self.still)),
                0.0,
            ],
            ring: look.ring,
            secondary: look.secondary,
            tertiary: look.tertiary,
        };
        (shown, uniforms)
    }
}

/// What the club is drawn with.
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

/// How far the dive has rocketed in at `u`, 0 to 1: nothing through the wind-up, then slowly at
/// first and ever faster to the end.
fn rocket(u: f64) -> f64 {
    ((u - WIND_UP) / (1.0 - WIND_UP)).clamp(0.0, 1.0).powf(2.4)
}

/// How far the whole picture is magnified at `u`, as a share of the logarithm of `fill`: a touch
/// below nothing as the camera eases back in the wind-up, then the rocket in.
fn depth(u: f64, fill: f64) -> f64 {
    if u < WIND_UP {
        let back = 1.0 - WIND_BACK * (std::f64::consts::PI * u / WIND_UP).sin();
        return back.ln() / fill.ln().max(1e-6);
    }
    rocket(u)
}

fn eased(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn smooth(low: f64, high: f64, x: f64) -> f64 {
    eased(((x - low) / (high - low)).clamp(0.0, 1.0))
}

/// How hard sixteenth `step` of `band` hits: how much louder it is than the one before, 0 to 1.
fn onset(band: &[u8; STEPS], step: usize) -> f64 {
    let level = |n: usize| f64::from(band[n % STEPS] - b'0');
    ((level(step) - level(step + STEPS - 1)) / 9.0).max(0.0)
}

/// How much of `band`'s hits is still ringing at `beat`, each dying away over `decay` seconds.
fn ringing(band: &[u8; STEPS], beat: f64, decay: f64) -> f64 {
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

/// The kicks, snares and hi-hats ringing at `beat`, and how far into the breakdown's build it is.
fn hits(beat: f64) -> [f32; 4] {
    let bars = beat / 4.0;
    let build = if (30.0..32.0).contains(&bars) {
        (bars - 30.0) / 2.0
    } else {
        0.0
    };
    [
        ringing(KICKS, beat, KICK_DECAY) as f32,
        ringing(SNARES, beat, SNARE_DECAY) as f32,
        ringing(HATS, beat, HAT_DECAY) as f32,
        build as f32,
    ]
}

/// How far into the breakdown bar `bar` is: all of it from its first bar, easing off over the
/// last two as they build into the drop.
fn breakdown(bar: usize) -> f64 {
    match bar {
        24..=29 => 1.0,
        30 => 0.6,
        31 => 0.3,
        _ => 0.0,
    }
}

/// How much of each part of the show is on at `beat`: the tunnel, the fans, the liquid sky and
/// the chase. The cue changes on a bar's downbeat, snapping in over a sixteenth as a show's does.
/// The first crash opens in the tunnel and goes to the fans, chasing through bar eight's fill;
/// the second drops the sky with the hi-hats chasing under it; the third brings everything at
/// once, chasing through bar twenty-four's fill; the breakdown keeps a few beams and the sky
/// turning slowly, and its last two bars build into the drop.
fn cue(beat: f64) -> [f32; 4] {
    if beat < 0.0 {
        return [0.0; 4];
    }
    let part = |bar: usize| -> [f32; 4] {
        match bar % 32 {
            0..=3 => [1.0, 0.5, 0.0, 0.0],
            4..=6 => [0.3, 1.0, 0.0, 0.2],
            7 => [0.3, 1.0, 0.0, 1.0],
            8..=11 => [0.0, 0.4, 1.0, 0.6],
            12..=15 => [0.5, 0.7, 1.0, 0.8],
            16..=19 => [1.0, 1.0, 0.6, 0.3],
            20..=22 => [0.8, 1.0, 0.8, 0.5],
            23 => [0.8, 1.0, 0.8, 1.0],
            24..=29 => [0.6, 0.3, 0.6, 0.0],
            _ => [0.8, 1.0, 0.3, 0.6],
        }
    };
    let bar = (beat / 4.0).floor() as usize;
    let into_bar = beat / 4.0 - (beat / 4.0).floor();
    let snap = (into_bar * 16.0).clamp(0.0, 1.0) as f32;
    let (now, before) = (part(bar), part(bar + 31));
    std::array::from_fn(|n| before[n] + (now[n] - before[n]) * snap)
}

/// The club's shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    place: [f32; 4],
    screen: [f32; 2],
    beat: [f32; 4],
    hits: [f32; 4],
    cue: [f32; 4],
    mood: [f32; 4],
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
            .with("mood", self.mood)
            .with("ring", self.ring)
            .with("secondary", self.secondary)
            .with("tertiary", self.tertiary)
    }
}

/// Marks the club's program in a renderer's keeping.
struct Program;

fn program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    shader::program::<Program, _>(
        renderer,
        |renderer| renderer.compile_custom_pixel_shader(CLUB, &shader::names::<Uniforms>()),
        "the rave's",
    )
}

/// The camera: the screen drawn off screen and shown through a lens that dives into the bar.
#[derive(Default)]
struct Camera {
    program: Option<GlesTexProgram>,
    canvas: Option<tilt::Canvas>,
    /// The shader or the texture failed, so the screen is shown as it is.
    broken: bool,
}

impl Camera {
    /// `elements` drawn off screen and shown as `shot` sees them, about `centre` in screen pixels.
    fn shot(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
        sizes: tilt::Sizes,
        centre: Point<f64, smithay::utils::Physical>,
        shot: Shot,
    ) -> Option<TextureShaderElement> {
        if self.broken {
            return None;
        }
        if self.program.is_none() {
            let names = [
                UniformName::new("size", UniformType::_2f),
                UniformName::new("centre", UniformType::_2f),
                UniformName::new("view", UniformType::_3f),
            ];
            match renderer.compile_custom_texture_shader(CAMERA, &names) {
                Ok(program) => self.program = Some(program),
                Err(err) => {
                    tracing::warn!("the rave's camera didn't compile, so it zooms plainly: {err}");
                    self.broken = true;
                    return None;
                }
            }
        }
        tilt::draw_offscreen(
            renderer,
            &mut self.canvas,
            &mut self.broken,
            elements,
            sizes,
            "the rave's camera",
        )?;
        let texture = &self.canvas.as_ref()?.texture;
        let (w, h) = (sizes.physical.w as f32, sizes.physical.h as f32);
        let picture = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            (0.0, 0.0),
            texture.clone(),
            1,
            Transform::Normal,
            None,
            Some(Rectangle::from_size((w as f64, h as f64).into())),
            Some(sizes.logical),
            None,
            Kind::Unspecified,
        );
        Some(TextureShaderElement::new(
            picture,
            self.program.clone()?,
            vec![
                Uniform::new("size", (w, h)),
                Uniform::new("centre", (centre.x as f32, centre.y as f32)),
                Uniform::new(
                    "view",
                    (shot.level as f32, shot.blur as f32, shot.dark as f32),
                ),
            ],
        ))
    }
}

/// The camera's lens: the screen magnified about the bar's middle, streaked back along the dive
/// in a zoom blur as the speed builds, its colours fringing towards the edges, and sinking into
/// the dark.
const CAMERA: &str = r#"#version 100

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

// The screen in screen pixels, the bar's middle in the same, and how far the camera magnifies, how
// much the world streaks and how far it has sunk into the dark.
uniform vec2 size;
uniform vec2 centre;
uniform vec3 view;

vec3 seen(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec3(0.0);
    }
    return texture2D(tex, uv).rgb;
}

void main() {
    vec2 p = v_coords * size;
    vec2 rel = p - centre;
    float fringe = 0.012 * view.y * length(rel) / size.y;
    vec3 sum = vec3(0.0);
    for (int k = 0; k < 12; k++) {
        // Further back along the dive, the way the world streaks past.
        float back = view.x * (1.0 + view.y * float(k) / 11.0);
        sum.r += seen(centre + rel * (1.0 - fringe) / back).r;
        sum.g += seen(centre + rel / back).g;
        sum.b += seen(centre + rel * (1.0 + fringe) / back).b;
    }
    vec3 colour = sum / 12.0;
    float edge = length(rel / size);
    colour *= 1.0 - view.z * (0.55 + 0.45 * smoothstep(0.0, 0.7, edge));
    colour *= 1.0 - 0.6 * view.y * smoothstep(0.15, 0.9, edge);
    vec4 out_colour = vec4(colour, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        out_colour = vec4(0.0, 0.2, 0.0, 0.2) + out_colour * 0.8;
#endif
    gl_FragColor = out_colour;
}
"#;

/// The club, in metres in the room unless it says otherwise: x across, y up, z away from you.
const CLUB: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Where the club is shown on the screen, in screen pixels: its corner and its size.
uniform vec4 place;
// The screen's size in screen pixels.
uniform vec2 screen;
// The beat in the music's loop (below zero before the drop), how far the club has faded in over
// the bar, how near it has come (it starts far off), and the bar's middle, how far down the screen.
uniform vec4 beat;
// The kicks, snares and hi-hats ringing now, and how far into the breakdown's build it is.
uniform vec4 hits;
// How much of the tunnel, the fans, the liquid sky and the chase is on.
uniform vec4 cue;
// How far into the breakdown it is, the drop's impact, and the flare passing through the glass.
uniform vec4 mood;
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
    return clamp(mix(vec3(grey), c, 1.7), 0.0, 1.0);
}

// The way a beam aimed `across` to the side and `up` goes, from the stage towards you.
vec3 aimed(float across, float up) {
    return normalize(vec3(sin(across) * cos(up), sin(up), -cos(across) * cos(up)));
}

// A beam from `from` along `dir`, `lit` bright, seen along the ray from the eye `ray`, `pix` the
// angle of a pixel: the light the haze scatters at the beam's nearest point to the ray. A beam is a
// few centimetres across; drawn wider as its distance makes it smaller than a pixel, and dimmer in
// step, so it never steps or breaks up, with a photographic bloom round it. Seen along its length
// it is brighter, as more of it is lit in the line of sight, and pointed at you it blazes, as haze
// scatters light forwards.
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
    float sigma = 0.015 + pix * along_ray * 0.75;
    float gap2 = dot(off, off) / (sigma * sigma);
    if (gap2 > 120.0) return vec3(0.0);
    float core = exp(-gap2) * 0.015 / sigma;
    float bloom = exp(-sqrt(gap2) * 0.9) * 0.015 / sigma * 0.22;
    float towards = 1.0 + 6.0 * pow(max(-b, 0.0), 10.0);
    float lens = 1.0 / max(sqrt(den), 0.06);
    float seen = (0.35 + haze(from + dir * along_beam, t)) * towards * lens * exp(-along_ray * 0.015) * lit;
    vec3 colour = vivid(hue);
    return (colour * (1.6 * core + bloom) + vec3(0.8) * core * core) * seen;
}

// The tunnel: a cone a projector scans, opening `wide` (the cosine of its half-angle) towards you,
// so you stand inside it. Where the eye's ray crosses its wall the haze there lights up, more where
// the ray grazes it; gaps blanked into the scan turn about the axis.
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
        float blank = smoothstep(0.3, 0.42, fract(round_about / (2.0 * PI) * 10.0 + turn));
        vec3 normal = normalize(from_apex - axis * depth / wide);
        float graze = 1.0 / max(abs(dot(ray, normal)), 0.1);
        light += vivid(hue) * (0.3 + haze(at, t)) * graze * blank * 0.09 * exp(-hit * 0.025);
    }
    return light;
}

// The liquid sky: a sheet of light overhead at `height`, scanned from the stage, lit where the haze
// in it is thick, so it ripples and streams as the haze moves through it.
vec3 sky(vec3 ray, float height, vec3 hue, float t) {
    if (ray.y < 0.01) return vec3(0.0);
    float hit = (height - EYE.y) / ray.y;
    vec3 at = EYE + ray * hit;
    if (at.z < 1.0 || at.z > 26.0 || abs(at.x) > 16.0) return vec3(0.0);
    float streams = noise(vec3(at.x * 0.45, at.z * 0.35 - t * 0.9, t * 0.3));
    float liquid = smoothstep(0.3, 0.85, streams) * (0.3 + haze(at, t));
    float graze = 1.0 / max(ray.y, 0.06);
    float edge = smoothstep(26.0, 20.0, at.z) * smoothstep(16.0, 11.0, abs(at.x));
    return vivid(hue) * liquid * graze * edge * 0.16;
}

// Where `at` in the room shows on the screen, as the view's coordinates (height 1 to the top).
vec2 shown(vec3 at, vec3 forward, vec3 right, vec3 up) {
    vec3 v = at - EYE;
    float z = max(dot(v, forward), 0.01);
    return vec2(dot(v, right), dot(v, up)) / (z * VIEW);
}

float segment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a;
    vec2 ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// The crowd between you and the stage, as the view's coordinates `n` see it: how much of the
// pixel it covers. Heads and shoulders in a row, some arms up, bobbing to the kick.
float crowd(vec2 n, float kick, float b, float aa) {
    float wide = 0.075;
    float i = floor(n.x / wide);
    float cover = smoothstep(-0.82 + aa, -0.82 - aa, n.y);
    for (int m = -1; m <= 1; m++) {
        float c = i + float(m);
        float h = hash(vec3(c, 7.0, 3.0));
        float x = (c + 0.5) * wide + (h - 0.5) * 0.5 * wide;
        float bob = kick * 0.014 * (0.5 + h) + 0.005 * sin(b * PI * 0.5 + h * 6.0);
        float head_y = -0.68 + 0.05 * (h - 0.5) + bob;
        float shape = length((n - vec2(x, head_y)) * vec2(1.0, 0.85)) - 0.027 * (0.9 + 0.3 * h);
        shape = min(shape, length((n - vec2(x, head_y - 0.085)) * vec2(0.55, 1.0)) - 0.055);
        if (n.y < head_y - 0.1 && abs(n.x - x) < wide * 0.7) shape = -1.0;
        if (h > 0.62) {
            vec2 shoulder = vec2(x + (h > 0.81 ? 0.035 : -0.035), head_y - 0.05);
            vec2 hand = shoulder + vec2((h - 0.7) * 0.25 + 0.01 * sin(b * PI + h * 9.0), 0.16 + 0.03 * kick);
            shape = min(shape, segment(n, shoulder, hand) - 0.008);
        }
        cover = max(cover, smoothstep(aa, -aa, shape));
    }
    return cover;
}

void main() {
    // This pixel on the screen, and where it is in the club, which comes from far off about the
    // bar's middle as the glass comes in, and kicks as the drop lands.
    vec2 q = place.xy + v_coords * place.zw;
    vec2 focus = vec2(0.5 * screen.x, beat.w);
    vec2 club = focus + (q - focus) / beat.z;
    club.y += (1.0 - beat.z) * (0.5 * screen.y - beat.w);
    vec2 n = vec2(club.x - 0.5 * screen.x, 0.5 * screen.y - club.y) / (0.5 * screen.y);
    float b = beat.x;
    float drop = mood.y;
    n += vec2(sin(b * 97.0), cos(b * 83.0)) * 0.006 * drop;
    vec3 forward = normalize(vec3(0.0, 0.14, 1.0));
    vec3 right = vec3(1.0, 0.0, 0.0);
    vec3 up = cross(forward, right);
    vec3 ray = normalize(forward + n.x * VIEW * right + n.y * VIEW * up);
    float pix = 2.0 * VIEW / screen.y / beat.z;

    float t = max(b, 0.0) * 60.0 / 118.0;
    float kick = hits.x;
    float snare = hits.y;
    float hat = hits.z;
    float build = hits.w;
    float calm = mood.x;
    float bar = floor(max(b, 0.0) / 4.0);
    float sixteenth = floor(max(b, 0.0) * 4.0);
    // Before the drop the projectors stand by, faintly; the drop brings them up at once.
    float on = b < 0.0 ? 0.2 : 1.0;
    // The colours take turns on the projectors, a section at a time.
    float turn = mod(floor(bar / 8.0), 3.0);
    vec3 c1 = turn < 0.5 ? ring : (turn < 1.5 ? secondary : tertiary);
    vec3 c2 = turn < 0.5 ? secondary : (turn < 1.5 ? tertiary : ring);
    vec3 c3 = turn < 0.5 ? tertiary : (turn < 1.5 ? ring : secondary);
    vec3 light = vec3(0.0);

    // The tunnel: two cones from the centre of the stage, turning against each other, opening with
    // the kicks and bursting open as the drop lands.
    if (cue.x > 0.0) {
        vec3 axis = normalize(vec3(0.12 * sin(t * 0.4), -0.06, -1.0));
        float open = 0.988 - 0.012 * kick - 0.035 * drop;
        light += tunnel(ray, vec3(0.0, 3.2, 26.0), axis, open, c1, b * 0.125, t) * cue.x * (0.8 + 0.8 * kick) * on;
        light += tunnel(ray, vec3(0.0, 3.2, 26.0), axis, open - 0.008, c2, -b * 0.09, t) * cue.x * 0.6 * on;
    }

    // The fans: five projectors along the stage, ten beams each, mirror images either side of the
    // middle. A new look each bar's downbeat; the kicks snap them open, waves roll across them a
    // bar at a time, they sweep as mirror images, the snares stab alternate beams and the hi-hats
    // chase across them. In the breakdown only the middle one turns, slowly; over its last two bars
    // every beam closes in on you.
    if (cue.y > 0.0 || cue.w > 0.0) {
        float look = mod(bar, 4.0);
        float spread = (0.035 + 0.018 * look) * (1.0 + 0.9 * kick);
        float tilt = 0.05 + 0.04 * mod(bar, 3.0);
        float wave = mix(0.07, 0.03, calm);
        float sweep = mix(0.14, 0.04, calm) * sin(PI * b * mix(0.5, 0.125, calm));
        for (int j = 0; j < 5; j++) {
            float fj = float(j) - 2.0;
            float mirror = sign(fj);
            vec3 from = vec3(fj * 5.0, 2.6, 24.0 - abs(fj) * 0.5);
            vec3 hue = abs(fj) < 0.5 ? c1 : (abs(fj) < 1.5 ? c2 : c3);
            float only_middle = mix(1.0, abs(fj) < 0.5 ? 1.0 : 0.05, calm);
            for (int k = 0; k < 10; k++) {
                float fk = float(k);
                float across = mirror * 0.22 + (fk - 4.5) * spread * (mirror == 0.0 ? 1.0 : mirror)
                    + mirror * sweep;
                float rise = tilt + wave * sin(2.0 * PI * b / 4.0 - fk * 0.55 - abs(fj) * 0.8);
                vec3 dir = aimed(across, rise);
                dir = normalize(mix(dir, normalize(EYE + vec3(fj * 0.3, 0.3, 0.0) - from), build));
                float stab = mod(fk + float(j) + bar, 2.0) < 0.5 ? snare : 0.0;
                float chased = mod(fk, 10.0) == mod(sixteenth + float(j) * 2.0, 10.0) ? 1.0 : 0.12;
                float lit = cue.y * (0.55 + 0.6 * kick + 0.9 * stab) + cue.w * chased * (0.5 + hat);
                lit = mix(lit * only_middle, 1.3, build);
                light += beam(ray, from, dir, hue, lit * on, t, pix);
            }
        }
        // Two towers either side, crossing beams down over the crowd to the far side, stabbing
        // with the snares.
        for (int s = 0; s < 2; s++) {
            float side = s == 0 ? -1.0 : 1.0;
            vec3 from = vec3(side * 13.0, 7.5, 17.0);
            for (int k = 0; k < 6; k++) {
                float fk = float(k);
                float across = -side * (0.5 + 0.12 * sin(PI * b / 4.0)) + (fk - 2.5) * 0.05;
                vec3 dir = aimed(across, -0.16 - 0.03 * fk);
                float lit = (cue.y * 0.5 + snare * 0.8) * (1.0 - calm);
                light += beam(ray, from, dir, c3, lit * on, t, pix);
            }
        }
    }

    // The liquid sky, dropping in with the second crash and rising and falling with the bass.
    if (cue.z > 0.0) {
        float height = 5.5 - 0.6 * kick + 2.0 * exp(-max(b - 32.0, 0.0) * 2.0) * step(32.0, b) * step(b, 40.0);
        light += sky(ray, height, c2, t) * cue.z * (0.8 + 0.6 * hat) * on;
    }

    // Before the drop, the bar's own lasers: the centre projector throws the wind tunnel's fan,
    // four beams each way in the ring's colour opening and closing like scissors, out to either
    // side, so the dive carries straight on into them. As the drop lands they swing round towards
    // you and every other projector fires.
    float before = 1.0 - smoothstep(-0.25, 1.0, b);
    if (before > 0.0) {
        float scissor = 0.014 * sin(2.0 * PI * beat.x / 15.0);
        vec3 from = vec3(0.0, 2.6, 24.0);
        for (int i = 0; i < 8; i++) {
            float fi = float(i);
            float side = i < 4 ? -1.0 : 1.0;
            float up = mod(fi, 2.0) < 0.5 ? 1.0 : -1.0;
            bool inner = mod(floor(fi / 2.0), 2.0) < 0.5;
            float rise = up * (inner ? 0.022 + scissor : 0.05 - scissor) * 3.0;
            float swing = smoothstep(-0.25, 1.0, b);
            float across = side * mix(1.45, 0.3 + 0.1 * fi, swing);
            light += beam(ray, from, aimed(across, rise), ring, before * 1.4, t, pix);
        }
    }

    // The projectors themselves, flaring across the lens as a film camera's do.
    for (int j = 0; j < 7; j++) {
        vec3 at = j < 5 ? vec3((float(j) - 2.0) * 5.0, 2.6, 24.0 - abs(float(j) - 2.0) * 0.5)
                        : vec3(j == 5 ? -13.0 : 13.0, 7.5, 17.0);
        vec2 d = n - shown(at, forward, right, up);
        float flare = exp(-dot(d, d) * 4000.0) * 0.9 + exp(-dot(d, d) * 300.0) * 0.12
            + exp(-abs(d.y) * 160.0) * exp(-abs(d.x) * 2.5) * 0.12;
        float busy = j < 5 ? (cue.x * 0.4 + cue.y + cue.w) * (1.0 - calm * 0.8) : cue.y * (1.0 - calm);
        light += mix(j == 2 ? c1 : c2, vec3(1.0), 0.5) * flare * busy * (0.6 + 0.8 * kick) * on;
    }

    // The room: the haze lit by everything that's on, so the dark has depth.
    float room = haze(EYE + ray * 12.0, t) * 0.035 * (cue.x + cue.y + cue.z + cue.w) * on;
    vec3 colour = vec3(0.004, 0.005, 0.009) + mix(c1, c3, 0.5 + 0.5 * n.x) * room + light;

    // The crowd, dark against the light, its edges caught by it.
    float aa = 2.0 / screen.y / beat.z;
    float people = crowd(n, kick, b, aa);
    colour = mix(colour, vec3(0.002, 0.002, 0.004) + c1 * 0.012 * (cue.y + cue.x), people);

    // As the camera passes through the glass, a flare sweeps across it.
    vec2 local = v_coords * place.zw;
    float streak = exp(-abs(q.y - focus.y) / (6.0 + 0.05 * place.w)) * mood.z;
    colour += mix(vivid(ring), vec3(1.0), 0.4) * streak * 1.5 + vivid(ring) * mood.z * 0.15;
    // While the glass is still coming in, its edge shows round the club.
    float edge = min(min(local.x, place.z - local.x), min(local.y, place.w - local.y));
    colour += vivid(ring) * exp(-edge * edge / 6.0) * (1.0 - beat.y) * 0.6;

    colour = 1.0 - exp(-colour * 1.8 * (1.0 + 0.6 * drop));
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

    #[test]
    fn the_machines_own_keys_pass_through() {
        for key in [113, 114, 115, 164, 224, 225] {
            assert!(passes_through(key), "{key}");
        }
        // Escape, a letter and Return end it.
        for key in [1, 30, 28] {
            assert!(!passes_through(key), "{key}");
        }
    }

    fn rave(still: bool) -> Rave {
        let bar = Rectangle::new((0.0, 0.0).into(), (1536.0, 32.0).into());
        Rave {
            screen: "eDP-1".into(),
            bar,
            fill: fill((1536.0, 960.0).into(), bar),
            began: 10.0,
            last_knock: 10.0,
            ending: None,
            music: None,
            music_due: false,
            still,
            kept: shader::Kept::default(),
            camera: Camera::default(),
        }
    }

    #[test]
    fn more_knocks_than_ten_carry_on_rather_than_ending_it() {
        let mut rave = rave(false);
        for n in 1..=3 {
            assert!(!rave.press(10.0 + n as f64 * 0.3));
        }
        assert!(!rave.press(11.5), "still diving");
        assert!(rave.press(20.0), "a click on its own, later, ends it");
        assert!(rave.ending.is_some());
    }

    #[test]
    fn the_camera_eases_back_then_rockets_in_ever_faster_then_lands() {
        let rave = rave(false);
        let at = |share: f64| rave.shot(10.0 + DIVE * share).expect("diving");
        assert!(at(WIND_UP / 2.0).level < 1.0, "eases back first");
        let (early, mid, late) = (at(0.3), at(0.6), at(0.95));
        assert!(early.level < mid.level && mid.level < late.level);
        assert!(
            late.level / mid.level > mid.level / early.level,
            "ever faster"
        );
        assert!(late.blur > early.blur && late.dark > early.dark);
        assert!(rave.shot(10.0 + DIVE).is_none(), "landed");
    }

    #[test]
    fn the_camera_pulls_back_out_when_it_ends() {
        let mut rave = rave(false);
        rave.end(20.0);
        assert!(rave.shot(20.0 + PULL_OUT / 2.0).is_some());
        assert!(!rave.tick(20.0 + PULL_OUT), "gone");
    }

    #[test]
    fn the_bar_and_its_glass_fill_any_screen_by_the_end_of_the_dive() {
        for (w, h) in [
            (1536.0, 960.0),
            (1280.0, 2560.0),
            (7680.0, 2160.0),
            (800.0, 600.0),
        ] {
            let bar = Rectangle::new((0.0, 0.0).into(), (w, 32.0).into());
            let all = fill((w, h).into(), bar);
            assert!(16.0 * all > h - 16.0, "{w}x{h}");
            assert!(w / 2.0 * all > w / 2.0, "{w}x{h}");
        }
    }

    #[test]
    fn the_drop_lands_as_the_camera_does() {
        let rave = rave(false);
        assert!(rave.beat(10.0 + DIVE - 0.1) < 0.0, "silent while diving");
        assert!(rave.beat(10.0 + DIVE + 0.01) >= 0.0);
    }

    #[test]
    fn the_cues_follow_the_music() {
        // The kick on the first downbeat, the snare on beat two, the off-beat stab in bar two.
        assert!(hits(0.02)[0] > 0.3);
        assert!(hits(1.02)[1] > 0.5);
        assert!(onset(KICKS, 16 + 10) > 0.3, "bar two's stab");
        // The tunnel opens the loop, the sky drops with the second crash, the breakdown calms it
        // and its last two bars build.
        assert_eq!(cue(0.5)[0], 1.0);
        assert_eq!(cue(32.5)[2], 1.0);
        assert_eq!(breakdown(26), 1.0);
        assert!(hits(126.0)[3] > 0.6);
        assert_eq!(hits(100.0)[3], 0.0);
    }

    #[test]
    fn the_loop_is_thirty_two_bars_with_the_breakdown() {
        assert_eq!(LOOP_BEATS as usize * 4, STEPS);
        let loud: u32 = KICKS[..STEPS * 3 / 4]
            .iter()
            .map(|c| u32::from(c - b'0'))
            .sum();
        let calm: u32 = KICKS[STEPS * 3 / 4..]
            .iter()
            .map(|c| u32::from(c - b'0'))
            .sum();
        assert!(loud / 3 > calm, "the last eight bars are the breakdown");
    }

    #[test]
    fn with_reduced_motion_there_is_no_camera_move_and_the_show_holds_still() {
        let mut rave = rave(true);
        assert!(rave.shot(10.0).is_none());
        assert_eq!(rave.beat(10.0), rave.beat(99.0));
        rave.end(20.0);
        assert!(!rave.tick(20.0), "gone at once");
    }

    #[test]
    fn its_brightness_changes_no_faster_than_the_beat() {
        // Under three a second, where flashing can bring on a seizure: the kicks that pulse the
        // whole show come at most once a beat.
        const { assert!(BPM / 60.0 < 3.0) };
        for bar in 0..32 {
            let kicks = (0..16)
                .filter(|&n| onset(KICKS, bar * 16 + n) > 0.3)
                .count();
            assert!(kicks <= 4, "bar {bar}: {kicks}");
        }
    }

    #[test]
    fn the_music_ships_with_its_licence() {
        assert!(MUSIC.starts_with(b"OggS") && HINT.starts_with(b"OggS"));
        let licence = include_str!("../../../assets/music/LICENSE.txt");
        assert!(licence.contains("REKLAW") && licence.contains("CC BY-NC-ND 4.0"));
    }
}
