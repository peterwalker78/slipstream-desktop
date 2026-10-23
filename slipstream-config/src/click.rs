//! The click the volume makes when it changes, from the keys, quick settings or Settings, so the
//! level is heard as well as seen. Played with PipeWire's `pw-play`.

use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

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

/// Whether a click may play now: true at most once in `BLIP_EVERY`.
pub fn due() -> bool {
    let mut last = LAST_BLIP.lock().unwrap();
    let now = Instant::now();
    if last.is_some_and(|then| now.duration_since(then) < BLIP_EVERY) {
        return false;
    }
    *last = Some(now);
    true
}

/// The click as a file, written under the runtime directory the first time it's wanted. Generated
/// rather than shipped, so there's no sample to licence and nothing to find at runtime.
pub fn file() -> Option<PathBuf> {
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
