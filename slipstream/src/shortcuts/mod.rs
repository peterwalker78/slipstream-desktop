//! Shortcuts apps keep while another app has the keyboard: a voice chat's push to talk, a
//! recorder's start and stop.
//!
//! Apps ask through the global shortcuts portal (`portal.rs`). Nothing is bound until the person
//! says so on a card (`ask.rs`), where they can take the keys the app would like or press their
//! own; an app asking again for shortcuts it was already given gets them without a card. The
//! answers are written to the state folder (`slipstream_config::shortcuts`) and the Settings app
//! takes them away there.
//!
//! The keys themselves never reach any app. Slipstream keeps the press and its release and the
//! app is only told, through the portal, that its shortcut went down and came up again, so a held
//! push to talk stays held for exactly as long as the key does, whichever window has the
//! keyboard.

pub mod ask;
pub mod own;
pub mod portal;
pub mod trigger;

#[cfg(test)]
mod through_portal;

use std::collections::{HashMap, VecDeque};

use smithay::input::keyboard::{Keycode, Keysym};

use slipstream_config::shortcuts::{Granted, Shortcut};

use crate::keys::Mods;

use self::{
    ask::{Act, Ask, Row},
    portal::{Listed, Signal, Wanted},
    trigger::{Refusal, Trigger},
};

/// An app's session with the portal.
struct Session {
    /// Its app ID. Without one nothing it is given is written down: another app with no ID
    /// would read as the same app.
    app: String,
    name: String,
    /// What it was given, once it asked.
    bound: Vec<Bound>,
}

#[derive(Debug, Clone, PartialEq)]
struct Bound {
    id: String,
    description: String,
    trigger: Trigger,
}

impl Bound {
    fn listed(&self) -> Listed {
        Listed {
            id: self.id.clone(),
            description: self.description.clone(),
            keys: self.trigger.label(),
        }
    }
}

/// A request waiting for its card.
pub struct Pending {
    session: String,
    wanted: Vec<Wanted>,
    reply: async_channel::Sender<Option<Vec<Listed>>>,
}

/// A shortcut held down: the key that went down, and whose shortcut it was.
struct Held {
    keycode: Keycode,
    session: String,
    id: String,
}

#[derive(Default)]
pub struct Shortcuts {
    granted: Granted,
    sessions: HashMap<String, Session>,
    held: Vec<Held>,
    /// The card on screen, and the request it answers.
    asking: Option<(Pending, Ask)>,
    waiting: VecDeque<Pending>,
    /// The person's own shortcuts that work, which no app can be given.
    own: Vec<own::OwnKey>,
}

impl Shortcuts {
    pub fn new(granted: Granted) -> Self {
        Self {
            granted,
            ..Self::default()
        }
    }

    pub fn set_own(&mut self, own: Vec<own::OwnKey>) {
        self.own = own;
    }

    pub fn own(&self) -> &[own::OwnKey] {
        &self.own
    }

    /// What the person's own shortcut on these keys does.
    pub fn own_key(&self, mods: Mods, key: Keysym) -> Option<own::Does> {
        self.own
            .iter()
            .find(|own| own.trigger.matches(mods, key))
            .map(|own| own.does.clone())
    }

    pub fn granted(&self) -> &Granted {
        &self.granted
    }

    pub fn ask(&self) -> Option<&Ask> {
        self.asking.as_ref().map(|(_, ask)| ask)
    }

    pub fn ask_mut(&mut self) -> Option<&mut Ask> {
        self.asking.as_mut().map(|(_, ask)| ask)
    }

    /// An app opened a session; `name` is what the card calls it.
    pub fn opened(&mut self, session: String, app: String, name: String) {
        tracing::info!(app, session, "an app opened a shortcuts session");
        self.sessions.insert(
            session,
            Session {
                app,
                name,
                bound: Vec::new(),
            },
        );
    }

