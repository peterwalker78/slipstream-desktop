//! The app explorer on a tapped Super. Type to search the installed apps, Settings' pages and the
//! files in the home folder, arrow keys to choose, Enter to open, Esc to close. While it's open it
//! takes every key, so nothing reaches the focused app.
//!
//! Its side column lists recent files and the system's actions: lock, quick settings and log out.
//! With a query that matches no app, Enter runs the query as a command, as Windows' Run dialog
//! does, unless a setting or a file matches it better.

mod catalog;
mod draw;
mod index;
mod results;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::renderer::{
        ImportMem, Renderer,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
        },
    },
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    apps::{self, App, Icons},
    icons,
    keys::Mods,
    motion::{HYPR, REDUCED_FADE},
    paint::{self, Painter},
    panel::{self, DESIGN_PX},
    text::{self, Face, Style},
};
use catalog::*;
use draw::*;
pub use results::System;
use results::*;

// Sizes in design pixels.
const WIDTH: f32 = 1040.0;
const TOP: f32 = 130.0;
const SEARCH_H: f32 = 85.0;
const BODY_H: f32 = 440.0;
const FOOT_H: f32 = 44.0;
const HEIGHT: f32 = SEARCH_H + 1.0 + BODY_H + FOOT_H;
/// Room around the frame for its shadow.
const MARGIN: f32 = 64.0;
const COLUMNS: usize = 4;
const ROWS: usize = 3;
/// How many rows PgUp and PgDn move.
const PAGE_ROWS: usize = 3;
const TILE_H: f32 = 128.0;
const TILE_GAP: f32 = 6.0;
const ICON: f32 = 66.0;
const ROW_H: f32 = 42.0;
/// Opening: 180 ms from 10 px higher and transparent. Closing is instant.
const OPEN: f64 = 0.18;

/// Something the explorer can open.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    App(App),
    File(PathBuf),
    /// A command typed into the search box.
    Run(String),
    /// A page of the Settings app, by its id.
    Setting(&'static str),
    System(System),
    /// A sum or a conversion worked out from the query: Enter copies it.
    Answer(crate::calc::Answer),
    /// An emoji found by a `:` query, and its name: Enter types it.
    Emoji(&'static str, &'static str),
}

pub enum Outcome {
    Nothing,
    Close,
    /// Open this, and whether to start a new instance of an app even if it's already open.
    Activate(Item, bool),
}

/// How long an answer takes to decode when it changes.
const ANSWER_DECODE: f64 = 0.32;

/// How many recent files are kept for the side column and the search.
const RECENT: usize = 24;
/// How often the home folder is walked again for the search, at most.
const REINDEX: Duration = Duration::from_secs(120);

/// The keys the explorer names, as the current keyboard style binds them. Empty for an action
/// with no key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keys {
    pub explorer: String,
    pub sheet: String,
    pub lock: String,
    pub quick: String,
    pub way_out: String,
}

impl Keys {
    /// Each key, labelled by `label`.
    pub fn new(label: impl Fn(crate::keys::Action) -> String) -> Self {
        use crate::keys::Action;
        Self {
            explorer: label(Action::Explorer),
            sheet: label(Action::ShortcutSheet),
            lock: label(Action::Lock),
            quick: label(Action::QuickSettings),
            way_out: label(Action::WayOut),
        }
    }
}

pub struct Explorer {
    open: bool,
    query: String,
    selected: usize,
    /// The first row of apps showing.
    scroll: usize,
    /// When it opened, on the animation clock.
    opened_at: f64,
    catalog: Arc<Mutex<Catalog>>,
    scanned: Option<Instant>,
    read_recent_files: RecentReader,
    walk_home: HomeWalker,
    /// When the home folder was last walked for the search.
    indexed: Option<Instant>,
    /// The app last chosen in the grid, which ← and ↑ go back to from the side column.
    grid_at: usize,
    /// Icons rasterised so far, by name and size in device pixels.
    icon_cache: HashMap<(String, u32), Option<Pixmap>>,
    shown: Option<Look>,
    buffer: Option<Painted>,
    caret: Option<draw::Caret>,
    /// The caret's corner, in logical pixels from the painted area's.
    caret_at: Point<f64, Logical>,
    /// The catalog version and icon size last asked to be warmed.
    warmed: Option<(u64, u32)>,
    /// The last Run query whose first word was sent to be looked at.
    run_asked: Option<String>,
    /// The painted area's corner and the frame, in logical pixels from the output's corner.
    origin: Point<f64, Logical>,
    frame: Rectangle<f64, Logical>,
    targets: Vec<(Item, Rectangle<f64, Logical>)>,
    pub reduced_motion: bool,
    /// The answer last shown, and when it changed, so a new one decodes into place.
    answer_since: Option<(String, f64)>,
    /// How long the shown answer has been decoding, for the paint.
    answer_age: f64,
}

