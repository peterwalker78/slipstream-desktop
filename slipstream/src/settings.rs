//! The settings file (`slipstream-config`): read at startup, then applied again whenever the
//! Settings app or a hand edit saves it (`watch.rs`). The environment variables used for testing,
//! `SLIPSTREAM_IDLE_SECS`, `SLIPSTREAM_REDUCED_MOTION` and `SLIPSTREAM_WALLPAPER`, still win over
//! the file.

use slipstream_config::Settings;

/// The settings to start with: the file's, or every default if it can't be read.
pub fn startup() -> Settings {
    with_env(read().unwrap_or_default())
}

/// The settings after the file changed, or `None` to keep the current ones when it can't be read
/// (a hand edit saved half-way through, say).
pub fn reload() -> Option<Settings> {
    read().map(with_env)
}

fn read() -> Option<Settings> {
    let path = slipstream_config::path();
    slipstream_config::read(&path)
        .inspect_err(|err| {
            tracing::warn!(path = %path.display(), "can't read the settings file: {err}");
        })
        .ok()
}

fn with_env(mut settings: Settings) -> Settings {
    if let Some(secs) = std::env::var("SLIPSTREAM_IDLE_SECS")
        .ok()
        .and_then(|secs| secs.trim().parse().ok())
    {
        settings.wallpaper.fade_after_secs = secs;
    }
    if let Ok(reduced) = std::env::var("SLIPSTREAM_REDUCED_MOTION") {
        settings.motion.reduced = reduced == "1";
    }
    // SLIPSTREAM_WALLPAPER=decode, or a comma-separated list to cycle through.
    if let Ok(variations) = std::env::var("SLIPSTREAM_WALLPAPER") {
        settings.wallpaper.variations = variations
            .split(',')
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
    }
    settings
}
