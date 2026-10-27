//! Plugins: little programs of the person's own that each add a card to the plugin panel, the one
//! that drops from the chip beside the clock.
//!
//! A plugin is a folder holding a `plugin.toml`:
//!
//! ```toml
//! name = "Disk space"
//! description = "Free space on each drive"
//! exec = ["./disk.sh", "card"]   # the program and its words, run in the plugin's folder
//! every-secs = 60                # leave it out for a program that keeps running
//! needs = ["network"]            # what it reaches beyond its own folders
//! ```
//!
//! The program is run as it is written, with no shell, so what Settings shows before a plugin is
//! turned on is exactly what runs. It runs from a read-only copy of its folder (`store`), so
//! whatever it keeps goes in a folder of its own beside that copy, which it is given as
//! `SLIPSTREAM_PLUGIN_STATE`. Folders it reads from outside are named in its manifest
//! (`read = ["$XDG_CACHE_HOME/weather"]`) and given as `SLIPSTREAM_PLUGIN_READ_0`, `_1` and on.
//!
//! A plugin runs in a sandbox (`sandbox`), seeing the system's programs and libraries, its own
//! folders and the folders it reads, and nothing else of the person's: no home folder, session
//! bus, display or network unless it `needs` them (`Need`). It starts from a cleared environment:
//! only the variables that say who and where the person is, and their language, are passed on
//! (`ENVIRONMENT`). The one thing a sandbox can't give is the session bus, since whatever reaches
//! it can have the person's service manager start anything outside: a plugin that needs
//! `everything` runs with their whole session and no sandbox, and Settings says so.
//!
//! The program prints a card as one line of JSON, every time it runs or, when it keeps running,
//! whenever the card changes:
//!
//! ```json
//! {"lines": ["212 GB free"], "glance": "212 GB",
//!  "sections": [{"name": "Home", "meters": [{"label": "Used", "percent": 55}]}],
//!  "buttons": [{"id": "refresh", "label": "Refresh"}], "attention": false}
//! ```
//!
//! Every field is optional. `sections` are named groups of gauges (`card`), each with when it
//! starts over. A pressed button runs the plugin's own program
//! again with `press <id>` after it, and that is the only thing anything on a card can do: a card
//! is shown, as text and gauges in Slipstream's own style, and nothing in it is ever run. A
//! plugin that needs `notify` can put `"notify": {"title": …, "body": …}` on a card to send a
//! notification, and one that needs `notifications` is told of each one that arrives, as a line
//! of JSON on its standard input (`notification_line`).
//!
//! Plugins live in `$XDG_DATA_HOME/slipstream/plugins/<id>/` (the person's own) and in
//! `slipstream/plugins/` under each of `$XDG_DATA_DIRS` (installed for everyone). A plugin is off
//! until it is turned on (`[plugins] on`), and turning it on records the fingerprint of its folder
//! that Settings showed (`contents`), and copies the folder as it was then. A plugin whose folder
//! has changed since is held, and doesn't run, until it is turned on again: an update can't slip
//! in unseen. What it asked for is kept beside that
//! (`approval`), so a held plugin can say whether its update is like for like or asks for more.
//! That guards against a plugin changing underneath its approval, not against something already
//! running as the person, which could change the settings file as easily.

pub mod approval;
pub mod contents;
pub mod sandbox;
pub mod seccomp;
pub mod store;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::card::{Report, Section};

/// The file in a plugin's folder that says what it is.
pub const MANIFEST: &str = "plugin.toml";
/// The most lines, sections and buttons a card shows.
pub const MAX_LINES: usize = 6;
pub const MAX_SECTIONS: usize = 4;
pub const MAX_BUTTONS: usize = 3;
/// The longest a line or a glance is shown, in characters.
const LINE_CHARS: usize = 80;
const GLANCE_CHARS: usize = 12;
const NAME_CHARS: usize = 40;
const LABEL_CHARS: usize = 20;
/// A gauge this full or fuller asks for attention.
pub const ATTENTION_PERCENT: u8 = 75;
/// The largest `plugin.toml` read.
const MOST_MANIFEST: u64 = 64 << 10;
/// The environment a plugin's program is given, unless it runs as the person: who and where the
/// person is, their language, and the way to the network through a proxy. Everything else, the
/// session bus, the display and the SSH and GPG agents among it, is left out.
pub const ENVIRONMENT: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// `[plugins]` in the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Plugins {
    /// The plugins turned on, by id, in the order their cards are shown.
    pub on: Vec<String>,
    /// The plugin whose headline shows on the chip, if any.
    pub glance: Option<String>,
    /// Each plugin's fingerprint when it was turned on, by id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fingerprints: BTreeMap<String, String>,
    /// The sections of each plugin's card not to show, by plugin id and section name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub hidden: BTreeMap<String, Vec<String>>,
}

impl Plugins {
    pub fn is_on(&self, id: &str) -> bool {
        self.on.iter().any(|on| on == id)
    }

    /// Turns `id` on, with its folder's fingerprint as it is now, or off.
    pub fn set_on(&mut self, id: &str, on: Option<String>) {
        self.on.retain(|other| other != id);
        match on {
            Some(fingerprint) => {
                self.on.push(id.to_string());
                self.fingerprints.insert(id.to_string(), fingerprint);
            }
            None => {
                self.fingerprints.remove(id);
                if self.glance.as_deref() == Some(id) {
                    self.glance = None;
                }
            }
        }
    }

    /// The plugin whose headline is on the chip, while it is on.
    pub fn glance(&self) -> Option<&str> {
        self.glance.as_deref().filter(|id| self.is_on(id))
    }

    /// The sections of plugin `id`'s card not to show.
    pub fn hidden(&self, id: &str) -> &[String] {
        self.hidden.get(id).map_or(&[], Vec::as_slice)
    }

    /// Shows plugin `id`'s section `name` if it was hidden, or hides it.
    pub fn toggle_hidden(&mut self, id: &str, name: &str) {
        let list = self.hidden.entry(id.to_string()).or_default();
        if list.iter().any(|hidden| hidden == name) {
            list.retain(|hidden| hidden != name);
        } else {
            list.push(name.to_string());
        }
        self.hidden.retain(|_, list| !list.is_empty());
    }

    /// Whether `plugin` may run: it's on, and its folder is as it was when it was turned on.
    pub fn approved(&self, plugin: &Plugin) -> bool {
        plugin.too_big.is_none()
            && self.is_on(&plugin.id)
            && self.fingerprints.get(&plugin.id) == Some(&plugin.fingerprint)
    }
}

