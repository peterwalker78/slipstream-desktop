//! The card Focus prints, as one line of JSON: the session running with a gauge of its time, what
//! today has come to, buttons for what can be done next, the headline for the chip and the tile
//! for quick settings. Slipstream draws all of it; Focus only says what's there.

use serde_json::{Value, json};

use crate::{
    timer::{Ask, Kind, State},
    words,
};

/// What's playing, as Slipstream last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    pub title: String,
    pub player: String,
    pub playing: bool,
}

/// A notification to send with the card, and its sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub sound: &'static str,
}

/// Slipstream's own button for what's playing: it pauses or plays when pressed, without running
/// Focus. A plugin can't press it itself, so pausing is always the person's choice.
const PLAY_PAUSE: &str = "media:play-pause";

/// The card at `now`. `problem` is why the log can't be written, if it can't.
pub fn card(
    state: &State,
    media: Option<&Playing>,
    now: i64,
    notice: Option<&Notice>,
    problem: Option<&str>,
) -> Value {
    let today = state.today_so_far(now);
    let mut lines = vec![if today > 0 {
        format!("{} of focus today", words::duration(today))
    } else {
        "No focus yet today".to_string()
    }];
    let mut buttons = Vec::new();
    let mut sections = Vec::new();
    let mut glance = None;
    // The tile says what it does while off, and how long is left while on, over the plugin's name.
    let mut tile = "Focus".to_string();
    let button = |ask: Ask, label: &str| json!({"id": ask.id(), "label": label});

    match &state.session {
        Some(session) => {
            let (name, length, stop) = match session.kind {
                Kind::Focus => ("Focusing", "session", "Stop"),
                Kind::Break => ("On a break", "break", "End break"),
            };
            // No percent: the gauge shows only the time gone by and the time left.
            sections.push(json!({
                "name": name,
                "detail": format!("until {}", words::local(session.ends()).time),
                "meters": [{
                    "label": format!("{} {length}", words::duration(session.secs)),
                    "resets": session.ends(),
                    "window": session.secs,
                }],
            }));
            buttons.push(button(Ask::Stop, stop));
            buttons.push(button(Ask::More, "+5 min"));
            let left = words::left(session.ends(), now);
            if session.kind == Kind::Focus {
                tile = format!("{left} left");
            }
            glance = Some(match session.kind {
                Kind::Focus => left,
                Kind::Break => format!("{left} break"),
            });
        }
        None => {
            if let Some(last) = &state.last {
                lines.push(format!("Last: {last}"));
            }
            buttons.push(button(Ask::Start(Kind::Focus, 25), "25 min"));
            buttons.push(button(Ask::Start(Kind::Focus, 50), "50 min"));
        }
    }

    // In a break, what's playing can be paused from here, and played again once it was.
    let resting = matches!(&state.session, Some(session) if session.kind == Kind::Break);
    if let Some(media) = media {
        let what = if media.title.is_empty() {
            &media.player
        } else {
            &media.title
        };
        if resting && media.playing {
            lines.push(format!("Playing: {what}"));
            buttons.push(json!({"id": PLAY_PAUSE, "label": "Pause"}));
        } else if state.paused_in_break && !media.playing {
            lines.push(format!("Paused in your break: {what}"));
            buttons.push(json!({"id": PLAY_PAUSE, "label": "Play"}));
        }
    }
    if let Some(problem) = problem {
        lines.push(problem.to_string());
    }

    let mut card = json!({
        "lines": lines,
        "sections": sections,
        "buttons": buttons,
        "tile": {"id": Ask::Toggle.id(), "label": tile, "icon": "eye", "on": state.focusing()},
    });
    if let Some(glance) = glance {
        card["glance"] = json!(glance);
    }
    if let Some(notice) = notice {
        card["notify"] = json!({"title": notice.title, "body": notice.body});
        card["sound"] = json!(notice.sound);
    }
    card
}

