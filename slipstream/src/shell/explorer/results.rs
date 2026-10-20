//! What a query finds: apps for the grid, and the side column's sections. Enter opens the top
//! app when one matches, and otherwise the best match in the side column.

use slipstream_config::{SETTINGS_PAGES, SettingsPage};

use super::{index::Found, *};

/// How many emoji a `:` query lists.
const EMOJI_SHOWN: usize = 6;
/// The most settings pages a query lists.
const SETTINGS_SHOWN: usize = 3;
/// The most files a query lists, and the recent files listed with none.
const FILES_SHOWN: usize = 4;
const RECENT_SHOWN: usize = 3;
/// The side column's height for its sections, in design pixels: the body less its padding.
const SIDE_ROOM: f32 = BODY_H - 14.0 - 6.0;
/// A section title's height, and a row's with the gap under it.
pub(super) const HEADER_H: f32 = 32.0;
pub(super) const ROW_STEP: f32 = ROW_H + 3.0;

/// The side column's system actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Lock,
    QuickSettings,
    LogOut,
    Restart,
    ShutDown,
    /// The shortcut sheet.
    Shortcuts,
    /// Settings' About page: the version, and how much Slipstream weighs.
    About,
}

impl System {
    const ALL: [System; 7] = [
        System::Lock,
        System::QuickSettings,
        System::LogOut,
        System::Restart,
        System::ShutDown,
        System::Shortcuts,
        System::About,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            System::Lock => "Lock screen",
            System::QuickSettings => "Quick settings",
            System::LogOut => "Log out",
            System::Restart => "Restart",
            System::ShutDown => "Shut down",
            System::Shortcuts => "Keyboard shortcuts",
            System::About => "About Slipstream",
        }
    }

    /// Other names it's found by.
    fn also(self) -> &'static [&'static str] {
        match self {
            System::Lock => &[],
            System::QuickSettings => &[],
            System::LogOut => &["logout", "sign out", "log off"],
            System::Restart => &["reboot"],
            System::ShutDown => &["shutdown", "power off", "turn off"],
            System::Shortcuts => &["keys", "help", "shortcuts"],
            System::About => &["version", "bloat", "weight"],
        }
    }

    /// Listed before anything is typed. The rest wait to be asked for by name; the shortcut
    /// sheet has its own hint to teach its key.
    fn always(self) -> bool {
        matches!(self, System::Lock | System::QuickSettings | System::LogOut)
    }

    pub(super) fn icon(self) -> &'static str {
        match self {
            System::Lock => icons::LOCK,
            System::QuickSettings => icons::GAUGE,
            System::Shortcuts => icons::KEYBOARD,
            System::About => icons::INFO,
            System::LogOut | System::Restart | System::ShutDown => icons::POWER,
        }
    }

    /// Its key, from the keys the explorer was given, if it has one.
    pub(super) fn key(self, keys: &Keys) -> Option<String> {
        let key = match self {
            System::Lock => &keys.lock,
            System::QuickSettings => &keys.quick,
            // The way out's key opens the card that logs out, restarts and shuts down.
            System::LogOut => &keys.way_out,
            System::Shortcuts => &keys.sheet,
            System::Restart | System::ShutDown | System::About => return None,
        };
        (!key.is_empty()).then(|| key.clone())
    }

    fn score(self, query: &str) -> Option<u8> {
        std::iter::once(self.label())
            .chain(self.also().iter().copied())
            .filter_map(|name| index::name_score(&name.to_lowercase(), query))
            .filter(|score| *score < 3)
            .min()
    }
}

/// A settings page a query found, and what on it matched.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Setting {
    pub(super) page: &'static SettingsPage,
    /// The setting that matched, or `None` when the page's own title did.
    pub(super) matched: Option<&'static str>,
    score: u8,
}

