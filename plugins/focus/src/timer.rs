//! The timer itself: the session running, if any, what today has added up to, and what each
//! thing asked of it does. Everything here takes the time it is given, so the tests can step
//! through a session without waiting for one.
//!
//! What it keeps is written to `state.json` in the plugin's own folder after every change, so a
//! session carries on across Slipstream restarting, or Focus being switched off and on.

use std::{collections::BTreeMap, io, path::Path};

use serde::{Deserialize, Serialize};

/// The two kinds of spell the timer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Focus,
    Break,
}

/// The session running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub kind: Kind,
    /// When it started, in seconds since the Unix epoch.
    pub started: i64,
    /// How long it is set to run, in seconds.
    pub secs: i64,
    /// The seconds each app had the keyboard during it, by app id: only for a focus session.
    #[serde(default)]
    pub apps: BTreeMap<String, i64>,
}

impl Session {
    pub fn ends(&self) -> i64 {
        self.started + self.secs
    }
}

/// Everything Focus keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub session: Option<Session>,
    /// The day `today` counts, as `2026-10-27`.
    pub day: String,
    /// The seconds of focus finished that day.
    pub today: i64,
    /// What the last focus session came to, as the idle card says it.
    pub last: Option<String>,
    /// Something playing was paused during a break, so the card offers to play it again.
    pub paused_in_break: bool,
}

/// What was asked of the timer, by a button on its card, its tile, an answer to a search or the
/// keys given to its action. Each arrives as the id that was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Start a session of this kind, this many minutes long, ending any running.
    Start(Kind, i64),
    Stop,
    /// Five minutes more on the session running.
    More,
    /// Stop a focus session, or start one of 25 minutes.
    Toggle,
}

/// The longest session that can be asked for, in minutes.
pub const MOST_MINUTES: i64 = 600;
/// How long a session runs from the tile or the action's keys, in minutes.
pub const USUAL_MINUTES: i64 = 25;

impl Ask {
    /// What a press's id asks: `focus-25`, `break-5`, `stop`, `more` or `toggle`.
    pub fn from_id(id: &str) -> Option<Ask> {
        match id {
            "stop" => return Some(Ask::Stop),
            "more" => return Some(Ask::More),
            "toggle" => return Some(Ask::Toggle),
            _ => {}
        }
        let (kind, minutes) = id.split_once('-')?;
        let kind = match kind {
            "focus" => Kind::Focus,
            "break" => Kind::Break,
            _ => return None,
        };
        let minutes: i64 = minutes.parse().ok()?;
        (1..=MOST_MINUTES)
            .contains(&minutes)
            .then_some(Ask::Start(kind, minutes))
    }

    /// The id that asks this.
    pub fn id(self) -> String {
        match self {
            Ask::Start(Kind::Focus, minutes) => format!("focus-{minutes}"),
            Ask::Start(Kind::Break, minutes) => format!("break-{minutes}"),
            Ask::Stop => "stop".into(),
            Ask::More => "more".into(),
            Ask::Toggle => "toggle".into(),
        }
    }
}

/// A session that has come to an end, as its log line and notification tell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub kind: Kind,
    pub started: i64,
    /// How long it was set to run, and how long it did, in seconds.
    pub planned: i64,
    pub spent: i64,
    /// It was stopped before its time.
    pub early: bool,
    /// Its time ran out while Focus wasn't running to see it, so nobody is told now.
    pub unseen: bool,
    /// The apps that had the keyboard, most time first.
    pub apps: Vec<(String, i64)>,
}

/// A session that ended this long ago when Focus first sees it ended unseen.
const UNSEEN_AFTER: i64 = 60;

/// Which app has the keyboard, and since when, to add up a focus session's time in each. It
/// isn't kept: after a restart Slipstream says again which windows are open.
#[derive(Debug, Default)]
pub struct Keyboard {
    app: Option<String>,
    since: i64,
}

