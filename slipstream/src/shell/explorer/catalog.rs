//! The explorer's catalogue of installed apps, recent files, the home folder's files and icons,
//! read and rasterised off the event loop.

use super::*;

/// The most icons kept rasterised. Notifications can name any icon they like, so the cache starts
/// again when it fills rather than keeping every name ever sent.
pub(super) const ICON_CACHE: usize = 256;

#[derive(Default)]
pub(super) struct Catalog {
    pub(super) apps: Vec<App>,
    pub(super) icons: Icons,
    /// Recently used files that still exist, newest first.
    pub(super) recent: Vec<PathBuf>,
    /// The recent files are being read on a thread of their own.
    pub(super) reading_recent: bool,
    /// They were asked for again while being read, so that thread reads them once more.
    pub(super) recent_stale: bool,
    /// The home folder's files and folders, as last walked.
    pub(super) home: Vec<super::index::Entry>,
    /// The home folder is being walked on a thread of its own.
    pub(super) indexing: bool,
    /// Goes up with every scan, and whenever the recent or home files change, so the explorer
    /// repaints.
    pub(super) version: u64,
    /// The apps' icons, rasterised on a thread of their own at the grid's size, by icon name.
    /// `None` for a name no theme has.
    pub(super) tile_icons: HashMap<String, Option<Pixmap>>,
    /// The size they're rasterised at, in device pixels.
    pub(super) tile_px: u32,
    /// Goes up as they land, so an open explorer repaints with them.
    pub(super) tile_icons_landed: u64,
    /// The icons are being rasterised, and were asked for again meanwhile.
    pub(super) warming: bool,
    pub(super) warm_stale: bool,
    /// A Run query that starts with a path, and whether that path is a program: looked at on a
    /// thread, since the path can be on a mount that doesn't answer.
    pub(super) run_program: Option<(String, bool)>,
}

/// Reads the recent files: the list on disk in the session, a stand-in in tests.
pub(super) type RecentReader = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;
/// Walks the home folder: the real one in the session, a stand-in in tests.
pub(super) type HomeWalker = Arc<dyn Fn() -> Vec<super::index::Entry> + Send + Sync>;

impl Explorer {
    /// Whether the installed apps have been read yet. They are scanned on another thread at
    /// startup, so anything that needs to match an app id waits for this.
    pub fn apps_read(&self) -> bool {
        self.catalog.lock().unwrap().version > 0
    }