/// The settings pages that hold what `query` (lower case) names, best first, one row a page.
/// A title counts as an app's name does; a setting on the page as an app's keyword.
pub(super) fn find_settings(query: &str) -> Vec<Setting> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<Setting> = SETTINGS_PAGES
        .iter()
        .filter_map(|page| {
            if let Some(score) = index::name_score(&page.title.to_lowercase(), query)
                && score < 3
            {
                return Some(Setting {
                    page,
                    matched: None,
                    score,
                });
            }
            page.holds
                .iter()
                .find(|held| matches!(index::name_score(&held.to_lowercase(), query), Some(0 | 1)))
                .map(|held| Setting {
                    page,
                    matched: Some(held),
                    score: 2,
                })
        })
        .collect();
    found.sort_by_key(|setting| setting.score);
    found.truncate(SETTINGS_SHOWN);
    found
}

pub(super) struct Results {
    pub(super) apps: Vec<App>,
    pub(super) answer: Option<crate::calc::Answer>,
    pub(super) emoji: Vec<(&'static str, &'static str)>,
    pub(super) settings: Vec<Setting>,
    pub(super) files: Vec<Found>,
    pub(super) run: Option<String>,
    pub(super) system: Vec<System>,
    /// Where Enter lands before anything is chosen: the best match, in selection order.
    pub(super) best: usize,
}

impl Results {
    /// Everything selectable, in selection order: the grid, then the side column top to bottom.
    pub(super) fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self.apps.iter().cloned().map(Item::App).collect();
        items.extend(self.answer.clone().map(Item::Answer));
        items.extend(
            self.emoji
                .iter()
                .map(|(emoji, name)| Item::Emoji(emoji, name)),
        );
        items.extend(
            self.settings
                .iter()
                .map(|found| Item::Setting(found.page.id)),
        );
        items.extend(
            self.files
                .iter()
                .map(|found| Item::File(found.path.clone())),
        );
        items.extend(self.run.clone().map(Item::Run));
        items.extend(self.system.iter().copied().map(Item::System));
        items
    }

    /// Where each group starts in selection order, for Tab: the grid, then each section of the
    /// side column.
    pub(super) fn group_starts(&self) -> Vec<usize> {
        let sizes = [
            self.apps.len(),
            usize::from(self.answer.is_some()),
            self.emoji.len(),
            self.settings.len(),
            self.files.len(),
            usize::from(self.run.is_some()),
            self.system.len(),
        ];
        let mut starts = Vec::new();
        let mut at = 0;
        for size in sizes {
            if size > 0 {
                starts.push(at);
            }
            at += size;
        }
        starts
    }

    /// Where Enter lands: the top app when an app matches what is typed, since apps are what the
    /// explorer is for; Tab reaches the rest. Only an app with nothing but the query's letters
    /// somewhere in its name in order isn't counted, and then the strongest of each kind's
    /// first wins, on the apps' scale (doubled so a Run can sit between a keyword and a part of
    /// a name).
    fn best(&self, app_score: Option<u8>, query: &str) -> usize {
        if app_score.is_some_and(|score| score < apps::LETTERS_IN_ORDER) {
            return 0;
        }
        let mut ranked: Vec<u8> = Vec::new();
        ranked.extend(app_score.map(|score| score * 2));
        ranked.extend(self.apps.iter().skip(1).map(|_| u8::MAX));
        // A sum's answer is as good as a word of an app's name.
        ranked.extend(self.answer.as_ref().map(|_| 2));
        ranked.extend(self.emoji.iter().map(|_| 0));
        ranked.extend(self.settings.iter().map(|found| found.score * 2));
        ranked.extend(self.files.iter().map(|found| found.score * 2));
        // What is typed runs ahead of a file it is only part of the name of.
        ranked.extend(self.run.as_ref().map(|_| 5));
        ranked.extend(
            self.system
                .iter()
                .map(|action| action.score(query).map_or(u8::MAX, |score| score * 2)),
        );
        ranked
            .iter()
            .enumerate()
            .min_by_key(|(at, score)| (**score, *at))
            .map_or(0, |(at, _)| at)
    }
}

impl Explorer {
    /// The sum or conversion the query answers, on its own: what each frame needs, without
    /// searching every app for the rest of the results.
    pub(super) fn answer(&self) -> Option<crate::calc::Answer> {
        if self.query.trim_start().starts_with(':') {
            return None;
        }
        crate::calc::answer(&self.query)
    }

