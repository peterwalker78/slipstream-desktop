//! The media keys: play/pause, next, previous and stop, sent over MPRIS to whichever player is
//! playing, or the one last paused. Every call goes on a thread of its own, since a player that
//! doesn't answer would otherwise hold up the event loop, and what's playing afterwards comes back
//! through a channel for the on-screen display.
//!
//! What's playing is also watched for plugins that need `media` (`Watch`): looked at every couple
//! of seconds, on a thread of its own, for only as long as one of them runs.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::Duration,
};

use smithay::reexports::calloop::channel::Sender;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// A player answers locally, at once; one that doesn't isn't waited for.
const TIMEOUT: Duration = Duration::from_millis(800);
/// Long enough for a player to have moved on to the next track before it's asked what's playing.
const SETTLE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    PlayPause,
    Next,
    Previous,
    Stop,
}

impl Transport {
    fn method(self) -> &'static str {
        match self {
            Transport::PlayPause => "PlayPause",
            Transport::Next => "Next",
            Transport::Previous => "Previous",
            Transport::Stop => "Stop",
        }
    }
}

/// What a player was doing after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playing {
    /// No player is running.
    Nobody,
    Track {
        playing: bool,
        /// The title, and the artist after it when the player gives one.
        title: String,
    },
}

/// A player's playback status, as MPRIS spells it, ranked: the one playing is asked first.
fn rank(status: &str) -> u8 {
    match status {
        "Playing" => 0,
        "Paused" => 1,
        _ => 2,
    }
}

/// The player a key goes to, from the bus names running and each one's status: the first one
/// playing, else the first paused, else the first there is.
pub fn choose(players: &[(String, String)]) -> Option<&str> {
    players
        .iter()
        .enumerate()
        .min_by_key(|(index, (_, status))| (rank(status), *index))
        .map(|(_, (name, _))| name.as_str())
}

/// A track's line for the display: "Title — Artist", or whichever of them there is.
pub fn track_line(title: Option<&str>, artist: Option<&str>) -> String {
    let title = title.map(str::trim).filter(|title| !title.is_empty());
    let artist = artist.map(str::trim).filter(|artist| !artist.is_empty());
    match (title, artist) {
        (Some(title), Some(artist)) => format!("{title} — {artist}"),
        (Some(one), None) | (None, Some(one)) => one.to_string(),
        (None, None) => String::new(),
    }
}

/// Sends `transport` to the player, then says what's playing through `reply`.
pub fn send(transport: Transport, reply: Sender<Playing>) {
    let spawned = std::thread::Builder::new()
        .name("media-key".into())
        .spawn(move || match run(transport) {
            Ok(playing) => {
                let _ = reply.send(playing);
            }
            Err(err) => tracing::warn!(?transport, "the media key reached no player: {err}"),
        });
    if let Err(err) = spawned {
        tracing::warn!("couldn't send the media key: {err}");
    }
}

fn connect() -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .build()
}

/// The players running, by bus name, each with its playback status.
fn players(connection: &zbus::blocking::Connection) -> zbus::Result<Vec<(String, String)>> {
    let names: Vec<String> = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )?
        .body()
        .deserialize()?;
    Ok(names
        .into_iter()
        .filter(|name| name.starts_with(PREFIX))
        .map(|name| {
            let status = property(connection, &name, "PlaybackStatus")
                .and_then(|value| String::try_from(value).ok())
                .unwrap_or_default();
            (name, status)
        })
        .collect())
}

fn run(transport: Transport) -> zbus::Result<Playing> {
    let connection = connect()?;
    let players = players(&connection)?;
    let Some(player) = choose(&players).map(String::from) else {
        return Ok(Playing::Nobody);
    };
    connection.call_method(
        Some(player.as_str()),
        PATH,
        Some(PLAYER),
        transport.method(),
        &(),
    )?;
    tracing::info!(?transport, "media key sent to a player");
    std::thread::sleep(SETTLE);
    let playing = property(&connection, &player, "PlaybackStatus")
        .and_then(|value| String::try_from(value).ok())
        .is_some_and(|status| status == "Playing");
    let (title, artist) = track(&connection, &player);
    Ok(Playing::Track {
        playing,
        title: track_line(title.as_deref(), artist.as_deref()),
    })
}

/// The title and the first artist of what `player` is playing, as far as it says.
fn track(
    connection: &zbus::blocking::Connection,
    player: &str,
) -> (Option<String>, Option<String>) {
    let metadata = property(connection, player, "Metadata");
    let (title, artist) = metadata
        .map(|value| {
            let map: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
                value.try_into().unwrap_or_default();
            let title = map
                .get("xesam:title")
                .and_then(|value| String::try_from(value.clone()).ok());
            let artist = map
                .get("xesam:artist")
                .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
                .and_then(|artists| artists.into_iter().next());
            (title, artist)
        })
        .unwrap_or_default();
    (title, artist)
}