/// The answers to a search for `words`: a focus session or a break that many minutes long, each
/// saying when it would end. None for words that aren't minutes.
pub fn answers(words: &str, now: i64) -> Value {
    let Some(minutes) = words::minutes(words) else {
        return json!({"results": []});
    };
    let until = format!("until {}", words::local(now + minutes * 60).time);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    json!({"results": [
        {
            "id": Ask::Start(Kind::Focus, minutes).id(),
            "label": format!("Focus for {minutes} {unit}"),
            "detail": until,
            "icon": "eye",
        },
        {
            "id": Ask::Start(Kind::Break, minutes).id(),
            "label": format!("Take a {minutes} minute break"),
            "detail": until,
            "icon": "moon",
        },
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::{Keyboard, Session};

    fn running(kind: Kind) -> State {
        State {
            session: Some(Session {
                kind,
                started: 0,
                secs: 1500,
                apps: Default::default(),
            }),
            ..State::default()
        }
    }

    fn ids(card: &Value) -> Vec<&str> {
        card["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|button| button["id"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn a_session_shows_its_time_left_and_lights_the_tile() {
        let card = card(&running(Kind::Focus), None, 7 * 60, None, None);
        assert_eq!(card["glance"], "18 min");
        assert_eq!(card["tile"]["on"], true);
        assert_eq!(card["tile"]["label"], "18 min left");
        assert_eq!(card["sections"][0]["name"], "Focusing");
        let meter = &card["sections"][0]["meters"][0];
        assert_eq!(meter["label"], "25 min session");
        assert_eq!(
            (meter["resets"].as_i64(), meter["window"].as_i64()),
            (Some(1500), Some(1500))
        );
        assert!(meter.get("percent").is_none());
        assert_eq!(ids(&card), ["stop", "more"]);
        assert_eq!(card["lines"][0], "7 min of focus today");
    }

    #[test]
    fn idle_it_offers_two_lengths_and_says_what_today_came_to() {
        let mut state = State {
            today: 3000,
            last: Some("25 minutes, mostly in Code".into()),
            ..State::default()
        };
        let card = card(&state, None, 0, None, None);
        assert_eq!(ids(&card), ["focus-25", "focus-50"]);
        assert_eq!(
            card["lines"],
            json!(["50 min of focus today", "Last: 25 minutes, mostly in Code"])
        );
        assert_eq!(card["tile"]["on"], false);
        assert_eq!(card["tile"]["label"], "Focus");
        assert!(card.get("glance").is_none());
        // Its tile, pressed, starts the usual session.
        state.ask(Ask::from_id("toggle").unwrap(), 0, &mut Keyboard::default());
        assert!(state.focusing());
    }

    /// Pausing is only ever offered, never done: a button in a break while something plays, and
    /// one to play it again once it was paused.
    #[test]
    fn what_s_playing_can_be_paused_in_a_break_and_played_after() {
        let playing = Playing {
            title: "Night Drive".into(),
            player: "spotify".into(),
            playing: true,
        };
        let card_now = card(&running(Kind::Break), Some(&playing), 0, None, None);
        assert_eq!(ids(&card_now), ["stop", "more", PLAY_PAUSE]);
        assert_eq!(card_now["lines"][1], "Playing: Night Drive");
        assert_eq!(card_now["glance"], "25 min break");
        assert_eq!(
            ids(&card(&running(Kind::Focus), Some(&playing), 0, None, None)),
            ["stop", "more"],
            "nothing to pause while focusing"
        );
        let paused = Playing {
            playing: false,
            ..playing
        };
        let after = State {
            paused_in_break: true,
            ..State::default()
        };
        let card_after = card(&after, Some(&paused), 0, None, None);
        assert_eq!(ids(&card_after), ["focus-25", "focus-50", PLAY_PAUSE]);
        assert_eq!(card_after["buttons"][2]["label"], "Play");
    }

    #[test]
    fn a_notice_goes_with_its_sound() {
        let notice = Notice {
            title: "Time for a break".into(),
            body: "25 minutes, mostly in Code".into(),
            sound: "complete",
        };
        let card = card(&State::default(), None, 0, Some(&notice), None);
        assert_eq!(card["notify"]["title"], "Time for a break");
        assert_eq!(card["sound"], "complete");
    }

    #[test]
    fn a_search_for_minutes_offers_a_session_or_a_break() {
        let found = answers("45", 0);
        let results = found["results"].as_array().unwrap();
        assert_eq!(results[0]["id"], "focus-45");
        assert_eq!(results[0]["label"], "Focus for 45 minutes");
        assert_eq!(results[1]["id"], "break-45");
        assert_eq!(results[1]["label"], "Take a 45 minute break");
        for (result, ask) in results.iter().zip([Kind::Focus, Kind::Break]) {
            let id = result["id"].as_str().unwrap();
            assert_eq!(Ask::from_id(id), Some(Ask::Start(ask, 45)));
        }
        assert_eq!(answers("1", 0)["results"][0]["label"], "Focus for 1 minute");
        assert_eq!(answers("later", 0), json!({"results": []}));
    }
}
