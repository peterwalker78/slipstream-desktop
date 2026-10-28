//! The places outside its card where a plugin can show up, each drawn by Slipstream from what the
//! plugin says, never by the plugin:
//!
//! - a tile in quick settings, for a plugin whose manifest says `tile = true`, from the `tile` on
//!   its card: `{"tile": {"id": "awake", "label": "Keep awake", "icon": "sun", "on": true}}`;
//! - answers in the explorer, for a plugin whose manifest names a `keyword`: only a search that
//!   starts with it reaches the plugin, run as `search <words>`, and it prints
//!   `{"results": [{"id": …, "label": …, "detail": …, "icon": …}]}`, and an `example` search
//!   shows in Settings;
//! - keys, for the `actions` its manifest lists (`[[actions]] id = "start" label = "Start"`),
//!   which the person binds in Settings. A plugin never chooses or grabs keys itself.
//!
//! Pressing a tile, choosing an answer or pressing an action's keys does what a button on its
//! card does: the plugin's program runs with `press` and the id.

use serde::Deserialize;

use super::{Manifest, clip, valid_id};

/// The most answers one search shows.
pub const MAX_HITS: usize = 6;
/// The most actions a plugin can offer keys for.
pub const MAX_ACTIONS: usize = 8;
/// The longest keyword, in characters.
const KEYWORD_CHARS: usize = 3;
/// What Slipstream's own search starts with: `:` for emoji, and `/`, `~` and `.` for paths.
const OWN_STARTS: [char; 4] = [':', '/', '~', '.'];
/// The most of a search passed on, in characters.
pub const SEARCH_CHARS: usize = 200;
const TILE_LABEL_CHARS: usize = 24;
const ACTION_LABEL_CHARS: usize = 40;
const HIT_LABEL_CHARS: usize = 80;
const HIT_DETAIL_CHARS: usize = 32;
const EXAMPLE_CHARS: usize = 24;

/// Something a plugin offers keys for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
}

/// A plugin's tile in quick settings, as its card last gave it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Tile {
    pub id: String,
    pub label: String,
    /// One of the names Slipstream has a picture for; any other gets a plain one.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub on: bool,
}

/// One answer to a search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hit {
    pub id: String,
    pub label: String,
    /// A word or two shown at the end of its row.
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Deserialize)]
struct Answered {
    #[serde(default)]
    results: Vec<Hit>,
}

/// Whether `keyword` can start a plugin's searches: one to three symbols, such as `=` or `?`,
/// none a letter, a digit or a space, so that no ordinary search starts with it, and not
/// starting the way Slipstream's own emoji and path searches do.
pub fn valid_keyword(keyword: &str) -> bool {
    let count = keyword.chars().count();
    (1..=KEYWORD_CHARS).contains(&count)
        && keyword.chars().all(|c| c.is_ascii_punctuation())
        && !keyword.starts_with(OWN_STARTS)
}

/// The manifest's surfaces checked: a keyword that can't be one, or an action without a usable
/// id or label, refuses it, and labels are cut to fit.
pub(super) fn check(manifest: &mut Manifest) -> Result<(), String> {
    use super::MANIFEST;
    if let Some(keyword) = &manifest.keyword
        && !valid_keyword(keyword)
    {
        return Err(format!(
            "{MANIFEST} gives it the keyword {keyword:?}: a keyword is one to three symbols, \
             such as = or ?, and can't start with :, /, ~ or ., which Slipstream's own search uses"
        ));
    }
    // An example is kept only when it is a search that would reach the plugin.
    manifest.example = manifest
        .example
        .as_deref()
        .map(|example| clip(example, EXAMPLE_CHARS))
        .filter(|example| {
            manifest
                .searched(example)
                .is_some_and(|words| !words.is_empty())
        });
    if manifest.actions.len() > MAX_ACTIONS {
        return Err(format!("{MANIFEST} lists more than {MAX_ACTIONS} actions"));
    }
    for action in &mut manifest.actions {
        action.label = clip(&action.label, ACTION_LABEL_CHARS);
        if !valid_id(&action.id) || action.label.is_empty() {
            return Err(format!(
                "{MANIFEST} lists an action {:?} without a label, or whose id isn't letters, \
                 digits, '.', '-' and '_'",
                action.id
            ));
        }
    }
    let mut ids: Vec<&str> = manifest.actions.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("{MANIFEST} lists two actions with the same id"));
    }
    Ok(())
}