    /// The desktop entry for this app ID or X11 class. The whole entry, because putting a layout
    /// back needs its `Exec` line as well as its name.
    pub fn app_for(&self, id: &str) -> Option<App> {
        self.catalog
            .lock()
            .unwrap()
            .apps
            .iter()
            .find(|app| {
                app.id.eq_ignore_ascii_case(id)
                    || app
                        .wm_class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(id))
            })
            .cloned()
    }

    /// The desktop entry of a tray item that no window's client is known for: by its ID or title,
    /// then by the program its process runs.
    pub fn app_for_item(
        &self,
        names: &[&str],
        process: Option<&crate::tray::matcher::Process>,
    ) -> Option<App> {
        let catalog = self.catalog.lock().unwrap();
        crate::tray::matcher::entry(&catalog.apps, names, process).cloned()
    }

    /// Goes up whenever the installed apps, or the files the explorer lists, are read again.
    pub fn catalog_version(&self) -> u64 {
        self.catalog.lock().unwrap().version
    }

    /// The name of the app with this app ID or X11 class, from its desktop entry.
    pub fn app_name(&self, id: &str) -> Option<String> {
        self.app_for(id).map(|app| app.name)
    }

    /// The icon of the app with this app ID or X11 class, `px` pixels square.
    pub fn app_icon(&mut self, id: &str, px: u32) -> Option<Pixmap> {
        let app = self
            .catalog
            .lock()
            .unwrap()
            .apps
            .iter()
            .find(|app| {
                app.id.eq_ignore_ascii_case(id)
                    || app
                        .wm_class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(id))
            })
            .cloned()?;
        self.icon(&app, px)
    }

    /// Reads the recent files again on a thread of their own, never waiting for it: checking
    /// that each still exists can take as long as a hung network mount. A read already under way
    /// goes round once more when it finishes, so the newest list always lands last.
    pub fn read_recent(&self) {
        {
            let mut catalog = self.catalog.lock().unwrap();
            if catalog.reading_recent {
                catalog.recent_stale = true;
                return;
            }
            catalog.reading_recent = true;
        }
        let catalog = self.catalog.clone();
        let read = self.read_recent_files.clone();
        std::thread::spawn(move || {
            loop {
                let files = read();
                let mut catalog = catalog.lock().unwrap();
                if catalog.recent != files {
                    catalog.recent = files;
                    catalog.version += 1;
                }
                if !std::mem::take(&mut catalog.recent_stale) {
                    catalog.reading_recent = false;
                    return;
                }
            }
        });
    }

    /// Walks the home folder again on a thread of its own, at most every two minutes, never
    /// waiting for it. A walk stuck on a mount that doesn't answer keeps the last list, and no
    /// other walk starts until it's done.
    pub(super) fn index_home(&mut self) {
        if self.indexed.is_some_and(|at| at.elapsed() < REINDEX) {
            return;
        }
        {
            let mut catalog = self.catalog.lock().unwrap();
            if catalog.indexing {
                return;
            }
            catalog.indexing = true;
        }
        self.indexed = Some(Instant::now());
        let catalog = self.catalog.clone();
        let walk = self.walk_home.clone();
        std::thread::spawn(move || {
            let found = walk();
            tracing::debug!(found = found.len(), "walked the home folder");
            let mut catalog = catalog.lock().unwrap();
            if catalog.home != found {
                catalog.home = found;
                catalog.version += 1;
            }
            catalog.indexing = false;
        });
    }

    /// Apps were installed, removed or updated: read them again now, with fresh icons.
    pub fn rescan_now(&mut self) {
        self.scanned = None;
        self.icon_cache.clear();
        {
            let mut catalog = self.catalog.lock().unwrap();
            catalog.tile_icons.clear();
            catalog.tile_icons_landed += 1;
        }
        self.warmed = None;
        self.rescan();
    }

    /// Rasterises every app's icon at the grid's size for a screen at `scale`, on a thread of its
    /// own, so neither opening the explorer nor typing into it waits for SVGs. Cheap to call every
    /// frame: it only starts work when the apps or the size have changed.
    pub fn warm_icons(&mut self, scale: f64) {
        let px = (ICON * scale as f32 * DESIGN_PX).round() as u32;
        let version = self.catalog.lock().unwrap().version;
        if version == 0 || self.warmed == Some((version, px)) {
            return;
        }
        self.warmed = Some((version, px));
        {
            let mut catalog = self.catalog.lock().unwrap();
            if catalog.tile_px != px {
                catalog.tile_icons.clear();
                catalog.tile_px = px;
            }
            if catalog.warming {
                catalog.warm_stale = true;
                return;
            }
            catalog.warming = true;
        }
        let catalog = self.catalog.clone();
        std::thread::spawn(move || {
            loop {
                // What's still to do, in the order the grid shows the apps, so the first page
                // lands first. The paths are found under the lock, the pixels drawn outside it.
                let (px, todo): (u32, Vec<(String, Option<PathBuf>)>) = {
                    let catalog = catalog.lock().unwrap();
                    let mut seen = std::collections::HashSet::new();
                    let todo = apps::search(&catalog.apps, "")
                        .into_iter()
                        .filter_map(|app| app.icon.clone())
                        .filter(|name| !catalog.tile_icons.contains_key(name))
                        .filter(|name| seen.insert(name.clone()))
                        .map(|name| {
                            let path = catalog.icons.find(&name, catalog.tile_px);
                            (name, path)
                        })
                        .collect();
                    (catalog.tile_px, todo)
                };
                for batch in todo.chunks(ROWS * COLUMNS) {
                    let drawn: Vec<(String, Option<Pixmap>)> = batch
                        .iter()
                        .map(|(name, path)| {
                            let pixmap = path.as_ref().and_then(|path| paint::load_image(path, px));
                            (name.clone(), pixmap)
                        })
                        .collect();
                    let mut catalog = catalog.lock().unwrap();
                    if catalog.tile_px != px {
                        break;
                    }
                    catalog.tile_icons.extend(drawn);
                    catalog.tile_icons_landed += 1;
                }
                let mut catalog = catalog.lock().unwrap();
                if !std::mem::take(&mut catalog.warm_stale) {
                    catalog.warming = false;
                    return;
                }
            }
        });
    }

    /// An app's icon for the grid: drawn, known to be missing (`Some(None)`), or not rasterised
    /// yet (`None`).
    pub(super) fn tile_icon(&self, app: &App, px: u32) -> Option<Option<Pixmap>> {
        let Some(name) = &app.icon else {
            return Some(None);
        };
        let catalog = self.catalog.lock().unwrap();
        if catalog.tile_px != px {
            return None;
        }
        catalog.tile_icons.get(name).cloned()
    }

    /// Reads the installed apps and icon themes again in the background, at most every 30 s.
    pub(super) fn rescan(&mut self) {
        if self
            .scanned
            .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        self.scanned = Some(Instant::now());
        let catalog = self.catalog.clone();
        std::thread::spawn(move || {
            let (apps, icons) = (apps::scan(), Icons::scan());
            tracing::info!(apps = apps.len(), "read the installed apps");
            let mut catalog = catalog.lock().unwrap();
            catalog.apps = apps;
            catalog.icons = icons;
            catalog.version += 1;
        });
    }

    pub(super) fn icon(&mut self, app: &App, px: u32) -> Option<Pixmap> {
        let name = app.icon.clone()?;
        self.named_icon(&name, px)
    }

    /// An icon from the icon themes by its name (or a file's path), `px` pixels square.
    pub fn named_icon(&mut self, name: &str, px: u32) -> Option<Pixmap> {
        let key = (name.to_string(), px);
        if let Some(cached) = self.icon_cache.get(&key) {
            return cached.clone();
        }
        let path = self.catalog.lock().unwrap().icons.find(name, px);
        let pixmap = path.and_then(|path| paint::load_image(&path, px));
        if self.icon_cache.len() >= ICON_CACHE {
            self.icon_cache.clear();
        }
        self.icon_cache.insert(key, pixmap.clone());
        pixmap
    }

    /// Whether a Run query's first word is a program file, as far as is known yet: a path is
    /// looked at on a thread, and the row says Open until the answer lands and repaints it.
    pub(super) fn run_is_program(&mut self, command: &str) -> bool {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        if !matches!(
            apps::run_query(command, &home, |_| false),
            apps::Run::Path(_)
        ) {
            return false;
        }
        if let Some((query, program)) = &self.catalog.lock().unwrap().run_program
            && query == command
        {
            return *program;
        }
        if self.run_asked.as_deref() != Some(command) {
            self.run_asked = Some(command.to_string());
            let (catalog, command) = (self.catalog.clone(), command.to_string());
            std::thread::spawn(move || {
                let program = matches!(
                    apps::run_query(&command, &home, crate::launch::executable),
                    apps::Run::Command(_)
                );
                let mut catalog = catalog.lock().unwrap();
                catalog.run_program = Some((command, program));
                catalog.version += 1;
            });
        }
        false
    }
}