    /// The session ended. Its keys are let go of without telling it — it isn't listening — and
    /// a card asking for it has nothing left to ask. Returns whether the card went.
    pub fn closed(&mut self, session: &str) -> bool {
        self.sessions.remove(session);
        self.held.retain(|held| held.session != session);
        self.waiting.retain(|pending| pending.session != session);
        if self
            .asking
            .as_ref()
            .is_some_and(|(pending, _)| pending.session == session)
        {
            self.asking = None;
            return true;
        }
        false
    }

    /// What the session has, or before it asked, what its app was given last time.
    pub fn list(&self, session: &str) -> Vec<Listed> {
        let Some(open) = self.sessions.get(session) else {
            return Vec::new();
        };
        if !open.bound.is_empty() {
            return open.bound.iter().map(Bound::listed).collect();
        }
        self.granted
            .app(&open.app)
            .filter(|_| !open.app.is_empty())
            .map(|app| {
                app.shortcuts
                    .iter()
                    .filter(|s| !s.trigger.is_empty())
                    .map(|s| Listed {
                        id: s.id.clone(),
                        description: s.description.clone(),
                        keys: s.keys.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Why `trigger` can't be one of `app`'s shortcuts, if it can't.
    fn refusal(&self, app: &str, trigger: Trigger) -> Option<Refusal> {
        if let Err(refusal) = trigger.check() {
            return Some(refusal);
        }
        let spec = trigger.spec();
        if let Some(other) = self.granted.held_by_another(&spec, app) {
            return Some(Refusal::Taken(other.to_string()));
        }
        if let Some(own) = self.own.iter().find(|own| own.trigger == trigger) {
            return Some(own.refusal());
        }
        self.sessions
            .values()
            .filter(|open| open.app != app || app.is_empty())
            .find(|open| open.bound.iter().any(|b| b.trigger == trigger))
            .map(|open| Refusal::Taken(open.name.clone()))
    }

    /// An app asks for shortcuts. Ones it was given before are bound straight away; anything new
    /// waits for the person, on a card. Returns whether a card has come up.
    pub fn bind(&mut self, pending: Pending, now: f64, reduced_motion: bool) -> bool {
        let Some(open) = self.sessions.get(&pending.session) else {
            let _ = pending.reply.try_send(None);
            return false;
        };
        let app = open.app.clone();
        // Every shortcut asked for was answered before: given keys, or left out.
        let remembered: Option<Vec<Option<Bound>>> = (!app.is_empty())
            .then(|| {
                pending
                    .wanted
                    .iter()
                    .map(|wanted| {
                        let given = self.granted.shortcut(&app, &wanted.id)?;
                        if given.trigger.is_empty() {
                            return Some(None);
                        }
                        let trigger = Trigger::parse(&given.trigger)?;
                        trigger.check().ok()?;
                        Some(Some(Bound {
                            id: wanted.id.clone(),
                            description: wanted.description.clone(),
                            trigger,
                        }))
                    })
                    .collect()
            })
            .flatten();
        if let Some(bound) = remembered {
            tracing::info!(app, "binding shortcuts answered before");
            let bound: Vec<Bound> = bound.into_iter().flatten().collect();
            let listed = bound.iter().map(Bound::listed).collect();
            if let Some(open) = self.sessions.get_mut(&pending.session) {
                open.bound = bound;
            }
            let _ = pending.reply.try_send(Some(listed));
            return false;
        }
        self.waiting.push_back(pending);
        self.next(now, reduced_motion)
    }

    /// Puts up the card for the next request, if none is up.
    fn next(&mut self, now: f64, reduced_motion: bool) -> bool {
        if self.asking.is_some() {
            return false;
        }
        while let Some(pending) = self.waiting.pop_front() {
            let Some(open) = self.sessions.get(&pending.session) else {
                let _ = pending.reply.try_send(None);
                continue;
            };
            let (app, name) = (open.app.clone(), open.name.clone());
            let mut rows: Vec<Row> = Vec::new();
            for wanted in &pending.wanted {
                // What it was given before, else what it would like.
                let given = self
                    .granted
                    .shortcut(&app, &wanted.id)
                    .filter(|_| !app.is_empty());
                let trigger = match given {
                    Some(given) => Trigger::parse(&given.trigger),
                    None => wanted.preferred.as_deref().and_then(Trigger::parse),
                }
                .filter(|trigger| self.refusal(&app, *trigger).is_none())
                .filter(|trigger| !rows.iter().any(|row| row.trigger == Some(*trigger)));
                rows.push(Row {
                    id: wanted.id.clone(),
                    description: wanted.description.clone(),
                    trigger,
                });
            }
            tracing::info!(
                app,
                shortcuts = rows.len(),
                "asking about an app's shortcuts"
            );
            self.asking = Some((pending, Ask::new(name, rows, now, reduced_motion)));
            return true;
        }
        false
    }

    /// A key while the card is up.
    pub fn key(&mut self, key: Keysym, mods: Mods, now: f64, reduced_motion: bool) -> Answer {
        let Some((_, ask)) = self.asking.as_mut() else {
            return Answer::Nothing;
        };
        let act = ask.key(key, mods, now);
        self.act(act, now, reduced_motion)
    }

    pub fn click(&mut self, x: f64, y: f64, now: f64, reduced_motion: bool) -> Answer {
        let Some((_, ask)) = self.asking.as_mut() else {
            return Answer::Nothing;
        };
        let act = ask.click(x, y, now);
        self.act(act, now, reduced_motion)
    }

    fn act(&mut self, act: Act, now: f64, reduced_motion: bool) -> Answer {
        match act {
            Act::Nothing => Answer::Nothing,
            Act::Pick(trigger) => {
                let app = self
                    .asking
                    .as_ref()
                    .and_then(|(pending, _)| self.sessions.get(&pending.session))
                    .map(|open| open.app.clone())
                    .unwrap_or_default();
                let refusal = self.refusal(&app, trigger);
                if let Some((_, ask)) = self.asking.as_mut() {
                    match refusal {
                        Some(refusal) => ask.refuse(refusal.say(&trigger.label())),
                        None => ask.set(trigger),
                    }
                }
                Answer::Nothing
            }
            Act::Allow => {
                let written = self.allow();
                self.next(now, reduced_motion);
                Answer::Answered { written }
            }
            Act::Deny => {
                if let Some((pending, _)) = self.asking.take() {
                    tracing::info!("an app's shortcuts were turned down");
                    let _ = pending.reply.try_send(None);
                }
                self.next(now, reduced_motion);
                Answer::Answered { written: false }
            }
        }
    }

    /// Binds what the card shows, and writes it down. Returns whether there is something new to
    /// write to the file.
    fn allow(&mut self) -> bool {
        let Some((pending, ask)) = self.asking.take() else {
            return false;
        };
        let bound: Vec<Bound> = ask
            .rows
            .iter()
            .filter_map(|row| {
                Some(Bound {
                    id: row.id.clone(),
                    description: row.description.clone(),
                    trigger: row.trigger?,
                })
            })
            .collect();
        let _ = pending
            .reply
            .try_send(Some(bound.iter().map(Bound::listed).collect()));
        let Some(open) = self.sessions.get_mut(&pending.session) else {
            return false;
        };
        tracing::info!(
            app = open.app,
            shortcuts = bound.len(),
            "an app's shortcuts were allowed"
        );
        open.bound = bound.clone();
        if open.app.is_empty() {
            return false;
        }
        // Left out is an answer too: the app isn't asked about it again.
        let shortcuts = ask
            .rows
            .iter()
            .map(|row| Shortcut {
                id: row.id.clone(),
                description: row.description.clone(),
                trigger: row.trigger.map(|t| t.spec()).unwrap_or_default(),
                keys: row.trigger.map(|t| t.label()).unwrap_or_default(),
            })
            .collect();
        let (app, name) = (open.app.clone(), open.name.clone());
        self.granted.set(&app, &name, shortcuts);
        true
    }

    /// A key went down. Every session whose shortcut it is hears about it; the key is then
    /// Slipstream's until it comes up. Nothing when the keys are no app's shortcut.
    pub fn press(&mut self, keycode: Keycode, mods: Mods, key: Keysym, time: u64) -> Vec<Signal> {
        let mut signals = Vec::new();
        for (session, open) in &self.sessions {
            for bound in open.bound.iter().filter(|b| b.trigger.matches(mods, key)) {
                self.held.push(Held {
                    keycode,
                    session: session.clone(),
                    id: bound.id.clone(),
                });
                signals.push(Signal::Activated {
                    session: session.clone(),
                    id: bound.id.clone(),
                    time,
                });
            }
        }
        signals
    }

    /// Whether this key went down as a shortcut and hasn't come up yet.
    pub fn is_held(&self, keycode: Keycode) -> bool {
        self.held.iter().any(|held| held.keycode == keycode)
    }

    /// The key came up: whatever it set off is let go of, whatever else was let go first.
    pub fn release(&mut self, keycode: Keycode, time: u64) -> Vec<Signal> {
        let mut signals = Vec::new();
        self.held.retain(|held| {
            if held.keycode != keycode {
                return true;
            }
            signals.push(Signal::Deactivated {
                session: held.session.clone(),
                id: held.id.clone(),
                time,
            });
            false
        });
        signals
    }

    /// The file changed: a shortcut was taken away in Settings, say. Sessions holding a shortcut
    /// their app no longer has lose it, and are told what they have left.
    pub fn reload(&mut self, granted: Granted, time: u64) -> Vec<Signal> {
        if granted == self.granted {
            return Vec::new();
        }
        self.granted = granted;
        let mut signals = Vec::new();
        for (session, open) in self.sessions.iter_mut() {
            if open.app.is_empty() {
                continue;
            }
            let before = open.bound.len();
            let granted = &self.granted;
            open.bound.retain(|bound| {
                granted
                    .shortcut(&open.app, &bound.id)
                    .is_some_and(|given| Trigger::parse(&given.trigger) == Some(bound.trigger))
            });
            if open.bound.len() != before {
                // A shortcut held down as it went is let go of.
                let ids: Vec<&String> = open.bound.iter().map(|b| &b.id).collect();
                self.held.retain(|held| {
                    if held.session != *session || ids.contains(&&held.id) {
                        return true;
                    }
                    signals.push(Signal::Deactivated {
                        session: held.session.clone(),
                        id: held.id.clone(),
                        time,
                    });
                    false
                });
                signals.push(Signal::Changed {
                    session: session.clone(),
                    shortcuts: open.bound.iter().map(Bound::listed).collect(),
                });
            }
        }
        signals
    }
}

/// What answering the card came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Nothing,
    /// The card went; `written` when there are new answers for the file.
    Answered {
        written: bool,
    },
}

impl Pending {
    pub fn new(
        session: String,
        wanted: Vec<Wanted>,
        reply: async_channel::Sender<Option<Vec<Listed>>>,
    ) -> Self {
        Self {
            session,
            wanted,
            reply,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_42/talk";
    const OTHER: &str = "/org/freedesktop/portal/desktop/session/1_43/rec";
    const CTRL_ALT: Mods = Mods {
        ctrl: true,
        alt: true,
        shift: false,
        logo: false,
    };

    fn wanted(id: &str, preferred: Option<&str>) -> Wanted {
        Wanted {
            id: id.to_string(),
            description: format!("{id} described"),
            preferred: preferred.map(str::to_string),
        }
    }

    type Reply = async_channel::Receiver<Option<Vec<Listed>>>;

    fn ask_for(shortcuts: &mut Shortcuts, session: &str, wanted: Vec<Wanted>) -> (bool, Reply) {
        let (reply, answer) = async_channel::bounded(1);
        let shown = shortcuts.bind(Pending::new(session.to_string(), wanted, reply), 0.0, true);
        (shown, answer)
    }

    fn talk() -> Shortcuts {
        let mut shortcuts = Shortcuts::new(Granted::new());
        shortcuts.opened(SESSION.into(), "org.example.Talk".into(), "Talk".into());
        shortcuts
    }

    fn enter(shortcuts: &mut Shortcuts) -> Answer {
        shortcuts.key(Keysym::Return, Mods::default(), 5.0, true)
    }

    #[test]
    fn nothing_is_bound_until_the_person_allows_it() {
        let mut shortcuts = talk();
        let (shown, answer) = ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        assert!(shown);
        assert!(answer.try_recv().is_err());
        assert!(
            shortcuts
                .press(Keycode::new(28), CTRL_ALT, Keysym::t, 1)
                .is_empty()
        );
        assert_eq!(enter(&mut shortcuts), Answer::Answered { written: true });
        let bound = answer.try_recv().unwrap().unwrap();
        assert_eq!(bound[0].keys, "Ctrl+Alt+T");
        assert_eq!(
            shortcuts
                .press(Keycode::new(28), CTRL_ALT, Keysym::t, 1)
                .len(),
            1
        );
    }

    #[test]
    fn turned_down_binds_nothing_and_writes_nothing() {
        let mut shortcuts = talk();
        let (_, answer) = ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        let answered = shortcuts.key(Keysym::Escape, Mods::default(), 5.0, true);
        assert_eq!(answered, Answer::Answered { written: false });
        assert_eq!(answer.try_recv().unwrap(), None);
        assert!(shortcuts.granted().apps.is_empty());
        assert!(
            shortcuts
                .press(Keycode::new(28), CTRL_ALT, Keysym::t, 1)
                .is_empty()
        );
    }

    #[test]
    fn press_and_release_pair_up_whatever_is_let_go_first() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        enter(&mut shortcuts);
        let key = Keycode::new(28);
        assert_eq!(
            shortcuts.press(key, CTRL_ALT, Keysym::t, 100),
            vec![Signal::Activated {
                session: SESSION.into(),
                id: "talk".into(),
                time: 100
            }]
        );
        assert!(shortcuts.is_held(key));
        // Ctrl and Alt came up first; the release still lets go of the shortcut.
        assert!(shortcuts.release(Keycode::new(37), 150).is_empty());
        assert_eq!(
            shortcuts.release(key, 180),
            vec![Signal::Deactivated {
                session: SESSION.into(),
                id: "talk".into(),
                time: 180
            }]
        );
        assert!(!shortcuts.is_held(key));
        assert!(shortcuts.release(key, 200).is_empty());
    }

    #[test]
    fn an_app_asking_again_gets_what_it_was_given_without_a_card() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        enter(&mut shortcuts);
        let granted = shortcuts.granted().clone();
        // The next login: the file is read back and the app opens a new session.
        let mut next = Shortcuts::new(granted);
        next.opened(OTHER.into(), "org.example.Talk".into(), "Talk".into());
        assert_eq!(next.list(OTHER)[0].keys, "Ctrl+Alt+T");
        let (shown, answer) = ask_for(&mut next, OTHER, vec![wanted("talk", Some("F9"))]);
        assert!(!shown);
        assert_eq!(answer.try_recv().unwrap().unwrap()[0].keys, "Ctrl+Alt+T");
    }

    #[test]
    fn a_shortcut_left_out_isnt_asked_about_again() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t")), wanted("mute", None)],
        );
        enter(&mut shortcuts);
        let again = vec![wanted("talk", None), wanted("mute", None)];
        let (shown, answer) = ask_for(&mut shortcuts, SESSION, again);
        assert!(!shown);
        assert_eq!(answer.try_recv().unwrap().unwrap().len(), 1);
        assert_eq!(shortcuts.list(SESSION).len(), 1);
    }

    #[test]
    fn a_new_shortcut_asks_again_with_the_old_ones_kept() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        enter(&mut shortcuts);
        let (shown, _) = ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", None), wanted("mute", Some("CTRL+ALT+m"))],
        );
        assert!(shown);
        let rows = &shortcuts.ask().unwrap().rows;
        assert_eq!(rows[0].trigger.unwrap().label(), "Ctrl+Alt+T");
        assert_eq!(rows[1].trigger.unwrap().label(), "Ctrl+Alt+M");
    }