    pub(super) fn results(&self) -> Results {
        // `:` and a word looks for emoji and nothing else.
        if let Some(words) = self.query.trim_start().strip_prefix(':') {
            return Results {
                apps: Vec::new(),
                answer: None,
                emoji: crate::emoji::search(words, EMOJI_SHOWN),
                settings: Vec::new(),
                files: Vec::new(),
                run: None,
                system: Vec::new(),
                best: 0,
            };
        }
        let answer = self.answer();
        let query = self.query.trim().to_lowercase();
        let catalog = self.catalog.lock().unwrap();
        let apps: Vec<App> = apps::search(&catalog.apps, &query)
            .into_iter()
            .cloned()
            .collect();
        let app_score = apps.first().and_then(|app| apps::score(app, &query));
        // What is typed is offered as a command only once no app and no sum has matched.
        let run = (!query.is_empty() && apps.is_empty() && answer.is_none())
            .then(|| self.query.trim().to_string());
        let files = if query.is_empty() {
            catalog
                .recent
                .iter()
                .take(RECENT_SHOWN)
                .map(|path| Found {
                    path: path.clone(),
                    folder: false,
                    score: 0,
                })
                .collect()
        } else {
            index::search(&catalog.recent, &catalog.home, &query, FILES_SHOWN)
        };
        drop(catalog);
        let system = System::ALL
            .into_iter()
            .filter(|action| {
                if query.is_empty() {
                    action.always()
                } else {
                    action.score(&query).is_some()
                }
            })
            .collect();
        let mut results = Results {
            apps,
            answer,
            emoji: Vec::new(),
            settings: find_settings(&query),
            files,
            run,
            system,
            best: 0,
        };
        results.fit_side_column();
        results.best = results.best(app_score, &query);
        results
    }
}

impl Results {
    /// Lists fewer of the longest sections until the side column fits, worst matches first.
    fn fit_side_column(&mut self) {
        let section = |rows: usize| {
            if rows == 0 {
                0.0
            } else {
                HEADER_H + rows as f32 * ROW_STEP
            }
        };
        loop {
            // An empty files list still shows its title and a row that says so.
            let height = section(usize::from(self.answer.is_some()))
                + section(self.settings.len())
                + section(self.files.len().max(1))
                + section(usize::from(self.run.is_some()))
                + section(self.system.len());
            if height <= SIDE_ROOM {
                return;
            }
            let longest = [self.files.len(), self.settings.len(), self.system.len()];
            let most = longest.iter().copied().max().unwrap_or(0);
            if most <= 1 {
                return;
            }
            if self.files.len() == most {
                self.files.pop();
            } else if self.settings.len() == most {
                self.settings.pop();
            } else {
                self.system.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_found_by_page_and_by_what_they_hold() {
        let pages = |query: &str| -> Vec<(&str, Option<&str>)> {
            find_settings(query)
                .iter()
                .map(|found| (found.page.id, found.matched))
                .collect()
        };
        assert_eq!(pages("wallpaper"), [("wallpaper", None)]);
        assert_eq!(pages("night light"), [("appearance", Some("Night light"))]);
        assert_eq!(pages("night"), [("appearance", Some("Night light"))]);
        assert_eq!(pages("battery"), [("power", Some("Battery"))]);
        // A title ahead of a setting on another page.
        assert_eq!(
            pages("s")[..2],
            [("screens", None), ("sound", None)],
            "titles first"
        );
        assert_eq!(pages("s").len(), SETTINGS_SHOWN);
        assert!(pages("").is_empty(), "nothing before a query");
        assert!(pages("zebra").is_empty());
    }

    #[test]
    fn system_actions_are_found_by_their_everyday_names() {
        let found = |query: &str| -> Vec<System> {
            System::ALL
                .into_iter()
                .filter(|action| action.score(query).is_some())
                .collect()
        };
        assert_eq!(found("lock"), [System::Lock]);
        assert_eq!(found("quick"), [System::QuickSettings]);
        assert_eq!(found("sign out"), [System::LogOut]);
        assert_eq!(found("shut"), [System::ShutDown]);
        assert_eq!(found("reboot"), [System::Restart]);
        assert_eq!(found("help"), [System::Shortcuts]);
        for query in ["about", "version", "bloat", "weight"] {
            assert_eq!(found(query), [System::About], "{query}");
        }
        assert!(found("ock").is_empty(), "only the start of a word");
    }
}