impl Explorer {
    /// Paints its text again next frame: a face for characters it lacked has landed.
    pub fn forget_painted_text(&mut self) {
        self.shown = None;
    }

    pub fn new(reduced_motion: bool) -> Self {
        let mut explorer = Self::idle(reduced_motion);
        // Read the apps and the recent files now, so the first tap of Super finds them.
        explorer.rescan();
        explorer.read_recent();
        explorer.index_home();
        explorer
    }

    /// Closed, and with no apps read yet.
    fn idle(reduced_motion: bool) -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            scroll: 0,
            opened_at: 0.0,
            catalog: Arc::default(),
            scanned: None,
            read_recent_files: Arc::new(|| apps::recent_files(RECENT, |path| path.is_file())),
            walk_home: Arc::new(|| {
                std::env::var_os("HOME")
                    .filter(|home| !home.is_empty())
                    .map_or_else(Vec::new, |home| index::walk(std::path::Path::new(&home)))
            }),
            indexed: None,
            grid_at: 0,
            icon_cache: HashMap::new(),
            shown: None,
            buffer: None,
            caret: None,
            caret_at: Point::default(),
            warmed: None,
            run_asked: None,
            origin: Point::default(),
            frame: Rectangle::from_size((0.0, 0.0).into()),
            targets: Vec::new(),
            reduced_motion,
            answer_since: None,
            answer_age: f64::MAX,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, now: f64) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.scroll = 0;
        self.opened_at = now;
        self.rescan();
        // What was last read shows now. A file deleted since goes from the list once this read
        // is back, and a hung mount keeps the last list up rather than the explorer shut.
        self.read_recent();
        self.index_home();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = None;
        self.buffer = None;
        self.targets.clear();
    }

    /// Back to the row Enter lands on for what is typed now: the best match.
    fn reset_selection(&mut self) {
        self.selected = self.results().best;
        self.grid_at = 0;
    }

    /// A key while the explorer is open, with the modifiers held. `ch` is the character it types,
    /// if any; nothing types while a chord is held.
    pub fn key(&mut self, sym: Keysym, ch: Option<char>, mods: Mods) -> Outcome {
        let results = self.results();
        let apps = results.apps.len();
        let total = results.items().len();
        let at = self.selected;
        // Where ← and ↑ go back to from the side column: the app last chosen.
        let back_to_grid = self.grid_at.min(apps.saturating_sub(1));
        match sym {
            // Esc backs out one level: a query first, then the explorer.
            Keysym::Escape if !self.query.is_empty() => {
                self.query.clear();
                self.reset_selection();
                self.scroll = 0;
                return Outcome::Nothing;
            }
            Keysym::Escape => return Outcome::Close,
            Keysym::Return | Keysym::KP_Enter => {
                return results
                    .items()
                    .into_iter()
                    .nth(self.selected)
                    .map_or(Outcome::Nothing, |item| Outcome::Activate(item, mods.shift));
            }
            // Ctrl+Backspace and Ctrl+U clear the query, as in a shell or a browser's address bar.
            Keysym::BackSpace | Keysym::u if mods.ctrl => {
                self.query.clear();
                self.reset_selection();
            }
            Keysym::BackSpace => {
                self.query.pop();
                self.reset_selection();
            }
            // ← and → move along the grid's rows; → from its last column crosses to the side
            // column, and ← from there comes back.
            Keysym::Left if at >= apps && apps > 0 => self.selected = back_to_grid,
            Keysym::Left => self.selected = at.saturating_sub(1),
            Keysym::Right if at < apps => {
                if at % COLUMNS != COLUMNS - 1 && at + 1 < apps {
                    self.selected = at + 1;
                } else if total > apps {
                    self.selected = apps;
                }
            }
            Keysym::Right if apps == 0 => self.selected = at + 1,
            Keysym::Right => {}
            // Tab jumps to the next group, the grid or a section of the side column, and wraps;
            // Shift+Tab goes back one.
            Keysym::Tab | Keysym::ISO_Left_Tab if total > 0 => {
                let starts = results.group_starts();
                let group = starts.iter().rposition(|start| *start <= at).unwrap_or(0);
                let back = mods.shift || sym == Keysym::ISO_Left_Tab;
                let to = if back {
                    (group + starts.len() - 1) % starts.len()
                } else {
                    (group + 1) % starts.len()
                };
                self.selected = starts[to];
            }
            Keysym::Home => self.selected = 0,
            Keysym::End => self.selected = total.saturating_sub(1),
            // Down a row in the grid, to the last app from a row with nothing straight below,
            // and from the last row to the side column. With nothing below at all, the
            // selection stays rather than jumping sideways.
            Keysym::Down => {
                let to = if at < apps && at + COLUMNS < apps {
                    at + COLUMNS
                } else if at < apps && at / COLUMNS < (apps - 1) / COLUMNS {
                    apps - 1
                } else if at < apps {
                    apps
                } else {
                    at + 1
                };
                if to < total {
                    self.selected = to;
                }
            }
            Keysym::Up if at < apps => self.selected = at.saturating_sub(COLUMNS),
            Keysym::Up if at == apps && apps > 0 => self.selected = back_to_grid,
            Keysym::Up => self.selected = at.saturating_sub(1),
            // PgUp and PgDn move three rows, within the grid or within the side column.
            Keysym::Prior | Keysym::Next => {
                let down = sym == Keysym::Next;
                self.selected = if at < apps {
                    let step = PAGE_ROWS * COLUMNS;
                    if down {
                        (at + step).min(apps - 1)
                    } else {
                        at.saturating_sub(step)
                    }
                } else if down {
                    (at + PAGE_ROWS).min(total.saturating_sub(1))
                } else {
                    at.saturating_sub(PAGE_ROWS).max(apps)
                };
            }
            _ => match ch {
                Some(ch) if !ch.is_control() => {
                    self.query.push(ch);
                    self.reset_selection();
                }
                _ => return Outcome::Nothing,
            },
        }
        let results = self.results();
        let apps = results.apps.len();
        self.selected = self.selected.min(results.items().len().saturating_sub(1));
        if self.selected < apps {
            self.grid_at = self.selected;
            // Keep the selected app's row in view.
            let row = self.selected / COLUMNS;
            if row < self.scroll {
                self.scroll = row;
            } else if row >= self.scroll + ROWS {
                self.scroll = row + 1 - ROWS;
            }
        }
        self.scroll = self.scroll.min(apps.div_ceil(COLUMNS).saturating_sub(ROWS));
        Outcome::Nothing
    }

    /// A click, in logical pixels from the output's corner. A middle click starts a new instance
    /// of an app that's already open.
    pub fn click(&self, x: f64, y: f64, middle: bool) -> Outcome {
        if let Some((item, _)) = self.targets.iter().find(|(_, area)| area.contains((x, y))) {
            Outcome::Activate(item.clone(), middle)
        } else if self.frame.contains((x, y)) {
            Outcome::Nothing
        } else {
            Outcome::Close
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_apps(names: &[&str]) -> Explorer {
        // Never this machine's recent files.
        let explorer = Explorer {
            scanned: Some(Instant::now()),
            indexed: Some(Instant::now()),
            read_recent_files: Arc::new(Vec::new),
            walk_home: Arc::new(Vec::new),
            ..Explorer::idle(true)
        };
        explorer.catalog.lock().unwrap().apps = names
            .iter()
            .map(|name| App {
                id: name.to_lowercase(),
                name: name.to_string(),
                generic_name: String::new(),
                keywords: Vec::new(),
                exec: vec![name.to_lowercase()],
                icon: None,
                terminal: false,
                wm_class: None,
                emulates_terminal: false,
            })
            .collect();
        explorer
    }

    /// Types a query into an open explorer, a character at a time, as a person would.
    fn type_in(explorer: &mut Explorer, query: &str) {
        for ch in query.chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
    }

    #[test]
    fn a_command_is_offered_only_when_no_app_matches() {
        let mut explorer = with_apps(&["Files"]);
        explorer.open(0.0);
        type_in(&mut explorer, "files");
        // An app matched, so nothing is offered to run.
        assert!(explorer.results().run.is_none());
    }

    fn selected(explorer: &Explorer) -> Option<Item> {
        explorer
            .results()
            .items()
            .into_iter()
            .nth(explorer.selected)
    }

    #[test]
    fn open_never_waits_for_recent_files() {
        const XBEL: &str = r#"<xbel>
  <bookmark href="file:///srv/share/notes.txt" modified="2026-09-10T10:00:00Z"></bookmark>
  <bookmark href="file:///srv/share/plan.odt" modified="2026-09-11T10:00:00Z"></bookmark>
</xbel>"#;
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let mut explorer = Explorer {
            // As a file on a hung mount would: each check waits, here until the test lets go.
            read_recent_files: Arc::new(move || {
                apps::recent_in(XBEL, RECENT, |_| {
                    let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
                    true
                })
            }),
            ..with_apps(&["Firefox"])
        };
        explorer.read_recent();
        let started = Instant::now();
        explorer.open(0.0);
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(explorer.results().files.is_empty(), "nothing read yet");
        let version = explorer.catalog.lock().unwrap().version;
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while explorer.results().files.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            explorer
                .results()
                .files
                .into_iter()
                .map(|found| found.path)
                .collect::<Vec<_>>(),
            [
                PathBuf::from("/srv/share/plan.odt"),
                PathBuf::from("/srv/share/notes.txt")
            ]
        );
        assert!(
            explorer.catalog.lock().unwrap().version > version,
            "it repaints"
        );
    }

    #[test]
    fn icon_names_from_notifications_never_pile_up() {
        let mut explorer = with_apps(&[]);
        for n in 0..1000 {
            assert!(
                explorer
                    .named_icon(&format!("made-up-icon-{n}"), 48)
                    .is_none()
            );
            assert!(explorer.icon_cache.len() <= ICON_CACHE);
        }
    }

    #[test]
    fn typing_filters_and_enter_opens_the_best_match() {
        let mut explorer = with_apps(&["Dolphin", "Firefox", "Konsole"]);
        explorer.open(0.0);
        for ch in "fire".chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
        match explorer.key(Keysym::Return, None, Mods::default()) {
            Outcome::Activate(Item::App(app), false) => assert_eq!(app.name, "Firefox"),
            _ => panic!("Enter should open Firefox"),
        }
    }

    #[test]
    fn shift_enter_asks_for_a_new_window() {
        let mut explorer = with_apps(&["Foot"]);
        explorer.open(0.0);
        explorer.key(Keysym::NoSymbol, Some('f'), Mods::default());
        assert!(matches!(
            explorer.key(
                Keysym::Return,
                None,
                Mods {
                    shift: true,
                    ..Mods::default()
                }
            ),
            Outcome::Activate(Item::App(_), true)
        ));
    }

    #[test]
    fn a_query_no_app_matches_can_run_as_a_command() {
        let mut explorer = with_apps(&["Firefox"]);
        explorer.open(0.0);
        for ch in "htop -d 5".chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), Mods::default());
        }
        assert_eq!(selected(&explorer), Some(Item::Run("htop -d 5".into())));
    }

    #[test]
    fn arrows_move_through_the_grid_and_on_to_the_side_column() {
        let names: Vec<String> = (0..14).map(|i| format!("App {i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut explorer = with_apps(&names);
        explorer.open(0.0);
        explorer.key(Keysym::Right, None, Mods::default());
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 5);
        explorer.key(Keysym::Down, None, Mods::default());
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 13, "the fourth row");
        assert_eq!(explorer.scroll, 1, "scrolled to keep it in view");
        explorer.key(Keysym::Down, None, Mods::default());
        assert_eq!(explorer.selected, 14, "past the apps: the side column");
        assert_eq!(selected(&explorer), Some(Item::System(System::Lock)));
        explorer.key(Keysym::Down, None, Mods::default());
        explorer.key(Keysym::Up, None, Mods::default());
        explorer.key(Keysym::Up, None, Mods::default());
        assert_eq!(explorer.selected, 13, "back to the app it left");
    }

    #[test]
    fn right_crosses_to_the_side_column_and_left_comes_back() {
        let names: Vec<String> = (0..10).map(|i| format!("App {i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut explorer = with_apps(&names);
        explorer.open(0.0);
        explorer.key(Keysym::Down, None, NONE);
        explorer.key(Keysym::Right, None, NONE);
        explorer.key(Keysym::Right, None, NONE);
        explorer.key(Keysym::Right, None, NONE);
        assert_eq!(explorer.selected, 7, "the row's last app");
        explorer.key(Keysym::Right, None, NONE);
        assert_eq!(selected(&explorer), Some(Item::System(System::Lock)));
        explorer.key(Keysym::Down, None, NONE);
        explorer.key(Keysym::Right, None, NONE);
        assert_eq!(
            selected(&explorer),
            Some(Item::System(System::QuickSettings))
        );
        explorer.key(Keysym::Left, None, NONE);
        assert_eq!(explorer.selected, 7);
        // From a row with nothing straight below, Down goes to the last app.
        explorer.key(Keysym::Down, None, NONE);
        assert_eq!(explorer.selected, 9);
    }

    const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        logo: false,
    };

    fn typed(explorer: &mut Explorer, text: &str) {
        for ch in text.chars() {
            explorer.key(Keysym::NoSymbol, Some(ch), NONE);
        }
    }

    #[test]
    fn escape_clears_the_query_before_it_closes() {
        let mut explorer = with_apps(&["Foot", "Firefox"]);
        explorer.open(0.0);
        typed(&mut explorer, "fi");
        explorer.key(Keysym::Down, None, NONE);
        assert!(matches!(
            explorer.key(Keysym::Escape, None, NONE),
            Outcome::Nothing
        ));
        assert_eq!(
            (explorer.query.as_str(), explorer.selected, explorer.scroll),
            ("", 0, 0)
        );
        assert!(matches!(
            explorer.key(Keysym::Escape, None, NONE),
            Outcome::Close
        ));
    }

    #[test]
    fn tab_jumps_from_section_to_section_and_wraps() {
        let recent = || vec![PathBuf::from("/srv/share/notes.txt")];
        let mut explorer = Explorer {
            read_recent_files: Arc::new(recent),
            ..with_apps(&["A", "B", "C"])
        };
        explorer.catalog.lock().unwrap().recent = recent();
        explorer.open(0.0);
        explorer.key(Keysym::Right, None, NONE);
        // Three apps, a recent file and the system's rows.
        explorer.key(Keysym::Tab, None, NONE);
        assert_eq!(
            selected(&explorer),
            Some(Item::File("/srv/share/notes.txt".into()))
        );
        explorer.key(Keysym::Tab, None, NONE);
        assert_eq!(selected(&explorer), Some(Item::System(System::Lock)));
        explorer.key(Keysym::Down, None, NONE);
        explorer.key(Keysym::Tab, None, NONE);
        assert_eq!(explorer.selected, 0, "round to the grid");
        explorer.key(Keysym::ISO_Left_Tab, None, NONE);
        assert_eq!(selected(&explorer), Some(Item::System(System::Lock)));
        let shift = Mods {
            shift: true,
            ..NONE
        };
        explorer.key(Keysym::Tab, None, shift);
        assert_eq!(explorer.selected, 3, "Shift+Tab goes back too");
    }

    #[test]
    fn enter_opens_a_matching_app_before_anything_else() {
        let mut explorer = with_apps(&["Budgie Settings", "Notes", "Wallpaper Downloader"]);
        explorer.catalog.lock().unwrap().home = vec![
            index::entry("/home/sam/budget.ods", 0, false),
            index::entry("/home/sam/notes.txt", 0, false),
            index::entry("/home/sam/downloads", 0, true),
        ];
        explorer.open(0.0);
        let enter = |explorer: &mut Explorer| match explorer.key(Keysym::Return, None, NONE) {
            Outcome::Activate(item, _) => Some(item),
            _ => None,
        };
        // Only its letters are in an app's name, scattered, and a file is named for it.
        typed(&mut explorer, "budget");
        assert_eq!(
            enter(&mut explorer),
            Some(Item::File("/home/sam/budget.ods".into()))
        );
        // An app and a file of the same name: the app.
        explorer.key(Keysym::Escape, None, NONE);
        typed(&mut explorer, "notes");
        assert!(matches!(enter(&mut explorer), Some(Item::App(app)) if app.name == "Notes"));
        // A folder named for it outright, and an app with a word that starts with it: the app,
        // and Tab reaches the folder.
        explorer.key(Keysym::Escape, None, NONE);
        typed(&mut explorer, "download");
        assert!(
            matches!(enter(&mut explorer), Some(Item::App(app)) if app.name == "Wallpaper Downloader")
        );
        assert!(
            explorer
                .results()
                .items()
                .contains(&Item::File("/home/sam/downloads".into())),
            "the folder is still listed"
        );
        // A setting held on a page, ahead of running what was typed.
        explorer.key(Keysym::Escape, None, NONE);
        typed(&mut explorer, "night light");
        assert_eq!(enter(&mut explorer), Some(Item::Setting("appearance")));
        // An app's name beats a page's title.
        explorer.key(Keysym::Escape, None, NONE);
        typed(&mut explorer, "wallpaper");
        assert!(matches!(enter(&mut explorer), Some(Item::App(_))));
        assert!(
            explorer
                .results()
                .items()
                .contains(&Item::Setting("wallpaper")),
            "the page is still listed"
        );
        // The system's actions by name.
        explorer.key(Keysym::Escape, None, NONE);
        typed(&mut explorer, "lock");
        assert_eq!(enter(&mut explorer), Some(Item::System(System::Lock)));
    }

    #[test]
    fn the_side_column_never_lists_more_than_fits() {
        let mut explorer = with_apps(&["Settings Manager"]);
        explorer.catalog.lock().unwrap().home = (0..10)
            .map(|n| index::entry(&format!("/home/sam/s{n}.txt"), 0, false))
            .collect();
        explorer.open(0.0);
        typed(&mut explorer, "s");
        let results = explorer.results();
        assert!(!results.settings.is_empty() && !results.files.is_empty());
        let sections = [
            results.settings.len(),
            results.files.len(),
            results.system.len(),
        ];
        let height: f32 = sections
            .iter()
            .filter(|rows| **rows > 0)
            .map(|rows| HEADER_H + *rows as f32 * ROW_STEP)
            .sum();
        assert!(height <= BODY_H - 20.0, "{sections:?} is {height} high");
    }

    #[test]
    fn system_rows_show_the_keys_each_style_binds() {
        use crate::keys;
        use slipstream_config::KeyStyle;
        // Log out shows the key for the card it opens on, which also restarts and shuts down.
        for (style, lock, quick, log_out) in [
            (KeyStyle::Windows, "Super+L", "Super+A", "Ctrl+Alt+Del"),
            (KeyStyle::Mac, "Super+Ctrl+Q", "Super+A", "Super+Shift+Q"),
            (
                KeyStyle::Omarchy,
                "Super+Ctrl+L",
                "Super+Ctrl+A",
                "Super+Esc",
            ),
        ] {
            let bindings = keys::checked(keys::defaults(style), style);
            let shown = Keys::new(|action| {
                keys::binding_for(&bindings, action)
                    .map(|binding| keys::label(binding.mods, binding.key))
                    .unwrap_or_default()
            });
            assert_eq!(System::Lock.key(&shown).as_deref(), Some(lock), "{style:?}");
            assert_eq!(
                System::QuickSettings.key(&shown).as_deref(),
                Some(quick),
                "{style:?}"
            );
            assert_eq!(
                System::LogOut.key(&shown).as_deref(),
                Some(log_out),
                "{style:?}"
            );
            assert_eq!(System::Restart.key(&shown), None);
        }
    }

    #[test]
    fn down_with_nothing_below_stays_put() {
        let mut explorer = with_apps(&["Foot", "Foot Client", "Foot Server", "Kate"]);
        explorer.open(0.0);
        typed(&mut explorer, "foot");
        assert_eq!(
            explorer.results().items().len(),
            3,
            "one row, no side column"
        );
        explorer.key(Keysym::Down, None, NONE);
        explorer.key(Keysym::Down, None, NONE);
        assert_eq!(explorer.selected, 0);
    }

    #[test]
    fn home_end_and_page_keys_jump() {
        let names: Vec<String> = (0..30).map(|i| format!("App {i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut explorer = with_apps(&names);
        explorer.open(0.0);
        explorer.key(Keysym::End, None, NONE);
        assert_eq!(selected(&explorer), Some(Item::System(System::LogOut)));
        explorer.key(Keysym::Home, None, NONE);
        assert_eq!(explorer.selected, 0);
        explorer.key(Keysym::Right, None, NONE);
        explorer.key(Keysym::Next, None, NONE);
        assert_eq!(explorer.selected, 13, "three rows down");
        assert_eq!(explorer.scroll, 1);
        explorer.key(Keysym::Next, None, NONE);
        explorer.key(Keysym::Next, None, NONE);
        assert_eq!(explorer.selected, 29, "no further than the last app");
        explorer.key(Keysym::Prior, None, NONE);
        assert_eq!(explorer.selected, 17);
    }

    #[test]
    fn chords_never_type_and_ctrl_clears() {
        let mut explorer = with_apps(&["Foot"]);
        explorer.open(0.0);
        typed(&mut explorer, "foo");
        let ctrl = Mods { ctrl: true, ..NONE };
        explorer.key(Keysym::v, None, ctrl);
        assert_eq!(explorer.query, "foo", "Ctrl+V types nothing");
        explorer.key(Keysym::BackSpace, None, ctrl);
        assert_eq!(explorer.query, "");
        typed(&mut explorer, "fo");
        explorer.key(Keysym::u, None, ctrl);
        assert_eq!(explorer.query, "");
    }

    #[test]
    fn icons_are_rasterised_off_the_event_loop_before_the_explorer_opens() {
        let mut explorer = with_apps(&["Foot", "Kate"]);
        {
            let mut catalog = explorer.catalog.lock().unwrap();
            for app in &mut catalog.apps {
                app.icon = Some(format!("made-up-{}", app.id));
            }
            catalog.version = 1;
        }
        let foot = explorer.catalog.lock().unwrap().apps[0].clone();
        let px = (ICON * 1.25 * DESIGN_PX).round() as u32;
        assert_eq!(
            explorer.tile_icon(&foot, px),
            None,
            "nothing rasterised yet"
        );
        explorer.warm_icons(1.25);
        let deadline = Instant::now() + Duration::from_secs(5);
        while explorer.catalog.lock().unwrap().warming && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        // No theme has it, so the tile gets its lettered square, and it never looks again.
        assert_eq!(explorer.tile_icon(&foot, px), Some(None));
        let landed = explorer.catalog.lock().unwrap().tile_icons_landed;
        explorer.warm_icons(1.25);
        assert!(
            !explorer.catalog.lock().unwrap().warming,
            "nothing new to do"
        );
        assert_eq!(explorer.catalog.lock().unwrap().tile_icons_landed, landed);
        // Another scale needs another size.
        assert_eq!(explorer.tile_icon(&foot, px + 1), None);
    }

    #[test]
    fn a_run_path_that_starts_with_a_program_says_run() {
        assert_eq!(
            run_label("~/bin/tool --flag", true),
            (false, "Run “~/bin/tool --flag”".into())
        );
        assert_eq!(
            run_label("~/Documents", false),
            (true, "Open “~/Documents”".into())
        );
        assert!(!run_label("htop -d 5", false).0);
        assert!(run_label("https://example.org", false).0);
        // The look-up itself: a program file in a made-up folder.
        let dir = crate::files::test_scratch("run-label");
        let tool = dir.join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let query = format!("{} --flag", tool.display());
        let mut explorer = with_apps(&[]);
        assert!(!explorer.run_is_program(&query), "not known yet");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !explorer.run_is_program(&query) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(explorer.run_is_program(&query));
        assert!(
            !explorer.run_is_program(&dir.display().to_string()),
            "a folder opens"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