/// `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The program and the words it is run with. A program with a `/` in it is the plugin's own,
    /// found from its folder; one without is looked for on `PATH`.
    pub exec: Vec<String>,
    /// How often the program runs; none for one that keeps running and prints a line per change.
    #[serde(default)]
    pub every_secs: Option<u64>,
    /// Folders outside its own it reads, each an absolute path or one starting with one of the
    /// `$XDG_…_HOME` folders, `$XDG_RUNTIME_DIR` or `$HOME`.
    #[serde(default)]
    pub read: Vec<String>,
    /// What it reaches beyond its folders, in the sandbox or from Slipstream.
    #[serde(default)]
    pub needs: Vec<Need>,
}

/// Something a plugin reaches beyond its own folders and the folders it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Need {
    /// The internet and the local network.
    Network,
    /// The display, to open windows of its own.
    Windows,
    /// Sending notifications, from its card.
    Notify,
    /// Seeing the notifications that arrive, on its standard input.
    Notifications,
    /// Everything the person's session has, the session bus among it: no sandbox at all.
    Everything,
}

impl Need {
    pub const ALL: [Need; 5] = [
        Need::Network,
        Need::Windows,
        Need::Notify,
        Need::Notifications,
        Need::Everything,
    ];

    /// What it lets a plugin do, in everyday words, to follow "it can".
    pub fn says(self) -> &'static str {
        match self {
            Need::Network => "go online",
            Need::Windows => "open windows of its own",
            Need::Notify => "send you notifications",
            Need::Notifications => "read your notifications",
            Need::Everything => "do anything you can on this computer",
        }
    }
}

/// How much thought something a plugin can do deserves, from someone deciding whether to switch
/// it on: none, some, or the trust given to a program they run themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Care {
    Safe,
    Thought,
    Full,
}

impl Care {
    /// What it comes to, said beside the plugin's name.
    pub fn verdict(self) -> &'static str {
        match self {
            Care::Safe => "Safe",
            Care::Thought => "Worth a thought",
            Care::Full => "Full access",
        }
    }
}

/// Which kind of thing a plugin can do, for the picture beside its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Nothing,
    Files,
    Notifications,
    Online,
    Windows,
    Notify,
    Everything,
}

/// One thing a plugin can do, in everyday words, and how much thought it deserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ability {
    pub what: What,
    pub says: String,
    pub care: Care,
}

/// What a program running outside the sandbox can do, said under any ability that is one: the
/// compositor lets such a program be an input method, a virtual keyboard and a clipboard manager,
/// and nothing stops it writing a login item, so seeing keys and outliving its switch are real.
/// sudo still wants the password, but it could be caught on the way in.
pub const OUTSIDE_THE_SANDBOX: [&str; 3] = [
    "Open, change or send away any of your files, including saved passwords, browser sign-ins \
     and SSH keys",
    "See your screen and what you type and copy, and type into your other windows",
    "Set itself up to start whenever you log in, so switching it off or removing it may not \
     stop it",
];

/// What it can't do, said after that: become root by itself.
pub const NOT_AS_ROOT: &str = "It can't use sudo or act as root by itself, but it could catch \
                               your password as you type it, or type into a terminal where you've \
                               just used sudo.";

/// The folders a manifest's `read` names, as paths, every link in them followed. One that isn't
/// there is left out; one `off_limits`, or holding a socket, means the plugin doesn't run, and why.
pub fn read_folders(manifest: &Manifest) -> Result<Vec<ReadFolder>, String> {
    let mut folders = Vec::new();
    for folder in &manifest.read {
        let Some(named) = expand(folder) else {
            continue;
        };
        let Ok(path) = std::fs::canonicalize(&named) else {
            continue;
        };
        let shown = shown_folder(folder);
        if off_limits(&path) {
            return Err(format!(
                "It asks to look in {shown}, where your session keeps what lets programs reach \
                 each other"
            ));
        }
        // Plugins write in there, their own state folders. One that could change a folder it
        // reads between this check and its sandbox starting could put a link there in its place.
        let plugins = canonical(&store::root());
        if path.starts_with(&plugins) || plugins.starts_with(&path) {
            return Err(format!(
                "It asks to look in {shown}, where plugins keep their own things"
            ));
        }
        socket_free(&path).map_err(|why| format!("It asks to look in {shown}, which {why}"))?;
        folders.push(ReadFolder { named, real: path });
    }
    Ok(folders)
}

/// A folder a plugin reads: where it looks for it, as its manifest names it with the session's
/// `HOME` and the rest put in, and where that really is, every link followed. They differ where
/// the home folder is a link, as `/home` is to `/var/home` on some systems: in its sandbox the
/// folder is found where the plugin looks, while what's checked is where it really is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFolder {
    pub named: PathBuf,
    pub real: PathBuf,
}

/// The most a folder a plugin reads may hold before it is too much to check for sockets.
const MOST_IN_A_READ: usize = 4096;

