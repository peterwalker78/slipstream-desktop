//! The rave, an easter egg of the wind tunnel bar: knock on the bar's empty glass ten times
//! quickly and the camera dives into the bar. It eases back a touch, then rockets in on the bar's
//! middle, the world streaking past in a zoom blur, its colours fringing and the screen sinking
//! into the dark, while the bar's own glass, kept sharp, comes at you faster still; a flare sweeps
//! across as you pass through the glass, and the music drops as you land: the loud section of "The
//! Mundane Insane" by REKLAW and the breakdown after it, looped, with a laser show in the club
//! inside the wind tunnel. Any key, or a click that isn't more of the knocking, ends it: the music
//! stops at once and the camera pulls back out into the bar. Over the middle of the dive, and of the
//! pull out, the desktop's CRT look comes and goes: scanlines, flicker, tears and split colours. The keys a machine has for itself,
//! volume, brightness and media, go on doing their jobs while it plays.
//!
//! The club is a room rendered in 3D, seen only by its lasers: a floor, walls, a stage and a truss
//! over it, five projectors along the stage and two hung at the truss's ends. Each beam is a line
//! through the room's drifting, lumpy haze, a core that spreads with distance inside a shaft of
//! lit haze, brightest where the haze billows, seen along its length or pointed at you, and hidden
//! behind the stage or a wall; it lands on the room as a spot and spills a glow round it. A
//! projector can also scan beams round a cone, a tunnel you stand inside, or a line into a sheet
//! of light overhead, a "liquid sky" streaming with the haze. The camera drifts a little as a
//! hand-held one does, and sees as a film camera does: highlights roll off to white, with a little
//! grain.
//!
//! The show is cued to the loop the way a laser show is programmed to a track: the kicks, snares
//! and hi-hats of each sixteenth note were measured from the music and are kept here. The kicks
//! snap the fans open, the snares stab alternate beams, the hi-hats drive the
//! chases, and waves roll across the fans a bar at a time while they sweep as mirror images. The
//! look changes on each bar's downbeat and the sections with the crashes: the tunnel opens it,
//! the fans take over, the sky drops with the second crash, everything goes at once for the third,
//! and the breakdown slows to a few beams turning in the haze before its last two bars build,
//! every beam closing in on you, into the drop. Big changes of brightness
//! come no faster than the beat, under the three a second that can bring on a seizure; only thin
//! beams chase faster. It is drawn in the theme's three colours.
//!
//! The music plays through PipeWire (`pw-play`), as the volume click does. It loops where the
//! track itself comes back round, from its drop to where it returns to it, about three minutes,
//! and the show's beat follows it from when it starts. The build-up into the drop plays from the
//! moment the dive starts, so the drop lands as the camera does. If Slipstream goes, the music
//! stops with it. Nested runs, whose sound would come out of the desktop around them, play the
//! show without it.
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
/// How often the CRT look tears the picture during the transition, a chance each of the tube's
/// frames.
const TEAR_CHANCE: f32 = 0.12;
/// How far back the club is as the glass comes in, as a share of its size on screen.
const CLUB_FAR: f64 = 0.15;
/// The music's tempo, and how many bars, beats and sixteenths there are in the loop: from the drop
/// to where the track comes back round to it.
const BPM: f64 = 118.0;
const BARS: usize = 89;
const LOOP_BEATS: f64 = BARS as f64 * 4.0;
const STEPS: usize = BARS * 16;
/// How long the music takes to be heard after `pw-play` is started, seconds.
const LATENCY: f64 = 0.12;
/// The beat the show is held at with reduced motion: the fans, a bar into the second.
const STILL_BEAT: f64 = 21.0;
/// How quickly each kind of hit dies away, seconds: kicks, snares, hi-hats.
const KICK_DECAY: f64 = 0.14;
const SNARE_DECAY: f64 = 0.11;
const HAT_DECAY: f64 = 0.06;

/// "The Mundane Insane" by REKLAW from its drop to where it comes back round to it (CC BY-NC-ND
/// 4.0; see `assets/music/LICENSE.txt`).
static MUSIC: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-loop.opus");
/// The build-up that leads into the drop, as long as the dive less the music's latency: it plays
/// from the moment the dive starts and runs into the drop as the camera lands.
static INTRO: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-intro.opus");
/// The drop's first 0.4 seconds, faded out: the hint.
static HINT: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-hint.opus");
/// How loud the hint plays at its first knock and at the knock before the rave, 0 to 1.
const HINT_QUIET: f64 = 0.06;
const HINT_LOUD: f64 = 0.45;

