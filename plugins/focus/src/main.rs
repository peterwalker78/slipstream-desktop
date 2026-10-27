//! Focus, the plugin that comes with Slipstream: a timer for spells of work, and an honest log of
//! what they were spent on.
//!
//! It is also a worked example of a plugin. Slipstream runs its program in three ways:
//!
//! - `focus`, with no words, is the program that keeps running (its manifest has no
//!   `every-secs`). It reads what Slipstream tells it on its standard input, a line of JSON at a
//!   time: which windows are open, for `open-windows`, and what's playing, for `media`. It prints
//!   its card, a line of JSON, whenever what the card shows changes.
//! - `focus press <id>` runs when a button on its card, its tile, an answer to a search or the
//!   keys given to its action is pressed. It runs beside the program that keeps running, so all it
//!   does is leave the id in a folder of its own (`asked/`) for that one to act on: only one
//!   program ever changes the timer, so two can't write over each other.
//! - `focus search <words>` answers a search in the explorer that started with its keyword, `!`,
//!   and exits.
//!
//! Everything it keeps is in its own folder, given as `SLIPSTREAM_PLUGIN_STATE`, and the log goes
//! in the folder its manifest's `write` names, given as `SLIPSTREAM_PLUGIN_WRITE_0`. Its sandbox
//! lets it reach nothing else: notifications, sounds and pausing what's playing are all done by
//! Slipstream, from what its card says.

mod card;
mod timer;
mod words;

use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use card::{Notice, Playing};
use timer::{Ask, Finished, Keyboard, Kind, State};

/// Where presses wait for the program that keeps running, in its own folder.
const ASKED: &str = "asked";
/// What it keeps, in its own folder.
const KEPT: &str = "state.json";
/// The log, in the folder it writes to.
const LOG: &str = "log.md";
/// How often the program that keeps running looks for presses and at the time.
const LOOK: Duration = Duration::from_millis(250);
/// A gap this long between two looks, in seconds, means nothing ran in between.
const AWAY: i64 = 30;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match words.as_slice() {
        [] => keep_running(),
        ["press", id] => press(id),
        ["search", words] => {
            println!("{}", card::answers(words, now()));
            Ok(())
        }
        _ => Err("run it as focus, focus press <id> or focus search <words>".into()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("focus: {why}");
            ExitCode::FAILURE
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// Its own folder, which Slipstream gives every plugin for what it keeps.
fn own_folder() -> Result<PathBuf, String> {
    std::env::var_os("SLIPSTREAM_PLUGIN_STATE")
        .map(PathBuf::from)
        .ok_or_else(|| "SLIPSTREAM_PLUGIN_STATE isn't set: Slipstream sets it for a plugin".into())
}

/// Leaves `id` for the program that keeps running, which acts on it within a quarter of a second.
fn press(id: &str) -> Result<(), String> {
    if Ask::from_id(id).is_none() {
        return Err(format!("nothing is pressed as {id:?}"));
    }
    let asked = own_folder()?.join(ASKED);
    fs::create_dir_all(&asked).map_err(|err| err.to_string())?;
    // Named by when, so they're taken in order, and written under a hidden name first, so a
    // half-written one is never read.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let name = format!("{nanos:024}-{}", std::process::id());
    let partial = asked.join(format!(".{name}"));
    fs::write(&partial, id).map_err(|err| err.to_string())?;
    fs::rename(&partial, asked.join(name)).map_err(|err| err.to_string())
}

/// The presses waiting in `folder`, oldest first, each taken away as it's read.
fn take_asked(folder: &Path) -> Vec<Ask> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut waiting: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect();
    waiting.sort();
    waiting
        .into_iter()
        .filter_map(|path| {
            let id = fs::read_to_string(&path).ok();
            let _ = fs::remove_file(&path);
            Ask::from_id(id?.trim())
        })
        .collect()
}

/// The lines Slipstream writes on standard input, read on a thread of their own so the timer
/// never waits on them.
fn listen() -> mpsc::Receiver<String> {
    let (said, heard) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if said.send(line).is_err() {
                break;
            }
        }
    });
    heard
}

/// Takes in one line Slipstream wrote: the windows open, or what's playing.
fn hear(
    line: &str,
    now: i64,
    state: &mut State,
    keyboard: &mut Keyboard,
    media: &mut Option<Playing>,
) {
    let Ok(said) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if let Some(windows) = said.get("windows").and_then(Value::as_array) {
        let focused = windows
            .iter()
            .find(|window| window["focused"] == true)
            .and_then(|window| window["app"].as_str())
            .filter(|app| !app.is_empty())
            .map(str::to_string);
        keyboard.moved(focused, now, state);
    }
    if let Some(told) = said.get("media") {
        let playing = (!told.is_null()).then(|| Playing {
            title: told["title"].as_str().unwrap_or_default().to_string(),
            player: told["player"].as_str().unwrap_or_default().to_string(),
            playing: told["playing"].as_bool().unwrap_or(false),
        });
        let resting = matches!(&state.session, Some(session) if session.kind == Kind::Break);
        let was_playing = media.as_ref().is_some_and(|before| before.playing);
        match &playing {
            Some(after) if resting && was_playing && !after.playing => state.paused_in_break = true,
            Some(after) if !after.playing => {}
            // Playing again, or nothing to play: there's nothing to offer.
            _ => state.paused_in_break = false,
        }
        *media = playing;
    }
}