/// Whether `path` holds nothing that reaches another program, which a read-only folder doesn't
/// stop: a socket, a named pipe or a device. Says what it holds otherwise. Links aren't followed,
/// since in the sandbox they lead only where it can already look.
fn socket_free(path: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::FileTypeExt;
    let mut looked = 0;
    let mut waiting = vec![path.to_path_buf()];
    while let Some(next) = waiting.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        let kind = meta.file_type();
        if kind.is_socket() || kind.is_fifo() || kind.is_block_device() || kind.is_char_device() {
            return Err("holds a way to reach another program, so it isn't given");
        }
        looked += 1;
        if looked > MOST_IN_A_READ {
            return Err("holds too much to check, so it isn't given");
        }
        if kind.is_dir()
            && let Ok(entries) = std::fs::read_dir(&next)
        {
            waiting.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    Ok(())
}

/// Where a plugin is never given a look: anything holding sockets, which a read-only folder
/// doesn't stop a program connecting to, or the system's live state. Your session's bus, sound,
/// display and keys all have theirs in `/run` and `$XDG_RUNTIME_DIR`, X11's are in `/tmp`, the
/// SSH and GPG agents can keep theirs in `~/.ssh` and `~/.gnupg` beside the keys, and `/proc`
/// lists every program's command line. A folder inside one of them, or holding one, as `/` and
/// your home folder do, is off limits.
const OFF_LIMITS: &[&str] = &[
    "/run", "/var/run", "/tmp", "/var/tmp", "/proc", "/sys", "/dev",
];

fn off_limits(path: &Path) -> bool {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let home = crate::xdg_home("HOME", "");
    // Where sockets turn up later: Flatpak's bus proxies and helpers, its apps' own folders,
    // and containers' consoles.
    let keys = [
        ".ssh",
        ".gnupg",
        ".var",
        ".cache/.dbus-proxy",
        ".cache/.flatpak-helper",
        ".local/share/containers",
    ]
    .map(|keys| home.join(keys));
    OFF_LIMITS
        .iter()
        .map(PathBuf::from)
        .chain(runtime)
        .chain(keys.iter().flat_map(|keys| [keys.clone(), canonical(keys)]))
        .any(|limit| path.starts_with(&limit) || limit.starts_with(path))
}

/// `path` with every link followed, or as it is where it can't be.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A folder `read` names as a path, its `$HOME`, `$XDG_RUNTIME_DIR` or `$XDG_…_HOME` put in.
fn expand(folder: &str) -> Option<PathBuf> {
    let (name, rest) = match folder.strip_prefix('$') {
        Some(after) => {
            let after = after.strip_prefix('{').map_or(after, |a| a);
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let rest = after[end..].strip_prefix('}').unwrap_or(&after[end..]);
            (Some(&after[..end]), rest)
        }
        None => (None, folder),
    };
    let base = match name {
        None => PathBuf::from("/"),
        Some("HOME") => crate::xdg_home("HOME", ""),
        Some("XDG_RUNTIME_DIR") => std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())?,
        Some("XDG_CONFIG_HOME") => crate::xdg_home("XDG_CONFIG_HOME", ".config"),
        Some("XDG_DATA_HOME") => crate::xdg_home("XDG_DATA_HOME", ".local/share"),
        Some("XDG_CACHE_HOME") => crate::xdg_home("XDG_CACHE_HOME", ".cache"),
        Some("XDG_STATE_HOME") => crate::xdg_home("XDG_STATE_HOME", ".local/state"),
        Some(_) => return None,
    };
    let path = base.join(rest.trim_start_matches('/'));
    (path.is_absolute() && !rest.split('/').any(|part| part == "..")).then_some(path)
}

/// A folder `read` names as a person would write it: `~/.cache/weather` rather than
/// `$XDG_CACHE_HOME/weather`.
pub fn shown_folder(folder: &str) -> String {
    let Some(path) = expand(folder) else {
        return folder.to_string();
    };
    let home = crate::xdg_home("HOME", "");
    match path.strip_prefix(&home) {
        Ok(rest) if home != Path::new("/") && rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) if home != Path::new("/") => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// `items` as a list in a sentence: "a", "a and b", "a, b and c".
pub fn listed(items: &[String]) -> String {
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

impl Manifest {
    pub fn needs(&self, need: Need) -> bool {
        self.needs.contains(&need)
    }

    /// It runs with the person's whole session, and no sandbox.
    pub fn unconfined(&self) -> bool {
        self.needs(Need::Everything)
    }

    /// What it can do, each in everyday words with how much thought it deserves, for Settings to
    /// show before it is turned on.
    pub fn abilities(&self) -> Vec<Ability> {
        if self.unconfined() {
            return vec![Ability {
                what: What::Everything,
                says: "Runs outside the sandbox, as you, so it can:".into(),
                care: Care::Full,
            }];
        }
        let mut can = Vec::new();
        let mut add = |what, says: String, care| can.push(Ability { what, says, care });
        if !self.read.is_empty() {
            let folders: Vec<String> = self.read.iter().map(|f| shown_folder(f)).collect();
            add(
                What::Files,
                format!("Looks at the files in {}", listed(&folders)),
                Care::Safe,
            );
        }
        if self.needs(Need::Notifications) {
            add(
                What::Notifications,
                "Reads your notifications, which can hold messages and sign-in codes".into(),
                Care::Thought,
            );
        }
        if self.needs(Need::Network) {
            // Online it shares the computer's own network, so it can reach the programs here
            // that take connections, games' and printers' among them; with something of yours
            // to see, it could also send it away.
            if self.read.is_empty() && !self.needs(Need::Notifications) {
                add(
                    What::Online,
                    "Goes online, and can reach programs on this computer that take connections"
                        .into(),
                    Care::Thought,
                );
            } else {
                add(
                    What::Online,
                    "Goes online, so what it sees could leave this computer".into(),
                    Care::Thought,
                );
            }
        }
        if self.needs(Need::Windows) {
            add(What::Windows, "Opens windows of its own".into(), Care::Safe);
        }
        if self.needs(Need::Notify) {
            add(What::Notify, "Sends you notifications".into(), Care::Safe);
        }
        if can.is_empty() {
            can.push(Ability {
                what: What::Nothing,
                says: "Keeps to itself: it can't see your files or go online".into(),
                care: Care::Safe,
            });
        }
        can
    }

    /// `every_secs`, kept between a second and a day.
    pub fn every(&self) -> Option<std::time::Duration> {
        self.every_secs
            .map(|secs| std::time::Duration::from_secs(secs.clamp(1, 86_400)))
    }
}

/// A plugin found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    /// Its folder's name.
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// In the person's own folder, so it can be removed from Settings.
    pub own: bool,
    /// What its folder holds now (`contents`); empty when it is too big to check.
    pub fingerprint: String,
    /// Each file's own fingerprint, by its path in the folder.
    pub files: BTreeMap<String, String>,
    /// Why its folder is too big to check, if it is: it can't be turned on.
    pub too_big: Option<String>,
}

/// Whether `id` can name a plugin: a folder name of letters, digits, `.`, `-` and `_`, starting
/// with a letter or digit.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The person's own plugins folder.
pub fn own_dir() -> PathBuf {
    crate::xdg_home("XDG_DATA_HOME", ".local/share")
        .join("slipstream")
        .join("plugins")
}

/// Every folder plugins are looked for in, the person's own first.
pub fn dirs() -> Vec<PathBuf> {
    let shared = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(own_dir())
        .chain(
            shared
                .split(':')
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("slipstream").join("plugins")),
        )
        .collect()
}

/// Reads the plugin in `dir`, or says why it isn't one. Its folder's limits are checked from
/// metadata first: one past them is read no further than its manifest, and can't be turned on.
pub fn read(dir: &Path) -> Result<Plugin, String> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| valid_id(id))
        .ok_or("its folder's name can only have letters, digits, '.', '-' and '_'")?
        .to_string();
    let listed = match contents::list(dir) {
        Ok(_) => None,
        Err(contents::Unchecked::TooBig(why)) => Some(why),
        Err(contents::Unchecked::NotAPlugin(why)) => return Err(why),
    };
    let (manifest, fingerprint, files, too_big) = match listed {
        None => match checked(dir) {
            Ok((found, manifest)) => (manifest?, found.fingerprint, found.files, None),
            Err(contents::Unchecked::TooBig(why)) => {
                let manifest = read_manifest(&dir.join(MANIFEST))?;
                (manifest, String::new(), BTreeMap::new(), Some(why))
            }
            Err(contents::Unchecked::NotAPlugin(why)) => return Err(why),
        },
        Some(why) => {
            let manifest = read_manifest(&dir.join(MANIFEST))?;
            (manifest, String::new(), BTreeMap::new(), Some(why))
        }
    };
    Ok(Plugin {
        id,
        dir: dir.to_path_buf(),
        manifest,
        own: dir.starts_with(own_dir()),
        fingerprint,
        files,
        too_big,
    })
}