/// How loud each sixteenth note of the loop is at its start, 0 to 9, measured from the music:
/// the bass and kick drum (below 120 Hz), the snare (200 Hz to 2.5 kHz) and the hi-hats (above
/// 6 kHz), sixteen to a bar.
const KICKS: &[u8; STEPS] = b"45433011000030116743301100684111796430110110301179784411011130116853301101103011674330110167400167533011010030116777431101003223554331111111311156433011015630015643311111113111576743110110300156433111111131115643301100563001564331111111311156664311011030014543311111113111564230110156300156433111111031115666331101103001564331111111311155422111115631115643311111113111565633211111311222211111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111444331111111211145433111115631115543211111113111565533111111211146433111111121114543311111463111454321111111211146553311111121114432211111002011453221011045300045422001000021114555331101012000454221001000211145322011004530004532211101002111455533111111211144322111111021114532200100453001454221111111211156553311000020014542211101002111453220010045300145422111111021114656331100002001444221111100211145322001004530014542211111002111565533110000200145422111010021114532200100453001454221111110211145301001010010014443211111112111454321111155300045432111111120115554331111112100454321111111211145432111114530004543211111112011455533111111211134432111111121114543211111453000454321111111201145543311111121004543211111112111454321111145300045432111111120111222222222221211111111111100000000000000000000000000000001110000100000000000011000000000011111100011111111111001000001101111111101221000000000000000000000100111";
const SNARES: &[u8; STEPS] = b"53328222222183226322833222648323743283322222932274647332222293226322932222229322632283222253832363228322222193226353722222218556633383222222832253228223225383225322732222228322535372221121831253227322222172225322722222537322532273221221722253537222111172125333732212217222532272232253732253227322222172225353622211117212532273222222722253227222225373225322732222227322534363333332733353335333233253423222532333225332433252232332544232325323343252223232532323334342332252232322533242325323333253323232532323325322533373333333734353327433345373335343633333337443535363333432742253437333343373435332643333536333543363333432643353536333343373225333733333336343533363333453643353436333442263335343633333326322533373333332633342336333344363334333633333226333534353333433633353226222111162114322622222426222432262221111621142426212111162114322622211116211432262222143622243226222111162114242621211116211432262221111621243226222224262224322622211116211424262121111621143226222111162124322622221436211432262221111622243224222232352235332632323226332432263231343633143226323132262324342522313226322432263231322633243226323134363314322632313226232434252243462644343226223232263324332632323536332432263222322623242435223222263224322632323226332432263232343633243226323232262323333232322232333211111111111111111111111111111111111111111111111111111111111122111111111111112211111111111111122112222221123222211221110000000000000111113522443";
const HATS: &[u8; STEPS] = b"76758654655486645354866575548666636587657675876453547665756596645355866475659664535486546554855553548654655486645354755465648777778686545444744344457456654375654455754454447443433364445444744344457544544474434455745564447553434465445443744343336444544474436676654454447443444464556543755543446544544374433333644454437443444465445444644344446454644465434344654454436443434364546544755587758534343373423232733333337322423273333432743242327334343273224132743333337442332273333332732242327433343273323232843433327332657575545443754552436554555375335254755454447545524365545552752351447554543475455243655455527533525465535533654452436554555274237687767475567575635665757655767563676674665675756244657465657563636676746556756563566575665475756367666465567565624565746565766456656543433363333334534454436454334464434333633332325333433363433344644343336333334464545443644333436443433363333232533343336443566565434333633333445344544364443344644343336333323253334333644333446443433363334344645454436443334464434333644344446554685795667886765534346444433364343342655242436444343364544333644434436444433364443433644443336434344265524233644434346453433364576795765555646444343364444343643433426553323364443433634333335444343364443333644434336443333364342342655333336444343364443444333323331344423322323222213131221131322331323123113132233132312311313223313231231010000101100000000000000112111121111111111110110000000000000111111215755656";
/// How loud each bar of the loop is, 0 to 9, measured from the music: the breakdowns are the quiet
/// ones.
const LOUDNESS: &[u8; BARS] =
    b"68897879676767676767676754444444676767676767676756565656565656556666666756666665222222323";

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
            return Knock::Rave;
        }
        if self.count >= HINT_FROM {
            let share = f64::from(self.count - HINT_FROM) / f64::from(KNOCKS - 1 - HINT_FROM);
            return Knock::Hint(share);
        }
        Knock::Nothing
    }

    /// Plays the hint, `share` of the way from very quiet to louder, cutting off the one before.
    /// The last one plays to its end as the rave starts: nothing cuts it but another knock.
    pub fn hint(&mut self, share: f64) {
        if let Some(mut before) = self.hint.take() {
            let _ = before.kill();
            let _ = before.wait();
        }
        let volume = HINT_QUIET + (HINT_LOUD - HINT_QUIET) * share;
        self.hint = written("slipstream-rave-hint.opus", HINT)
            .and_then(|path| spawn(player(&path).arg(format!("--volume={volume:.3}")), "hint"));
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
    /// The build-up into the drop, playing while the camera dives.
    intro: Option<Music>,
    /// Whether the music has yet to be started: it waits so the drop lands as the camera does.
    music_due: bool,
    /// Reduced motion: no camera move, and the show held still.
    still: bool,
    kept: shader::Kept,
    camera: Camera,
}