/// A tile as a card printed it, kept only with an id and a label.
pub(super) fn tile(printed: Option<Tile>) -> Option<Tile> {
    let tile = printed?;
    let label = clip(&tile.label, TILE_LABEL_CHARS);
    (valid_id(&tile.id) && !label.is_empty()).then_some(Tile { label, ..tile })
}

/// The answers in what a search printed, up to `MAX_HITS`; those without an id or a label are
/// left out.
pub fn hits(text: &str) -> Result<Vec<Hit>, String> {
    let answered: Answered = serde_json::from_str(text.trim()).map_err(|err| err.to_string())?;
    Ok(answered
        .results
        .into_iter()
        .map(|hit| Hit {
            label: clip(&hit.label, HIT_LABEL_CHARS),
            detail: clip(&hit.detail, HIT_DETAIL_CHARS),
            ..hit
        })
        .filter(|hit| valid_id(&hit.id) && !hit.label.is_empty())
        .take(MAX_HITS)
        .collect())
}

/// Which of `keywords` a search typed in the explorer goes to: the longest it starts with, so a
/// plugin with `!` never sees the searches meant for one with `!!`.
pub fn claimed<'a>(query: &str, keywords: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let query = query.trim_start();
    keywords
        .into_iter()
        .filter(|keyword| query.starts_with(keyword))
        .max_by_key(|keyword| keyword.len())
}