/// What `dir` holds, and its manifest read from the very bytes its fingerprint was taken from, so
/// what is shown and run is exactly what was approved, whatever changes in the folder meanwhile.
pub(crate) fn checked(
    dir: &Path,
) -> Result<(contents::Contents, Result<Manifest, String>), contents::Unchecked> {
    let (found, bytes) = contents::contents_keeping(dir, Some((MANIFEST, MOST_MANIFEST)))?;
    let manifest = match bytes {
        Some(bytes) => parse_manifest(&bytes),
        None if found.files.contains_key(MANIFEST) => Err(too_long()),
        None => Err(format!("no {MANIFEST}")),
    };
    Ok((found, manifest))
}

fn too_long() -> String {
    format!("{MANIFEST} is longer than {} KB", MOST_MANIFEST >> 10)
}

/// The manifest at `path`: a file of its own, not a link, and small.
fn read_manifest(path: &Path) -> Result<Manifest, String> {
    use std::io::Read;
    let (file, _) = contents::open_regular(path).map_err(|err| format!("no {MANIFEST}: {err}"))?;
    let mut bytes = Vec::new();
    file.take(MOST_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("{MANIFEST}: {err}"))?;
    if bytes.len() as u64 > MOST_MANIFEST {
        return Err(too_long());
    }
    parse_manifest(&bytes)
}

/// A manifest from its bytes, refused when it names no plugin or no program, or a program of its
/// own outside its folder.
fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let text = std::str::from_utf8(bytes).map_err(|err| format!("{MANIFEST}: {err}"))?;
    let mut manifest: Manifest =
        toml::from_str(text).map_err(|err| format!("{MANIFEST}: {err}"))?;
    manifest.name = clip(&manifest.name, NAME_CHARS);
    manifest.description = clip(&manifest.description, LINE_CHARS);
    if manifest.name.is_empty() {
        return Err(format!("{MANIFEST} gives it no name"));
    }
    let Some(program) = manifest
        .exec
        .first()
        .filter(|program| !program.trim().is_empty())
    else {
        return Err(format!("{MANIFEST} gives it no program to run"));
    };
    if program.contains('/')
        && (program.starts_with('/') || program.split('/').any(|part| part == ".."))
    {
        return Err(format!(
            "{MANIFEST} runs {program}, outside the plugin's folder: a program of its own is \
             named from its folder, without '..', and any other by its name alone"
        ));
    }
    if let Some(folder) = manifest.read.iter().find(|folder| {
        folder.contains("XDG_RUNTIME_DIR") || expand(folder).is_some_and(|path| off_limits(&path))
    }) {
        return Err(format!(
            "{MANIFEST} asks to look in {folder}, where your session keeps what lets programs \
             reach each other: no plugin is given that"
        ));
    }
    Ok(manifest)
}

/// Every plugin in `dirs`, by name, with why the folders that aren't plugins were left out. Where
/// two folders share an id, the first in `dirs` wins.
pub fn find_in(dirs: &[PathBuf]) -> (Vec<Plugin>, Vec<String>) {
    let mut found: Vec<Plugin> = Vec::new();
    let mut problems = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            match read(&folder) {
                Ok(plugin) if found.iter().any(|other| other.id == plugin.id) => {}
                Ok(plugin) => found.push(plugin),
                Err(why) => problems.push(format!("{}: {why}", folder.display())),
            }
        }
    }
    found.sort_by_key(|plugin| plugin.manifest.name.to_lowercase());
    (found, problems)
}

/// Every plugin there is.
pub fn find() -> (Vec<Plugin>, Vec<String>) {
    find_in(&dirs())
}

/// Moves a `[meter] command` from a settings file from before plugins into a plugin of its own,
/// once: the first read of the settings file that finds one writes it out as `meter/` in the
/// person's plugins folder, where it waits to be turned on like any other plugin. It happens when
/// the file is first read, by the compositor or the Settings app, so the command is out before
/// either writes the file back without it. Says where it went, or why it couldn't go, when there
/// was one to move.
pub fn move_meter_once() -> Option<Result<PathBuf, String>> {
    static MOVED: std::sync::OnceLock<Option<Result<PathBuf, String>>> = std::sync::OnceLock::new();
    MOVED
        .get_or_init(|| {
            let record = crate::xdg_home("XDG_STATE_HOME", ".local/state")
                .join("slipstream")
                .join("meter-moved");
            move_meter(&crate::path(), &own_dir(), &record)
        })
        .clone()
}

/// The most folders tried for a moved `[meter]`: `meter`, then `meter-2` and on.
const MOST_METER_FOLDERS: usize = 9;

/// `move_meter_once`, reading `settings`, writing into `plugins`, and keeping `record` to say it
/// has. The command runs as `sh -c <command> meter`, so a button's words reach it as `$1` and `$2`
/// and nothing is added to the line it was written as. A folder already holding just this plugin
/// is taken as the move done; one holding anything else is left alone, and the next free name
/// is used. The record is only kept once the command is in a plugin.
pub fn move_meter(
    settings: &Path,
    plugins: &Path,
    record: &Path,
) -> Option<Result<PathBuf, String>> {
    if record.exists() {
        return None;
    }
    let text = std::fs::read_to_string(settings).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let meter = table.get("meter")?.as_table()?;
    let command = meter.get("command")?.as_str()?.trim();
    if command.is_empty() {
        return None;
    }
    let every = meter
        .get("every-secs")
        .and_then(toml::Value::as_integer)
        .unwrap_or(60)
        .clamp(10, 86_400);
    let mut manifest = toml::Table::new();
    manifest.insert("name".into(), "Meter".into());
    manifest.insert(
        "description".into(),
        "The command from the settings file's [meter]".into(),
    );
    manifest.insert(
        "exec".into(),
        toml::Value::Array(vec![
            "sh".into(),
            "-c".into(),
            command.into(),
            "meter".into(),
        ]),
    );
    manifest.insert("every-secs".into(), every.into());
    // The person's own command, which ran with everything they have: it still does.
    manifest.insert(
        "needs".into(),
        toml::Value::Array(vec!["everything".into()]),
    );
    let manifest = match toml::to_string(&manifest) {
        Ok(manifest) => manifest,
        Err(err) => return Some(Err(err.to_string())),
    };
    let moved = (1..=MOST_METER_FOLDERS).find_map(|n| {
        let dir = match n {
            1 => plugins.join("meter"),
            n => plugins.join(format!("meter-{n}")),
        };
        write_meter(&dir, &manifest).transpose()
    });
    // Tried again at the next start, when it couldn't be written.
    let moved = match moved {
        Some(Ok(dir)) => dir,
        Some(Err(why)) => return Some(Err(why)),
        None => {
            return Some(Err(format!(
                "{} already holds {MOST_METER_FOLDERS} other meters",
                plugins.display()
            )));
        }
    };
    if let Some(parent) = record.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(record, "");
    Some(Ok(moved))
}

