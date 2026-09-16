//! Starting apps: from key bindings, using the default terminal or whichever file manager is
//! installed, and from the explorer.

use std::{
    ffi::OsStr,
    io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::keys::App;

/// XWayland's display, for the apps started from here, once it's ready.
static X_DISPLAY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_x_display(display: String) {
    *X_DISPLAY.lock().unwrap() = Some(display);
}

/// Whether commands that change the machine itself are only logged: the volume, the microphone,
/// the backlight, Wi-Fi, Bluetooth, the power mode, and Sleep, Restart and Shut down. A nested
/// compositor runs inside another desktop on the same computer, where those would act on the real
/// one. Off unless the backend sets it.
static MACHINE_COMMANDS_HELD: AtomicBool = AtomicBool::new(false);

/// Whether a compositor holds its machine commands: only when nested, and not when
/// `SLIPSTREAM_REAL_COMMANDS=1` asks for real changes on purpose.
pub fn holds_machine_commands(nested: bool, real_commands: Option<&OsStr>) -> bool {
    nested && real_commands != Some(OsStr::new("1"))
}

/// Set once, when the backend is chosen.
pub fn hold_machine_commands(held: bool) {
    MACHINE_COMMANDS_HELD.store(held, Ordering::Relaxed);
}

pub fn machine_commands_held() -> bool {
    MACHINE_COMMANDS_HELD.load(Ordering::Relaxed)
}

const TERMINALS: &[&[&str]] = &[
    &["konsole"],
    &["ptyxis"],
    &["gnome-terminal"],
    &["foot"],
    &["alacritty"],
    &["kitty"],
    &["wezterm"],
    &["ghostty"],
    &["xterm"],
];
const FILE_MANAGERS: &[&[&str]] = &[
    &["dolphin"],
    &["nautilus"],
    &["thunar"],
    &["nemo"],
    &["pcmanfm-qt"],
];
/// Why something wasn't started.
#[derive(Debug)]
pub enum Failure {
    /// Nothing is installed that could do it.
    NothingInstalled,
    /// The program is there, but starting it failed.
    Spawn(io::Error),
}

pub fn launch(app: App) -> Result<(), Failure> {
    let command = match app {
        App::Terminal => terminal(),
        App::Files => first_installed(FILE_MANAGERS),
        App::Settings => settings_app(),
        App::Browser => browser(),
    };
    match command {
        Some(command) => {
            tracing::info!(?app, ?command, "launching");
            spawn(&command).map_err(Failure::Spawn)
        }
        None => {
            tracing::warn!(?app, "nothing installed to launch");
            Err(Failure::NothingInstalled)
        }
    }
}

/// Launches the default terminal set by the Default Terminal Specification
/// (`~/.config/xdg-terminals.list`), the same choice other desktops' apps follow.
const XDG_TERMINAL_EXEC: &str = "xdg-terminal-exec";

/// `$TERMINAL`, else the desktop's default terminal through `xdg-terminal-exec`, else the first
/// terminal installed.
fn terminal() -> Option<Vec<String>> {
    std::env::var("TERMINAL")
        .ok()
        .filter(|terminal| installed(terminal))
        .map(|terminal| vec![terminal])
        .or_else(|| first_installed(&[&[XDG_TERMINAL_EXEC]]))
        .or_else(|| first_installed(TERMINALS))
}

/// `$BROWSER`, else whichever browser the desktop is set to open a web page with. Nothing is
/// preferred over anything else: this is the same choice every other app on the machine follows,
/// so Super+B opens the browser the person already chose.
fn browser() -> Option<Vec<String>> {
    std::env::var("BROWSER")
        .ok()
        .filter(|browser| installed(browser))
        .map(|browser| vec![browser])
        .or_else(default_browser)
}

/// The desktop entry ID set to open a web page, from one `mimeapps.list`. `https` wins over
/// `http`, and where several entries are listed the first is the one to use.
pub fn web_browser_id(text: &str) -> Option<String> {
    let mut secure = None;
    let mut plain = None;
    let mut in_defaults = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
            continue;
        }
        if !in_defaults || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let held = match key.trim() {
            "x-scheme-handler/https" => &mut secure,
            "x-scheme-handler/http" => &mut plain,
            _ => continue,
        };
        if held.is_none() {
            *held = value
                .split(';')
                .map(str::trim)
                .find(|id| !id.is_empty())
                .map(str::to_owned);
        }
    }
    secure.or(plain)
}

