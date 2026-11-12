//! The rave, an easter egg of the wind tunnel bar: knock on the bar's empty glass ten times
//! quickly and the camera dives into the bar. It rockets in on the bar's middle, the world
//! streaking past in a zoom blur, its colours fringing and the screen sinking into the dark, while
//! the bar's own glass, kept sharp, comes at you faster still. Half-way in, the desktop's CRT look
//! flares up hard: deep scanlines, split colours, torn and bulging rows, static and a rolling band.
//! The bar's lasers and smoke carry straight on into the club, and as the music drops the lasers
//! swing round into the show: the loud section of "The Mundane Insane" by REKLAW and the breakdown
//! after it, in the club inside the wind tunnel. Any key, or a click that isn't more of the
//! knocking, ends it: the music stops at once and the camera pulls back out into the bar. The keys
//! a machine has for itself, volume, brightness and media, go on doing their jobs while it plays.
//!
//! The club is a room rendered in 3D, seen only by its lasers: a floor, walls, a stage and a truss
//! over it, five projectors along the stage and two hung at the truss's ends. The room is full of
//! smoke, soft clouds drifting out from the stage over the crowd, which shows only where a laser
//! lights it, brightest looking back towards the projector, and glows faintly between them. Each
//! beam is a thin line through it, hidden behind the stage or a wall, kept over the crowd's heads
//! as a show keeps them, and lands on the room as a spot. In the slower routines a
//! projector scans a sheet of light between its beams, a wedge that thins with distance, its edges
//! bright where the scan turns, the smoke's clouds showing in it as a slice through them. A
//! projector can also scan beams round a cone, a tunnel you stand inside, or a sheet of light
//! overhead, a "liquid sky". The camera arrives in its place in the crowd and drifts a little as a
//! hand-held one does. The bar's smoke streams out of where the camera dived in, and gives way to
//! the room's own. It sees as a film camera does: highlights roll off to white, with a little grain.
//! It is drawn at half the screen's resolution and scaled up, so it runs smoothly on a laptop.
//!
//! The show is programmed to the track as a laser show is: one routine to a phrase of a few bars,
//! moving smoothly within it and easing into the next so it arrives on the downbeat. The fans open
//! slowly, every beam sweeps together in unison, the two sides cross in an X whose meeting point
//! moves on each downbeat, waves roll across the fans, and a pulse chases from beam to beam across
//! every projector; over each eight-bar block's last bar every beam draws into one point, which
//! bursts open on the next downbeat. The kicks, snares and hi-hats of each sixteenth note were
//! measured from the music and are kept here: the kicks light the centre fan and the snares the pair
//! beside it, so the hits add light without moving a beam. A breakdown slows to the centre fan
//! turning in the smoke before its last two bars build, every beam closing in on you as the chase
//! speeds up, into a dark half beat and the drop. Big changes of brightness come no faster than the
//! beat, under the three a second that can bring on a seizure; only thin beams chase faster. It is
//! drawn in the theme's three colours.
//!
//! The music plays through PipeWire (`pw-play`), as the volume click does, as one gapless stream:
//! the build-up into the drop, then the track's loop, from its drop to where it comes back round to
//! it, twice over. It starts with the dive, so the drop lands as the camera does, and if it runs out
//! it starts again, its build-up leading into the drop as the loop's own does. If Slipstream goes,
//! the music stops with it. Nested runs, whose sound would come out of the desktop around them, play
//! the show without it.
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
        shader::{self, Values},
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
/// How much of the dive's magnification the camera does, as a share of its logarithm: the rest is
/// the bar's glass coming nearer. Together they are worked out from the screen, to just more than
/// fill it, whatever its size.
const CAMERA_SHARE: f64 = 0.7;
const FILL_MORE: f64 = 1.1;
/// How fast the bar's lasers and smoke in the club go on growing once the club has faded in over
/// the bar, as a share of the logarithm of the dive's magnification. Until then they grow with the
/// camera, so they lie on the bar's own.
const SEEN_SHARE: f64 = 0.25;
/// How often the CRT look tears the picture during the transition, a chance each of the tube's
/// frames; at its peak every frame tears.
const TEAR_CHANCE: f32 = 0.12;
/// How far back the club is as the glass comes in, as a share of its size on screen.
const CLUB_FAR: f64 = 0.15;
/// The music's tempo, and how many bars, beats and sixteenths there are in the loop: from the drop
/// to where the track comes back round to it.
const BPM: f64 = 118.0;
const BARS: usize = 89;
const LOOP_BEATS: f64 = BARS as f64 * 4.0;
const STEPS: usize = BARS * 16;
/// How long the music takes to be heard after `pw-play` is started, seconds, and how far ahead of
/// it the show is drawn, about a frame, so a move is on screen as its beat is heard.
const LATENCY: f64 = 0.04;
const LEAD: f64 = 1.0 / 60.0;
/// How long the music's build-up runs before its drop, seconds: the dive less the latency, so the
/// drop lands as the camera does.
const INTRO_SECS: f64 = 1.96;
/// The beat the show is held at with reduced motion: the fans, a bar into the second.
const STILL_BEAT: f64 = 21.0;
/// How quickly each kind of hit dies away, seconds: kicks, snares, hi-hats.
const KICK_DECAY: f64 = 0.14;
const SNARE_DECAY: f64 = 0.11;
const HAT_DECAY: f64 = 0.06;
/// How far a sixteenth must jump in loudness, in the steps it's measured in, to count as a hit of
/// each kind: only the strongest third of them do, so the show moves on fewer hits and each one
/// shows; and how much harder a hit moves and lights the show than the music measures it.
const KICK_LEAST: u8 = 3;
const SNARE_LEAST: u8 = 4;
const HAT_LEAST: u8 = 3;
const HIT_GAIN: f64 = 3.0;
/// How much of the screen's resolution the club is drawn at, across and down: a quarter of the
/// pixels, scaled up. Its light is soft and every beam is at least a pixel wide, so it shows
/// little, and it keeps the show smooth on a laptop's graphics.
const RESOLUTION: f64 = 0.5;
/// The club's camera and stage, which its shader is given: half the view's height as the tangent
/// of its angle, the eye's height in metres and how far it looks up (the rise of its gaze for a
/// metre ahead), and the stage's top and front, in metres. The projectors stand half a metre
/// over the stage, a metre and a half behind its front.
const VIEW: f32 = 0.62;
const EYE_HEIGHT: f32 = 1.75;
const LOOK_UP: f32 = 0.14;
const STAGE_TOP: f32 = 1.0;
const STAGE_FRONT: f32 = 21.0;
/// The bar's height in the units its glass is drawn in.
const BAR_UNITS: f64 = 40.0;

/// "The Mundane Insane" by REKLAW (CC BY-NC-ND 4.0; see `assets/music/LICENSE.txt`) as one
/// gapless stream: the build-up into the drop, then the loop from the drop to where the track comes
/// back round to it, twice.
static MUSIC: &[u8] = include_bytes!("../../../assets/music/the-mundane-insane-loop.opus");
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

/// The music while it plays: `pw-play`, when it was started, and whether it has come round again
/// after its first time through.
struct Music {
    child: Child,
    started: f64,
    again: bool,
}