/// Writes a plugin of `manifest` alone into the new folder `dir`. `None` when `dir` holds
/// something else already; the folder when it now holds the plugin, or held it before.
fn write_meter(dir: &Path, manifest: &str) -> Result<Option<PathBuf>, String> {
    let fail = |err: std::io::Error| format!("{}: {err}", dir.display());
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let holds_only_it = std::fs::read_dir(dir).is_ok_and(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name())
                    .eq([std::ffi::OsString::from(MANIFEST)])
            }) && std::fs::read_to_string(dir.join(MANIFEST))
                .is_ok_and(|was| was == manifest);
            return Ok(holds_only_it.then(|| dir.to_path_buf()));
        }
        Err(err) => return Err(fail(err)),
    }
    if let Err(err) = std::fs::write(dir.join(MANIFEST), manifest) {
        let _ = std::fs::remove_dir_all(dir);
        return Err(fail(err));
    }
    Ok(Some(dir.to_path_buf()))
}

/// A button on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub id: String,
    pub label: String,
}

/// What a plugin shows, as its program last printed it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Card {
    pub lines: Vec<String>,
    pub sections: Vec<Section>,
    pub buttons: Vec<Button>,
    /// The headline the plugin gave for the chip, if it gave one (`glance`).
    pub headline: Option<String>,
    /// The plugin says something wants looking at (`attention`).
    pub flagged: bool,
    /// A notification to send as this card arrives, for a plugin that needs `notify`.
    pub notify: Option<Notify>,
}

/// A notification a plugin sends from its card.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Notify {
    pub title: String,
    #[serde(default)]
    pub body: String,
}

const TITLE_CHARS: usize = 80;
const BODY_CHARS: usize = 400;

/// The line a plugin that needs `notifications` reads on its standard input when one arrives:
/// `{"notification": {"app": …, "title": …, "body": …, "time": …}}`, the time in seconds since
/// the Unix epoch.
pub fn notification_line(app: &str, title: &str, body: &str, time: i64) -> String {
    let notification = serde_json::json!({
        "notification": {
            "app": clip(app, TITLE_CHARS),
            "title": clip(title, TITLE_CHARS),
            "body": clip(body, BODY_CHARS),
            "time": time,
        }
    });
    format!("{notification}\n")
}

/// A plugin's headline on the chip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Glance {
    pub text: String,
    /// Up to two gauges drawn before the text, in percent.
    pub gauges: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct Printed {
    #[serde(flatten)]
    report: Report,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    buttons: Vec<PrintedButton>,
    #[serde(default)]
    glance: Option<String>,
    #[serde(default)]
    attention: bool,
    #[serde(default)]
    notify: Option<Notify>,
}

#[derive(Debug, Deserialize)]
struct PrintedButton {
    id: String,
    label: String,
}