/// Where the desktop's default applications are written down, most specific first.
fn mimeapps_files() -> Vec<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let mut files = vec![config_home.join("mimeapps.list")];
    let config_dirs = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".to_string());
    files.extend(
        config_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("mimeapps.list")),
    );
    files.extend(
        crate::apps::application_dirs()
            .into_iter()
            .map(|dir| dir.join("mimeapps.list")),
    );
    files
}

/// The command of the desktop's chosen browser, wherever its entry is installed. A Flatpak is
/// started the way its own entry says to, so a browser installed that way works like any other.
fn default_browser() -> Option<Vec<String>> {
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_else(|_| "Slipstream".to_string())
        .split(':')
        .map(String::from)
        .collect();
    for file in mimeapps_files() {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Some(id) = web_browser_id(&text) else {
            continue;
        };
        let stem = id.strip_suffix(".desktop").unwrap_or(&id);
        for dir in crate::apps::application_dirs() {
            let Ok(entry) = std::fs::read_to_string(dir.join(format!("{stem}.desktop"))) else {
                continue;
            };
            if let Some(app) = crate::apps::parse(stem, &entry, &desktops, installed)
                && !app.exec.is_empty()
            {
                return Some(app.exec);
            }
        }
    }
    None
}

const SETTINGS_APP: &str = "slipstream-settings";

/// Slipstream's settings app opened on one of its pages, such as `power`.
pub fn settings_app_on(page: &str) -> Option<Vec<String>> {
    let mut command = settings_app()?;
    command.extend(["--page".to_string(), page.to_string()]);
    Some(command)
}

/// Slipstream's settings app: the one installed beside this compositor, else one on `PATH`.
fn settings_app() -> Option<Vec<String>> {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(SETTINGS_APP))
        .filter(|path| installed(&path.to_string_lossy()))
        .map(|path| vec![path.to_string_lossy().into_owned()])
        .or_else(|| first_installed(&[&[SETTINGS_APP]]))
}

/// Starts an app's command from its desktop entry. Terminal apps open inside a terminal.
pub fn app(exec: &[String], in_terminal: bool) -> Result<(), Failure> {
    if !in_terminal {
        return spawn(exec).map_err(Failure::Spawn);
    }
    let Some(command) = in_a_terminal(exec, terminal()) else {
        tracing::warn!(?exec, "no terminal installed to run it in");
        return Err(Failure::NothingInstalled);
    };
    spawn(&command).map_err(Failure::Spawn)
}