/// The camera's move at one moment of the dive: how far it magnifies about the bar's middle, how
/// much the world streaks with the speed, how far it has sunk into the dark, and the CRT look over
/// it: how strong, the tube's brightness this frame, and how far it is torn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub level: f64,
    pub blur: f64,
    pub dark: f64,
    pub crt: [f32; 4],
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
            intro: None,
            music_due: true,
            still,
            kept: shader::Kept::default(),
            camera: Camera::default(),
        };
        // The build-up plays from the start of the dive, straight into the drop.
        if !still {
            rave.intro = written("slipstream-rave-intro.opus", INTRO)
                .and_then(|path| spawn(&mut player(&path), "intro"))
                .map(|child| Music {
                    child,
                    started: now,
                });
        }
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
            self.intro = None;
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
        let (flicker, torn) = crate::effects::fx::tube(now, TEAR_CHANCE);
        Some(Shot {
            level: self.fill.powf(CAMERA_SHARE * depth(u, self.fill)),
            blur: 0.3 * w.powf(1.4),
            dark: smooth(0.35, 0.9, w),
            crt: [
                tube_look(u) as f32,
                flicker,
                torn,
                ((now / crate::effects::fx::TUBE_FRAME).floor() % 97.0) as f32,
            ],
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
        let shot = self.shot(now);
        let area = match shot {
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
        let moves = moves(beat);
        let [resting, impact, sky_drop] = mood(beat);
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
                resting,
                impact,
                // The flare as the camera passes through the glass.
                (-((w - 0.9) / 0.05).powi(2)).exp() as f32 * f32::from(u8::from(!self.still)),
                sky_drop,
            ],
            crt: shot.map_or([0.0; 4], |shot| shot.crt),
            centre: moves.centre.values(),
            inner: moves.inner.values(),
            outer: moves.outer.values(),
            motion: moves.motion,
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

/// How strong the desktop's CRT look is over the transition at `u`: it ramps in over its second
/// quarter and back out over its third, so it lies over the middle half.
fn tube_look(u: f64) -> f64 {
    smooth(0.25, 0.5, u) * (1.0 - smooth(0.5, 0.75, u))
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

/// Where a moment sits in the song.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// A breakdown.
    Rest,
    /// The last two bars of a breakdown, 0 to 1 through them, building into the drop.
    Build(f64),
    /// A loud run, after a breakdown: which of its eight-bar blocks it's in, how many beats into
    /// that block and into the run, and how loud the bar is, 0 to 1.
    Run {
        block: usize,
        into_block: f64,
        into_run: f64,
        level: f64,
    },
}

/// How loud bar `bar` of the loop is, 0 to 1.
fn loudness(bar: usize) -> f64 {
    f64::from(LOUDNESS[bar % BARS] - b'0') / 9.0
}

/// Whether bar `bar` is in a breakdown: one of the quiet ones.
fn resting(bar: usize) -> bool {
    loudness(bar) < 0.5
}

/// Where `beat` sits in the song, read from how loud its bars are.
fn part(beat: f64) -> Part {
    let b = beat.max(0.0);
    let bar = (b / 4.0).floor() as usize % BARS;
    let into_bar = b / 4.0 - (b / 4.0).floor();
    if resting(bar) {
        return if !resting(bar + 1) {
            Part::Build(0.5 + into_bar / 2.0)
        } else if !resting(bar + 2) {
            Part::Build(into_bar / 2.0)
        } else {
            Part::Rest
        };
    }
    let mut start = bar;
    for _ in 0..BARS {
        if resting(start + BARS - 1) {
            break;
        }
        start = (start + BARS - 1) % BARS;
    }
    let into_run = ((bar + BARS - start) % BARS) as f64 * 4.0 + into_bar * 4.0;
    Part::Run {
        block: (into_run / 32.0) as usize,
        into_block: into_run % 32.0,
        into_run,
        level: loudness(bar),
    }
}

/// The kicks, snares and hi-hats ringing at `beat`, and how far into a build it is.
fn hits(beat: f64) -> [f32; 4] {
    let build = match part(beat) {
        Part::Build(through) if beat >= 0.0 => through,
        _ => 0.0,
    };
    [
        ringing(KICKS, beat, KICK_DECAY) as f32,
        ringing(SNARES, beat, SNARE_DECAY) as f32,
        ringing(HATS, beat, HAT_DECAY) as f32,
        build as f32,
    ]
}

/// The show's mood at `beat`: how far into a breakdown it is (easing off as it builds), the
/// impact as a loud run lands, and the liquid sky dropping in as a block of the fans gives way
/// to it.
fn mood(beat: f64) -> [f32; 3] {
    if beat < 0.0 {
        return [0.0; 3];
    }
    match part(beat) {
        Part::Rest => [1.0, 0.0, 0.0],
        Part::Build(through) => [(0.6 - 0.3 * through) as f32, 0.0, 0.0],
        Part::Run {
            block,
            into_block,
            into_run,
            ..
        } => [
            0.0,
            if into_run < 2.0 {
                (-into_run * 3.0).exp() as f32
            } else {
                0.0
            },
            if block % 3 == 1 {
                (-into_block * 2.0).exp() as f32
            } else {
                0.0
            },
        ],
    }
}

/// How much of each part of the show is on at `beat`: the tunnel, the fans, the liquid sky and
/// the chase. Each loud run cycles through three blocks of eight bars: the tunnel giving way to the
/// fans, the sky dropping in with the hi-hats chasing under it, and everything at once, each
/// chasing through its last bar into the next. A breakdown keeps a few beams and the sky turning
/// slowly; its last two bars build into the drop. The cue changes on a bar's downbeat, snapping in
/// over a sixteenth as a show's does.
fn cue(beat: f64) -> [f32; 4] {
    if beat < 0.0 {
        return [0.0; 4];
    }
    let at = |beat: f64| -> [f32; 4] {
        match part(beat) {
            Part::Rest => [0.6, 0.3, 0.6, 0.0],
            Part::Build(_) => [0.8, 1.0, 0.3, 0.6],
            Part::Run {
                block, into_block, ..
            } => {
                let last_bar = into_block >= 28.0;
                match block % 3 {
                    0 if into_block < 16.0 => [1.0, 0.5, 0.0, 0.0],
                    0 => [0.3, 1.0, 0.0, if last_bar { 1.0 } else { 0.2 }],
                    1 if into_block < 16.0 => [0.0, 0.4, 1.0, 0.6],
                    1 => [0.5, 0.7, 1.0, 0.8],
                    _ => [1.0, 1.0, 0.6, if last_bar { 1.0 } else { 0.3 }],
                }
            }
        }
    };
    let bar_start = (beat / 4.0).floor() * 4.0;
    let snap = ((beat - bar_start) * 4.0).clamp(0.0, 1.0) as f32;
    let (now, before) = (at(beat), at((bar_start - 0.01).rem_euclid(LOOP_BEATS)));
    std::array::from_fn(|n| before[n] + (now[n] - before[n]) * snap)
}

/// How long a beam takes to snap from one position to the next on the beat, seconds: quick enough
/// to land with the hit, slow enough to be seen moving.
const SNAP: f64 = 0.07;

/// One fan's look: where it aims to the side, how far apart its beams are, how high it tilts, and
/// how brightly it's lit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Fan {
    aim: f64,
    spread: f64,
    tilt: f64,
    gate: f64,
}

impl Fan {
    fn values(self) -> [f32; 4] {
        [
            self.aim as f32,
            self.spread as f32,
            self.tilt as f32,
            self.gate as f32,
        ]
    }
}

/// The show's moves at one moment: the centre fan, the pair either side of it and the outer pair;
/// and how far the waves roll across them, where the rolling has stepped to, which beam the chase
/// is on, and how brightly the truss's projectors fire.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Moves {
    centre: Fan,
    inner: Fan,
    outer: Fan,
    motion: [f32; 4],
}

/// `table` stepped through every `every` beats from `beat`, each move snapping from the last in
/// `SNAP` seconds and holding till the next.
fn stepped(table: &[f64], beat: f64, every: f64) -> f64 {
    let at = (beat / every).floor();
    let since = (beat - at * every) * 60.0 / BPM;
    let len = table.len() as i64;
    let now = table[(at as i64).rem_euclid(len) as usize];
    let before = table[(at as i64 - 1).rem_euclid(len) as usize];
    before + (now - before) * eased((since / SNAP).clamp(0.0, 1.0))
}

