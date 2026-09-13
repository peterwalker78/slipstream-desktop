//! Installed apps, for the explorer: desktop entries from the XDG data directories (Flatpak
//! exports included), their icons, ranked search, and the recently used files list.
//!
//! Parsing and ranking are pure, so they're unit-tested without a desktop.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::launch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The desktop file ID, e.g. `org.kde.dolphin`. Wayland apps usually use it as their app ID.
    pub id: String,
    pub name: String,
    pub generic_name: String,
    pub keywords: Vec<String>,
    /// The command, with field codes removed.
    pub exec: Vec<String>,
    pub icon: Option<String>,
    /// Runs inside a terminal.
    pub terminal: bool,
    /// The X11 class its windows have, when it isn't the ID.
    pub wm_class: Option<String>,
}

/// Reads a desktop entry, or `None` if it isn't an app to list on `desktops` (the names in
/// `XDG_CURRENT_DESKTOP`). `installed` checks `TryExec`.
pub fn parse(
    id: &str,
    text: &str,
    desktops: &[String],
    installed: impl Fn(&str) -> bool,
) -> Option<App> {
    let mut in_entry = false;
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }
    let get = |key: &str| fields.get(key).copied();
    let yes = |key: &str| get(key) == Some("true");
    let list = |key: &str| get(key).map(split_list).unwrap_or_default();
    if get("Type") != Some("Application") || yes("NoDisplay") || yes("Hidden") {
        return None;
    }
    let only = list("OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|desktop| desktops.contains(desktop)) {
        return None;
    }
    if list("NotShowIn")
        .iter()
        .any(|desktop| desktops.contains(desktop))
    {
        return None;
    }
    if get("TryExec").is_some_and(|program| !installed(program)) {
        return None;
    }
    let name = get("Name")?.to_string();
    let icon = get("Icon")
        .filter(|icon| !icon.is_empty())
        .map(String::from);
    let exec = command(get("Exec")?, &name, icon.as_deref());
    if exec.is_empty() {
        return None;
    }
    Some(App {
        id: id.to_string(),
        name,
        generic_name: get("GenericName").unwrap_or_default().to_string(),
        keywords: list("Keywords"),
        exec,
        icon,
        terminal: yes("Terminal"),
        wm_class: get("StartupWMClass").map(String::from),
    })
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
}

/// Splits a command line into words as a desktop entry's `Exec` is split: at unquoted
/// whitespace, with double quotes keeping a word together and a backslash inside them escaping
/// the next character. Nothing else is interpreted, since no shell is involved.
pub fn split_args(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => word.extend(chars.next()),
            ch if ch.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            ch => {
                word.push(ch);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// What the explorer's Run does with a query that matches no app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// A URL, handed to the default app for it.
    Url(String),
    /// A file or folder, with `~` expanded, handed to the default app for it.
    Path(PathBuf),
    /// A program and its arguments, started directly.
    Command(Vec<String>),
}

/// How a Run query is taken. A path (`/…`, `~` or `~/…`) or anything with `://` is opened rather
/// than run, unless its first word is a program file, which is run with the rest as arguments.
/// `is_program` says whether a path is an executable file.
pub fn run_query(query: &str, home: &Path, is_program: impl Fn(&Path) -> bool) -> Run {
    let query = query.trim();
    let expand = |text: &str| -> PathBuf {
        match text.strip_prefix('~') {
            Some("") => home.to_path_buf(),
            Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
            _ => PathBuf::from(text),
        }
    };
    if query.contains("://") {
        return Run::Url(query.to_string());
    }
    if query.starts_with('/') || query == "~" || query.starts_with("~/") {
        let mut words = split_args(query);
        if let Some(first) = words.first_mut() {
            let program = expand(first);
            if is_program(&program) {
                *first = program.to_string_lossy().into_owned();
                return Run::Command(words);
            }
        }
        return Run::Path(expand(query));
    }
    Run::Command(split_args(query))
}

/// Splits an `Exec` line into words, honouring quotes, and drops the field codes that would
/// have been files or URLs.
fn command(exec: &str, name: &str, icon: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for word in split_args(exec) {
        match word.as_str() {
            // Files and URLs, and Flatpak's markers around them.
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" | "%k" | "@@"
            | "@@u" | "@@f" => {}
            "%i" => {
                if let Some(icon) = icon {
                    out.extend(["--icon".to_string(), icon.to_string()]);
                }
            }
            _ => {
                let mut clean = String::new();
                let mut chars = word.chars();
                while let Some(ch) = chars.next() {
                    if ch != '%' {
                        clean.push(ch);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => clean.push('%'),
                        Some('c') => clean.push_str(name),
                        _ => {}
                    }
                }
                if !clean.is_empty() {
                    out.push(clean);
                }
            }
        }
    }
    out
}

pub fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
}