impl Manifest {
    /// What a search typed in the explorer passes on to this plugin: the words after its
    /// keyword, when it starts with it. None for any other search, and for a plugin without one.
    pub fn searched<'a>(&self, query: &'a str) -> Option<&'a str> {
        let keyword = self.keyword.as_deref()?;
        query.trim_start().strip_prefix(keyword).map(str::trim)
    }

    /// The action with this id, if it lists one.
    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.id == id)
    }

    /// Where it shows up besides its card, in everyday words, for Settings.
    pub fn adds(&self) -> Vec<String> {
        let mut adds = Vec::new();
        if self.tile {
            adds.push("Adds a tile to quick settings".to_string());
        }
        if let Some(keyword) = &self.keyword {
            adds.push(match &self.example {
                Some(example) => {
                    format!("Answers searches that start with {keyword}, such as {example}")
                }
                None => format!("Answers searches that start with {keyword}"),
            });
        }
        if !self.actions.is_empty() {
            let labels: Vec<String> = self
                .actions
                .iter()
                .map(|action| action.label.clone())
                .collect();
            adds.push(format!(
                "Offers {} you can give keys in Shortcuts: {}",
                if labels.len() == 1 {
                    "an action"
                } else {
                    "actions"
                },
                labels.join(", ")
            ));
        }
        adds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{Card, parse_manifest};

    #[test]
    fn a_keyword_is_a_few_symbols_no_ordinary_search_starts_with() {
        for keyword in ["=", "?", "!", "#", "==", "$$$"] {
            assert!(valid_keyword(keyword), "{keyword}");
        }
        for keyword in [
            "", "w", "=a", "1", " =", "= ", "====", ":", "::", "/", "~", ".", "€",
        ] {
            assert!(!valid_keyword(keyword), "{keyword:?}");
        }
    }

    #[test]
    fn a_manifest_with_a_keyword_that_can_t_be_one_is_refused() {
        let why =
            parse_manifest(b"name = \"Emoji\"\nexec = [\"./e\"]\nkeyword = \":\"\n").unwrap_err();
        assert!(why.contains("keyword"), "{why}");
        let manifest =
            parse_manifest(b"name = \"Sums\"\nexec = [\"./s\"]\nkeyword = \"=\"\n").unwrap();
        assert_eq!(manifest.keyword.as_deref(), Some("="));
    }

    /// Settings shows an example search beside the keyword, when it is one that would reach the
    /// plugin.
    #[test]
    fn settings_shows_an_example_search_only_when_it_reaches_the_plugin() {
        let sums = |example: &str| {
            let toml = format!(
                "name = \"Sums\"\nexec = [\"./s\"]\nkeyword = \"=\"\nexample = \"{example}\"\n"
            );
            parse_manifest(toml.as_bytes()).unwrap().adds()
        };
        assert_eq!(
            sums("=2+2"),
            ["Answers searches that start with =, such as =2+2"]
        );
        for unusable in ["2+2", "=", "?2+2"] {
            assert_eq!(
                sums(unusable),
                ["Answers searches that start with ="],
                "{unusable}"
            );
        }
    }

    /// No keyword, no search: a plugin without one never sees what is typed, and one with one
    /// sees only the searches that start with it, without the keyword.
    #[test]
    fn only_searches_starting_with_its_keyword_reach_a_plugin() {
        let none = parse_manifest(b"name = \"Disk\"\nexec = [\"./d\"]\n").unwrap();
        assert_eq!(none.searched("firefox"), None);
        assert_eq!(none.searched("=2+2"), None);
        let sums = parse_manifest(b"name = \"Sums\"\nexec = [\"./s\"]\nkeyword = \"=\"\n").unwrap();
        assert_eq!(sums.searched("firefox"), None);
        assert_eq!(sums.searched("a=b"), None);
        assert_eq!(sums.searched("= 2+2 "), Some("2+2"));
        assert_eq!(sums.searched("  =pi"), Some("pi"));
        assert_eq!(sums.searched("="), Some(""));
    }

    #[test]
    fn a_search_goes_to_the_longest_keyword_it_starts_with() {
        let keywords = ["!", "!!", "="];
        assert_eq!(claimed("!! 5", keywords), Some("!!"));
        assert_eq!(claimed("  !5", keywords), Some("!"));
        assert_eq!(claimed("=!", keywords), Some("="));
        assert_eq!(claimed("firefox", keywords), None);
    }

    #[test]
    fn actions_need_an_id_and_a_label_each_and_say_where_they_show() {
        let manifest = parse_manifest(
            b"name = \"Timer\"\nexec = [\"./t\"]\ntile = true\nkeyword = \"!\"\n\
              [[actions]]\nid = \"start\"\nlabel = \"Start the timer\"\n\
              [[actions]]\nid = \"stop\"\nlabel = \"Stop it\"\n",
        )
        .unwrap();
        assert_eq!(manifest.action("stop").unwrap().label, "Stop it");
        assert_eq!(manifest.action("press"), None);
        assert_eq!(
            manifest.adds(),
            [
                "Adds a tile to quick settings",
                "Answers searches that start with !",
                "Offers actions you can give keys in Shortcuts: Start the timer, Stop it",
            ]
        );
        assert!(
            parse_manifest(b"name = \"T\"\nexec = [\"./t\"]\n")
                .unwrap()
                .adds()
                .is_empty()
        );
        for bad in [
            "[[actions]]\nid = \"st art\"\nlabel = \"Start\"\n",
            "[[actions]]\nid = \"start\"\nlabel = \" \"\n",
            "[[actions]]\nid = \"a\"\nlabel = \"A\"\n[[actions]]\nid = \"a\"\nlabel = \"B\"\n",
        ] {
            let text = format!("name = \"T\"\nexec = [\"./t\"]\n{bad}");
            assert!(parse_manifest(text.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_card_s_tile_needs_an_id_and_a_label() {
        let card = Card::parse(
            r#"{"tile": {"id": "awake", "label": "Keep awake", "icon": "sun", "on": true}}"#,
            false,
            0,
        )
        .unwrap();
        assert_eq!(
            card.tile,
            Some(Tile {
                id: "awake".into(),
                label: "Keep awake".into(),
                icon: Some("sun".into()),
                on: true,
            })
        );
        for bad in [
            r#"{"tile": {"id": "", "label": "Keep awake"}}"#,
            r#"{"tile": {"id": "awake", "label": " "}}"#,
            r#"{"tile": {"id": "a b", "label": "Keep awake"}}"#,
        ] {
            assert_eq!(Card::parse(bad, false, 0).unwrap().tile, None, "{bad}");
        }
    }

    #[test]
    fn answers_are_read_cut_to_size_and_kept_to_a_few() {
        let many: Vec<String> = (0..10)
            .map(|n| format!(r#"{{"id": "r{n}", "label": "Result {n}"}}"#))
            .collect();
        let text = format!(r#"{{"results": [{}]}}"#, many.join(","));
        assert_eq!(hits(&text).unwrap().len(), MAX_HITS);
        let found = hits(
            r#"{"results": [{"id": "four", "label": "4", "detail": "2 + 2", "icon": "tick"},
                            {"id": "bad id", "label": "x"}, {"id": "empty", "label": ""}]}"#,
        )
        .unwrap();
        assert_eq!(
            found,
            [Hit {
                id: "four".into(),
                label: "4".into(),
                detail: "2 + 2".into(),
                icon: Some("tick".into()),
            }]
        );
        assert_eq!(hits("{}").unwrap(), []);
        assert!(hits("not json").is_err());
    }
}