impl Drop for Music {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the music at `now`, unless this is a nested run or it can't be written out or played.
fn play(now: f64, again: bool) -> Option<Music> {
    let path = written("slipstream-rave.opus", MUSIC)?;
    spawn(&mut player(&path), "music").map(|child| Music {
        child,
        started: now,
        again,
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
    /// The bar's air clock when it started, seconds, which the bar's lasers and smoke in the club
    /// carry on from.
    air: f64,
    /// How far the bar has to be magnified, all told, about its middle to fill the screen.
    fill: f64,
    began: f64,
    /// The last knock, which more quick ones carry on from rather than ending the show.
    last_knock: f64,
    /// When it was asked to end, and how far into the dive it was then.
    ending: Option<(f64, f64)>,
    music: Option<Music>,
    /// Reduced motion: no camera move, and the show held still.
    still: bool,
    kept: shader::Kept,
    camera: Camera,
    /// The club drawn at a fraction of the screen's resolution, and whether it can't be.
    canvas: Option<tilt::Canvas>,
    canvas_broken: bool,
}

/// The camera's move at one moment of the dive: how far it magnifies about the bar's middle, how
/// much the world streaks with the speed, how far it has sunk into the dark, and the CRT look over
/// it: how strong, the tube's brightness this frame, how far it is torn, and which frame it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub level: f64,
    pub blur: f64,
    pub dark: f64,
    pub crt: [f32; 4],
}

impl Rave {
    /// The rave starting at `now` on `screen`, `size` logical pixels, whose bar is at `bar` with
    /// its air clock at `air`.
    pub fn begin(
        screen: String,
        size: Size<f64, Logical>,
        bar: Rectangle<f64, Logical>,
        air: f64,
        now: f64,
        still: bool,
    ) -> Self {
        let mut rave = Self {
            screen,
            fill: fill(size, bar),
            bar,
            air,
            began: now,
            last_knock: now,
            ending: None,
            // The music starts with the dive: its build-up runs into the drop as the camera lands.
            music: play(now, false),
            still,
            kept: shader::Kept::default(),
            camera: Camera::default(),
            canvas: None,
            canvas_broken: false,
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
        }
    }

    /// Moves on to `now`, starting the music again if it has run out. Whether it is still on
    /// screen.
    pub fn tick(&mut self, now: f64) -> bool {
        if let Some(music) = self.music.as_mut()
            && !matches!(music.child.try_wait(), Ok(None))
        {
            self.music = play(now, true);
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
        let look = tube_look(u);
        let (flicker, torn) = crate::effects::fx::tube(now, TEAR_CHANCE);
        Some(Shot {
            level: self.fill.powf(CAMERA_SHARE * w),
            blur: 0.3 * w.powf(1.4),
            dark: smooth(0.35, 0.9, w),
            crt: [
                look as f32,
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

    /// When the music's drop lands, by the clock, and whether the music has come round again:
    /// from when the music started, or, without it, as the camera lands.
    fn drop_at(&self) -> (f64, bool) {
        self.music
            .as_ref()
            .map_or((self.began + DIVE, false), |music| {
                (music.started + LATENCY + INTRO_SECS, music.again)
            })
    }

    /// The beat at `now`, counted from the start of the music's loop. Before the first drop it is
    /// below zero.
    fn beat(&self, now: f64) -> f64 {
        if self.still {
            return STILL_BEAT;
        }
        let (drop, again) = self.drop_at();
        looped((now - drop + LEAD) * BPM / 60.0, again)
    }

    /// How many beats it is at `now` since the camera landed, however the music has looped.
    fn landed(&self, now: f64) -> f64 {
        (now - self.began - DIVE + LEAD) * BPM / 60.0
    }

    /// Where the bar's middle is shown at `now` on a screen `size` logical pixels: the camera dives
    /// in on it, carrying it to where the club's centre projector shows by the time it lands, so
    /// the bar's lasers finish where the show's middle cluster fires from.
    fn held(&self, now: f64, size: Size<f64, Logical>) -> Point<f64, Logical> {
        let bar = self.bar_middle();
        let to = centre_projector(size);
        let travel = smooth(0.0, 1.0, self.dived(now));
        (
            bar.x + (to.x - bar.x) * travel,
            bar.y + (to.y - bar.y) * travel,
        )
            .into()
    }

    /// The club as `look` says at `now`, over the screen or, during the dive, over the bar's glass
    /// coming nearer, drawn at `RESOLUTION` and scaled up, or at the screen's own if it can't be.
    /// `None` if the shader didn't compile.
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        look: &Look,
        now: f64,
    ) -> Option<OutputElement> {
        let program = program(renderer)?;
        if !self.canvas_broken {
            let scale = look.scale.x * RESOLUTION;
            let sizes = tilt::Sizes {
                logical: look.screen.to_i32_round(),
                physical: look.screen.to_physical(scale).to_i32_round(),
                scale,
            };
            let small = Look {
                scale: Scale::from(scale),
                ..*look
            };
            let (area, uniforms) = self.uniforms(&small, now);
            let club = OutputElement::Bar(self.kept.pixel(program.clone(), area, &uniforms));
            if let Some(drawn) = tilt::draw_offscreen(
                renderer,
                &mut self.canvas,
                &mut self.canvas_broken,
                &[club],
                sizes,
                "the rave",
            ) {
                return Some(OutputElement::Texture(drawn));
            }
        }
        let (area, uniforms) = self.uniforms(look, now);
        Some(OutputElement::Bar(
            self.kept.pixel(program, area, &uniforms),
        ))
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
        let held = self
            .held(now, sizes.logical.to_f64())
            .to_physical(sizes.scale);
        self.camera
            .shot(renderer, elements, sizes, (centre, held), shot)
    }

    /// Where the club is drawn and its shader's numbers, as `look` says at `now`.
    fn uniforms(&self, look: &Look, now: f64) -> (Rectangle<i32, Logical>, Uniforms) {
        let u = self.dived(now);
        let w = if self.still { 1.0 } else { rocket(u) };
        let s = look.scale.x;
        let screen = look.screen;
        let middle = self.held(now, screen);
        // During the dive, the bar's glass, nearer about its middle, magnified with the camera;
        // once landed, the screen itself.
        let shot = self.shot(now);
        let area = match shot {
            Some(shot) => {
                let glass = self.fill.powf((1.0 - CAMERA_SHARE) * w);
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
        let [kick, snare, hat] = hits(beat);
        // The bar's lasers and smoke grow with the camera about the bar's middle until the club has
        // faded in over them, so they lie on the bar's own, then more slowly.
        let seen = self
            .fill
            .powf(CAMERA_SHARE * w.min(0.5) + SEEN_SHARE * (w - 0.5).max(0.0));
        let f = (f64::from(crate::shell::bar::HEIGHT) * s).round() / BAR_UNITS;
        // The bar's own fan shows until the club's fans take over, and its smoke gives way to the
        // room's as the camera comes through the glass; pulling out, both come back as the club
        // goes.
        let leaving = if self.ending.is_some() {
            1.0 - smooth(0.5, 1.0, w)
        } else {
            0.0
        };
        let (room, fan, air) = if self.still {
            (1.0, 0.0, self.air)
        } else {
            let landed = self.landed(now);
            (
                smooth(0.6, 1.0, w) * (1.0 - leaving),
                (1.0 - smooth(0.0, 0.5, landed)).max(leaving),
                self.air + now - self.began,
            )
        };
        let uniforms = Uniforms {
            place: [
                (shown.loc.x as f64 * s) as f32,
                (shown.loc.y as f64 * s) as f32,
                (shown.size.w as f64 * s) as f32,
                (shown.size.h as f64 * s) as f32,
            ],
            screen: [
                (screen.w * s) as f32,
                (screen.h * s) as f32,
                (middle.x * s) as f32,
                (middle.y * s) as f32,
            ],
            beat: [
                beat as f32,
                // The bar's text fades as its glass comes in, and the club comes from far off.
                smooth(0.1, 0.5, w) as f32,
                (CLUB_FAR + (1.0 - CLUB_FAR) * w * w) as f32,
                room as f32,
            ],
            bar: [air as f32, f as f32, seen as f32, fan as f32],
            hits: [kick, snare, hat, moves.converge as f32],
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
            motion: [moves.wave, moves.rolled, moves.chased, moves.truss].map(|n| n as f32),
            shape: [moves.unison, moves.cross, moves.meet, moves.chase].map(|n| n as f32),
            scanned: [moves.sheet as f32],
            ring: look.ring,
            secondary: look.secondary,
            tertiary: look.tertiary,
        };
        (shown, uniforms)
    }
}

/// `beats` since the music's drop as a beat of its loop, given whether the music has come round
/// again: then the beats before the drop are its build-up, the end of the loop; the first time,
/// they are the dive, below zero.
fn looped(beats: f64, again: bool) -> f64 {
    if beats >= 0.0 {
        beats % LOOP_BEATS
    } else if again {
        beats + LOOP_BEATS
    } else {
        beats
    }
}

/// Where the club's centre projector shows on a screen `size` logical pixels as the camera lands,
/// looking straight down the room from its place in the crowd, a little up.
fn centre_projector(size: Size<f64, Logical>) -> Point<f64, Logical> {
    let (rise, ahead) = (
        f64::from(STAGE_TOP) + 0.5 - f64::from(EYE_HEIGHT),
        f64::from(STAGE_FRONT) + 1.5,
    );
    let look = f64::from(LOOK_UP);
    let length = (1.0 + look * look).sqrt();
    // Its height and distance as the camera sees them, tilted up by the camera's look.
    let (up, forward) = (
        (rise - ahead * look) / length,
        (rise * look + ahead) / length,
    );
    let shown = up / (forward * f64::from(VIEW));
    (size.w / 2.0, size.h / 2.0 * (1.0 - shown)).into()
}

/// What the club is drawn with.
#[derive(Clone, Copy)]
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

/// How far the dive has rocketed in at `u`, 0 to 1, as a share of the logarithm of its whole
/// magnification: slowly at first and ever faster to the end, never back.
fn rocket(u: f64) -> f64 {
    u.clamp(0.0, 1.0).powf(2.4)
}

/// How strong the desktop's CRT look is over the transition at `u`: it ramps in over its second
/// quarter and back out over its third, so it lies over the middle half, its peak at the middle.
fn tube_look(u: f64) -> f64 {
    smooth(0.25, 0.5, u) * (1.0 - smooth(0.5, 0.75, u))
}

fn eased(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn smooth(low: f64, high: f64, x: f64) -> f64 {
    eased(((x - low) / (high - low)).clamp(0.0, 1.0))
}

fn lerp(from: f64, to: f64, x: f64) -> f64 {
    from + (to - from) * x
}

/// How hard sixteenth `step` of `band` hits: how much louder it is than the one before, 0 to 1.
fn onset(band: &[u8; STEPS], step: usize) -> f64 {
    let level = |n: usize| f64::from(band[n % STEPS] - b'0');
    ((level(step) - level(step + STEPS - 1)) / 9.0).max(0.0)
}

/// How hard sixteenth `step` of `band` hits, if it jumps by at least `least` steps, else 0.
fn hit(band: &[u8; STEPS], step: usize, least: u8) -> f64 {
    let jump = onset(band, step);
    if jump * 9.0 + 1e-6 >= f64::from(least) {
        jump
    } else {
        0.0
    }
}

/// How much of `band`'s hits of at least `least` steps is still ringing at `beat`, each dying away
/// over `decay` seconds.
fn ringing(band: &[u8; STEPS], least: u8, beat: f64, decay: f64) -> f64 {
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
            hit(band, step, least) * (-ago / decay).exp()
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

/// The kicks, snares and hi-hats ringing at `beat`.
fn hits(beat: f64) -> [f32; 3] {
    [
        (HIT_GAIN * ringing(KICKS, KICK_LEAST, beat, KICK_DECAY)) as f32,
        (HIT_GAIN * ringing(SNARES, SNARE_LEAST, beat, SNARE_DECAY)) as f32,
        (HIT_GAIN * ringing(HATS, HAT_LEAST, beat, HAT_DECAY)) as f32,
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

/// How much of the tunnel, the fans and the liquid sky is on at `beat`, and how thick the smoke
/// is, a little thinner in the loud runs. Each loud run cycles through three blocks of eight bars:
/// the tunnel giving way to the fans, the sky dropping in, and everything at once. A breakdown keeps
/// the tunnel and the sky turning slowly; its last two bars build into the drop. The cue changes on
/// a bar's downbeat, coming in over a sixteenth as a show's does.
fn cue(beat: f64) -> [f32; 4] {
    if beat < 0.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let at = |beat: f64| -> [f32; 4] {
        match part(beat) {
            Part::Rest => [0.6, 0.3, 0.6, 1.0],
            Part::Build(_) => [0.8, 1.0, 0.3, 1.0],
            Part::Run {
                block, into_block, ..
            } => match block % 3 {
                0 if into_block < 16.0 => [1.0, 0.5, 0.0, 0.8],
                0 => [0.3, 1.0, 0.0, 0.8],
                1 if into_block < 16.0 => [0.0, 0.4, 1.0, 0.8],
                1 => [0.5, 0.7, 1.0, 0.8],
                _ => [1.0, 1.0, 0.6, 0.8],
            },
        }
    };
    let bar_start = (beat / 4.0).floor() * 4.0;
    let snap = ((beat - bar_start) * 4.0).clamp(0.0, 1.0) as f32;
    let (now, before) = (at(beat), at((bar_start - 0.01).rem_euclid(LOOP_BEATS)));
    std::array::from_fn(|n| before[n] + (now[n] - before[n]) * snap)
}

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

    /// `x` of the way from this look to `to`.
    fn towards(self, to: Fan, x: f64) -> Fan {
        Fan {
            aim: lerp(self.aim, to.aim, x),
            spread: lerp(self.spread, to.spread, x),
            tilt: lerp(self.tilt, to.tilt, x),
            gate: lerp(self.gate, to.gate, x),
        }
    }
}

/// The show's moves at one moment: the centre fan, the pair either side of it and the outer pair;
/// how far waves roll across them and where the rolling has got to; how far the fans aim the same
/// way rather than as mirror images; how far every beam is drawn to one meeting point, and where
/// across the room it is, in metres; how much a pulse chases from beam to beam and which beam it
/// has got to; how brightly the truss's projectors fire; how far every beam closes in on you; and
/// how far the projectors scan sheets of light between their beams rather than drawing beams.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Moves {
    centre: Fan,
    inner: Fan,
    outer: Fan,
    wave: f64,
    rolled: f64,
    unison: f64,
    cross: f64,
    meet: f64,
    chase: f64,
    chased: f64,
    truss: f64,
    converge: f64,
    sheet: f64,
}

impl Moves {
    /// `x` of the way from these moves to `to`.
    fn towards(self, to: Moves, x: f64) -> Moves {
        Moves {
            centre: self.centre.towards(to.centre, x),
            inner: self.inner.towards(to.inner, x),
            outer: self.outer.towards(to.outer, x),
            wave: lerp(self.wave, to.wave, x),
            rolled: lerp(self.rolled, to.rolled, x),
            unison: lerp(self.unison, to.unison, x),
            cross: lerp(self.cross, to.cross, x),
            meet: lerp(self.meet, to.meet, x),
            chase: lerp(self.chase, to.chase, x),
            chased: lerp(self.chased, to.chased, x),
            truss: lerp(self.truss, to.truss, x),
            converge: lerp(self.converge, to.converge, x),
            sheet: lerp(self.sheet, to.sheet, x),
        }
    }
}

/// The routines the show is made of, one to a phrase.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Routine {
    /// The fans opening slowly as mirror images, rising as they go.
    Open,
    /// Every beam tight and aimed the same way, sweeping left and right over two bars.
    Sweep,
    /// The two sides crossing in an X, its meeting point moving to a new place on each downbeat.
    Cross,
    /// Waves rolling across the fans.
    Waves,
    /// A pulse chasing from beam to beam across every projector.
    Chase,
    /// Every beam drawn into one point over a block's last bar, to burst open on the next downbeat.
    Converge,
    /// A breakdown: the centre fan turning slowly in the smoke.
    Rest,
    /// A breakdown's last two bars: every beam closing in on you, the chase speeding up, and a dark
    /// half beat before the drop.
    Build,
}

/// Whether the routine after `routine` bursts out of it on its downbeat, rather than the moves
/// easing into the next routine to arrive on it.
fn bursts(routine: Routine) -> bool {
    matches!(routine, Routine::Converge | Routine::Build)
}

/// How long a burst open takes, beats.
const BURST: f64 = 0.5;

/// Where the X's meeting point is on each downbeat in turn, metres across the room.
const MEETS: [f64; 6] = [0.0, -4.5, 3.5, -2.0, 5.0, -3.0];

/// The stretch of the show one routine plays over: which, and its first beat and length in beats.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Phrase {
    routine: Routine,
    start: f64,
    len: f64,
}

/// What a bar of the loop is part of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stretch {
    Run,
    Rest,
    Build,
}

fn stretch(bar: usize) -> Stretch {
    if !resting(bar) {
        Stretch::Run
    } else if !resting(bar + 1) || !resting(bar + 2) {
        Stretch::Build
    } else {
        Stretch::Rest
    }
}

/// The phrase `beat` is in. Each eight-bar block of a loud run has two routines, four bars and
/// then three, and draws every beam together over its last bar; the blocks cycle through three
/// pairs. A breakdown is one routine, and its build another.
fn phrase(beat: f64) -> Phrase {
    let b = beat.rem_euclid(LOOP_BEATS);
    let bar = (b / 4.0) as usize % BARS;
    let here = stretch(bar);
    let same = |n: usize| stretch(n % BARS) == here;
    let back = (1..BARS).take_while(|&n| same(bar + BARS - n)).count();
    let ahead = (1..BARS).take_while(|&n| same(bar + n)).count();
    let start = (bar as f64 - back as f64) * 4.0;
    let len = (back + ahead + 1) as f64 * 4.0;
    match here {
        Stretch::Rest => Phrase {
            routine: Routine::Rest,
            start,
            len,
        },
        Stretch::Build => Phrase {
            routine: Routine::Build,
            start,
            len,
        },
        Stretch::Run => {
            let block = ((b - start) / 32.0).floor();
            let from = start + block * 32.0;
            let left = (start + len - from).min(32.0);
            let (first, second) = match block as usize % 3 {
                0 => (Routine::Open, Routine::Waves),
                1 => (Routine::Sweep, Routine::Chase),
                _ => (Routine::Cross, Routine::Chase),
            };
            let at = b - from;
            let part = |routine, begins: f64, ends: f64| Phrase {
                routine,
                start: from + begins,
                len: ends - begins,
            };
            if at < 16.0 || left <= 16.0 {
                part(first, 0.0, left.min(16.0))
            } else if left < 32.0 {
                part(second, 16.0, left)
            } else if at < 28.0 {
                part(second, 16.0, 28.0)
            } else {
                part(Routine::Converge, 28.0, 32.0)
            }
        }
    }
}

/// Where `routine`'s beams are `local` beats into a phrase `len` long, and how brightly its fans
/// are lit before the hits add to them. Each moves smoothly through its phrase.
fn pose(routine: Routine, local: f64, len: f64) -> Moves {
    use std::f64::consts::TAU;
    let fan = |aim, spread, tilt, gate| Fan {
        aim,
        spread,
        tilt,
        gate,
    };
    let through = (local / len).clamp(0.0, 1.0);
    let rest = Moves::default();
    match routine {
        Routine::Open => {
            let x = eased(through);
            Moves {
                centre: fan(0.0, 0.02 + 0.07 * x, 0.04 + 0.26 * x, 0.5),
                inner: fan(0.18 + 0.1 * x, 0.02 + 0.07 * x, 0.03 + 0.2 * x, 0.45),
                outer: fan(0.4 + 0.15 * x, 0.02 + 0.06 * x, 0.02 + 0.15 * x, 0.4),
                truss: 0.35,
                sheet: 1.0,
                ..rest
            }
        }
        Routine::Sweep => {
            let aim = 0.42 * (TAU * local / 8.0).sin();
            Moves {
                centre: fan(aim, 0.008, 0.14, 0.5),
                inner: fan(aim, 0.008, 0.12, 0.5),
                outer: fan(aim, 0.008, 0.1, 0.5),
                unison: 1.0,
                truss: 0.4,
                ..rest
            }
        }
        Routine::Cross => {
            // Holding for half the bar, then moving on to arrive on the next downbeat.
            let bar = (local / 4.0).floor();
            let moving = smooth(2.0, 4.0, local - bar * 4.0);
            let at = |n: f64| MEETS[n as usize % MEETS.len()];
            Moves {
                centre: fan(0.0, 0.012, 0.1, 0.5),
                inner: fan(0.3, 0.012, 0.1, 0.5),
                outer: fan(0.5, 0.012, 0.1, 0.5),
                cross: 1.0,
                meet: lerp(at(bar), at(bar + 1.0), moving),
                truss: 0.5,
                sheet: 1.0,
                ..rest
            }
        }
        Routine::Waves => Moves {
            centre: fan(0.0, 0.05, 0.12, 0.5),
            inner: fan(0.3, 0.05, 0.12, 0.5),
            outer: fan(0.55, 0.05, 0.1, 0.45),
            wave: 0.08,
            truss: 0.45,
            sheet: 1.0,
            ..rest
        },
        Routine::Chase => Moves {
            centre: fan(0.0, 0.06, 0.2, 0.7),
            inner: fan(0.32, 0.06, 0.18, 0.7),
            outer: fan(0.6, 0.05, 0.15, 0.65),
            wave: 0.03,
            chase: 1.0,
            truss: 0.5,
            ..rest
        },
        Routine::Converge => {
            let x = eased(through);
            let gate = 0.45 + 0.35 * x;
            Moves {
                centre: fan(0.0, 0.03 * (1.0 - x), 0.1, gate),
                inner: fan(0.3, 0.03 * (1.0 - x), 0.1, gate),
                outer: fan(0.5, 0.025 * (1.0 - x), 0.1, gate),
                cross: 1.0,
                ..rest
            }
        }
        Routine::Rest => Moves {
            centre: fan(
                0.25 * (TAU * local / 16.0).sin(),
                0.05,
                0.15 + 0.05 * (TAU * local / 8.0).sin(),
                0.45,
            ),
            inner: fan(0.3, 0.04, 0.15, 0.03),
            outer: fan(0.6, 0.03, 0.1, 0.03),
            wave: 0.03,
            sheet: 1.0,
            ..rest
        },
        Routine::Build => {
            // Brighter as it builds, fading out over the beat before the last half, which is dark.
            let gate = (0.3 + 0.7 * through) * (1.0 - smooth(len - 1.0, len - 0.5, local));
            Moves {
                centre: fan(0.0, 0.05, 0.15, gate),
                inner: fan(0.35, 0.05, 0.15, gate),
                outer: fan(0.65, 0.04, 0.12, gate),
                chase: 1.0,
                converge: through,
                ..rest
            }
        }
    }
}

/// The moves at `beat`, as a show is programmed to a track: each phrase's routine, easing over the
/// beat before the next phrase so its moves arrive on the downbeat, or after every beam has drawn
/// together, bursting open on it. Waves roll across the fans a bar at a time, and the chase moves
/// on a beam each sixteenth, in a build each eighth and then each sixteenth. The hits only add
/// light: the kicks to the centre fan and the snares to the pair beside it, never dark between
/// them, so the whole picture brightens no more than once a beat. Quieter bars light it less, and
/// the drop lights everything at once.
fn moves(beat: f64) -> Moves {
    // Before the first drop the club's projectors are dark: the bar's own fan is all there is.
    if beat < 0.0 {
        return Moves::default();
    }
    let b = beat.rem_euclid(LOOP_BEATS);
    let here = phrase(b);
    let local = b - here.start;
    let mut moves = pose(here.routine, local, here.len);
    let before = phrase((here.start - 0.01).rem_euclid(LOOP_BEATS));
    if local < BURST && bursts(before.routine) && before.routine != here.routine {
        let from = pose(before.routine, before.len, before.len);
        moves = from.towards(moves, 1.0 - (1.0 - local / BURST).powi(3));
    } else if !bursts(here.routine) && local > here.len - 1.0 {
        let next = phrase((here.start + here.len).rem_euclid(LOOP_BEATS));
        if next.routine != here.routine {
            let to = pose(next.routine, 0.0, next.len);
            moves = moves.towards(to, eased(local - (here.len - 1.0)));
        }
    }
    moves.rolled = b * std::f64::consts::FRAC_PI_2;
    moves.chased = if here.routine == Routine::Build {
        let half = here.len / 2.0;
        2.0 * local.min(half) + 4.0 * (local - half).max(0.0)
    } else {
        4.0 * b
    };
    let [kick, snare, _] = hits(beat).map(f64::from);
    if beat >= 0.0
        && let Part::Run {
            into_run, level, ..
        } = part(b)
    {
        let lit = 0.6 + 0.4 * level;
        moves.centre.gate = (moves.centre.gate + 0.8 * kick) * lit;
        moves.inner.gate = (moves.inner.gate + 0.9 * snare) * lit;
        moves.outer.gate *= lit;
        moves.truss *= 1.0 + 3.0 * snare;
        // The drop lights everything at once, dying back over half a beat.
        let landing = 1.5 * (1.0 - into_run / 0.5).max(0.0);
        for fan in [&mut moves.centre, &mut moves.inner, &mut moves.outer] {
            fan.gate = fan.gate.max(landing);
        }
    }
    moves
}

/// The club's shader's numbers, all plain values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Uniforms {
    place: [f32; 4],
    screen: [f32; 4],
    beat: [f32; 4],
    bar: [f32; 4],
    hits: [f32; 4],
    cue: [f32; 4],
    mood: [f32; 4],
    crt: [f32; 4],
    centre: [f32; 4],
    inner: [f32; 4],
    outer: [f32; 4],
    motion: [f32; 4],
    shape: [f32; 4],
    scanned: [f32; 1],
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
            .with("bar", self.bar)
            .with("hits", self.hits)
            .with("cue", self.cue)
            .with("mood", self.mood)
            .with("crt", self.crt)
            .with("centre", self.centre)
            .with("inner", self.inner)
            .with("outer", self.outer)
            .with("motion", self.motion)
            .with("shape", self.shape)
            .with("scanned", self.scanned)
            .with("ring", self.ring)
            .with("secondary", self.secondary)
            .with("tertiary", self.tertiary)
    }
}

/// Marks the club's program in a renderer's keeping.
struct Program;

/// The club's shader, with the bar's lasers' numbers, which it carries on from.
fn club_source() -> String {
    use crate::shell::bar::glass;
    shader::with_constants(
        CLUB,
        &[
            ("SCISSOR", &glass::SCISSOR),
            ("SCISSOR_PERIOD", &glass::SCISSOR_PERIOD),
            ("VIEW", &VIEW),
            ("EYE_HEIGHT", &EYE_HEIGHT),
            ("LOOK_UP", &LOOK_UP),
            ("STAGE_TOP", &STAGE_TOP),
            ("STAGE_FRONT", &STAGE_FRONT),
        ],
    )
}

fn program(renderer: &mut GlesRenderer) -> Option<GlesPixelProgram> {
    shader::program::<Program, _>(
        renderer,
        |renderer| {
            renderer.compile_custom_pixel_shader(club_source(), &shader::names::<Uniforms>())
        },
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
    /// `elements` drawn off screen and shown as `shot` sees them, about the bar's middle in screen
    /// pixels, which shows at `held`.
    fn shot(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[OutputElement],
        sizes: tilt::Sizes,
        (centre, held): (
            Point<f64, smithay::utils::Physical>,
            Point<f64, smithay::utils::Physical>,
        ),
        shot: Shot,
    ) -> Option<TextureShaderElement> {
        if self.broken {
            return None;
        }
        if self.program.is_none() {
            let names = [
                UniformName::new("size", UniformType::_2f),
                UniformName::new("centre", UniformType::_2f),
                UniformName::new("held", UniformType::_2f),
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
                Uniform::new("held", (held.x as f32, held.y as f32)),
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
/// the dark, with the desktop's CRT look over the middle of the transition, hard at its peak.
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

// The screen in screen pixels, the bar's middle in the same and where it shows now, carried to the
// screen's middle as the camera dives in, and how far the camera magnifies, how much the world
// streaks and how far it has sunk into the dark.
uniform vec2 size;
uniform vec2 centre;
uniform vec2 held;
uniform vec3 view;
// The desktop's CRT look over the middle of the transition: how strong, the tube's brightness
// this frame, how far it is torn, and which frame it is.
uniform vec4 crt;

// Every other row as dark as the tube's scanlines; the colours split apart and rows torn sideways
// as far as these, in screen pixels. At the look's peak the rows go darker still, the colours
// split and the rows tear further, every frame, and the picture bulges as a tube's glass does.
const float SCANLINE = 0.62;
const float SCAN_PEAK = 0.12;
const float SPLIT = 2.5;
const float SPLIT_PEAK = 18.0;
const float TEAR = 14.0;
const float TEAR_PEAK = 60.0;
const float BULGE = 0.15;

float hash1(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}

// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash2(vec2 c) {
    vec3 p3 = fract(vec3(c.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 seen(vec2 at) {
    vec2 uv = at / size;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec3(0.0);
    }
    return texture2D(tex, uv).rgb;
}

void main() {
    // The look's strength, and its peak, which comes on only near its height.
    float k = crt.x;
    float peak = k * k * k;
    vec2 c = v_coords - 0.5;
    vec2 p = (0.5 + c * (1.0 + BULGE * peak * 4.0 * dot(c, c))) * size;
    // On a torn frame, bands of rows jerk sideways; at the peak, every frame tears.
    float band = floor(p.y / 18.0);
    p.x += crt.z * k * TEAR * step(0.7, hash1(band + crt.w * 7.0))
        + peak * TEAR_PEAK * (2.0 * hash1(band * 3.1 + crt.w * 13.0) - 1.0)
            * step(0.55, hash1(band + crt.w * 7.0 + 50.0));
    float split = SPLIT * k + SPLIT_PEAK * peak;
    vec2 rel = p - held;
    float fringe = 0.012 * view.y * length(rel) / size.y;
    vec3 sum = vec3(0.0);
    for (int i = 0; i < 12; i++) {
        // Further back along the dive, the way the world streaks past.
        float back = view.x * (1.0 + view.y * float(i) / 11.0);
        sum.r += seen(centre + (rel * (1.0 - fringe) + vec2(split, 0.0)) / back).r;
        sum.g += seen(centre + rel / back).g;
        sum.b += seen(centre + (rel * (1.0 + fringe) - vec2(split, 0.0)) / back).b;
    }
    vec3 colour = sum / 12.0;
    float edge = length(rel / size);
    colour *= 1.0 - view.z * (0.55 + 0.45 * smoothstep(0.0, 0.7, edge));
    colour *= 1.0 - 0.6 * view.y * smoothstep(0.15, 0.9, edge);
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : mix(SCANLINE, SCAN_PEAK, peak);
    colour *= mix(1.0, scan * crt.y, k);
    // At the peak, fine static, and a brighter band rolling slowly down the tube.
    colour += (hash2(gl_FragCoord.xy + crt.w * 37.0) - 0.5) * 0.25 * peak;
    float roll = (v_coords.y - fract(crt.w / 48.5)) * 14.0;
    colour += vec3(0.1) * peak * exp(-roll * roll);
    vec4 out_colour = vec4(max(colour, 0.0), 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        out_colour = vec4(0.0, 0.2, 0.0, 0.2) + out_colour * 0.8;
#endif
    gl_FragColor = out_colour;
}
"#;

/// The club, a room rendered in 3D, in metres unless it says otherwise: x across, y up, z away from
/// you. Every pixel's ray from the camera finds the surface it meets, the floor, a wall, the stage
/// or the truss; each beam adds the light the smoke scatters along the ray up to there, and lights
/// that surface where it lands and with the glow it spills, so the room is seen only by its lasers.
/// Over it lie the bar's smoke and, until the drop, the bar's own fan, drawn as the bar draws them.
const CLUB: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// Where the club is shown on the screen, in screen pixels: its corner and its size.
uniform vec4 place;
// The screen's size in screen pixels, and the bar's middle on it, which the camera dives into.
uniform vec4 screen;
// The beat in the music's loop (below zero before the drop), how far the club has faded in over
// the bar, how near it has come (it starts far off), and how much of the smoke is the room's own
// rather than the bar's.
uniform vec4 beat;
// The bar's air clock in seconds, screen pixels to one of the bar's units, how far the bar's lasers
// and smoke are magnified about its middle, and how much of the bar's own fan shows.
uniform vec4 bar;
// The kicks, snares and hi-hats ringing now, and how far every beam closes in on you.
uniform vec4 hits;
// How much of the tunnel, the fans and the liquid sky is on, and how thick the smoke is.
uniform vec4 cue;
// How far into a breakdown it is, the drop's impact, the flare passing through the glass, and the
// liquid sky dropping in.
uniform vec4 mood;
// The desktop's CRT look over the middle of the transition: how strong, the tube's brightness
// this frame, how far it is torn, and which frame it is.
uniform vec4 crt;
// The fans' looks: the centre fan, the pair beside it and the outer pair, each where it aims to
// the side, how far apart its beams are, how high it tilts and how brightly it's lit.
uniform vec4 centre;
uniform vec4 inner;
uniform vec4 outer;
// How far waves roll across the fans and where the rolling has got to, which beam the chase has
// got to, and how brightly the truss's projectors fire.
uniform vec4 motion;
// How far the fans aim the same way rather than as mirror images, how far every beam is drawn to
// one meeting point and where across the room it is, and how much the chase shows.
uniform vec4 shape;
// How far the projectors scan sheets of light between their beams rather than drawing beams.
uniform float scanned;
uniform vec3 ring;
uniform vec3 secondary;
uniform vec3 tertiary;

const float PI = 3.14159265;
// The room, in metres: its floor and ceiling, its side walls, the wall behind the stage and the one
// behind you; the stage's front, top and half-width; the truss over it, its height, depth and
// radius.
const float CEILING = 9.0;
const float SIDE = 15.0;
const float BACK = 32.0;
const float BEHIND = -8.0;
const float STAGE_HALF = 11.0;
const float TRUSS_Y = 7.2;
const float TRUSS_Z = 22.0;
const float TRUSS_R = 0.18;
// Where the beams meet when they are drawn together, how high and how far away.
const float MEET_Y = 4.5;
const float MEET_Z = 11.0;
// The smoke as the bar draws it: the colour it glows, the thin haze everywhere as a share of the
// thickest smoke, and how fast it comes down the tunnel towards you, in its depth a second.
const vec3 PALE = vec3(0.86, 0.97, 1.0);
const float HAZE = 0.12;
const float TOWARDS = 0.22;
// The CRT look, as the camera draws it.
const float SCANLINE = 0.62;
const float SCAN_PEAK = 0.12;
const float TEAR = 14.0;
const float TEAR_PEAK = 60.0;
const float BULGE = 0.15;

// Where the camera is this frame.
vec3 eye;

// A hash without sine, by Dave Hoskins. Distributed under the MIT License.
float hash(vec3 p) {
    p = fract(p * 0.1031);
    p += dot(p, p.zyx + 31.32);
    return fract((p.x + p.y) * p.z);
}

float hash1(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}

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

// The smoke's fine grain, as the bar's: small soft lumps that come down the tunnel with it.
float grain(vec3 at, float t) {
    vec3 q = vec3((at.x * 5.4 + TOWARDS * t) * 2.2, at.y * 6.0, at.z * 11.0 + t * 0.05);
    return 0.5 + 0.5 * (0.65 * snoise(q) + 0.35 * snoise(q * 2.3 + 4.1));
}

// The bar's smoke at `q` on the screen at its air clock `t`, as the bar draws it, magnified about
// the vanishing point as far as the bar's numbers say: soft billows streaming out of it towards
// you, in the tunnel the bar draws them in, far at the middle and near at the edges. Along the
// bar's glass the tunnel is the bar's own; round the rest of the vanishing point it carries on the
// same way, so the smoke streams out in every direction. While the club fades in over the bar,
// the smoke keeps to the bar's glass, `in_glass`; once in, it fills the room.
float smoke(vec2 q, float t, float in_glass) {
    vec2 d = (q - screen.zw) / (bar.y * bar.z);
    // The bar's tunnel is level across its middle, behind its clock; out here it eases in, so the
    // middle has no edge.
    float r = length(d);
    float out_from = mix(max(r, 40.0), sqrt(r * r + 1600.0), beat.y);
    float depth = 1000.0 / out_from;
    vec3 at = vec3(depth, 25.0 * d.y / out_from, d.x / out_from);
    float grained = grain(at, t);
    vec3 s = vec3((at.x * 5.4 + TOWARDS * t) * 0.5, at.y * 1.6, at.z * 5.0);
    vec2 w = vec2(snoise(s * 0.6 + vec3(0.0, 0.0, t * 0.007)),
                  snoise(s * 0.6 + vec3(5.2, 0.0, t * 0.007 + 3.0)));
    float body = 0.5 + 0.5 * fbm(s + vec3(1.2 * w, t * 0.01));
    // Where it gathers and clears changes round the vanishing point too, so it never lies in rings.
    float life = snoise(vec3((at.x * 5.4 + TOWARDS * t) * 0.12, at.z * 3.0 + at.y * 0.15, t * 0.012));
    float thick = smoothstep(0.25, 0.9, body + 0.55 * life + 0.25 * (grained - 0.5));
    thick *= 0.6 + 0.8 * grained;
    float near_n = (at.x * 2.7 + TOWARDS * t) * 1.2;
    float wisps = 0.5 + 0.5 * fbm(vec3(near_n, at.y * 0.4, at.z * 9.0 + t * 0.004));
    thick = max(thick, 0.6 * smoothstep(0.55, 0.9, wisps) * smoothstep(150.0, 600.0, out_from));
    thick = HAZE + (1.0 - HAZE) * thick;
    float far = mix(0.5, 1.0, smoothstep(1.5, 8.0, out_from / 40.0));
    float y = 20.0 + d.y;
    float edges = smoothstep(0.0, 7.0, y) * (0.75 + 0.25 * smoothstep(10.0, 39.0, y));
    return thick * far * mix(1.0, edges, in_glass);
}

// `c` with its colour pushed further from grey, as laser light is.
vec3 vivid(vec3 c) {
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    return clamp(mix(vec3(grey), c, 1.6), 0.0, 1.0);
}

// The room's smoke at `p`, 0 to 1, as real stage haze is: soft clouds a couple of metres across
// carried round in slow swirls, drawn into soft wisps where it folds, thinning into gaps but
// never quite clear, all drifting out from the stage over the crowd, rising and sliding sideways.
// It shows only where a laser lights it.
float haze(vec3 p) {
    float t = bar.x;
    vec3 s = (p - vec3(0.25 * t, 0.06 * t, -0.4 * t)) * 0.3;
    vec3 swirl = vec3(snoise(s * 0.5 + vec3(0.0, 0.0, 0.03 * t)),
                      snoise(s * 0.5 + vec3(5.2, 1.3, 0.03 * t)), 0.0);
    vec3 q = s + 0.8 * swirl;
    float clouds = snoise(q);
    float fold = snoise(q * 2.2 + vec3(7.1, 0.0, 0.05 * t));
    float wisps = exp(-fold * fold * 5.0);
    float fine = snoise(q * 6.5 + 3.3);
    return clamp(0.5 + 0.42 * clouds + 0.14 * (wisps - 0.45) + 0.05 * fine, 0.06, 1.0);
}

// The smoke at `p` as `haze` has it, its clouds without their swirls, wisps or grain: enough for
// a thin beam or a faint glow, at a third of the cost.
float thin_haze(vec3 p) {
    float t = bar.x;
    vec3 s = (p - vec3(0.25 * t, 0.06 * t, -0.4 * t)) * 0.3;
    float clouds = 0.65 * snoise(s) + 0.35 * snoise(s * 2.3 + vec3(7.1, 0.0, 0.05 * t));
    return clamp(0.5 + 0.4 * clouds, 0.06, 1.0);
}

// How much more light smoke scatters towards the eye than side on, where the light travelling
// `towards` a way makes that angle with the way back to the eye: smoke throws most of it on
// forwards, so a beam or sheet is brightest looking back along it to its projector.
float phase(float towards) {
    const float G = 0.45;
    float side_on = pow(1.0 + G * G, 1.5);
    return min(side_on / pow(1.0 + G * G - 2.0 * G * towards, 1.5), 4.0);
}

// The light the smoke scatters towards the eye where `ray` crosses the sheet a projector at `from`
// scans between the ways `a` and `b`, if that's nearer than the surface `wall` away. The beam's
// light is spread across the sheet, so it thins with distance and with how wide the sheet is;
// seen at a slant you look through more of it, so its edge-on parts and its edges are brightest.
vec3 sheet(vec3 ray, float wall, vec3 from, vec3 a, vec3 b, vec3 hue, float lit, float pix) {
    if (lit <= 0.002) return vec3(0.0);
    vec3 across = cross(a, b);
    float span = length(across);
    if (span < 1e-5) return vec3(0.0);
    vec3 normal = across / span;
    float facing = dot(ray, normal);
    if (abs(facing) < 1e-4) return vec3(0.0);
    float hit = dot(from - eye, normal) / facing;
    if (hit < 0.3 || hit > wall) return vec3(0.0);
    vec3 at = eye + ray * hit;
    vec3 v = at - from;
    if (dot(v, a + b) <= 0.0) return vec3(0.0);
    // How far inside each edge it is, in metres, softened over a pixel.
    float soft = max(pix * hit, 1e-3);
    float inside = smoothstep(-soft, soft, dot(cross(a, v), normal))
        * smoothstep(-soft, soft, dot(cross(v, b), normal));
    if (inside <= 0.0) return vec3(0.0);
    float r = length(v);
    vec3 u = v / max(r, 1e-4);
    float angle = span;
    float through = min(1.0 / abs(facing), 10.0);
    float spread = 1.0 / max(r * max(angle, 0.004), 0.3);
    float fade = smoothstep(0.5, 3.0, hit) * exp(-hit * 0.02);
    return vivid(hue) * (lit * inside * haze(at) * through * spread * phase(dot(u, -ray)) * fade * 0.05);
}

// Beam `i` of the bar's eight at the bar's air clock `t`, as the bar's projector throws it: its
// angle up from level, which side it goes, and how brightly it's lit.
float bar_beam(int i, float t, out float side, out float lit) {
    float fi = float(i);
    side = i < 4 ? -1.0 : 1.0;
    float up = mod(fi, 2.0) < 0.5 ? 1.0 : -1.0;
    bool pair = mod(floor(fi / 2.0), 2.0) < 0.5;
    float scissor = SCISSOR * sin(2.0 * PI * t / SCISSOR_PERIOD);
    float swell = sin(2.0 * PI * t / 23.0);
    lit = (0.85 + 0.15 * sin(2.0 * PI * t / 41.0))
        * (0.3 + 0.7 * smoothstep(-0.4, 0.7, pair ? swell : -swell));
    return up * (pair ? 0.022 + scissor : 0.05 - scissor);
}

// The bar's own fan at `q` on the screen, drawn as the bar draws it and magnified with the camera
// about the bar's middle. Lines through the middle stay put under any zoom about it, so these lie
// on the bar's own beams as the club fades in over them: sharp vivid lines with a white-hot
// middle and a breath of glow, catching the smoke `through` they cross.
vec3 bar_fan(vec2 q, float through) {
    vec3 acc = vec3(0.0);
    vec2 rel = q - screen.zw;
    float f = bar.y;
    float m = bar.z;
    vec3 colour = vivid(ring);
    float seen = 0.6 + 0.5 * through;
    for (int i = 0; i < 8; i++) {
        float side;
        float lit;
        float angle = bar_beam(i, bar.x, side, lit);
        vec2 dir = vec2(side * cos(angle), -sin(angle));
        float along = dot(rel, dir);
        float units = along / (f * m);
        float width = max(1.6 * f * (1.0 - units / 2600.0), 0.8) * m;
        float off = rel.x * dir.y - rel.y * dir.x;
        if (along < 0.0 || abs(off) > 8.0 * width) continue;
        float body = 1.0 - smoothstep(0.5 * width - 0.35, 0.5 * width + 0.35, abs(off));
        float hot = exp(-off * off / (0.06 * width * width + 0.04));
        float glow = exp(-off * off / (6.0 * width * width));
        float bloom = exp(-off * off / (40.0 * width * width)) * through;
        acc += (colour * body + vec3(0.55) * hot * body + 0.08 * colour * glow + 0.04 * colour * bloom)
            * exp(-units / 1800.0) * seen * lit;
    }
    return acc * 0.47;
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

// How much of a beam from `from` along `dir` is shown: as a show keeps its lasers over the
// audience's heads, one that would come out over the crowd lower than a little above your head, or
// come down on the floor among it, fades out, so nothing streaks down under the projectors. One
// that goes off to a side wall is left alone.
float over_crowd(vec3 from, vec3 dir) {
    float ends = landing(from, dir);
    if (dir.z < 0.0) ends = min(ends, (eye.z - from.z) / dir.z);
    vec3 end = from + dir * ends;
    if (end.z > STAGE_FRONT || abs(end.x) > SIDE - 1.0) return 1.0;
    return smoothstep(eye.y + 0.3, eye.y + 1.2, end.y);
}

// A beam from `from` along `dir`, `lit` bright, seen along `ray` from the eye up to the surface
// `wall` away, at `on`, through the smoke it lights. Adds to `lands` the spot it throws on that
// surface, if it lands here. Returns the light the smoke scatters towards the eye along the beam:
// a core a few millimetres across, widening as the beam spreads with distance and drawn no thinner
// than a pixel, dimmer in step, so it never steps; a shaft of lit smoke round it; hidden where the
// stage or a wall is in front of it. It shows only as bright as the smoke it passes through, and
// brightest looking back along it. A beam seen end on crowds into a short line, held to a little
// brighter than one seen side on, and one passing the lens fades out of focus, so a beam aimed near
// you never swells into a disc.
vec3 beam(vec3 ray, float wall, vec3 on, inout vec3 lands, vec3 from, vec3 dir,
          vec3 hue, float lit, float pix) {
    if (lit <= 0.002) return vec3(0.0);
    // Where the ray passes nearest the beam: most pixels are nowhere near it, so they stop here.
    vec3 w = eye - from;
    float b = dot(ray, dir);
    float d = dot(ray, w);
    float e = dot(dir, w);
    float den = max(1.0 - b * b, 1e-4);
    float along_ray = (b * e - d) / den;
    float along_beam = (e - b * d) / den;
    if (along_ray < 0.2 || along_beam < 0.0) return vec3(0.0);
    vec3 off = (eye + ray * along_ray) - (from + dir * along_beam);
    float gap = length(off);
    float radius = 0.006 + along_beam * 0.0012;
    float sigma = max(radius, pix * along_ray * 0.6);
    float shaft = 0.1 + along_beam * 0.006;
    if (gap > max(4.0 * sigma, 3.0 * shaft)) return vec3(0.0);

    vec3 colour = vivid(hue);
    float reach = landing(from, dir);
    vec3 off_end = on - (from + dir * reach);
    lands += colour * lit * exp(-dot(off_end, off_end) / 0.004) * 2.5;
    if (along_ray > wall || along_beam > reach) return vec3(0.0);
    float core = exp(-gap * gap / (sigma * sigma)) * radius / sigma;
    float glow = exp(-gap / shaft) * 0.012;
    float lens = 1.0 / max(sqrt(den), 0.7);
    float fade = exp(-along_ray * 0.018) / (1.0 + along_beam * along_beam / 900.0)
        * smoothstep(1.5, 6.0, along_ray);
    float seen = 0.15 + 1.3 * thin_haze(from + dir * along_beam);
    return (colour * (1.8 * core + glow) + vec3(0.9) * core * core) * seen * min(phase(-b), 2.5)
        * lens * fade * lit;
}

// The smoke lit all through the cone a projector at `from` throws along `axis` towards you, seen
// along `ray`: a round glow about the projector, brightest at its
// middle and looking back into it, fading out to the cone's edge. The smoke in it curls slowly
// round the cone's axis as it drifts, turning faster near the middle, so it lies in arcs round
// the light. Taken at three depths through the cone.
vec3 aura(vec3 ray, vec3 from, vec3 axis, vec3 hue, float lit) {
    float facing = dot(ray, axis);
    if (lit <= 0.002 || facing > -0.05) return vec3(0.0);
    vec3 side = normalize(cross(axis, vec3(0.0, 1.0, 0.0)));
    vec3 lift = cross(side, axis);
    float glowing = 0.0;
    for (int k = 0; k < 3; k++) {
        float depth = 5.0 + 6.0 * float(k);
        vec3 centre = from + axis * depth;
        float hit = dot(centre - eye, axis) / facing;
        if (hit < 0.5) continue;
        vec3 at = eye + ray * hit;
        // Thinning out towards the floor, the ceiling and the walls rather than stopping at them.
        // The smoke glows in front of the stage as much as over it, so the stage never cuts a dark
        // shape out of it.
        float inside = smoothstep(0.0, 1.5, at.y) * smoothstep(CEILING, CEILING - 2.0, at.y)
            * smoothstep(SIDE, SIDE - 2.0, abs(at.x));
        if (inside <= 0.0) continue;
        vec3 off = at - centre;
        vec2 round_axis = vec2(dot(off, side), dot(off, lift));
        float out_from = length(round_axis);
        float reach = depth * 0.28;
        float glow = exp(-out_from * out_from / (reach * reach));
        if (glow < 0.01) continue;
        float turn = atan(round_axis.y, round_axis.x) + 2.4 * exp(-out_from / reach) + 0.04 * bar.x;
        vec3 curled = centre + (side * cos(turn) + lift * sin(turn)) * out_from;
        glowing += glow * inside * haze(curled) / (1.0 + 0.012 * depth * depth);
    }
    return vivid(hue) * (glowing * lit * phase(-facing) * 0.35);
}

// The liquid sky: a sheet of light scanned out from the stage over the room at `height`, the
// smoke drifting through it, rolling slowly with the smoke's own currents. Its light spreads as it
// fans out, so it is brightest near the stage and fades over you, and brightest looking back
// towards its projector; far off, where a pixel covers metres of it, its smoke is seen as a whole.
vec3 sky(vec3 ray, float wall, float height, vec3 hue, float pix) {
    if (ray.y < 0.01) return vec3(0.0);
    float hit = (height - eye.y) / ray.y;
    vec3 at = eye + ray * hit;
    float roll = 0.4 * snoise(vec3(at.x * 0.07, at.z * 0.07, bar.x * 0.03));
    hit = (height + roll - eye.y) / ray.y;
    if (hit > wall) return vec3(0.0);
    at = eye + ray * hit;
    vec3 from = vec3(0.0, height, STAGE_FRONT + 1.5);
    vec3 v = at - from;
    float r = length(v);
    if (v.z > 0.0) return vec3(0.0);
    float across = abs(atan(v.x, -v.z));
    float fan = 1.0 - smoothstep(1.0, 1.25, across);
    float smoke = mix(haze(at), 0.5, smoothstep(0.05, 0.5, pix * hit));
    float graze = min(1.0 / ray.y, 6.0);
    float spread = 1.0 / max(r * 0.15, 0.6);
    return vivid(hue) * (smoke * graze * spread * fan * phase(dot(v / r, -ray)) * 0.06);
}

// Where `at` in the room shows, as the view's coordinates (height 1 to the top).
vec2 shown(vec3 at, vec3 forward, vec3 right, vec3 up) {
    vec3 v = at - eye;
    float z = max(dot(v, forward), 0.01);
    return vec2(dot(v, right), dot(v, up)) / (z * VIEW);
}

void main() {
    // The CRT look over the middle of the transition, as the camera draws it: this pixel moved as
    // the tube's glass bulges, and torn sideways on a torn frame, and on every one at its peak.
    float k = crt.x;
    float peak = k * k * k;
    vec2 q = place.xy + v_coords * place.zw;
    vec2 c = q / screen.xy - 0.5;
    q = (0.5 + c * (1.0 + BULGE * peak * 4.0 * dot(c, c))) * screen.xy;
    float band = floor(q.y / 18.0);
    q.x += crt.z * k * TEAR * step(0.7, hash1(band + crt.w * 7.0))
        + peak * TEAR_PEAK * (2.0 * hash1(band * 3.1 + crt.w * 13.0) - 1.0)
            * step(0.55, hash1(band + crt.w * 7.0 + 50.0));
    // Where it is in the club, which comes from far off about the bar's middle as the glass comes
    // in.
    vec2 focus = screen.zw;
    vec2 club = focus + (q - focus) / beat.z;
    vec2 middle = 0.5 * screen.xy;
    vec2 n = vec2(club.x - middle.x, middle.y - club.y) / (0.5 * screen.y);
    float b = beat.x;
    float drop = mood.y;
    float clock = b * 60.0 / 118.0;

    // The camera, in its place in the crowd, looks a little up, drifts and sways as a hand-held one
    // does, and kicks with the drop.
    eye = vec3(0.3 * sin(clock * 0.27), EYE_HEIGHT + 0.06 * sin(clock * 0.41), 0.25 * sin(clock * 0.19));
    float yaw = 0.025 * sin(clock * 0.21) + 0.004 * drop * sin(b * 97.0);
    float pitch = LOOK_UP + 0.015 * sin(clock * 0.33) + 0.004 * drop * cos(b * 83.0);
    vec3 forward = normalize(vec3(sin(yaw), pitch, cos(yaw)));
    vec3 right = normalize(cross(vec3(0.0, 1.0, 0.0), forward));
    vec3 up = cross(forward, right);
    vec3 ray = normalize(forward + n.x * VIEW * right + n.y * VIEW * up);
    float pix = 2.0 * VIEW / screen.y / beat.z;

    vec3 normal;
    float what;
    float wall = surface(ray, normal, what);

    // The smoke: the bar's, which its own fan goes on lighting, giving way to the room's own, which
    // the lasers light where they pass through it.
    float bars_own = beat.w < 1.0 || bar.w > 0.0 ? clamp(smoke(q, bar.x, 1.0 - beat.y), 0.0, 1.0) : 0.0;
    float here = beat.w > 0.0 ? mix(bars_own, thin_haze(eye + ray * min(wall, 7.0)), beat.w) : bars_own;
    here = clamp(here, 0.0, 1.0);

    float kick = hits.x;
    float calm = mood.x;
    float bars = floor(max(b, 0.0) / 4.0);
    // The colours take turns on the projectors, a section at a time.
    float turn = mod(floor(bars / 8.0), 3.0);
    vec3 c1 = turn < 0.5 ? ring : (turn < 1.5 ? secondary : tertiary);
    vec3 c2 = turn < 0.5 ? secondary : (turn < 1.5 ? tertiary : ring);
    vec3 c3 = turn < 0.5 ? tertiary : (turn < 1.5 ? ring : secondary);

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
        for (int i = 0; i < 12; i++) {
            float round_about = 2.0 * PI * float(i) / 12.0 + b * PI * 0.0625;
            vec3 dir = normalize(axis + open * (side * cos(round_about) + lift * sin(round_about)));
            vec3 hue = mod(float(i), 2.0) < 0.5 ? c1 : c2;
            air += beam(ray, wall, on_surface, lands, apex, dir, hue, cue.x * (0.35 + 0.4 * kick) * over_crowd(apex, dir), pix);
        }
        // Aimed at you, it lights the smoke all through its cone in a round glow.
        air += aura(ray, apex, axis, c1, cue.x);
    }

    // The fans: five projectors along the stage, eight beams each, mirror images either side of the
    // middle or all aimed the same way, or each drawn towards one meeting point, as the routine
    // says; waves roll across them, a pulse chases from beam to beam across every projector, and in
    // a build every beam closes in on you.
    if (cue.y > 0.0 || shape.w > 0.0) {
        for (int j = 0; j < 5; j++) {
            float fj = float(j) - 2.0;
            float mirror = sign(fj);
            vec4 look = abs(fj) < 0.5 ? centre : (abs(fj) < 1.5 ? inner : outer);
            vec3 from = vec3(fj * 4.5, STAGE_TOP + 0.5, STAGE_FRONT + 1.5);
            vec3 hue = abs(fj) < 0.5 ? c1 : (abs(fj) < 1.5 ? c2 : c3);
            float side = mix(mirror, 1.0, shape.x);
            vec3 to_meet = normalize(vec3(shape.z, MEET_Y, MEET_Z) - from);
            float meet_across = atan(to_meet.x, -to_meet.z);
            float meet_rise = asin(to_meet.y);
            vec3 last = vec3(0.0);
            float last_lit = 0.0;
            for (int i = 0; i < 8; i++) {
                float fi = float(i);
                float fanned = (fi - 3.5) * look.y * (mirror == 0.0 ? 1.0 : mirror);
                float across = mix(side * look.x, meet_across, shape.y) + fanned;
                float rise = mix(look.z + motion.x * sin(motion.y - fi * 0.6 - abs(fj) * 0.8), meet_rise, shape.y);
                vec3 dir = aimed(across, rise);
                dir = normalize(mix(dir, normalize(eye + vec3(fj * 0.3, 0.3, 0.0) - from), hits.w));
                // The chase: two soft pulses travelling beam by beam across every projector.
                float apart = mod(float(j) * 8.0 + fi - motion.z, 20.0);
                apart = min(apart, 20.0 - apart);
                float chased = 0.12 + 0.88 * exp(-apart * apart / 3.0);
                float lit = look.w * mix(1.0, chased, shape.w) * over_crowd(from, dir);
                // Scanning a sheet, the light is spread between the beams, and only the edges,
                // where the scan turns, stay as bright lines.
                float edge = i == 0 || i == 7 ? 1.0 : 1.0 - 0.85 * scanned;
                air += beam(ray, wall, on_surface, lands, from, dir, hue, lit * edge, pix);
                if (i > 0 && scanned > 0.0)
                    air += sheet(ray, wall, from, last, dir, hue, 0.5 * (lit + last_lit) * scanned, pix);
                last = dir;
                last_lit = lit;
            }
        }
        // Two projectors hung at the truss's ends, crossing beams down over the floor to the far
        // side, swinging from one side of the room to the other over two bars, firing with the
        // snares.
        if (motion.w > 0.0) {
            float reach_over = 0.5 + 0.125 * (1.0 - cos(PI * b / 4.0));
            for (int s = 0; s < 2; s++) {
                float side = s == 0 ? -1.0 : 1.0;
                vec3 from = vec3(side * 12.5, TRUSS_Y - 0.4, TRUSS_Z);
                for (int i = 0; i < 4; i++) {
                    float fi = float(i);
                    vec3 dir = aimed(-side * reach_over + (fi - 1.5) * 0.06, -0.14 - 0.03 * fi);
                    air += beam(ray, wall, on_surface, lands, from, dir, c3, motion.w, pix);
                }
            }
        }
    }

    // At the drop, the bar's fan carries on as the centre projector's beams, from just where the
    // bar drew them: aimed out to either side and up as far as shows on screen at the bar's own
    // slopes from where the camera arrives. They swing round towards you as every other projector
    // fires, then give way to the fans.
    float handed = (1.0 - bar.w) * (1.0 - smoothstep(1.5, 3.0, b)) * step(0.0, b);
    if (handed > 0.0) {
        float swing = smoothstep(0.0, 2.0, b);
        vec3 from = vec3(0.0, STAGE_TOP + 0.5, STAGE_FRONT + 1.5);
        for (int i = 0; i < 8; i++) {
            float fi = float(i);
            float side;
            float lit;
            float angle = bar_beam(i, bar.x, side, lit);
            float across = mix(1.2, 0.3 + 0.1 * fi, swing);
            float rise = atan(tan(angle) * sin(across)) * (1.0 + 2.0 * swing);
            vec3 dir = aimed(side * across, rise);
            air += beam(ray, wall, on_surface, lands, from, dir, ring, handed * lit * 1.4 * over_crowd(from, dir), pix);
        }
    }

    // The liquid sky, dropping in with the second crash and rising and falling with the bass.
    if (cue.z > 0.0) {
        float height = 5.2 + 2.0 * mood.w;
        air += sky(ray, wall, height, c2, pix) * cue.z;
    }

    // The room, lit only by the spots the lasers throw on its walls, floor and truss, the stage
    // left dark under its projectors; and the smoke between the lasers glowing faintly with the
    // light scattered through it, so its clouds show a little everywhere.
    vec3 albedo = what < 0.5 ? vec3(0.16, 0.17, 0.2) : vec3(0.4);
    vec3 room = albedo * (vec3(0.01) + (abs(what - 1.0) < 0.5 ? vec3(0.0) : lands));
    air += mix(c1, c2, 0.5 + 0.5 * n.x) * here * here * 0.04;

    // The projectors' lenses, bright points on the stage.
    vec3 colour = room + air;
    for (int j = 0; j < 5; j++) {
        vec2 d = n - shown(vec3((float(j) - 2.0) * 4.5, STAGE_TOP + 0.5, STAGE_FRONT + 1.5), forward, right, up);
        float flare = exp(-dot(d, d) * 20000.0);
        colour += mix(j == 2 ? c1 : c2, vec3(1.0), 0.6) * flare * (cue.y + shape.w + 0.4 * cue.x) * (1.0 - 0.8 * calm);
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

    // Over it, the bar's smoke lit as the bar lights it, as thick as the cue says, until the room's
    // own takes over, and the bar's own fan, so both carry on from the bar unchanged.
    vec3 fan = bar.w > 0.0 ? bar_fan(q, smoothstep(0.05, 0.9, bars_own)) * bar.w : vec3(0.0);
    float beside = smoothstep(0.0, 0.1, dot(fan, vec3(0.33)));
    float y = 20.0 + (q.y - screen.w) / (bar.y * bar.z);
    float lower = mix(smoothstep(14.0, 39.0, y), 0.5, beat.y);
    vec3 glowing = mix(PALE, ring, 0.25) * smoothstep(0.0, 1.0, here) * (0.095 + 0.07 * lower)
        * (1.0 - 0.5 * beside) * cue.w * (1.0 - beat.w);
    colour += min(0.8 * glowing, vec3(0.7, 0.9, 1.0)) + fan;

    // The CRT look over the middle of the transition, hard at its peak: darker rows, fine static
    // and a brighter band rolling down the tube.
    float scan = mod(floor(gl_FragCoord.y), 2.0) < 1.0 ? 1.0 : mix(SCANLINE, SCAN_PEAK, peak);
    colour *= mix(1.0, scan * crt.y, k);
    colour += (hash(vec3(gl_FragCoord.xy, crt.w + 7.0)) - 0.5) * 0.25 * peak;
    float roll = (q.y / screen.y - fract(crt.w / 48.5)) * 14.0;
    colour += vec3(0.1) * peak * exp(-roll * roll);
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
            air: 100.0,
            fill: fill((1536.0, 960.0).into(), bar),
            began: 10.0,
            last_knock: 10.0,
            ending: None,
            music: None,
            still,
            kept: shader::Kept::default(),
            camera: Camera::default(),
            canvas: None,
            canvas_broken: false,
        }
    }

    fn look() -> Look {
        Look {
            screen: (1536.0, 960.0).into(),
            scale: Scale::from(1.0),
            ring: [0.2, 0.6, 1.0],
            secondary: [1.0, 0.3, 0.6],
            tertiary: [0.5, 1.0, 0.4],
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
    fn the_camera_rockets_in_ever_faster_from_the_start_and_lands() {
        let rave = rave(false);
        let at = |share: f64| rave.shot(10.0 + DIVE * share).expect("diving");
        // It never goes back, not even at the start.
        let mut last = 1.0;
        for n in 1..100 {
            let level = at(n as f64 / 100.0).level;
            assert!(level >= last, "{n}");
            last = level;
        }
        let (early, mid, late) = (at(0.3), at(0.6), at(0.95));
        assert!(
            late.level / mid.level > mid.level / early.level,
            "ever faster"
        );
        assert!(late.blur > early.blur && late.dark > early.dark);
        assert!(rave.shot(10.0 + DIVE).is_none(), "landed");
    }

    #[test]
    fn the_crt_look_comes_and_goes_over_the_middle_half_of_the_transition_and_peaks_at_its_middle()
    {
        assert_eq!(tube_look(0.2), 0.0);
        assert_eq!(tube_look(0.5), 1.0);
        assert_eq!(tube_look(0.8), 0.0);
        // Its peak, the cube of its strength, is all but gone a little either side.
        assert!(tube_look(0.4).powi(3) < 0.5 && tube_look(0.6).powi(3) < 0.5);
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
    fn the_music_is_one_stream_whose_drop_lands_as_the_camera_does() {
        // Started with the dive, its build-up runs out as the camera lands.
        assert!((INTRO_SECS + LATENCY - DIVE).abs() < 1e-9);
        let rave = rave(false);
        assert!(rave.beat(10.0 + DIVE - 0.1) < 0.0, "the build while diving");
        // A frame early, so a move is on screen as its beat is heard.
        assert!(rave.beat(10.0 + DIVE) > 0.0);
        assert!(rave.beat(10.0 + DIVE - 2.0 * LEAD) < 0.0);
    }

    #[test]
    fn coming_round_again_the_music_plays_its_build_into_the_drop() {
        assert!(matches!(part(looped(-1.0, true)), Part::Build(_)));
        assert!(looped(-1.0, false) < 0.0, "the dive, the first time");
        assert_eq!(looped(LOOP_BEATS + 1.0, true), 1.0);
        // The stream's build-up is the end of the loop's own, its last two bars.
        const { assert!(INTRO_SECS * BPM / 60.0 < 8.0) };
    }

    #[test]
    fn the_bar_hands_its_lasers_and_smoke_over_to_the_club_and_takes_them_back() {
        let mut rave = rave(false);
        let at = |rave: &Rave, t: f64| rave.uniforms(&look(), t).1;
        let diving = at(&rave, 10.0 + DIVE * 0.4);
        assert_eq!(diving.bar[3], 1.0, "the bar's own fan");
        assert_eq!(diving.beat[3], 0.0, "and its smoke");
        // About the bar's middle, carried down to the centre projector as the camera lands.
        assert_eq!(at(&rave, 10.0).screen[2..], [768.0, 16.0]);
        assert_eq!(diving.screen[2], 768.0);
        assert!(diving.screen[3] > 16.0 && diving.screen[3] < 480.0);
        let landed_at = at(&rave, 10.0 + DIVE).screen;
        assert_eq!(landed_at[2], 768.0);
        assert!(
            (landed_at[3] - 0.622 * 960.0).abs() < 1.0,
            "at the centre projector"
        );
        assert_eq!(
            diving.bar[0],
            100.0 + (DIVE * 0.4) as f32,
            "on the bar's clock"
        );
        // Until the club has faded in, the lasers and smoke are magnified with the camera.
        let shot = rave.shot(10.0 + DIVE * 0.4).expect("diving");
        assert!((f64::from(diving.bar[2]) / shot.level - 1.0).abs() < 1e-4);
        let beat = 60.0 / BPM;
        let landed = at(&rave, 10.0 + DIVE + beat);
        assert_eq!(landed.bar[3], 0.0, "the club's own a beat after the drop");
        assert_eq!(landed.beat[3], 1.0, "and the room's own smoke");
        rave.end(30.0);
        let leaving = at(&rave, 30.0 + PULL_OUT * 0.8);
        assert_eq!(leaving.bar[3], 1.0, "and back as it pulls out");
        assert_eq!(leaving.beat[3], 0.0);
    }

    #[test]
    fn the_cues_follow_the_music() {
        // The kick on the first downbeat, the snare on beat two, the off-beat stab in bar two.
        assert!(hits(0.02)[0] > 0.3);
        assert!(hits(1.02)[1] > 0.5);
        assert!(onset(KICKS, 16 + 10) > 0.3, "bar two's stab");
        // The tunnel opens the loop, the sky drops with the second crash, the breakdown calms it.
        assert_eq!(cue(0.5)[0], 1.0);
        assert_eq!(cue(32.5)[2], 1.0);
        assert_eq!(part(26.0 * 4.0 + 1.0), Part::Rest);
        // The smoke thins a little in the loud runs.
        assert!(cue(0.5)[3] < cue(26.0 * 4.0 + 1.0)[3]);
    }

    #[test]
    fn the_show_changes_routine_every_few_bars_and_draws_together_over_each_blocks_last() {
        let phrase_at = |beat: f64| phrase(beat);
        assert_eq!(
            phrase_at(0.5),
            Phrase {
                routine: Routine::Open,
                start: 0.0,
                len: 16.0
            }
        );
        assert_eq!(
            phrase_at(17.0),
            Phrase {
                routine: Routine::Waves,
                start: 16.0,
                len: 12.0
            }
        );
        assert_eq!(
            phrase_at(29.0),
            Phrase {
                routine: Routine::Converge,
                start: 28.0,
                len: 4.0
            }
        );
        assert_eq!(phrase_at(33.0).routine, Routine::Sweep);
        assert_eq!(phrase_at(49.0).routine, Routine::Chase);
        assert_eq!(phrase_at(65.0).routine, Routine::Cross);
        assert_eq!(phrase_at(26.0 * 4.0).routine, Routine::Rest);
        assert_eq!(phrase_at(LOOP_BEATS - 1.0).routine, Routine::Build);
    }

    /// Where every beam points, leaving out how brightly it's lit.
    fn aims(moves: Moves) -> Vec<f64> {
        let mut all = vec![];
        for fan in [moves.centre, moves.inner, moves.outer] {
            all.extend([fan.aim, fan.spread, fan.tilt]);
        }
        all.extend([
            moves.wave,
            moves.unison,
            moves.cross,
            moves.meet / 10.0,
            moves.converge,
        ]);
        all
    }

    #[test]
    fn moves_arrive_on_the_downbeat() {
        // Into the waves at beat 16: still opening a beat before, then easing over to be in place
        // on the downbeat itself.
        assert_eq!(aims(moves(15.0)), aims(pose(Routine::Open, 15.0, 16.0)));
        assert_eq!(aims(moves(16.0)), aims(pose(Routine::Waves, 0.0, 12.0)));
        // The X's meeting point holds, then moves on to arrive on the next downbeat.
        assert_eq!(moves(65.0).meet, MEETS[0]);
        assert_eq!(moves(68.0).meet, MEETS[1]);
        assert_eq!(moves(69.0).meet, MEETS[1]);
    }

    #[test]
    fn beams_move_smoothly_and_only_burst_open_on_a_downbeat() {
        let step = 1.0 / 64.0;
        for n in 0..(LOOP_BEATS / step) as usize {
            let b = n as f64 * step;
            let (now, next) = (aims(moves(b)), aims(moves(b + step)));
            let jump = now
                .iter()
                .zip(&next)
                .map(|(a, z)| (a - z).abs())
                .fold(0.0, f64::max);
            let after = phrase(b + step);
            let bursting = b + step - after.start <= BURST + 1e-9
                && bursts(phrase((after.start - 0.01).rem_euclid(LOOP_BEATS)).routine);
            let most = if bursting { 0.15 } else { 0.03 };
            assert!(jump < most, "beat {b}: {jump}");
        }
    }

    #[test]
    fn the_hits_light_the_fans_but_never_move_them() {
        let (kick, between, snare) = (moves(4.02), moves(4.45), moves(5.02));
        assert!(kick.centre.gate > between.centre.gate + 0.3);
        assert!(snare.inner.gate > between.inner.gate + 0.3);
        assert!(
            between.centre.gate >= 0.3 && between.inner.gate >= 0.25,
            "never dark between them"
        );
        assert_eq!(aims(kick), aims(pose(Routine::Open, 4.02, 16.0)));
        assert_eq!(aims(snare), aims(pose(Routine::Open, 5.02, 16.0)));
    }

    #[test]
    fn the_breakdown_rests_and_its_build_closes_in_then_goes_dark_before_the_drop() {
        let breakdown = moves(26.0 * 4.0 + 1.0);
        assert!(breakdown.outer.gate < 0.1 && breakdown.centre.gate > 0.3);
        let building = moves(LOOP_BEATS - 4.0);
        assert!(building.converge > 0.4 && building.chase == 1.0);
        // The chase speeds from a beam each eighth to one each sixteenth.
        let pace = |beat: f64| moves(beat + 0.25).chased - moves(beat).chased;
        assert!(pace(LOOP_BEATS - 2.0) > 1.5 * pace(LOOP_BEATS - 7.0));
        let dark = moves(LOOP_BEATS - 0.2);
        assert!(dark.centre.gate < 0.1 && dark.inner.gate < 0.1);
        assert!(moves(0.1).outer.gate > 1.0, "the drop lights everything");
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
        let (_, numbers) = rave.uniforms(&look(), 50.0);
        assert_eq!(numbers.beat[3], 1.0, "the room's own smoke");
        assert_eq!(numbers.bar[3], 0.0, "no fan carried over from the bar");
        assert_eq!(numbers, rave.uniforms(&look(), 80.0).1, "held still");
        rave.end(20.0);
        assert!(!rave.tick(20.0), "gone at once");
    }

    #[test]
    fn the_projectors_scan_sheets_in_the_slow_routines_and_draw_beams_in_the_quick_ones() {
        for routine in [Routine::Open, Routine::Waves, Routine::Cross, Routine::Rest] {
            assert_eq!(pose(routine, 2.0, 16.0).sheet, 1.0, "{routine:?}");
        }
        for routine in [
            Routine::Sweep,
            Routine::Chase,
            Routine::Converge,
            Routine::Build,
        ] {
            assert_eq!(pose(routine, 2.0, 16.0).sheet, 0.0, "{routine:?}");
        }
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