/// The XDG data directories in order of precedence, with the Flatpak exports even if the
/// session didn't add them.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_home()];
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    dirs.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );
    dirs.push(data_home().join("flatpak/exports/share"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.to_string_lossy().trim_end_matches('/').to_string()));
    dirs
}

/// Every folder desktop entries are read from, for watching.
pub fn application_dirs() -> Vec<PathBuf> {
    data_dirs()
        .into_iter()
        .map(|dir| dir.join("applications"))
        .collect()
}

/// Every listed app, sorted by name. Where two directories have the same ID, the earlier one
/// wins, even if it hides the app.
pub fn scan() -> Vec<App> {
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_else(|_| "KDE".to_string())
        .split(':')
        .map(String::from)
        .collect();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in data_dirs() {
        let root = dir.join("applications");
        let mut files = Vec::new();
        desktop_files(&root, &root, &mut files);
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            // Whatever sits in an applications folder with a `.desktop` name, a FIFO included,
            // mustn't stall the scan the explorer and the layout restore wait for.
            let text = crate::files::read_small(&path, crate::files::DESKTOP_ENTRY_LIMIT)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            if let Some(text) = text {
                apps.extend(parse(&id, &text, &desktops, launch::installed));
            }
        }
    }
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps
}

/// Desktop files under `dir` with their IDs: the path below `root`, `/` becoming `-`.
fn desktop_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            desktop_files(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            if let Ok(relative) = path.strip_prefix(root) {
                let id = relative
                    .with_extension("")
                    .to_string_lossy()
                    .replace('/', "-");
                out.push((id, path));
            }
        }
    }
}

/// Apps matching `query`, best first: names that start with it, then names with a word that
/// does, then generic names, keywords and IDs, then names containing it, then names with its
/// letters in order. An empty query lists every app.
pub fn search<'a>(apps: &'a [App], query: &str) -> Vec<&'a App> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return apps.iter().collect();
    }
    let mut scored: Vec<(u8, usize, &App)> = apps
        .iter()
        .filter_map(|app| score(app, &query).map(|score| (score, app.name.len(), app)))
        .collect();
    scored.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
    scored.into_iter().map(|(_, _, app)| app).collect()
}