/// Adds `line` to the log at `log`, starting it with a heading if it's new.
fn add_to_log(log: &Path, line: &str) -> io::Result<()> {
    let new = !log.exists();
    let mut file = fs::OpenOptions::new().create(true).append(true).open(log)?;
    if new {
        file.write_all(b"# Focus\n\n")?;
    }
    writeln!(file, "{line}")
}

/// What a session coming to an end means: a line in the log, and for one whose time ran out
/// while you were here, a notification.
fn ended(
    finished: &Finished,
    state: &mut State,
    log: Option<&Path>,
    now: i64,
) -> (Option<Notice>, Option<String>) {
    let told = !finished.early && !finished.unseen;
    if finished.kind == Kind::Break {
        let notice = told.then(|| Notice {
            title: "Break's over".into(),
            body: format!(
                "{} of focus today",
                words::duration(state.today_so_far(now))
            ),
            sound: "bell",
        });
        return (notice, None);
    }
    let mut problem = None;
    if let Some(line) = words::log_line(finished, &words::local(finished.started)) {
        state.last = Some(words::summary(finished));
        match log {
            Some(log) => {
                if let Err(err) = add_to_log(log, &line) {
                    problem = Some(format!("Couldn't add to the log: {err}"));
                }
            }
            None => {
                problem = Some("No folder for the log: switch Focus off and on to make it".into())
            }
        }
    }
    let notice = told.then(|| Notice {
        title: "Time for a break".into(),
        body: words::summary(finished),
        sound: "complete",
    });
    (notice, problem)
}

/// The program that keeps running: it acts on presses and the time, takes in what Slipstream
/// tells it, keeps its state and prints its card whenever the card changes.
fn keep_running() -> Result<(), String> {
    let own = own_folder()?;
    let asked = own.join(ASKED);
    let kept = own.join(KEPT);
    let log =
        std::env::var_os("SLIPSTREAM_PLUGIN_WRITE_0").map(|folder| PathBuf::from(folder).join(LOG));
    let mut state = State::load(&kept);
    let mut saved = state.clone();
    let mut keyboard = Keyboard::default();
    let mut media = None;
    let mut problem = None;
    let mut printed = String::new();
    let heard = listen();
    let mut out = io::stdout();
    let mut seen = now();
    loop {
        let now = now();
        let mut finished: Vec<Finished> = Vec::new();
        if now - seen > AWAY {
            finished.extend(state.away(seen, &mut keyboard));
        }
        seen = now;
        state.new_day(&words::local(now).date);
        finished.extend(
            take_asked(&asked)
                .into_iter()
                .filter_map(|ask| state.ask(ask, now, &mut keyboard)),
        );
        finished.extend(state.tick(now, &mut keyboard));
        let mut notice = None;
        for finished in &finished {
            let (said, wrong) = ended(finished, &mut state, log.as_deref(), now);
            notice = said.or(notice);
            problem = wrong.or(problem);
        }

        let card = card::card(
            &state,
            media.as_ref(),
            now,
            notice.as_ref(),
            problem.as_deref(),
        )
        .to_string();
        if card != printed {
            // Slipstream has stopped listening: there's no one left to show anything to.
            if writeln!(out, "{card}").and_then(|()| out.flush()).is_err() {
                return Ok(());
            }
            printed = card;
        }
        if state != saved {
            if let Err(err) = state.save(&kept) {
                eprintln!("focus: couldn't keep its state: {err}");
            }
            saved = state.clone();
        }

        match heard.recv_timeout(LOOK) {
            Ok(line) => {
                hear(&line, now, &mut state, &mut keyboard, &mut media);
                while let Ok(line) = heard.try_recv() {
                    hear(&line, now, &mut state, &mut keyboard, &mut media);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Nothing more will be said: carry on with the clock alone.
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(LOOK),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use slipstream_config::plugins::{self, Care, Need};

    /// The manifest beside this program reads as Slipstream reads it, asks only for what Focus
    /// uses, and comes to "Worth a thought": it sees your windows' titles and changes files in
    /// one folder of yours, and can't go online.
    #[test]
    fn its_manifest_parses_and_is_worth_a_thought() {
        let plugin = plugins::read(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let manifest = &plugin.manifest;
        assert_eq!(manifest.name, "Focus");
        assert_eq!(
            manifest.needs,
            [
                Need::Notify,
                Need::Sound,
                Need::Media,
                Need::OpenWindows,
                Need::Write
            ]
        );
        assert!(manifest.tile && manifest.keyword.as_deref() == Some("!"));
        assert_eq!(manifest.actions.len(), 1);
        let care = manifest
            .abilities()
            .iter()
            .map(|ability| ability.care)
            .max();
        assert_eq!(care, Some(Care::Thought));
        assert_eq!(Care::Thought.verdict(), "Worth a thought");
    }

    /// Whatever it presses is something it acts on: its card's buttons, its tile, its answers
    /// and its action.
    #[test]
    fn every_id_it_offers_is_one_it_acts_on() {
        use super::{card, timer};
        let plugin = plugins::read(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut ids: Vec<String> = plugin
            .manifest
            .actions
            .iter()
            .map(|a| a.id.clone())
            .collect();
        let idle = card::card(&timer::State::default(), None, 0, None, None);
        let found = card::answers("45", 0);
        for value in [&idle["buttons"], &found["results"]] {
            ids.extend(
                value
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v["id"].as_str().unwrap().into()),
            );
        }
        ids.push(idle["tile"]["id"].as_str().unwrap().into());
        for id in ids {
            assert!(timer::Ask::from_id(&id).is_some(), "{id}");
        }
    }
}
