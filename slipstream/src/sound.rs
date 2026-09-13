//! Bullet time muffles the sound.
//!
//! Sound isn't slowed with the clock: a stream played back slowly drifts away from the app's own
//! timing and runs dry. Muffling it instead — quieter, with the highs rolled off — gives the sense
//! of everything slowing around you without touching what the app is doing, which is also why app
//! content itself keeps playing at full speed.
//!
//! PipeWire does the filtering. A helper process hosts a filter-chain sink (two low-pass biquads,
//! then a gain), and every stream playing at the time is moved onto it. Leaving bullet time moves
//! each stream back to the sink it came from and stops the helper, which takes the sink with it.
//!
//! The helper stops whenever Slipstream does, however it goes. It waits on a pipe from Slipstream
//! rather than on a signal, so closing that pipe — on quitting, on a crash, or when the watchdog
//! gives up on a stuck session — is what ends it. A muffled desktop left behind would be far worse
//! than no muffling at all.

use std::{
    io::Write,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

/// The filter sink's node name.
const SINK: &str = "slipstream_bullet_time";
/// Where the low-pass turns over, in hertz. Two biquads at this corner leave speech and music
/// recognisable while taking all the edge off them.
const CUTOFF: f64 = 700.0;
/// How much quieter it goes, on top of the filtering.
const GAIN: f64 = 0.55;
/// How long to wait for the helper's sink to appear before leaving the sound alone. It shows up in
/// a few tens of milliseconds.
const READY: Duration = Duration::from_millis(600);

/// Everything muffling needs, shared so that quitting can put the sound back on the spot rather
/// than leaving it to the thread below.
static MUFFLER: Mutex<Muffler> = Mutex::new(Muffler::new());

/// Puts the sound back now, on this thread. For quitting, where there's no time to wait for
/// another thread to get to it.
pub fn restore() {
    MUFFLER.lock().unwrap().stop();
}

/// Muffles everything playing, or puts it back as it was. Returns at once: starting the helper and
/// moving streams mean running programs, so they happen in order on a thread of their own.
pub fn muffle(on: bool) {
    let asks = ASKS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<bool>();
        thread::spawn(move || {
            for on in receiver {
                let mut muffler = MUFFLER.lock().unwrap();
                if on {
                    muffler.start();
                } else {
                    muffler.stop();
                }
            }
        });
        Mutex::new(sender)
    });
    let _ = asks.lock().unwrap().send(on);
}

static ASKS: OnceLock<Mutex<mpsc::Sender<bool>>> = OnceLock::new();

struct Muffler {
    /// The shell holding the filter sink open, while the sound is muffled. Slipstream keeps the
    /// write end of its standard input, so the filter goes when Slipstream does.
    helper: Option<Child>,
    /// Each stream moved onto the filter, and the sink it was playing on before.
    moved: Vec<(String, String)>,
}

impl Muffler {
    const fn new() -> Self {
        Self {
            helper: None,
            moved: Vec::new(),
        }
    }