/// The moves at `beat`, cued as a show is programmed to a track. Beams snap to new positions on
/// the beat and hold; they fire on the hits and fall back between them, never dark, so the whole
/// picture brightens no more than once a beat. In each loud run, the first block's centre fan
/// fires on the kicks, stepping through its tilts beat by beat, while the pair beside it fire on
/// the snares, jumping between mirrored aims; the second sends the fans up into the sky with the
/// outer pair stepping across on the eighth notes and the hi-hats chasing; the third brings
/// everything at once. Quieter bars light it less. A breakdown alone moves smoothly, the centre fan
/// turning slowly while the rest rest; its last two bars chase on the eighths, then the sixteenths,
/// as every beam closes in, and the last half beat goes dark before the drop lights everything.
fn moves(beat: f64) -> Moves {
    let [kick, snare, hat, _] = hits(beat).map(f64::from);
    let b = beat.max(0.0);
    let eighth = (b * 2.0).floor();
    let fan = |aim, spread, tilt, gate| Fan {
        aim,
        spread,
        tilt,
        gate,
    };
    let mut moves = match part(beat) {
        Part::Run {
            block,
            into_run,
            level,
            ..
        } => {
            let mut moves = match block % 3 {
                0 => Moves {
                    centre: fan(
                        0.0,
                        0.05 + 0.05 * kick,
                        stepped(&[0.05, 0.2, 0.1, 0.28, 0.06, 0.16, 0.24, 0.12], b, 1.0),
                        0.3 + kick,
                    ),
                    inner: fan(
                        stepped(&[0.18, 0.42, 0.28, 0.5], b - 1.0, 2.0),
                        0.04,
                        stepped(&[0.12, 0.22], b - 1.0, 2.0),
                        0.25 + 1.1 * snare,
                    ),
                    outer: fan(0.6, 0.03, 0.08, 0.2),
                    motion: [0.0, 0.0, (eighth * 2.0 % 8.0) as f32, 0.0],
                },
                1 => Moves {
                    centre: fan(0.0, 0.07, 0.3 + 0.05 * kick, 0.3 + 0.7 * kick),
                    inner: fan(
                        stepped(&[0.3, 0.45, 0.35, 0.55], b, 1.0),
                        0.05,
                        0.25,
                        0.25 + snare,
                    ),
                    outer: fan(
                        stepped(&[0.35, 0.5, 0.65, 0.8], b, 0.5),
                        0.04,
                        0.2,
                        0.3 + 0.7 * hat,
                    ),
                    motion: [
                        0.05,
                        (eighth * 0.8) as f32,
                        ((b * 4.0).floor() % 8.0) as f32,
                        (0.25 + 0.9 * snare) as f32,
                    ],
                },
                _ => Moves {
                    centre: fan(
                        0.0,
                        0.06 + 0.08 * kick,
                        stepped(&[0.06, 0.22, 0.12, 0.3], b, 1.0),
                        0.4 + kick,
                    ),
                    inner: fan(
                        stepped(&[0.2, 0.5, 0.3, 0.6], b - 1.0, 2.0),
                        0.05,
                        0.15,
                        0.35 + snare + 0.4 * kick,
                    ),
                    outer: fan(
                        stepped(&[0.4, 0.7, 0.55, 0.85], b, 0.5),
                        0.04,
                        stepped(&[0.05, 0.18], b, 0.5),
                        0.3 + hat + 0.3 * kick,
                    ),
                    motion: [
                        0.06,
                        (eighth * 0.8) as f32,
                        ((b * 4.0).floor() % 8.0) as f32,
                        (0.3 + snare) as f32,
                    ],
                },
            };
            // Quieter bars light it less; the drop lights everything at once.
            let lit = if into_run < 0.5 {
                1.5
            } else {
                0.6 + 0.4 * level
            };
            for fan in [&mut moves.centre, &mut moves.inner, &mut moves.outer] {
                fan.gate = if into_run < 0.5 { lit } else { fan.gate * lit };
            }
            moves
        }
        Part::Rest => Moves {
            centre: fan(
                0.25 * (std::f64::consts::TAU * b / 16.0).sin(),
                0.05,
                0.15 + 0.05 * (std::f64::consts::TAU * b / 8.0).sin(),
                0.45,
            ),
            inner: fan(0.3, 0.04, 0.15, 0.03),
            outer: fan(0.6, 0.03, 0.1, 0.03),
            motion: [0.03, (b * 0.4) as f32, 0.0, 0.0],
        },
        Part::Build(through) => {
            let rate = if through < 0.5 { 2.0 } else { 4.0 };
            // The last half beat of the build goes dark.
            let gate = if through >= 1.0 - 0.5 / 8.0 {
                0.05
            } else {
                0.3 + 0.7 * through
            };
            Moves {
                centre: fan(0.0, 0.05, 0.15, gate),
                inner: fan(0.35, 0.05, 0.15, gate),
                outer: fan(0.65, 0.04, 0.12, gate),
                motion: [0.0, 0.0, ((b * rate).floor() % 8.0) as f32, 0.0],
            }
        }
    };
    if beat < 0.0 {
        moves.motion[3] = 0.0;
    }
    moves
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
    crt: [f32; 4],
    centre: [f32; 4],
    inner: [f32; 4],
    outer: [f32; 4],
    motion: [f32; 4],
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
            .with("crt", self.crt)
            .with("centre", self.centre)
            .with("inner", self.inner)
            .with("outer", self.outer)
            .with("motion", self.motion)
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
                UniformName::new("crt", UniformType::_4f),
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
                Uniform::new("crt", (shot.crt[0], shot.crt[1], shot.crt[2], shot.crt[3])),
            ],
        ))
    }
}

/// The camera's lens: the screen magnified about the bar's middle, streaked back along the dive
/// in a zoom blur as the speed builds, its colours fringing towards the edges, and sinking into
/// the dark, with the desktop's CRT look over the middle of the transition.
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
// The desktop's CRT look over the middle of the transition: how strong, the tube's brightness
// this frame, how far it is torn, and which frame it is.
uniform vec4 crt;

// Every other row as dark as the tube's scanlines; the colours split apart and rows torn sideways
// as far as these, in screen pixels.
const float SCANLINE = 0.62;
const float SPLIT = 2.5;
const float TEAR = 14.0;

float hash1(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}

vec3 seen(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec3(0.0);
    }
    return texture2D(tex, uv).rgb;
}