fn score(app: &App, query: &str) -> Option<u8> {
    let words = |text: &str| {
        text.to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word.starts_with(query))
    };
    let name = app.name.to_lowercase();
    if name.starts_with(query) {
        Some(0)
    } else if words(&app.name) {
        Some(1)
    } else if words(&app.generic_name)
        || app
            .keywords
            .iter()
            .any(|keyword| keyword.to_lowercase().starts_with(query))
        || app
            .id
            .to_lowercase()
            .rsplit('.')
            .next()
            .is_some_and(|last| last.starts_with(query))
    {
        Some(2)
    } else if name.contains(query) {
        Some(3)
    } else {
        let mut letters = name.chars();
        query
            .chars()
            .all(|wanted| letters.any(|ch| ch == wanted))
            .then_some(4)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IconFile {
    /// Lower is preferred: Breeze, then hicolor, then loose pixmaps.
    theme: u8,
    /// Pixels; 0 for scalable.
    size: u32,
    path: PathBuf,
}

/// App icons by name, from the themes they're likely to be in.
#[derive(Debug, Default)]
pub struct Icons {
    files: HashMap<String, Vec<IconFile>>,
}

impl Icons {
    pub fn scan() -> Self {
        let mut icons = Self::default();
        for dir in data_dirs() {
            icons.add_theme(0, &dir.join("icons/breeze"));
            icons.add_theme(1, &dir.join("icons/hicolor"));
            icons.add_dir(2, 48, &dir.join("pixmaps"));
        }
        icons
    }

    /// Theme layouts: hicolor keeps `48x48/apps`, Breeze keeps `apps/48`.
    fn add_theme(&mut self, theme: u8, dir: &Path) {
        let Ok(outer) = fs::read_dir(dir) else {
            return;
        };
        for a in outer.flatten() {
            let a_name = a.file_name().to_string_lossy().into_owned();
            let Ok(inner) = fs::read_dir(a.path()) else {
                continue;
            };
            for b in inner.flatten() {
                let b_name = b.file_name().to_string_lossy().into_owned();
                let size = if b_name == "apps" {
                    icon_dir_size(&a_name)
                } else if a_name == "apps" {
                    icon_dir_size(&b_name)
                } else {
                    None
                };
                if let Some(size) = size {
                    self.add_dir(theme, size, &b.path());
                }
            }
        }
    }

    fn add_dir(&mut self, theme: u8, size: u32, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) else {
                continue;
            };
            let size = match ext.to_str() {
                Some("svg") => 0,
                Some("png") => size,
                _ => continue,
            };
            self.add(&stem.to_string_lossy(), theme, size, path.clone());
        }
    }

    fn add(&mut self, name: &str, theme: u8, size: u32, path: PathBuf) {
        self.files
            .entry(name.to_string())
            .or_default()
            .push(IconFile { theme, size, path });
    }

    /// The file to draw icon `name` at `px` pixels: from the most preferred theme that has it,
    /// the smallest bitmap at least that big, else a scalable one, else the biggest bitmap.
    pub fn find(&self, name: &str, px: u32) -> Option<PathBuf> {
        if name.starts_with('/') {
            let path = PathBuf::from(name);
            return path.exists().then_some(path);
        }
        let files = self.files.get(name)?;
        let theme = files.iter().map(|file| file.theme).min()?;
        let files: Vec<&IconFile> = files.iter().filter(|file| file.theme == theme).collect();
        files
            .iter()
            .filter(|file| file.size >= px)
            .min_by_key(|file| file.size)
            .or_else(|| files.iter().find(|file| file.size == 0))
            .or_else(|| files.iter().max_by_key(|file| file.size))
            .map(|file| file.path.clone())
    }
}

/// `48x48`, `48x48@2` (96 pixels), `48` or `scalable` (0).
fn icon_dir_size(name: &str) -> Option<u32> {
    if name == "scalable" {
        return Some(0);
    }
    let (base, factor) = match name.split_once('@') {
        Some((base, factor)) => (base, factor.parse().ok()?),
        None => (name, 1),
    };
    let pixels: u32 = base.split('x').next()?.parse().ok()?;
    Some(pixels * factor)
}

/// The list of recently used files GTK and KDE apps keep, in the data home.
pub const RECENT_LIST: &str = "recently-used.xbel";
/// The list is text; a long-used one is a few hundred kilobytes.
const RECENT_LIST_LIMIT: u64 = 16 << 20;

/// Recently used files that still exist, newest first, at most `limit`. `is_file` checks each
/// entry, and on a hung network mount it waits as long as the mount does, so this never runs on
/// the event loop.
pub fn recent_files(limit: usize, is_file: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    match crate::files::read_small(&data_home().join(RECENT_LIST), RECENT_LIST_LIMIT) {
        Ok(bytes) => recent_in(&String::from_utf8_lossy(&bytes), limit, is_file),
        Err(_) => Vec::new(),
    }
}