/// `exec` run inside `terminal`, told the way that terminal expects.
fn in_a_terminal(exec: &[String], terminal: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut command = terminal?;
    // How each terminal is told to run a command.
    let program = Path::new(&command[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let flag: &[&str] = match program.as_str() {
        "ptyxis" | "gnome-terminal" => &["--"],
        "wezterm" => &["start", "--"],
        "foot" | "kitty" | XDG_TERMINAL_EXEC => &[],
        _ => &["-e"],
    };
    command.extend(flag.iter().map(|part| part.to_string()));
    command.extend(exec.iter().cloned());
    Some(command)
}

/// The system settings pages quick settings' chevrons open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Network,
    Bluetooth,
}

/// A settings app to try, in order of preference.
enum Candidate {
    /// A KDE settings module, shown by `kcmshell6`.
    Kcm(&'static str),
    /// A program and its arguments.
    Program(&'static [&'static str]),
    /// A command-line tool, run inside a terminal.
    InTerminal(&'static str),
}

const NETWORK_PAGES: &[Candidate] = &[
    Candidate::Kcm("kcm_networkmanagement"),
    Candidate::Program(&["nm-connection-editor"]),
    Candidate::Program(&["gnome-control-center", "wifi"]),
    Candidate::InTerminal("nmtui"),
];

const BLUETOOTH_PAGES: &[Candidate] = &[
    Candidate::Kcm("kcm_bluetooth"),
    Candidate::Program(&["blueman-manager"]),
    Candidate::Program(&["gnome-control-center", "bluetooth"]),
    Candidate::InTerminal("bluetoothctl"),
];

/// Where installed things are looked for: the program search path, the XDG data folders (for
/// desktop files) and Qt's plugin folders (for settings modules).
struct Places {
    path: Vec<PathBuf>,
    data: Vec<PathBuf>,
    plugins: Vec<PathBuf>,
}

impl Places {
    fn from_env() -> Self {
        let split = |name: &str| {
            std::env::var_os(name)
                .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let mut plugins = split("QT_PLUGIN_PATH");
        plugins.extend(
            [
                "/usr/lib64/qt6/plugins",
                "/usr/lib/qt6/plugins",
                "/usr/lib/x86_64-linux-gnu/qt6/plugins",
                "/usr/lib/aarch64-linux-gnu/qt6/plugins",
            ]
            .map(PathBuf::from),
        );
        Self {
            path: split("PATH"),
            data: crate::apps::application_dirs()
                .into_iter()
                .filter_map(|dir| dir.parent().map(Path::to_path_buf))
                .collect(),
            plugins,
        }
    }

    fn program(&self, name: &str) -> bool {
        self.path.iter().any(|dir| executable(&dir.join(name)))
    }

    /// A KDE settings module, by its desktop file or its plugin.
    fn kcm(&self, module: &str) -> bool {
        let desktop = format!("applications/{module}.desktop");
        let plugin = format!("{module}.so");
        self.data.iter().any(|dir| dir.join(&desktop).is_file())
            || self.plugins.iter().any(|dir| {
                [
                    "plasma/kcms/systemsettings",
                    "plasma/kcms/systemsettings_qwidgets",
                ]
                .iter()
                .any(|kind| dir.join(kind).join(&plugin).is_file())
            })
    }
}

/// The command for a system settings page: the first of its apps that is installed, or `None`.
pub fn settings_page(page: Page) -> Option<Vec<String>> {
    find_settings_page(page, &Places::from_env(), terminal())
}

fn find_settings_page(
    page: Page,
    places: &Places,
    terminal: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let candidates = match page {
        Page::Network => NETWORK_PAGES,
        Page::Bluetooth => BLUETOOTH_PAGES,
    };
    candidates.iter().find_map(|candidate| match candidate {
        Candidate::Kcm(module) => (places.program("kcmshell6") && places.kcm(module))
            .then(|| strings(&["kcmshell6", module])),
        Candidate::Program(command) => places.program(command[0]).then(|| strings(command)),
        Candidate::InTerminal(tool) => places
            .program(tool)
            .then(|| in_a_terminal(&strings(&[tool]), terminal.clone()))
            .flatten(),
    })
}

/// The brightness keys' command, and where it leaves the backlight in percent, which the
/// on-screen display shows. logind lets the active session set the backlight without root.
pub fn brightness(percent: i8) -> Option<(Vec<String>, u8)> {
    let Some((name, current, max)) = backlight() else {
        tracing::warn!("no backlight to adjust");
        return None;
    };
    let step = max as i64 * percent as i64 / 100;
    let value = (current as i64 + step).clamp(dimmest(max) as i64, max as i64) as u64;
    let shown = (value * 100 + max / 2)
        .checked_div(max)
        .unwrap_or(0)
        .min(100) as u8;
    Some((set_backlight(&name, value), shown))
}

/// The command that sets the backlight to `percent` of its range, for quick settings' slider.
pub fn set_brightness(percent: u8) -> Option<Vec<String>> {
    let (name, _, max) = backlight()?;
    let value = (max * percent.min(100) as u64 / 100).clamp(dimmest(max), max);
    Some(set_backlight(&name, value))
}

/// Never fully black: a dark screen hides the fact that it's only dimmed.
fn dimmest(max: u64) -> u64 {
    (max / 100).max(1).min(max)
}

fn set_backlight(name: &str, value: u64) -> Vec<String> {
    strings(&[
        "busctl",
        "call",
        "org.freedesktop.login1",
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
        "SetBrightness",
        "ssu",
        "backlight",
        name,
        &value.to_string(),
    ])
}

/// The first backlight: its name, brightness and maximum.
pub fn backlight() -> Option<(String, u64, u64)> {
    let entry = std::fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .next()?;
    let read = |file: &str| {
        std::fs::read_to_string(entry.path().join(file))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    };
    Some((
        entry.file_name().to_string_lossy().into_owned(),
        read("brightness")?,
        read("max_brightness")?,
    ))
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn first_installed(candidates: &[&[&str]]) -> Option<Vec<String>> {
    candidates
        .iter()
        .find(|command| installed(command[0]))
        .map(|command| command.iter().map(|s| s.to_string()).collect())
}

/// `look`'s answer, run on a thread of its own, if it comes within `wait`: for a look at the disk
/// the event loop can't be left waiting on, since a hung mount never answers.
pub fn within<T: Send + 'static>(
    wait: std::time::Duration,
    look: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (answer, answered) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = answer.send(look());
    });
    answered.recv_timeout(wait).ok()
}

/// The toast for a Run path with nothing there. What was typed goes in the body, never the title:
/// titles are fixed wording, and they're logged.
pub fn nothing_at(query: &str) -> (&'static str, String) {
    (
        "Nothing there",
        format!("No file or folder at {}.", query.trim()),
    )
}

/// The toast for a Run command that couldn't start, with the program named in the body only.
pub fn couldnt_run(program: &str, err: &io::Error) -> (&'static str, String) {
    let body = if err.kind() == io::ErrorKind::NotFound {
        format!("No program called {program}.")
    } else {
        format!("{program}: {err}")
    };
    ("Couldn’t run that", body)
}

/// Whether `path` is a file anyone may execute.
pub fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn installed(program: &str) -> bool {
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(program))))
}