    fn start(&mut self) {
        if self.helper.is_some() {
            return;
        }
        let Some(conf) = config() else {
            return;
        };
        // The filter runs under a shell that waits on its standard input. Slipstream holds the
        // other end of that pipe, so the read ends the moment Slipstream does, however it goes,
        // and the filter is stopped rather than left running over the whole desktop's sound.
        let helper = crate::launch::command("sh")
            .arg("-c")
            .arg(r#"pipewire -c "$1" & filter=$!; read -r _ <&0; kill "$filter" 2>/dev/null"#)
            .arg("sh")
            .arg(&conf)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match helper {
            Ok(helper) => self.helper = Some(helper),
            Err(err) => {
                tracing::warn!("couldn't start bullet time's sound filter: {err}");
                return;
            }
        }
        if !wait_for_sink() {
            tracing::warn!("bullet time's sound filter didn't appear; the sound is left alone");
            self.stop();
            return;
        }
        // The sink is new, so nothing is on it yet: everything playing is somewhere else.
        self.moved = streams();
        for (stream, _) in &self.moved {
            move_stream(stream, SINK);
        }
        tracing::debug!(streams = self.moved.len(), "the sound is muffled");
    }

    fn stop(&mut self) {
        // Back to their own sinks first: moving them after the filter sink goes would leave
        // PipeWire to choose for them.
        for (stream, sink) in self.moved.drain(..) {
            move_stream(&stream, &sink);
        }
        if let Some(mut helper) = self.helper.take() {
            // Closing the pipe is what tells the shell to stop the filter; killing the shell
            // would leave the filter behind.
            drop(helper.stdin.take());
            let _ = helper.wait();
        }
    }
}

/// Writes the helper's configuration under the runtime directory and returns the path. It carries
/// the core modules itself, so nothing has to be left in the user's PipeWire configuration.
fn config() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("slipstream");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("bullet-time.conf");
    let conf = format!(
        r#"context.properties = {{ log.level = 0 }}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
    {{ name = libpipewire-module-rt flags = [ ifexists nofail ] }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = "Slipstream bullet time"
            media.name       = "Slipstream bullet time"
            filter.graph = {{
                nodes = [
                    {{ type = builtin name = lp1  label = bq_lowpass control = {{ "Freq" = {CUTOFF} "Q" = 0.7 }} }}
                    {{ type = builtin name = lp2  label = bq_lowpass control = {{ "Freq" = {CUTOFF} "Q" = 0.7 }} }}
                    {{ type = builtin name = gain label = linear     control = {{ "Mult" = {GAIN} "Add" = 0.0 }} }}
                ]
                links = [
                    {{ output = "lp1:Out" input = "lp2:In" }}
                    {{ output = "lp2:Out" input = "gain:In" }}
                ]
            }}
            audio.channels = 2
            audio.position = [ FL FR ]
            capture.props = {{
                node.name   = "{SINK}"
                media.class = Audio/Sink
            }}
            playback.props = {{
                node.name   = "{SINK}_out"
                node.passive = true
            }}
        }}
    }}
]
"#
    );
    let mut file = std::fs::File::create(&path).ok()?;
    file.write_all(conf.as_bytes()).ok()?;
    Some(path)
}

/// Waits for the filter sink to be there to move streams onto.
fn wait_for_sink() -> bool {
    let until = Instant::now() + READY;
    while Instant::now() < until {
        if output("pactl", &["list", "short", "sinks"]).is_some_and(|sinks| {
            sinks
                .lines()
                .any(|line| line.split('\t').nth(1) == Some(SINK))
        }) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Everything playing now: each stream's id, and the id of the sink it's playing on.
fn streams() -> Vec<(String, String)> {
    let Some(listing) = output("pactl", &["list", "short", "sink-inputs"]) else {
        return Vec::new();
    };
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some((fields.next()?.to_string(), fields.next()?.to_string()))
        })
        .collect()
}