/// The entries of an `xbel` list that `is_file` says exist, newest first, at most `limit`.
pub fn recent_in(xbel: &str, limit: usize, is_file: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    parse_recent(xbel)
        .into_iter()
        .filter(|path| is_file(path))
        .take(limit)
        .collect()
}

fn parse_recent(xbel: &str) -> Vec<PathBuf> {
    let mut entries: Vec<(String, PathBuf)> = xbel
        .split("<bookmark ")
        .skip(1)
        .filter_map(|chunk| {
            let tag = &chunk[..chunk.find('>')?];
            let attribute = |name: &str| {
                let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
                let end = start + tag[start..].find('"')?;
                Some(tag[start..end].to_string())
            };
            let path = attribute("href")?.strip_prefix("file://")?.to_string();
            Some((
                attribute("modified").unwrap_or_default(),
                PathBuf::from(unescape(&path)),
            ))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries.into_iter().map(|(_, path)| path).collect()
}

/// Undoes XML entities, then URL percent-encoding.
fn unescape(text: &str) -> String {
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| (bytes.get(at).copied()? as char).to_digit(16);
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Name=Firefox
Name[de]=Feuerfuchs
GenericName=Web Browser
Keywords=Internet;WWW;Browser;
Exec=/usr/bin/flatpak run --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@
Icon=org.mozilla.firefox
Type=Application

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window
";

    fn kde() -> Vec<String> {
        vec!["Slipstream".into(), "KDE".into()]
    }

    fn app(name: &str, generic: &str, keywords: &[&str]) -> App {
        App {
            id: name.to_lowercase(),
            name: name.into(),
            generic_name: generic.into(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            exec: vec![name.to_lowercase()],
            icon: None,
            terminal: false,
            wm_class: None,
        }
    }

    #[test]
    fn reads_an_app_and_drops_field_codes() {
        let app = parse("org.mozilla.firefox", FIREFOX, &kde(), |_| true).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.generic_name, "Web Browser");
        assert_eq!(app.keywords, ["Internet", "WWW", "Browser"]);
        assert_eq!(
            app.exec,
            [
                "/usr/bin/flatpak",
                "run",
                "--command=firefox",
                "--file-forwarding",
                "org.mozilla.firefox"
            ]
        );
        assert_eq!(app.icon.as_deref(), Some("org.mozilla.firefox"));
    }

    #[test]
    fn hidden_apps_and_other_desktops_apps_are_skipped() {
        let with = |extra: &str| FIREFOX.replacen("Type=Application", extra, 1);
        let skipped = |extra: &str| {
            let text = with(&format!("Type=Application\n{extra}"));
            parse("id", &text, &kde(), |program| program != "missing").is_none()
        };
        assert!(skipped("NoDisplay=true"));
        assert!(skipped("Hidden=true"));
        assert!(skipped("OnlyShowIn=GNOME;"));
        assert!(skipped("NotShowIn=KDE;"));
        assert!(skipped("TryExec=missing"));
        assert!(!skipped("OnlyShowIn=KDE;GNOME;"));
        assert!(parse("id", &with("Type=Link"), &kde(), |_| true).is_none());
    }

    #[test]
    fn quoted_arguments_stay_together() {
        assert_eq!(
            command(
                r#""/opt/My App/run" --name "a \"b\"" %F --class=%c 100%%"#,
                "Mine",
                None
            ),
            [
                "/opt/My App/run",
                "--name",
                "a \"b\"",
                "--class=Mine",
                "100%"
            ]
        );
    }

    #[test]
    fn split_args_keeps_quoted_words() {
        assert_eq!(
            split_args(r#"notify-send "two words"  "say \"hi\"" ''"#),
            ["notify-send", "two words", "say \"hi\"", "''"]
        );
        assert_eq!(split_args(r#"echo "" end"#), ["echo", "", "end"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn a_url_is_opened_not_run() {
        let home = Path::new("/home/alex");
        assert_eq!(
            run_query(" https://example.org/a b ", home, |_| true),
            Run::Url("https://example.org/a b".into())
        );
        assert_eq!(
            run_query("qqqzz nothing", home, |_| false),
            Run::Command(vec!["qqqzz".into(), "nothing".into()])
        );
    }

    #[test]
    fn a_tilde_path_expands() {
        let home = Path::new("/home/alex");
        let no_programs = |_: &Path| false;
        assert_eq!(
            run_query("~/Downloads", home, no_programs),
            Run::Path("/home/alex/Downloads".into())
        );
        assert_eq!(run_query("~", home, no_programs), Run::Path(home.into()));
        assert_eq!(
            run_query("/srv/My Files", home, no_programs),
            Run::Path("/srv/My Files".into())
        );
        // A tilde that isn't the home folder's is left alone, and so run.
        assert_eq!(
            run_query("~sam/x", home, no_programs),
            Run::Command(vec!["~sam/x".into()])
        );
        // A program given by its path runs, with its arguments.
        let script = |path: &Path| path == Path::new("/home/alex/bin/sync");
        assert_eq!(
            run_query("~/bin/sync --all \"two words\"", home, script),
            Run::Command(vec![
                "/home/alex/bin/sync".into(),
                "--all".into(),
                "two words".into()
            ])
        );
    }

    #[test]
    fn search_ranks_prefixes_then_words_then_keywords_then_fuzzy() {
        let apps = vec![
            app("Dolphin", "File Manager", &[]),
            app("Firefox", "Web Browser", &["Internet"]),
            app("Konsole", "Terminal", &["shell"]),
            app("Wildfire Game", "", &[]),
        ];
        let names = |query| -> Vec<&str> {
            search(&apps, query)
                .iter()
                .map(|app| app.name.as_str())
                .collect()
        };
        assert_eq!(names("fi"), ["Firefox", "Dolphin", "Wildfire Game"]);
        assert_eq!(names("game"), ["Wildfire Game"]);
        assert_eq!(names("shell"), ["Konsole"]);
        assert_eq!(names("dlp"), ["Dolphin"]);
        assert_eq!(names("  ").len(), 4);
        assert!(names("xyz").is_empty());
    }

    #[test]
    fn icons_come_from_the_preferred_theme_at_the_best_size() {
        let mut icons = Icons::default();
        icons.add("app", 1, 32, "hicolor/32.png".into());
        icons.add("app", 1, 128, "hicolor/128.png".into());
        icons.add("app", 1, 0, "hicolor/app.svg".into());
        assert_eq!(icons.find("app", 48), Some("hicolor/128.png".into()));
        assert_eq!(icons.find("app", 256), Some("hicolor/app.svg".into()));
        icons.add("app", 0, 48, "breeze/48.png".into());
        assert_eq!(icons.find("app", 64), Some("breeze/48.png".into()));
        assert_eq!(icons.find("other", 64), None);
        assert_eq!(icon_dir_size("48x48@2"), Some(96));
        assert_eq!(icon_dir_size("22"), Some(22));
        assert_eq!(icon_dir_size("symbolic"), None);
    }

    #[test]
    fn recent_files_come_newest_first_with_escapes_undone() {
        let xbel = r#"<xbel>
  <bookmark added="x" href="file:///home/p/a%20b.txt" modified="2026-09-01T10:00:00Z" visited="x">
    <info><bookmark:applications/></info>
  </bookmark>
  <bookmark added="x" href="file:///home/p/c&amp;d.txt" modified="2026-09-10T10:00:00Z" visited="x">
  </bookmark>
  <bookmark added="x" href="https://example.com/" modified="2026-09-11T10:00:00Z"></bookmark>
</xbel>"#;
        assert_eq!(
            parse_recent(xbel),
            [
                PathBuf::from("/home/p/c&d.txt"),
                PathBuf::from("/home/p/a b.txt")
            ]
        );
    }
}