/// What's playing, as a plugin that needs `media` is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    /// The player's own name on the bus, as `spotify`.
    pub player: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

/// What the player the media keys would go to is playing, or none when no player runs.
fn now_playing(connection: &zbus::blocking::Connection) -> zbus::Result<Option<NowPlaying>> {
    let players = players(connection)?;
    let Some((name, status)) =
        choose(&players).and_then(|chosen| players.iter().find(|(name, _)| name == chosen))
    else {
        return Ok(None);
    };
    let (title, artist) = track(connection, name);
    Ok(Some(NowPlaying {
        player: player_name(name).to_string(),
        title: title.unwrap_or_default(),
        artist: artist.unwrap_or_default(),
        playing: status == "Playing",
    }))
}

/// A player's name from its bus name: `spotify` from `org.mpris.MediaPlayer2.spotify`, with the
/// `.instance1234` some add to tell their windows apart left off.
fn player_name(bus: &str) -> &str {
    let name = bus.strip_prefix(PREFIX).unwrap_or(bus);
    name.split(".instance").next().unwrap_or(name)
}

/// How often what's playing is looked at while it is watched.
const LOOK: Duration = Duration::from_secs(2);

/// What's playing, watched on a thread of its own for as long as this is kept.
pub struct Watch {
    stop: Arc<AtomicBool>,
    again: Arc<AtomicBool>,
    thread: Thread,
}

impl Watch {
    /// Starts watching, calling `changed` from the watching thread with what's playing at the
    /// first look, and again whenever that changes.
    pub fn start(changed: impl Fn(Option<NowPlaying>) + Send + 'static) -> Option<Watch> {
        let stop = Arc::new(AtomicBool::new(false));
        let again = Arc::new(AtomicBool::new(true));
        let (stopped, told) = (stop.clone(), again.clone());
        let spawned = std::thread::Builder::new()
            .name("media-watch".into())
            .spawn(move || {
                let mut connection = None;
                let mut last = None;
                while !stopped.load(Ordering::SeqCst) {
                    if connection.is_none() {
                        connection = connect().ok();
                    }
                    match connection.as_ref().map(now_playing) {
                        Some(Ok(now)) => {
                            if told.swap(false, Ordering::SeqCst) || last.as_ref() != Some(&now) {
                                changed(now.clone());
                                last = Some(now);
                            }
                        }
                        // Connected again at the next look.
                        Some(Err(_)) => connection = None,
                        None => {}
                    }
                    std::thread::park_timeout(LOOK);
                }
            });
        match spawned {
            Ok(thread) => Some(Watch {
                stop,
                again,
                thread: thread.thread().clone(),
            }),
            Err(err) => {
                tracing::warn!("couldn't watch what's playing: {err}");
                None
            }
        }
    }

    /// Says what's playing at the next look whether it changed or not, for a newcomer.
    pub fn again(&self) {
        self.again.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

fn property(
    connection: &zbus::blocking::Connection,
    player: &str,
    name: &str,
) -> Option<zbus::zvariant::OwnedValue> {
    connection
        .call_method(Some(player), PATH, Some(PROPERTIES), "Get", &(PLAYER, name))
        .ok()?
        .body()
        .deserialize()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn players(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, status)| (name.to_string(), status.to_string()))
            .collect()
    }

    #[test]
    fn the_key_goes_to_the_one_playing_then_the_one_paused() {
        let list = players(&[
            ("org.mpris.MediaPlayer2.firefox", "Stopped"),
            ("org.mpris.MediaPlayer2.spotify", "Paused"),
            ("org.mpris.MediaPlayer2.chromium", "Playing"),
        ]);
        assert_eq!(choose(&list), Some("org.mpris.MediaPlayer2.chromium"));
        assert_eq!(choose(&list[..2]), Some("org.mpris.MediaPlayer2.spotify"));
        assert_eq!(choose(&list[..1]), Some("org.mpris.MediaPlayer2.firefox"));
        assert_eq!(choose(&[]), None);
    }

    #[test]
    fn a_player_is_named_from_its_bus_name() {
        assert_eq!(player_name("org.mpris.MediaPlayer2.spotify"), "spotify");
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.firefox.instance_1_42"),
            "firefox"
        );
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.vlc.instance1234"),
            "vlc"
        );
    }

    #[test]
    fn a_track_reads_title_then_artist() {
        assert_eq!(
            track_line(Some("Teardrop"), Some("Massive Attack")),
            "Teardrop — Massive Attack"
        );
        assert_eq!(track_line(Some(" Teardrop "), Some("")), "Teardrop");
        assert_eq!(track_line(None, None), "");
    }
}