    #[test]
    fn a_preferred_key_slipstream_uses_is_left_for_the_person_to_choose() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("LOGO+l"))],
        );
        assert_eq!(shortcuts.ask().unwrap().rows[0].trigger, None);
        // Pressing it on the card says why rather than taking it.
        shortcuts.key(
            Keysym::l,
            Mods {
                logo: true,
                ..Mods::default()
            },
            5.0,
            true,
        );
        assert_eq!(shortcuts.ask().unwrap().rows[0].trigger, None);
        shortcuts.key(Keysym::p, CTRL_ALT, 5.0, true);
        assert_eq!(
            shortcuts.ask().unwrap().rows[0].trigger.unwrap().spec(),
            "CTRL+ALT+p"
        );
    }

    #[test]
    fn two_apps_never_share_keys() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        enter(&mut shortcuts);
        shortcuts.opened(
            OTHER.into(),
            "org.example.Recorder".into(),
            "Recorder".into(),
        );
        ask_for(
            &mut shortcuts,
            OTHER,
            vec![wanted("record", Some("CTRL+ALT+t"))],
        );
        assert_eq!(shortcuts.ask().unwrap().rows[0].trigger, None);
        shortcuts.key(Keysym::t, CTRL_ALT, 5.0, true);
        assert_eq!(shortcuts.ask().unwrap().rows[0].trigger, None);
    }

    #[test]
    fn requests_wait_their_turn_behind_the_card() {
        let mut shortcuts = talk();
        shortcuts.opened(
            OTHER.into(),
            "org.example.Recorder".into(),
            "Recorder".into(),
        );
        let (first, _) = ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        let (second, answer) = ask_for(
            &mut shortcuts,
            OTHER,
            vec![wanted("rec", Some("CTRL+ALT+r"))],
        );
        assert!(first && !second);
        enter(&mut shortcuts);
        assert_eq!(shortcuts.ask().unwrap().app, "Recorder");
        // The next card ignores keys for its first moment too.
        enter(&mut shortcuts);
        assert!(answer.try_recv().is_err());
        shortcuts.key(Keysym::Return, Mods::default(), 10.0, true);
        assert!(answer.try_recv().unwrap().is_some());
        assert!(shortcuts.ask().is_none());
    }

    #[test]
    fn a_session_closing_takes_its_card_and_its_held_keys() {
        let mut shortcuts = talk();
        let (_, _answer) = ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        assert!(shortcuts.closed(SESSION));
        assert!(shortcuts.ask().is_none());
    }

    #[test]
    fn an_app_without_a_name_is_asked_every_time() {
        let mut shortcuts = Shortcuts::new(Granted::new());
        shortcuts.opened(SESSION.into(), String::new(), "An app".into());
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![wanted("talk", Some("CTRL+ALT+t"))],
        );
        assert_eq!(enter(&mut shortcuts), Answer::Answered { written: false });
        assert!(shortcuts.granted().apps.is_empty());
        assert_eq!(
            shortcuts
                .press(Keycode::new(28), CTRL_ALT, Keysym::t, 1)
                .len(),
            1
        );
    }

    #[test]
    fn taking_a_shortcut_away_in_settings_tells_the_app_and_lets_go() {
        let mut shortcuts = talk();
        ask_for(
            &mut shortcuts,
            SESSION,
            vec![
                wanted("talk", Some("CTRL+ALT+t")),
                wanted("mute", Some("CTRL+ALT+m")),
            ],
        );
        enter(&mut shortcuts);
        shortcuts.press(Keycode::new(28), CTRL_ALT, Keysym::t, 1);
        let mut granted = shortcuts.granted().clone();
        granted.forget("org.example.Talk", "talk");
        let signals = shortcuts.reload(granted, 9);
        assert!(signals.contains(&Signal::Deactivated {
            session: SESSION.into(),
            id: "talk".into(),
            time: 9
        }));
        let Some(Signal::Changed {
            shortcuts: left, ..
        }) = signals.last()
        else {
            panic!("the app wasn't told");
        };
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "mute");
        assert!(
            shortcuts
                .press(Keycode::new(28), CTRL_ALT, Keysym::t, 1)
                .is_empty()
        );
    }
}