void main() {
    vec2 p = v_coords * size;
    // On a torn frame, bands of rows jerk sideways.
    float band = floor(p.y / 18.0);
    p.x += crt.z * crt.x * TEAR * step(0.7, hash1(band + crt.w * 7.0));
    float split = SPLIT * crt.x;
    vec2 rel = p - centre;
    float fringe = 0.012 * view.y * length(rel) / size.y;
    vec3 sum = vec3(0.0);
    for (int k = 0; k < 12; k++) {
        // Further back along the dive, the way the world streaks past.
        float back = view.x * (1.0 + view.y * float(k) / 11.0);
        sum.r += seen(centre + (rel * (1.0 - fringe) + vec2(split, 0.0)) / back).r;
        sum.g += seen(centre + rel / back).g;
        sum.b += seen(centre + (rel * (1.0 + fringe) - vec2(split, 0.0)) / back).b;
    }
    vec3 colour = sum / 12.0;
    float edge = length(rel / size);
    colour *= 1.0 - view.z * (0.55 + 0.45 * smoothstep(0.0, 0.7, edge));
    colour *= 1.0 - 0.6 * view.y * smoothstep(0.15, 0.9, edge);
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : SCANLINE;
    colour *= mix(1.0, scan * crt.y, crt.x);
    vec4 out_colour = vec4(colour, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        out_colour = vec4(0.0, 0.2, 0.0, 0.2) + out_colour * 0.8;
#endif
    gl_FragColor = out_colour;
}
"#;

/// The club, a room rendered in 3D, in metres unless it says otherwise: x across, y up, z away from
/// you. Every pixel's ray from the camera finds the surface it meets, the floor, a wall, the stage
/// or the truss; each beam adds the light its haze scatters along the ray up to there, and lights
/// that surface where it lands and with the glow it spills, so the room is seen only by its lasers.
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
// How far into a breakdown it is, the drop's impact, the flare passing through the glass, and the
// liquid sky dropping in.
uniform vec4 mood;
// The desktop's CRT look over the middle of the transition: how strong, the tube's brightness
// this frame, how far it is torn, and which frame it is.
uniform vec4 crt;
// The fans' looks, cued to the music: the centre fan, the pair beside it and the outer pair, each
// where it aims to the side, how far apart its beams are, how high it tilts and how brightly it's
// lit; and how far waves roll across them, where the rolling has stepped to, which beam the chase
// is on, and how brightly the truss's projectors fire.
uniform vec4 centre;
uniform vec4 inner;
uniform vec4 outer;
uniform vec4 motion;
uniform vec3 ring;
uniform vec3 secondary;
uniform vec3 tertiary;

const float PI = 3.14159265;
// Half the view's height, as the tangent of its angle.
const float VIEW = 0.62;
// The room, in metres: its floor and ceiling, its side walls, the wall behind the stage and the one
// behind you; the stage's front, top and half-width; the truss over it, its height, depth and
// radius.
const float CEILING = 9.0;
const float SIDE = 15.0;
const float BACK = 32.0;
const float BEHIND = -8.0;
const float STAGE_FRONT = 21.0;
const float STAGE_TOP = 1.0;
const float STAGE_HALF = 11.0;
const float TRUSS_Y = 7.2;
const float TRUSS_Z = 22.0;
const float TRUSS_R = 0.18;
const float SCANLINE = 0.62;

// Where the camera is this frame: it drifts a little, as a hand-held one does.
vec3 eye;

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

// The haze at `p`: lumpy billows drifting towards you down the room, as the wind tunnel's smoke
// does, with clear air between them, thicker lower down.
float haze(vec3 p, float t) {
    vec3 q = p * 0.35 + vec3(0.0, 0.0, t * 0.6);
    float n = 0.5 * noise(q) + 0.3 * noise(q * 2.3 + 5.1) + 0.2 * noise(q * 5.1 + 2.7);
    return (0.15 + 1.7 * smoothstep(0.3, 0.8, n)) * exp(-max(p.y, 0.0) / 9.0);
}

vec3 vivid(vec3 c) {
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    return clamp(mix(vec3(grey), c, 1.7), 0.0, 1.0);
}

// The way a beam aimed `across` to the side and `up` goes, from the stage towards you.
vec3 aimed(float across, float up) {
    return normalize(vec3(sin(across) * cos(up), sin(up), -cos(across) * cos(up)));
}

// The nearest surface along `ray` from the eye: how far it is, its normal, and what it is: 0 the
// room's walls, floor and ceiling, 1 the stage, 2 the truss.
float surface(vec3 ray, out vec3 normal, out float what) {
    float best = 1e4;
    normal = vec3(0.0, 1.0, 0.0);
    what = 0.0;
    if (ray.y < 0.0) {
        best = -eye.y / ray.y;
    } else if (ray.y > 0.0) {
        best = (CEILING - eye.y) / ray.y;
        normal = vec3(0.0, -1.0, 0.0);
    }
    if (abs(ray.x) > 1e-4) {
        float t = (sign(ray.x) * SIDE - eye.x) / ray.x;
        if (t > 0.0 && t < best) {
            best = t;
            normal = vec3(-sign(ray.x), 0.0, 0.0);
        }
    }
    if (ray.z > 0.0) {
        float t = (BACK - eye.z) / ray.z;
        if (t < best) {
            best = t;
            normal = vec3(0.0, 0.0, -1.0);
        }
        float front = (STAGE_FRONT - eye.z) / ray.z;
        vec3 at = eye + ray * front;
        if (front < best && at.y < STAGE_TOP && abs(at.x) < STAGE_HALF) {
            best = front;
            normal = vec3(0.0, 0.0, -1.0);
            what = 1.0;
        }
        if (ray.y < 0.0) {
            float top = (STAGE_TOP - eye.y) / ray.y;
            vec3 on = eye + ray * top;
            if (top > 0.0 && top < best && on.z > STAGE_FRONT && abs(on.x) < STAGE_HALF) {
                best = top;
                normal = vec3(0.0, 1.0, 0.0);
                what = 1.0;
            }
        }
    }
    vec2 oc = vec2(eye.y - TRUSS_Y, eye.z - TRUSS_Z);
    vec2 rd = vec2(ray.y, ray.z);
    float qa = dot(rd, rd);
    float qb = 2.0 * dot(oc, rd);
    float qc = dot(oc, oc) - TRUSS_R * TRUSS_R;
    float disc = qb * qb - 4.0 * qa * qc;
    if (disc > 0.0) {
        float t = (-qb - sqrt(disc)) / (2.0 * qa);
        vec3 at = eye + ray * t;
        if (t > 0.0 && t < best && abs(at.x) < SIDE - 1.0) {
            best = t;
            normal = vec3(0.0, normalize(vec2(at.y - TRUSS_Y, at.z - TRUSS_Z)));
            what = 2.0;
        }
    }
    return best;
}

// How far a beam from `from` along `dir` goes before it lands on a wall, the floor or the ceiling.
float landing(vec3 from, vec3 dir) {
    float best = 80.0;
    if (dir.y < 0.0) best = min(best, -from.y / dir.y);
    if (dir.y > 0.0) best = min(best, (CEILING - from.y) / dir.y);
    if (abs(dir.x) > 1e-4) best = min(best, (sign(dir.x) * SIDE - from.x) / dir.x);
    if (dir.z < 0.0) best = min(best, (BEHIND - from.z) / dir.z);
    if (dir.z > 0.0) best = min(best, (BACK - from.z) / dir.z);
    return best;
}

// A beam from `from` along `dir`, `lit` bright, seen along `ray` from the eye up to the surface
// `wall` away, at `on`, facing `normal`. Adds to `lands` the light it throws on that surface: the
// spot where it lands, if that's here, and the glow its lit haze spills round it. Returns the
// light the haze scatters towards the eye along the beam: a core a few millimetres across, widening
// as the beam spreads with distance and drawn no thinner than a pixel, dimmer in step, so it never
// steps; a shaft of lit haze round it; brightest where the haze billows, seen along its length or
// pointed at you; hidden where the stage or a wall is in front of it.
vec3 beam(vec3 ray, float wall, vec3 on, vec3 normal, inout vec3 lands, vec3 from, vec3 dir,
          vec3 hue, float lit, float t, float pix) {
    if (lit <= 0.002) return vec3(0.0);
    vec3 colour = vivid(hue);
    float reach = landing(from, dir);
    vec3 end = from + dir * reach;
    vec3 off_end = on - end;
    float spot = dot(off_end, off_end);
    lands += colour * lit * (exp(-spot / 0.004) * 2.5 + exp(-spot / 0.25) * 0.2);
    vec3 to_surface = on - from;
    float near_beam = clamp(dot(to_surface, dir), 0.0, reach);
    vec3 spill = on - (from + dir * near_beam);
    lands += colour * lit * 0.06 / (1.0 + 1.2 * dot(spill, spill)) * max(dot(normal, -normalize(spill + 1e-4)), 0.0);

    vec3 w = eye - from;
    float b = dot(ray, dir);
    float d = dot(ray, w);
    float e = dot(dir, w);
    float den = max(1.0 - b * b, 1e-4);
    float along_ray = (b * e - d) / den;
    float along_beam = (e - b * d) / den;
    if (along_ray < 0.2 || along_ray > wall || along_beam < 0.0 || along_beam > reach) return vec3(0.0);
    vec3 off = (eye + ray * along_ray) - (from + dir * along_beam);
    float gap = length(off);
    float radius = 0.006 + along_beam * 0.0012;
    float sigma = max(radius, pix * along_ray * 0.6);
    float shaft = 0.1 + along_beam * 0.006;
    if (gap > max(4.0 * sigma, 5.0 * shaft)) return vec3(0.0);
    float core = exp(-gap * gap / (sigma * sigma)) * radius / sigma;
    float glow = exp(-gap / shaft) * 0.035;
    float towards = 1.0 + 1.5 * pow(max(-b, 0.0), 10.0);
    float lens = 1.0 / max(sqrt(den), 0.3);
    float fade = exp(-along_ray * 0.018) / (1.0 + along_beam * along_beam / 900.0);
    float billows = haze(from + dir * along_beam, t);
    return (colour * (1.8 * core + glow) + vec3(0.9) * core * core) * billows * towards * lens * fade * lit;
}

// The liquid sky: a sheet of light scanned overhead at `height`, a thin slab of lit haze that
// streams and ripples as the billows move through it, lit most where you look along it.
vec3 sky(vec3 ray, float wall, float height, vec3 hue, float t) {
    if (ray.y < 0.01) return vec3(0.0);
    float hit = (height - eye.y) / ray.y;
    if (hit > wall) return vec3(0.0);
    vec3 at = eye + ray * hit;
    if (at.z < 1.0 || at.z > STAGE_FRONT + 2.0 || abs(at.x) > SIDE) return vec3(0.0);
    // Long streaks across the room, rippling as the haze flows through the sheet.
    float ripple = at.z + 1.2 * noise(vec3(at.x * 0.25, t * 0.4, 1.0));
    float streams = 0.5 + 0.5 * sin(ripple * 3.5 - t * 2.0);
    float lit = (0.35 + 0.65 * streams * streams) * (0.4 + 0.6 * noise(vec3(at.x * 0.3, at.z * 0.5 - t * 0.6, t * 0.2)));
    float graze = 1.0 / max(ray.y, 0.05);
    float edge = smoothstep(STAGE_FRONT + 2.0, STAGE_FRONT - 4.0, at.z) * smoothstep(SIDE, SIDE - 4.0, abs(at.x));
    return vivid(hue) * lit * graze * edge * 0.07;
}

// Where `at` in the room shows, as the view's coordinates (height 1 to the top).
vec2 shown(vec3 at, vec3 forward, vec3 right, vec3 up) {
    vec3 v = at - eye;
    float z = max(dot(v, forward), 0.01);
    return vec2(dot(v, right), dot(v, up)) / (z * VIEW);
}

void main() {
    // This pixel on the screen, torn sideways on a torn frame of the CRT look.
    vec2 q = place.xy + v_coords * place.zw;
    float band = floor(q.y / 18.0);
    q.x += crt.z * crt.x * 14.0 * step(0.7, fract(sin((band + crt.w * 7.0) * 12.9898) * 43758.5453));
    // Where it is in the club, which comes from far off about the bar's middle as the glass comes
    // in, and kicks as the drop lands.
    vec2 focus = vec2(0.5 * screen.x, beat.w);
    vec2 club = focus + (q - focus) / beat.z;
    club.y += (1.0 - beat.z) * (0.5 * screen.y - beat.w);
    vec2 n = vec2(club.x - 0.5 * screen.x, 0.5 * screen.y - club.y) / (0.5 * screen.y);
    float b = beat.x;
    float drop = mood.y;
    float t = max(b, 0.0) * 60.0 / 118.0;
    float clock = b * 60.0 / 118.0;

    // The camera drifts and sways a little, as a hand-held one does, and kicks with the drop.
    eye = vec3(0.3 * sin(clock * 0.27), 1.75 + 0.06 * sin(clock * 0.41), 0.25 * sin(clock * 0.19));
    float yaw = 0.025 * sin(clock * 0.21) + 0.004 * drop * sin(b * 97.0);
    float pitch = 0.14 + 0.015 * sin(clock * 0.33) + 0.004 * drop * cos(b * 83.0);
    vec3 forward = normalize(vec3(sin(yaw), pitch, cos(yaw)));
    vec3 right = normalize(cross(vec3(0.0, 1.0, 0.0), forward));
    vec3 up = cross(forward, right);
    vec3 ray = normalize(forward + n.x * VIEW * right + n.y * VIEW * up);
    float pix = 2.0 * VIEW / screen.y / beat.z;

    float kick = hits.x;
    float snare = hits.y;
    float hat = hits.z;
    float build = hits.w;
    float calm = mood.x;
    float bar = floor(max(b, 0.0) / 4.0);
    float sixteenth = floor(max(b, 0.0) * 4.0);
    // Before the drop the projectors stand by, faintly; the drop brings them up at once.
    float on = b < 0.0 ? 0.15 : 1.0;
    // The colours take turns on the projectors, a section at a time.
    float turn = mod(floor(bar / 8.0), 3.0);
    vec3 c1 = turn < 0.5 ? ring : (turn < 1.5 ? secondary : tertiary);
    vec3 c2 = turn < 0.5 ? secondary : (turn < 1.5 ? tertiary : ring);
    vec3 c3 = turn < 0.5 ? tertiary : (turn < 1.5 ? ring : secondary);

    vec3 normal;
    float what;
    float wall = surface(ray, normal, what);
    vec3 on_surface = eye + ray * wall;
    vec3 lands = vec3(0.0);
    vec3 air = vec3(0.0);

    // The tunnel: twelve beams from high on the stage scanned round a cone that opens over you, so
    // you stand inside it, turning, opening with the kicks and bursting open as the drop lands.
    if (cue.x > 0.0) {
        vec3 apex = vec3(0.0, 6.5, 27.0);
        vec3 axis = normalize(eye + vec3(0.0, 3.0, 0.0) - apex);
        vec3 side = normalize(cross(axis, vec3(0.0, 1.0, 0.0)));
        vec3 lift = cross(side, axis);
        float open = 0.12 + 0.025 * kick + 0.05 * drop;
        for (int k = 0; k < 12; k++) {
            float round_about = 2.0 * PI * float(k) / 12.0 + b * PI * 0.0625;
            vec3 dir = normalize(axis + open * (side * cos(round_about) + lift * sin(round_about)));
            vec3 hue = mod(float(k), 2.0) < 0.5 ? c1 : c2;
            air += beam(ray, wall, on_surface, normal, lands, apex, dir, hue, cue.x * (0.35 + 0.4 * kick) * on, t, pix);
        }
    }

    // The fans: five projectors along the stage, eight beams each, mirror images either side of the
    // middle, each pair taking its look from the cues; the chase lights one beam of each at a time,
    // and over the build's last bars every beam closes in on you.
    if (cue.y > 0.0 || cue.w > 0.0) {
        for (int j = 0; j < 5; j++) {
            float fj = float(j) - 2.0;
            float mirror = sign(fj);
            vec4 look = abs(fj) < 0.5 ? centre : (abs(fj) < 1.5 ? inner : outer);
            vec3 from = vec3(fj * 4.5, STAGE_TOP + 0.5, STAGE_FRONT + 1.5);
            vec3 hue = abs(fj) < 0.5 ? c1 : (abs(fj) < 1.5 ? c2 : c3);
            for (int k = 0; k < 8; k++) {
                float fk = float(k);
                float across = mirror * look.x + (fk - 3.5) * look.y * (mirror == 0.0 ? 1.0 : mirror);
                float rise = look.z + motion.x * sin(motion.y - fk * 0.6 - abs(fj) * 0.8);
                vec3 dir = aimed(across, rise);
                dir = normalize(mix(dir, normalize(eye + vec3(fj * 0.3, 0.3, 0.0) - from), build));
                float chased = mod(fk + float(j) * 2.0, 8.0) == motion.z ? 1.0 : 0.15;
                float lit = look.w * mix(1.0, chased, cue.w);
                air += beam(ray, wall, on_surface, normal, lands, from, dir, hue, lit * on, t, pix);
            }
        }
        // Two projectors hung at the truss's ends, crossing beams down over the floor to the far
        // side, switching sides of the room bar by bar, firing with the snares.
        if (motion.w > 0.0) {
            for (int s = 0; s < 2; s++) {
                float side = s == 0 ? -1.0 : 1.0;
                vec3 from = vec3(side * 12.5, TRUSS_Y - 0.4, TRUSS_Z);
                float reach_over = mod(bar, 2.0) < 0.5 ? 0.5 : 0.75;
                for (int k = 0; k < 4; k++) {
                    float fk = float(k);
                    vec3 dir = aimed(-side * reach_over + (fk - 1.5) * 0.06, -0.14 - 0.03 * fk);
                    air += beam(ray, wall, on_surface, normal, lands, from, dir, c3, motion.w * on, t, pix);
                }
            }
        }
    }

    // Before the drop, the bar's own lasers: the centre projector throws the wind tunnel's fan,
    // four beams each way in the ring's colour opening and closing like scissors, out to either
    // side, so the dive carries straight on into them. As the drop lands they swing round towards
    // you and every other projector fires.
    float before = 1.0 - smoothstep(-0.25, 1.0, b);
    if (before > 0.0) {
        float scissor = 0.014 * sin(2.0 * PI * b / 15.0);
        vec3 from = vec3(0.0, STAGE_TOP + 0.5, STAGE_FRONT + 1.5);
        float swing = smoothstep(-0.25, 1.0, b);
        for (int i = 0; i < 8; i++) {
            float fi = float(i);
            float side = i < 4 ? -1.0 : 1.0;
            float up = mod(fi, 2.0) < 0.5 ? 1.0 : -1.0;
            bool inner = mod(floor(fi / 2.0), 2.0) < 0.5;
            float rise = up * (inner ? 0.022 + scissor : 0.05 - scissor) * 3.0;
            float across = side * mix(1.45, 0.3 + 0.1 * fi, swing);
            air += beam(ray, wall, on_surface, normal, lands, from, aimed(across, rise), ring, before * 1.4, t, pix);
        }
    }

    // The liquid sky, dropping in with the second crash and rising and falling with the bass.
    if (cue.z > 0.0) {
        float height = 5.2 - 0.5 * kick + 2.0 * mood.w;
        air += sky(ray, wall, height, c2, t) * cue.z * (0.8 + 0.6 * hat) * on;
    }

    // The room, lit only by what the lasers throw on it, and the air glowing faintly with the light
    // scattered through it.
    vec3 albedo = what < 0.5 ? vec3(0.16, 0.17, 0.2) : (what < 1.5 ? vec3(0.22) : vec3(0.4));
    vec3 room = albedo * (vec3(0.01) + lands);
    float busy = (cue.x + cue.y + cue.z + cue.w) * on;
    air += mix(c1, c2, 0.5 + 0.5 * n.x) * haze(eye + ray * min(wall, 14.0), t) * 0.006 * busy;

    // The projectors' lenses, bright points on the stage with a little flare.
    vec3 colour = room + air;
    for (int j = 0; j < 5; j++) {
        vec2 d = n - shown(vec3((float(j) - 2.0) * 4.5, STAGE_TOP + 0.5, STAGE_FRONT + 1.5), forward, right, up);
        float flare = exp(-dot(d, d) * 20000.0) + exp(-dot(d, d) * 900.0) * 0.08
            + exp(-abs(d.y) * 220.0) * exp(-abs(d.x) * 4.0) * 0.05;
        colour += mix(j == 2 ? c1 : c2, vec3(1.0), 0.6) * flare * (cue.y + cue.w + 0.4 * cue.x) * (1.0 - 0.8 * calm) * on;
    }

    // As the camera passes through the glass, a flare sweeps across it.
    vec2 local = v_coords * place.zw;
    float streak = exp(-abs(q.y - focus.y) / (6.0 + 0.05 * place.w)) * mood.z;
    colour += mix(vivid(ring), vec3(1.0), 0.4) * streak * 1.5 + vivid(ring) * mood.z * 0.15;
    // While the glass is still coming in, its edge shows round the club.
    float edge = min(min(local.x, place.z - local.x), min(local.y, place.w - local.y));
    colour += vivid(ring) * exp(-edge * edge / 6.0) * (1.0 - beat.y) * 0.6;

    // A film camera's response: highlights roll off to white, the drop blooms, and a little grain.
    colour *= 2.2 * (1.0 + 0.6 * drop);
    colour = clamp((colour * (2.51 * colour + 0.03)) / (colour * (2.43 * colour + 0.59) + 0.14), 0.0, 1.0);
    colour += (hash(vec3(gl_FragCoord.xy, floor(clock * 24.0))) - 0.5) * 0.02;
    // The CRT look over the middle of the transition.
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : SCANLINE;
    colour *= mix(1.0, scan * crt.y, crt.x);
    vec4 out_colour = vec4(max(colour, 0.0), 1.0) * beat.y * alpha;
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
            intro: None,
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
    fn the_crt_look_comes_and_goes_over_the_middle_half_of_the_transition() {
        assert_eq!(tube_look(0.2), 0.0);
        assert_eq!(tube_look(0.5), 1.0);
        assert_eq!(tube_look(0.8), 0.0);
        let rave = rave(false);
        assert_eq!(rave.shot(10.0 + DIVE * 0.5).expect("diving").crt[0], 1.0);
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
        assert_eq!(part(26.0 * 4.0 + 1.0), Part::Rest);
        assert!(hits(126.0)[3] > 0.6);
        assert_eq!(hits(100.0)[3], 0.0);
    }

    #[test]
    fn beams_snap_to_their_next_position_on_the_beat_and_hold_till_the_next() {
        let beat_secs = 60.0 / BPM;
        // The centre fan's tilt: still on the old one at the beat, on the new one a snap later,
        // and holding there till the next beat.
        let at = |beat: f64| moves(beat).centre.tilt;
        assert_eq!(at(1.0), 0.05);
        assert_eq!(at(1.0 + SNAP / beat_secs), 0.2);
        assert_eq!(at(1.9), 0.2);
    }

    #[test]
    fn the_centre_fan_fires_on_the_kicks_and_the_pair_beside_it_on_the_snares() {
        let first_kick = moves(4.02);
        let between = moves(4.45);
        assert!(first_kick.centre.gate > between.centre.gate + 0.3);
        let snare = moves(5.02);
        assert!(snare.inner.gate > between.inner.gate + 0.3);
        // Never dark between the hits.
        assert!(between.centre.gate >= 0.3 && between.inner.gate >= 0.25);
    }

    #[test]
    fn the_breakdown_rests_the_outer_fans_and_the_drop_lights_everything_after_a_dark_half_beat() {
        let breakdown = moves(26.0 * 4.0 + 1.0);
        assert!(breakdown.outer.gate < 0.1 && breakdown.centre.gate > 0.3);
        let dark = moves(LOOP_BEATS - 0.2);
        assert!(dark.centre.gate < 0.1 && dark.inner.gate < 0.1);
        assert!(moves(0.1).outer.gate > 1.0);
    }

    #[test]
    fn the_loop_runs_from_the_drop_to_where_the_track_comes_back_round_to_it() {
        assert_eq!(LOOP_BEATS as usize * 4, STEPS);
        assert!(
            matches!(part(0.5), Part::Run { block: 0, .. }),
            "it opens on the drop"
        );
        // Both breakdowns, the second building into the drop as the loop comes round.
        assert_eq!(part(25.0 * 4.0), Part::Rest);
        assert_eq!(part(84.0 * 4.0), Part::Rest);
        assert!(matches!(part(LOOP_BEATS - 1.0), Part::Build(_)));
        // The loud run after the first breakdown cycles on through its blocks.
        assert!(matches!(part(42.0 * 4.0), Part::Run { block: 1, .. }));
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
        assert!(
            MUSIC.starts_with(b"OggS") && INTRO.starts_with(b"OggS") && HINT.starts_with(b"OggS")
        );
        let licence = include_str!("../../../assets/music/LICENSE.txt");
        assert!(licence.contains("REKLAW") && licence.contains("CC BY-NC-ND 4.0"));
    }
}