impl Keyboard {
    /// The app with the keyboard is now `app`, or none: the time since the last change goes to
    /// the one that had it.
    pub fn moved(&mut self, app: Option<String>, now: i64, state: &mut State) {
        if app != self.app {
            self.credit(state.session.as_mut(), now);
            self.app = app;
        }
    }

    /// Adds the time since the last change, up to `until`, to the app that had the keyboard.
    fn credit(&mut self, session: Option<&mut Session>, until: i64) {
        if let (Some(session), Some(app)) = (session, &self.app)
            && session.kind == Kind::Focus
        {
            let from = self.since.max(session.started);
            let secs = until.min(session.ends()) - from;
            if secs > 0 {
                *session.apps.entry(app.clone()).or_default() += secs;
            }
        }
        self.since = self.since.max(until);
    }
}

impl State {
    /// What was kept at `path`, or a fresh start if nothing readable was.
    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Keeps the state at `path`, written beside it and renamed over it, so a restart never
    /// finds half of it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let new = path.with_extension("new");
        std::fs::write(&new, serde_json::to_vec(self)?)?;
        std::fs::rename(new, path)
    }

    /// Starts the count afresh on a new `day`.
    pub fn new_day(&mut self, day: &str) {
        if self.day != day {
            self.day = day.to_string();
            self.today = 0;
        }
    }

    /// Whether a focus session is running.
    pub fn focusing(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.kind == Kind::Focus)
    }

    /// The seconds of focus today, the session running included.
    pub fn today_so_far(&self, now: i64) -> i64 {
        let running = match &self.session {
            Some(session) if session.kind == Kind::Focus => {
                (now.min(session.ends()) - session.started).max(0)
            }
            _ => 0,
        };
        self.today + running
    }

    /// Does what `ask` asks at `now`, and gives the session it ended, if it ended one.
    pub fn ask(&mut self, ask: Ask, now: i64, keyboard: &mut Keyboard) -> Option<Finished> {
        match ask {
            Ask::Start(kind, minutes) => {
                let ended = self.end(now, true, keyboard);
                self.start(kind, minutes * 60, now, keyboard);
                ended
            }
            Ask::Stop => self.end(now, true, keyboard),
            Ask::More => {
                if let Some(session) = &mut self.session {
                    session.secs = (session.secs + 5 * 60).min(MOST_MINUTES * 60);
                }
                None
            }
            Ask::Toggle if self.focusing() => self.end(now, true, keyboard),
            Ask::Toggle => self.ask(Ask::Start(Kind::Focus, USUAL_MINUTES), now, keyboard),
        }
    }

    /// Ends the session running if its time is up at `now`, and gives it. A focus session seen
    /// to end is followed by a break a fifth of its length, five minutes at least.
    pub fn tick(&mut self, now: i64, keyboard: &mut Keyboard) -> Option<Finished> {
        let ends = self.session.as_ref()?.ends();
        if now < ends {
            return None;
        }
        let mut finished = self.end(ends, false, keyboard)?;
        finished.unseen = now - ends > UNSEEN_AFTER;
        if finished.kind == Kind::Focus && !finished.unseen {
            let rest = (finished.planned / 5).max(5 * 60);
            self.start(Kind::Break, rest, now, keyboard);
        }
        Some(finished)
    }

    fn start(&mut self, kind: Kind, secs: i64, now: i64, keyboard: &mut Keyboard) {
        self.session = Some(Session {
            kind,
            started: now,
            secs,
            apps: BTreeMap::new(),
        });
        keyboard.since = now;
    }

    /// Ends the session running at `now`, counting a focus session's time in today's.
    fn end(&mut self, now: i64, early: bool, keyboard: &mut Keyboard) -> Option<Finished> {
        keyboard.credit(self.session.as_mut(), now);
        let session = self.session.take()?;
        let ends = session.ends();
        let spent = (now.min(ends) - session.started).max(0);
        let mut apps: Vec<(String, i64)> = session.apps.into_iter().collect();
        apps.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if session.kind == Kind::Focus {
            self.today += spent;
        }
        Some(Finished {
            kind: session.kind,
            started: session.started,
            planned: session.secs,
            spent,
            early: early && now < ends,
            unseen: false,
            apps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_s_id_says_what_it_asks() {
        for ask in [
            Ask::Start(Kind::Focus, 25),
            Ask::Start(Kind::Break, 5),
            Ask::Stop,
            Ask::More,
            Ask::Toggle,
        ] {
            assert_eq!(Ask::from_id(&ask.id()), Some(ask));
        }
        for nothing in [
            "",
            "focus",
            "focus-",
            "focus-0",
            "focus-601",
            "nap-5",
            "focus--5",
        ] {
            assert_eq!(Ask::from_id(nothing), None, "{nothing}");
        }
    }

    #[test]
    fn a_session_adds_up_the_time_each_app_had_the_keyboard() {
        let (mut state, mut keyboard) = (State::default(), Keyboard::default());
        keyboard.moved(Some("code".into()), 0, &mut state);
        state.ask(Ask::Start(Kind::Focus, 25), 100, &mut keyboard);
        keyboard.moved(Some("org.mozilla.firefox".into()), 700, &mut state);
        keyboard.moved(Some("code".into()), 1000, &mut state);
        assert_eq!(state.today_so_far(1100), 1000);
        let finished = state.tick(1600, &mut keyboard).unwrap();
        assert_eq!(
            finished.apps,
            [
                ("code".to_string(), 1200),
                ("org.mozilla.firefox".into(), 300)
            ],
            "only time in the session counts"
        );
        assert_eq!((finished.spent, finished.early), (1500, false));
        assert_eq!(state.today, 1500);
        // A break follows, a fifth as long, and adds nothing to today.
        let rest = state.session.clone().unwrap();
        assert_eq!((rest.kind, rest.secs), (Kind::Break, 300));
        let over = state.tick(1900, &mut keyboard).unwrap();
        assert_eq!(over.kind, Kind::Break);
        assert!(over.apps.is_empty() && state.session.is_none());
        assert_eq!(state.today, 1500);
    }

    #[test]
    fn stopping_early_counts_what_was_done_and_toggling_starts_the_usual() {
        let (mut state, mut keyboard) = (State::default(), Keyboard::default());
        assert_eq!(state.ask(Ask::Toggle, 0, &mut keyboard), None);
        assert!(state.focusing());
        assert_eq!(state.session.as_ref().unwrap().secs, USUAL_MINUTES * 60);
        state.ask(Ask::More, 10, &mut keyboard);
        assert_eq!(state.session.as_ref().unwrap().secs, 30 * 60);
        let stopped = state.ask(Ask::Toggle, 600, &mut keyboard).unwrap();
        assert!(stopped.early);
        assert_eq!((stopped.spent, stopped.planned), (600, 1800));
        assert_eq!(state.today, 600);
        assert_eq!(state.tick(5000, &mut keyboard), None, "nothing runs");
    }

    #[test]
    fn a_session_that_ran_out_while_focus_was_away_ends_quietly() {
        let (mut state, mut keyboard) = (State::default(), Keyboard::default());
        state.ask(Ask::Start(Kind::Focus, 25), 0, &mut keyboard);
        let finished = state.tick(1500 + 3600, &mut keyboard).unwrap();
        assert!(finished.unseen && !finished.early);
        assert_eq!(finished.spent, 1500);
        assert!(state.session.is_none(), "no break starts an hour late");
    }

    #[test]
    fn a_new_day_starts_the_count_afresh_and_the_state_survives_a_restart() {
        let mut state = State {
            day: "2026-10-26".into(),
            today: 900,
            ..State::default()
        };
        state.new_day("2026-10-26");
        assert_eq!(state.today, 900);
        state.new_day("2026-10-27");
        assert_eq!(state.today, 0);

        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .join(format!("focus-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        state.ask(Ask::Start(Kind::Focus, 50), 0, &mut Keyboard::default());
        state.save(&path).unwrap();
        assert_eq!(State::load(&path), state);
        assert_eq!(State::load(&dir.join("none.json")), State::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