/// Moves one stream to a sink, by name or by id. A stream that has ended in the meantime is no
/// longer there to move, which is not worth a warning.
fn move_stream(stream: &str, sink: &str) {
    let moved = crate::launch::command("pactl")
        .args(["move-sink-input", stream, sink])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Ok(status) = moved
        && !status.success()
    {
        tracing::debug!(stream, sink, "the stream was gone before it could be moved");
    }
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = crate::launch::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------------------------
// The volume keys' blip.

/// The click's pitch, low enough not to be sharp and high enough to carry over what's playing.
const BLIP_HZ: f64 = 1000.0;
/// How long it lasts. Long enough to hear, short enough not to sit over the next keypress.
const BLIP_SECS: f64 = 0.040;
const BLIP_RATE: u32 = 48_000;
/// Loud enough to hear against the desktop, quiet enough not to be a shock at full volume. It is
/// played through the speakers at the level the keys have just set, which is the point of it.
const BLIP_PEAK: f64 = 0.35;
/// No more than one click in this long, so a key held down doesn't queue up dozens of them.
const BLIP_EVERY: Duration = Duration::from_millis(120);

static LAST_BLIP: Mutex<Option<Instant>> = Mutex::new(None);

/// A short click through the speakers, at whatever level the volume keys have just set — the
/// level is heard as well as seen, as it is on macOS and Windows. Volume only: never on mute,
/// which is the absence of sound, and never on brightness, which makes none.
pub fn blip() {
    // A nested run's click would play on the speakers of the desktop around it.
    if crate::launch::machine_commands_held() {
        return;
    }
    {
        let mut last = LAST_BLIP.lock().unwrap();
        let now = Instant::now();
        if last.is_some_and(|then| now.duration_since(then) < BLIP_EVERY) {
            return;
        }
        *last = Some(now);
    }
    let Some(path) = click_file() else {
        return;
    };
    // Without waiting, since the keypress has already done its work, and reaped when it ends.
    let _ = crate::launch::spawn(&["pw-play".to_string(), path.to_string_lossy().into_owned()]);
}

/// The click as a file, written under the runtime directory the first time it's wanted. Generated
/// rather than shipped, so there's no sample to licence and nothing to find at runtime.
fn click_file() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("slipstream");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("blip.wav");
        std::fs::write(&path, click_wav()).ok()?;
        Some(path)
    })
    .clone()
}

/// A 16-bit mono WAV of one click: a sine that rises in a couple of milliseconds and decays away,
/// which is a tap rather than a beep.
fn click_wav() -> Vec<u8> {
    let frames = (BLIP_RATE as f64 * BLIP_SECS) as u32;
    let mut samples = Vec::with_capacity(frames as usize * 2);
    for frame in 0..frames {
        let t = frame as f64 / BLIP_RATE as f64;
        // A 2 ms attack keeps the start from clicking in its own right; the decay is what makes
        // it a tap. The envelope reaches zero at the end, so the file can't end on a step.
        let attack = (t / 0.002).min(1.0);
        let decay = (-t / (BLIP_SECS / 3.0)).exp() - (-3.0f64).exp();
        let level = BLIP_PEAK * attack * decay.max(0.0) / (1.0 - (-3.0f64).exp());
        let value = (level * (std::f64::consts::TAU * BLIP_HZ * t).sin() * i16::MAX as f64) as i16;
        samples.extend_from_slice(&value.to_le_bytes());
    }
    wav(&samples, BLIP_RATE, 1)
}

/// `samples` wrapped in a canonical 44-byte WAV header: 16-bit PCM, `channels` at `rate`.
fn wav(samples: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let bytes_per_frame = u32::from(channels) * 2;
    let mut file = Vec::with_capacity(44 + samples.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes()); // PCM
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * bytes_per_frame).to_le_bytes());
    file.extend_from_slice(&(bytes_per_frame as u16).to_le_bytes());
    file.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    file.extend_from_slice(samples);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_click_is_a_playable_wav_that_starts_and_ends_at_silence() {
        let file = click_wav();
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(&file[36..40], b"data");
        let data = u32::from_le_bytes(file[40..44].try_into().unwrap()) as usize;
        assert_eq!(data, file.len() - 44);
        assert_eq!(
            u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize,
            file.len() - 8
        );
        let samples: Vec<i16> = file[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()))
            .collect();
        assert_eq!(samples.len(), (BLIP_RATE as f64 * BLIP_SECS) as usize);
        // Starting or ending part way up a wave is a click of its own, on top of this one.
        assert_eq!(samples[0], 0);
        assert!(samples.last().unwrap().abs() < 16, "{:?}", samples.last());
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        let wanted = (BLIP_PEAK * i16::MAX as f64) as u16;
        assert!(peak <= wanted, "{peak} is louder than it asked for");
        assert!(peak > wanted / 2, "{peak} is far quieter than it asked for");
    }
}