/// `text` cut to `chars` characters, on one line.
fn clip(text: &str, chars: usize) -> String {
    let line: String = text
        .chars()
        .filter(|c| !hides(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    match line.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line.to_string(),
    }
}

/// Characters that change how the text around them shows without showing themselves: the ones
/// that turn it round, as the right-to-left override does, and those of no width. Taken out of
/// anything a plugin prints, so what it says reads as what it is.
fn hides(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

impl Card {
    /// The card in a line a plugin printed, at `now` in seconds since the Unix epoch. `stale`
    /// marks its numbers old.
    pub fn parse(text: &str, stale: bool, now: i64) -> Result<Card, String> {
        let printed: Printed = serde_json::from_str(text.trim()).map_err(|err| err.to_string())?;
        let mut sections = printed.report.reading(stale, now).sections;
        sections.truncate(MAX_SECTIONS);
        let buttons = printed
            .buttons
            .into_iter()
            .filter(|button| valid_id(&button.id) && !button.label.trim().is_empty())
            .take(MAX_BUTTONS)
            .map(|button| Button {
                id: button.id,
                label: clip(&button.label, LABEL_CHARS),
            })
            .collect();
        Ok(Card {
            lines: printed
                .lines
                .iter()
                .map(|line| clip(line, LINE_CHARS))
                .filter(|line| !line.is_empty())
                .take(MAX_LINES)
                .collect(),
            sections,
            buttons,
            headline: printed
                .glance
                .filter(|glance| !glance.trim().is_empty())
                .map(|text| clip(&text, GLANCE_CHARS)),
            flagged: printed.attention,
            notify: printed
                .notify
                .filter(|notify| !notify.title.trim().is_empty())
                .map(|notify| Notify {
                    title: clip(&notify.title, TITLE_CHARS),
                    body: clip(&notify.body, BODY_CHARS),
                }),
        })
    }

    /// The card with its sections called one of `hidden` left out.
    pub fn without(mut self, hidden: &[String]) -> Card {
        self.sections
            .retain(|section| !hidden.contains(&section.name));
        self
    }

    /// What the chip shows for it, when it is the one chosen: the headline it gave, else its
    /// first section's first two gauges and the larger of their percentages.
    pub fn glance(&self) -> Glance {
        if let Some(text) = &self.headline {
            return Glance {
                text: text.clone(),
                gauges: Vec::new(),
            };
        }
        let gauges: Vec<u8> = self
            .sections
            .iter()
            .find(|section| !section.gauges.is_empty())
            .map(|section| section.gauges.iter().take(2).map(|g| g.percent).collect())
            .unwrap_or_default();
        Glance {
            text: gauges
                .iter()
                .max()
                .map(|percent| format!("{percent}%"))
                .unwrap_or_default(),
            gauges,
        }
    }

    /// Whether something on it wants looking at: the plugin says so, or a gauge is past three
    /// quarters.
    pub fn attention(&self) -> bool {
        self.flagged
            || self
                .sections
                .iter()
                .flat_map(|section| &section.gauges)
                .any(|gauge| gauge.percent >= ATTENTION_PERCENT)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    /// A folder of its own for one test, under the workspace's `target/test-scratch`, never in
    /// `/tmp`, which no plugin may read.
    fn scratch(name: &str) -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()
            .unwrap_or(manifest)
            .join("target/test-scratch")
            .join(format!("plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plugin(root: &Path, id: &str, manifest: &str) {
        std::fs::create_dir_all(root.join(id)).unwrap();
        std::fs::write(root.join(id).join(MANIFEST), manifest).unwrap();
    }

    /// A program of the plugin's own can't lead out of its approved copy, and Settings says why.
    #[test]
    fn a_program_outside_the_plugin_s_folder_is_refused() {
        let root = scratch("outside");
        for (id, program) in [
            ("rooted", "/usr/bin/env"),
            ("climbing", "./bin/../../timer"),
        ] {
            plugin(
                &root,
                id,
                &format!("name = \"Timer\"\nexec = [\"{program}\"]\n"),
            );
            let why = read(&root.join(id)).unwrap_err();
            assert!(why.contains("outside the plugin's folder"), "{why}");
        }
        plugin(
            &root,
            "inside",
            "name = \"Timer\"\nexec = [\"./bin/timer\"]\n",
        );
        read(&root.join("inside")).unwrap();
    }

    #[test]
    fn plugins_are_found_by_name_and_the_person_s_own_win() {
        let own = scratch("own");
        let shared = scratch("shared");
        plugin(&own, "timer", "name = \"Timer\"\nexec = [\"./timer\"]\n");
        plugin(
            &shared,
            "timer",
            "name = \"Shared timer\"\nexec = [\"timer\"]\n",
        );
        plugin(
            &shared,
            "disk",
            "name = \"Disk space\"\nexec = [\"df\"]\nevery-secs = 60\n",
        );
        plugin(&shared, "broken", "name = \"\"\nexec = [\"x\"]\n");
        std::fs::create_dir_all(shared.join("bad name")).unwrap();
        let (found, problems) = find_in(&[own.clone(), shared.clone()]);
        let names: Vec<_> = found.iter().map(|p| p.manifest.name.as_str()).collect();
        assert_eq!(names, ["Disk space", "Timer"]);
        assert_eq!(
            found[0].manifest.every(),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(found[1].manifest.every(), None);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    /// What Settings shows before a plugin is turned on: what it can do in everyday words, and
    /// what deserves a thought. Going online is harmless with nothing of yours to send; with
    /// your files or notifications to see, it's worth a thought; running unsandboxed is full
    /// access.
    #[test]
    fn a_manifest_says_what_its_plugin_can_do_and_how_safe_that_is() {
        let can = |text: &str| -> Vec<(String, Care)> {
            let manifest = parse_manifest(text.as_bytes()).unwrap();
            manifest
                .abilities()
                .into_iter()
                .map(|a| (a.says, a.care))
                .collect()
        };
        let bare = "name = \"Disk\"\nexec = [\"./disk\"]\n";
        assert_eq!(
            can(bare),
            [(
                "Keeps to itself: it can't see your files or go online".into(),
                Care::Safe
            )]
        );
        assert_eq!(
            can(&format!("{bare}needs = [\"windows\", \"notify\"]\n")),
            [
                ("Opens windows of its own".into(), Care::Safe),
                ("Sends you notifications".into(), Care::Safe)
            ]
        );
        assert_eq!(
            can(&format!("{bare}needs = [\"network\"]\n")),
            [(
                "Goes online, and can reach programs on this computer that take connections".into(),
                Care::Thought
            )]
        );
        assert_eq!(
            can(&format!(
                "{bare}needs = [\"network\"]\nread = [\"$HOME/notes\", \"$HOME/Music\"]\n"
            )),
            [
                (
                    "Looks at the files in ~/notes and ~/Music".into(),
                    Care::Safe
                ),
                (
                    "Goes online, so what it sees could leave this computer".into(),
                    Care::Thought
                )
            ]
        );
        let history = can(&format!("{bare}needs = [\"notifications\"]\n"));
        assert_eq!(history[0].1, Care::Thought);
        let full = can(&format!("{bare}needs = [\"everything\", \"network\"]\n"));
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].1, Care::Full);
        assert_eq!(Care::Full.verdict(), "Full access");
        let unknown = parse_manifest(format!("{bare}needs = [\"camera\"]\n").as_bytes());
        assert!(unknown.unwrap_err().contains("camera"));
    }

    /// A card's notification is only taken with a title, and cut to size.
    #[test]
    fn a_card_can_carry_a_notification() {
        let card = Card::parse(
            r#"{"notify": {"title": "Tea's ready", "body": "Steeped"}}"#,
            false,
            0,
        )
        .unwrap();
        let notify = card.notify.unwrap();
        assert_eq!(
            (notify.title.as_str(), notify.body.as_str()),
            ("Tea's ready", "Steeped")
        );
        assert!(
            Card::parse(r#"{"notify": {"title": " "}}"#, false, 0)
                .unwrap()
                .notify
                .is_none()
        );
        let turned = Card::parse("{\"lines\": [\"safe\\u202etxt.exe\"]}", false, 0).unwrap();
        assert_eq!(turned.lines, ["safetxt.exe"], "text can't be turned round");
        let line = notification_line("Mail", "Ferry\ndelayed", &"x".repeat(1000), 7);
        let read: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(read["notification"]["title"], "Ferry delayed");
        assert_eq!(read["notification"]["time"], 7);
        assert!(line.ends_with('\n') && line.len() < 600, "{}", line.len());
    }

    #[test]
    fn the_folders_a_plugin_reads_are_put_in_and_nothing_else_is() {
        let expanded: Vec<Option<PathBuf>> = [
            "$HOME/notes",
            "${XDG_CACHE_HOME}/weather",
            "/srv/shared",
            "relative",
            "$SSH_AUTH_SOCK",
            "$HOME/../root",
        ]
        .into_iter()
        .map(expand)
        .collect();
        assert!(expanded[0].as_ref().unwrap().ends_with("notes"));
        assert!(expanded[0].as_ref().unwrap().is_absolute());
        assert!(expanded[1].as_ref().unwrap().ends_with("weather"));
        assert_eq!(expanded[2], Some(PathBuf::from("/srv/shared")));
        assert_eq!(expanded[3], Some(PathBuf::from("/relative")));
        assert_eq!(expanded[4..], [None, None]);
    }

    /// The promise in the guide: a plugin is never given a look where your session keeps its
    /// sockets, whatever its manifest says, nor through a link that leads there.
    #[test]
    fn a_plugin_never_reads_where_sockets_live() {
        let root = scratch("off-limits");
        let notes = root.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let sneaky = root.join("sneaky");
        std::os::unix::fs::symlink("/run", &sneaky).unwrap();
        let manifest = |read: &[&str]| Manifest {
            name: "x".into(),
            description: String::new(),
            exec: vec!["x".into()],
            every_secs: None,
            read: read.iter().map(|folder| folder.to_string()).collect(),
            needs: Vec::new(),
        };
        let (notes_s, sneaky_s) = (notes.display().to_string(), sneaky.display().to_string());
        assert_eq!(
            read_folders(&manifest(&[&notes_s, "/no/such/folder"])).unwrap(),
            [ReadFolder {
                named: notes.clone(),
                real: std::fs::canonicalize(&notes).unwrap()
            }]
        );
        // Named through a link, it's found where the plugin looks, and checked where it is.
        let linked = root.join("linked");
        std::os::unix::fs::symlink(&notes, &linked).unwrap();
        let linked_s = linked.display().to_string();
        let through = read_folders(&manifest(&[&linked_s])).unwrap();
        assert_eq!(through[0].named, linked);
        assert_eq!(through[0].real, std::fs::canonicalize(&notes).unwrap());
        for refused in [sneaky_s.as_str(), "/", "/proc"] {
            let why = read_folders(&manifest(&[&notes_s, refused])).unwrap_err();
            assert!(why.contains("where your session keeps"), "{refused}: {why}");
        }
        // A socket of an app's own, somewhere a plugin may otherwise look.
        let app = root.join("app");
        std::fs::create_dir_all(app.join("deep")).unwrap();
        // Made with mknod, since a scratch path can be longer than a socket address allows.
        let sock = std::ffi::CString::new(app.join("deep/sock").into_os_string().into_vec());
        // SAFETY: a valid C string for a path; this only creates a file.
        let made = unsafe { libc::mknod(sock.unwrap().as_ptr(), libc::S_IFSOCK | 0o600, 0) };
        assert_eq!(made, 0, "{}", std::io::Error::last_os_error());
        let app_s = app.display().to_string();
        let why = read_folders(&manifest(&[&app_s])).unwrap_err();
        assert!(why.contains("a way to reach another program"), "{why}");
        // A named pipe is a way to reach a program too, read-only folder or not.
        let piped = root.join("piped");
        std::fs::create_dir_all(&piped).unwrap();
        let fifo = std::ffi::CString::new(piped.join("pipe").into_os_string().into_vec()).unwrap();
        // SAFETY: a valid C string for a path; this only creates a file.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let piped_s = piped.display().to_string();
        let why = read_folders(&manifest(&[&piped_s])).unwrap_err();
        assert!(why.contains("a way to reach another program"), "{why}");
        // Where the SSH and GPG agents can listen, and so the home folder holding them.
        for keys in ["$HOME/.ssh", "$HOME/.gnupg/private-keys-v1.d", "$HOME"] {
            let text = format!("name = \"x\"\nexec = [\"x\"]\nread = [\"{keys}\"]\n");
            assert!(parse_manifest(text.as_bytes()).is_err(), "{keys}");
        }

        for refused in [
            "/",
            "/run/user/1000",
            "/var/run",
            "/tmp/.X11-unix",
            "/proc",
            "/var",
        ] {
            let text = format!("name = \"x\"\nexec = [\"x\"]\nread = [\"{refused}\"]\n");
            // `/var` holds `/var/run`, and `/` holds them all.
            assert!(parse_manifest(text.as_bytes()).is_err(), "{refused}");
        }
        let runtime = "name = \"x\"\nexec = [\"x\"]\nread = [\"$XDG_RUNTIME_DIR/bus\"]\n";
        assert!(
            parse_manifest(runtime.as_bytes())
                .unwrap_err()
                .contains("$XDG_RUNTIME_DIR")
        );
        let fine = format!("name = \"x\"\nexec = [\"x\"]\nread = [\"{notes_s}\"]\n");
        assert!(parse_manifest(fine.as_bytes()).is_ok());
    }

    #[test]
    fn a_card_reads_with_everything_left_out() {
        assert_eq!(Card::parse("{}", false, 0).unwrap(), Card::default());
        assert!(Card::parse("not json", false, 0).is_err());
    }

    #[test]
    fn a_card_s_glance_comes_from_its_gauges_unless_it_gives_one() {
        let card = Card::parse(
            r#"{"sections": [{"name": "Allotment", "meters": [
                {"label": "Session", "percent": 62}, {"label": "Week", "percent": 81}]}]}"#,
            false,
            0,
        )
        .unwrap();
        assert_eq!(card.glance().gauges, [62, 81]);
        assert_eq!(card.glance().text, "81%");
        assert!(
            card.attention(),
            "a gauge past three quarters wants looking at"
        );
        let hidden = card.clone().without(&["Allotment".to_string()]);
        assert_eq!(
            hidden.glance().text,
            "",
            "a hidden section isn't on the chip"
        );
        assert!(!hidden.attention(), "nor does it want looking at");
        let card = Card::parse(r#"{"glance": "18:42 left on the clock"}"#, false, 0).unwrap();
        assert_eq!(card.glance().text, "18:42 left o…");
        assert!(card.glance().gauges.is_empty());
        assert!(!card.attention());
    }

    /// The promise in the guide: a card is only ever shown. Its buttons carry nothing but an id,
    /// which is handed back to the plugin's own program, and an id that could be read as anything
    /// else is dropped.
    #[test]
    fn a_card_carries_no_command_only_ids_for_its_own_program() {
        let card = Card::parse(
            r#"{"buttons": [{"id": "pause", "label": "Pause"},
                            {"id": "$(rm -rf ~)", "label": "Oops"},
                            {"id": "a b", "label": "Space"},
                            {"id": "skip", "label": "Skip", "exec": "rm -rf ~"}],
                "lines": ["one\ntwo", "", "three"], "exec": "rm -rf ~"}"#,
            false,
            0,
        )
        .unwrap();
        let ids: Vec<_> = card.buttons.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["pause", "skip"]);
        assert_eq!(card.lines, ["one two", "three"]);
    }

    #[test]
    fn hiding_a_section_is_kept_for_each_plugin() {
        let mut plugins = Plugins::default();
        plugins.toggle_hidden("tides", "Harbour");
        plugins.toggle_hidden("kettle", "Harbour");
        assert_eq!(plugins.hidden("tides"), ["Harbour"]);
        plugins.toggle_hidden("tides", "Harbour");
        assert!(plugins.hidden("tides").is_empty());
        assert_eq!(plugins.hidden("kettle"), ["Harbour"]);
        plugins.toggle_hidden("kettle", "Harbour");
        assert!(plugins.hidden.is_empty(), "no empty lists left in the file");
    }

    #[test]
    fn turning_a_plugin_off_takes_its_glance_too() {
        let mut plugins = Plugins::default();
        plugins.set_on("timer", Some("a".into()));
        plugins.set_on("disk", Some("b".into()));
        plugins.set_on("timer", Some("c".into()));
        assert_eq!(plugins.on, ["disk", "timer"]);
        assert_eq!(plugins.fingerprints["timer"], "c");
        plugins.glance = Some("timer".into());
        plugins.set_on("timer", None);
        assert_eq!(plugins.on, ["disk"]);
        assert_eq!(plugins.glance, None);
        assert!(!plugins.fingerprints.contains_key("timer"));
    }

    /// The promise in the guide: a plugin that changes after it was turned on doesn't run until
    /// it is turned on again.
    #[test]
    fn a_plugin_changed_since_it_was_turned_on_is_held() {
        let root = scratch("changed");
        plugin(&root, "timer", "name = \"Timer\"\nexec = [\"./timer\"]\n");
        std::fs::write(root.join("timer").join("timer"), "#!/bin/sh\necho {}\n").unwrap();
        let first = read(&root.join("timer")).unwrap();
        let mut plugins = Plugins::default();
        plugins.set_on("timer", Some(first.fingerprint.clone()));
        assert!(plugins.approved(&first));
        std::fs::write(
            root.join("timer").join("timer"),
            "#!/bin/sh\necho changed\n",
        )
        .unwrap();
        let changed = read(&root.join("timer")).unwrap();
        assert_ne!(changed.fingerprint, first.fingerprint);
        assert!(!plugins.approved(&changed));
        std::fs::write(root.join("timer").join("timer"), "#!/bin/sh\necho {}\n").unwrap();
        assert!(plugins.approved(&read(&root.join("timer")).unwrap()));
        {
            use std::os::unix::fs::PermissionsExt;
            let program = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(root.join("timer").join("timer"), program).unwrap();
        }
        assert!(
            !plugins.approved(&read(&root.join("timer")).unwrap()),
            "made a program since: held too"
        );
        std::fs::create_dir_all(root.join("timer").join("lib")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("timer").join("lib").join("x"))
            .unwrap();
        assert!(read(&root.join("timer")).unwrap_err().contains("link"));
    }

    /// The promise in the guide: a plugin too big to check is listed, and can't be turned on.
    #[test]
    fn a_plugin_too_big_to_check_can_t_be_turned_on() {
        let root = scratch("too-big");
        plugin(&root, "atlas", "name = \"Atlas\"\nexec = [\"./atlas\"]\n");
        std::fs::File::create(root.join("atlas").join("maps"))
            .unwrap()
            .set_len(100 << 30)
            .unwrap();
        let (found, problems) = find_in(std::slice::from_ref(&root));
        assert!(problems.is_empty(), "{problems:?}");
        let atlas = &found[0];
        assert_eq!(atlas.manifest.name, "Atlas");
        assert!(atlas.too_big.as_deref().unwrap().contains("MB"));
        let mut plugins = Plugins::default();
        plugins.set_on("atlas", Some(atlas.fingerprint.clone()));
        assert!(!plugins.approved(atlas));
    }

    /// The promise in the guide: a `[meter]` from before plugins becomes a plugin of its own,
    /// once, and waits to be turned on.
    #[test]
    fn a_meter_command_becomes_a_plugin_folder_once() {
        let root = scratch("meter");
        let settings = root.join("settings.toml");
        let record = root.join("state").join("meter-moved");
        let own = root.join("plugins");
        std::fs::write(
            &settings,
            "[meter]\ncommand = \"if true; then printf '{}'; fi # done\"\nevery-secs = 120\n",
        )
        .unwrap();
        let moved = move_meter(&settings, &own, &record).unwrap().unwrap();
        let meter = read(&moved).unwrap();
        assert_eq!(meter.id, "meter");
        assert_eq!(
            meter.manifest.exec,
            ["sh", "-c", "if true; then printf '{}'; fi # done", "meter"]
        );
        assert_eq!(meter.manifest.every_secs, Some(120));
        let parsed = crate::read(&settings).unwrap();
        assert!(
            !parsed.plugins.is_on("meter"),
            "it waits to be turned on, like any plugin"
        );
        std::fs::remove_dir_all(&moved).unwrap();
        assert_eq!(move_meter(&settings, &own, &record), None);
        assert!(!moved.exists(), "a second start leaves it removed");
    }

    /// A `meter/` that's already there is never written over: one holding another plugin sends
    /// the command to `meter-2`, and one holding just this command is the move done.
    #[test]
    fn a_meter_command_never_overwrites_a_folder_already_there() {
        let root = scratch("meter-taken");
        let settings = root.join("settings.toml");
        let own = root.join("plugins");
        std::fs::write(&settings, "[meter]\ncommand = \"uptime\"\n").unwrap();
        plugin(&own, "meter", "name = \"Gauge\"\nexec = [\"./gauge\"]\n");
        let record = root.join("moved-beside");
        let moved = move_meter(&settings, &own, &record).unwrap().unwrap();
        assert_eq!(moved, own.join("meter-2"));
        assert_eq!(read(&moved).unwrap().manifest.exec[2], "uptime");
        assert_eq!(read(&own.join("meter")).unwrap().manifest.name, "Gauge");
        assert!(record.exists());

        let record = root.join("moved-already");
        let again = move_meter(&settings, &own, &record).unwrap().unwrap();
        assert_eq!(
            again, moved,
            "the same command, already moved, isn't moved twice"
        );
        assert!(!own.join("meter-3").exists());
        assert!(record.exists());
    }

    #[test]
    fn a_settings_file_from_before_plugins_still_loads() {
        let settings: crate::Settings = toml::from_str(
            "[meter]\ncommand = \"quota\"\nhidden = [\"Mail\"]\n\n[plugins]\nglance = \"\"\n",
        )
        .unwrap();
        assert_eq!(
            settings.plugins.glance(),
            None,
            "an empty glance is nothing on the chip"
        );
    }

    #[test]
    fn an_empty_meter_command_moves_nothing() {
        let root = scratch("no-meter");
        let settings = root.join("settings.toml");
        std::fs::write(&settings, "[meter]\ncommand = \"\"\n").unwrap();
        let record = root.join("meter-moved");
        assert_eq!(move_meter(&settings, &root.join("plugins"), &record), None);
        assert!(!root.join("plugins").exists());
    }
}
