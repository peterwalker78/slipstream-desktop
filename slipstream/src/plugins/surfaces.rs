//! What Slipstream draws for a plugin outside its card: the pictures its tile and answers can name,
//! and the thread that asks it searches.
//!
//! A search runs the plugin's program with `search` and the words typed after its keyword, on a
//! thread of the plugin's own, never on the event loop. Only the newest search counts: one typed
//! while another runs ends that run, and a run that takes longer than `SEARCH_LIMIT` is ended
//! too, so a slow or silent plugin never holds typing up, it only has nothing to show.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::Duration,
};

use slipstream_config::{
    command::{self, Run},
    plugins::{
        store,
        surfaces::{self, Hit},
    },
};

use super::worker::{Approved, program, why};
use crate::icons;

/// How long a search may take before it is given up.
pub const SEARCH_LIMIT: Duration = Duration::from_secs(2);

/// The picture called `name`, from the few a plugin can choose from; a plain one for any other.
pub fn icon(name: Option<&str>) -> &'static str {
    match name.unwrap_or_default() {
        "bell" => icons::BELL,
        "eye" => icons::EYE,
        "file" => icons::FILE,
        "folder" => icons::FOLDER,
        "gauge" => icons::GAUGE,
        "keyboard" => icons::KEYBOARD,
        "lock" => icons::LOCK,
        "mic" => icons::MIC,
        "moon" => icons::MOON,
        "pause" => icons::PAUSE,
        "play" => icons::PLAY,
        "power" => icons::POWER,
        "run" => icons::RUN,
        "search" => icons::SEARCH,
        "sun" => icons::SUN,
        "tick" => icons::TICK,
        "volume" => icons::VOLUME,
        _ => icons::INFO,
    }
}

/// What a plugin last answered: the words it was asked, and its answers or why there are none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub words: String,
    pub found: Result<Vec<Hit>, String>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// The newest words asked and not yet taken up by the thread.
    asked: Mutex<Option<String>>,
    /// The words last asked, whether answered yet or not, so asking the same again does nothing.
    last: Mutex<Option<String>>,
    answer: Mutex<Option<Answer>>,
}

/// A plugin's search thread. Dropping it stops the thread, and any run under way.
pub struct Searcher {
    shared: Arc<Shared>,
    thread: Thread,
}

impl Searcher {
    pub fn start(approved: Approved) -> Searcher {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("plugin search".into())
                .spawn(move || answer(&approved, &shared))
                .expect("a thread for a plugin's searches")
        };
        Searcher {
            shared,
            thread: thread.thread().clone(),
        }
    }

    /// Asks it `words`, unless those were the last asked.
    pub fn ask(&self, words: &str) {
        let mut last = self.shared.last.lock().unwrap();
        if last.as_deref() == Some(words) {
            return;
        }
        *last = Some(words.to_string());
        *self.shared.asked.lock().unwrap() = Some(words.to_string());
        self.thread.unpark();
    }

    pub fn answer(&self) -> Option<Answer> {
        self.shared.answer.lock().unwrap().clone()
    }
}

impl Drop for Searcher {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

/// The thread: each search asked, in turn, the newest only.
fn answer(approved: &Approved, shared: &Shared) {
    let plugin = &approved.plugin;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Some(words) = shared.asked.lock().unwrap().take() else {
            std::thread::park();
            continue;
        };
        let found = store::verify(&plugin.dir, &plugin.fingerprint)
            .and_then(|()| program(plugin, &approved.state, &["search", &words]));
        let found = match found {
            Err(why) => Err(why),
            Ok(program) => {
                // A newer search ends this one: only the newest is shown.
                let newer =
                    || shared.stop.load(Ordering::SeqCst) || shared.asked.lock().unwrap().is_some();
                match command::run_tidily(program, SEARCH_LIMIT, &newer, None) {
                    None => continue,
                    Some(Run::Printed(printed)) => surfaces::hits(&printed)
                        .map_err(|err| format!("Its answers can't be read: {err}")),
                    Some(Run::TimedOut) => Err("It took too long to answer".into()),
                    Some(failed) => Err(why(&failed)),
                }
            }
        };
        if let Err(problem) = &found {
            tracing::debug!("plugin {}: a search: {problem}", plugin.id);
        }
        *shared.answer.lock().unwrap() = Some(Answer { words, found });
    }
}