/// A `Command` whose child starts with no signals blocked. Every program Slipstream runs goes
/// through here.
///
/// The SIGTERM watch in `main.rs` blocks SIGTERM for the whole compositor, and a blocked signal
/// stays blocked across fork and exec, so without this every app would ignore SIGTERM for good and
/// hand that on to its own children. Anything that stops a helper with SIGTERM and waits for it
/// then hangs: `rpm-ostree status` in a terminal, for one, waits forever on polkit's `pkttyagent`.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    // SAFETY: `sigemptyset` and `sigprocmask` are async-signal-safe, which is all `pre_exec` asks.
    unsafe {
        command.pre_exec(|| {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            if libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

/// Starts `command` with its output kept out of Slipstream's log, and reaps it when it exits.
/// An error means it never started: most often, no program by that name.
pub fn spawn(command: &[String]) -> io::Result<()> {
    let Some((program, args)) = command.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an empty command",
        ));
    };
    let mut process = self::command(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(display) = X_DISPLAY.lock().unwrap().as_deref() {
        process.env("DISPLAY", display);
    }
    match process.spawn() {
        Ok(mut child) => {
            tracing::debug!(?command, "started");
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(err) => {
            tracing::warn!(?command, "couldn't launch: {err}");
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_comes_from_the_desktops_own_choice() {
        let list = "\
[Added Associations]
x-scheme-handler/https=someone-elses.desktop;

[Default Applications]
text/html=chosen.desktop
x-scheme-handler/http=second-best.desktop;fallback.desktop;
x-scheme-handler/https=chosen.desktop;also-fine.desktop;
";
        assert_eq!(
            web_browser_id(list).as_deref(),
            Some("chosen.desktop"),
            "https wins, and only the first entry of the list is used"
        );
        assert_eq!(
            web_browser_id("[Default Applications]\nx-scheme-handler/http=only.desktop;")
                .as_deref(),
            Some("only.desktop"),
            "http will do when https says nothing"
        );
        assert_eq!(
            web_browser_id("[Added Associations]\nx-scheme-handler/https=not-a-default.desktop;"),
            None,
            "an association is not a default"
        );
        assert_eq!(web_browser_id(""), None);
    }

    #[test]
    fn what_was_typed_stays_out_of_toast_titles() {
        let typed = "~/Documets/Plan.odt";
        let (title, body) = nothing_at(typed);
        assert_eq!(title, "Nothing there");
        assert!(
            body.contains(typed),
            "the path is in the body, with its case"
        );
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let (title, body) = couldnt_run("htpo", &missing);
        assert_eq!(
            (title, body.as_str()),
            ("Couldn’t run that", "No program called htpo.")
        );
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let (title, body) = couldnt_run("./tool", &denied);
        assert!(!title.contains("tool") && body.starts_with("./tool: "));
    }

    #[test]
    fn children_start_with_no_signals_blocked() {
        // As `watch_for_sigterm` leaves the compositor's threads. Only this test's thread.
        unsafe {
            let mut term: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut term);
            libc::sigaddset(&mut term, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut());
        }
        // The mask a child starts with, read by grep itself: no shell in between, since dash
        // clears the mask when it starts. Whatever else the test runner blocked is inherited
        // too, so the plain child is checked for SIGTERM's bit alone.
        let blocked = |mut process: Command| {
            let out = process
                .args(["SigBlk", "/proc/self/status"])
                .output()
                .unwrap();
            let line = String::from_utf8(out.stdout).unwrap();
            u64::from_str_radix(line.trim().trim_start_matches("SigBlk:").trim(), 16).unwrap()
        };
        let sigterm = 1u64 << (libc::SIGTERM - 1);
        assert_ne!(blocked(Command::new("grep")) & sigterm, 0);
        assert_eq!(blocked(command("grep")), 0);
    }

    #[test]
    fn the_first_settings_app_found_wins() {
        let root = crate::files::test_scratch("settings-pages");
        let bin = root.join("bin");
        let data = root.join("share");
        let plugins = root.join("plugins");
        for dir in [&bin, &data.join("applications"), &plugins] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let stub = |name: &str| {
            let path = bin.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let places = Places {
            path: vec![bin.clone()],
            data: vec![data.clone()],
            plugins: vec![plugins.clone()],
        };
        let foot = Some(strings(&["foot"]));
        let page = |page| find_settings_page(page, &places, foot.clone());

        assert_eq!(page(Page::Network), None, "nothing installed");
        stub("bluetoothctl");
        assert_eq!(
            page(Page::Bluetooth),
            Some(strings(&["foot", "bluetoothctl"])),
            "the command-line tool, in a terminal"
        );
        assert_eq!(
            find_settings_page(Page::Bluetooth, &places, None),
            None,
            "no terminal to run it in"
        );
        stub("nmtui");
        stub("gnome-control-center");
        assert_eq!(
            page(Page::Network),
            Some(strings(&["gnome-control-center", "wifi"]))
        );
        stub("nm-connection-editor");
        assert_eq!(
            page(Page::Network),
            Some(strings(&["nm-connection-editor"]))
        );
        stub("kcmshell6");
        assert_eq!(
            page(Page::Network),
            Some(strings(&["nm-connection-editor"])),
            "kcmshell6 without the module isn't enough"
        );
        std::fs::write(data.join("applications/kcm_networkmanagement.desktop"), "").unwrap();
        assert_eq!(
            page(Page::Network),
            Some(strings(&["kcmshell6", "kcm_networkmanagement"]))
        );
        let kcms = plugins.join("plasma/kcms/systemsettings");
        std::fs::create_dir_all(&kcms).unwrap();
        std::fs::write(kcms.join("kcm_bluetooth.so"), "").unwrap();
        assert_eq!(
            page(Page::Bluetooth),
            Some(strings(&["kcmshell6", "kcm_bluetooth"])),
            "or by its plugin"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_switch_is_off_by_default() {
        // No test sets the process-wide switch, so this is the state the login session starts in.
        assert!(!machine_commands_held());
        assert!(!holds_machine_commands(false, None));
        assert!(!holds_machine_commands(false, Some(OsStr::new("1"))));
        assert!(holds_machine_commands(true, None));
        assert!(holds_machine_commands(true, Some(OsStr::new("0"))));
        assert!(!holds_machine_commands(true, Some(OsStr::new("1"))));
    }
}
