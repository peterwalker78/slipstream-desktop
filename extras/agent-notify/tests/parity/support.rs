//! A scratch machine for each test: its own home, config, cache and runtime folders, fake agents
//! and tools on its PATH, and a record of the notifications that would have been sent.
//!
//! Every test runs against the binary. With `AGENT_NOTIFY_SCRIPT` naming the shell script this
//! program replaced (it needs jq), each also runs against the script, and the two must leave the
//! same files and send the same notifications, apart from the differences named where they are
//! allowed.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The process ID the hooks are told is the agent's, unless a test walks a real process tree.
pub const AGENT_PID: &str = "4242";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Imp {
    Binary,
    Script,
}

impl Imp {
    fn program(self) -> PathBuf {
        match self {
            Imp::Binary => PathBuf::from(env!("CARGO_BIN_EXE_agent-notify")),
            Imp::Script => {
                PathBuf::from(std::env::var_os("AGENT_NOTIFY_SCRIPT").expect("checked before"))
            }
        }
    }
}

/// The implementations to check: the binary, and the script when it is named.
pub fn implementations() -> Vec<Imp> {
    let mut all = vec![Imp::Binary];
    if std::env::var_os("AGENT_NOTIFY_SCRIPT").is_some() {
        all.push(Imp::Script);
    }
    all
}

/// One notification call.
#[derive(Debug, PartialEq, Clone)]
pub enum Call {
    Notify {
        app: String,
        replaces: u32,
        summary: String,
        body: String,
        hints: BTreeMap<String, String>,
    },
    Close(u32),
}

/// A notification with the hints spelled out as pairs.
pub fn notify<K: AsRef<str>, V: AsRef<str>>(
    app: &str,
    replaces: u32,
    summary: &str,
    body: &str,
    hints: &[(K, V)],
) -> Call {
    Call::Notify {
        app: app.into(),
        replaces,
        summary: summary.into(),
        body: body.into(),
        hints: hints
            .iter()
            .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned()))
            .collect(),
    }
}

pub const QUIET: &[(&str, &str)] = &[("urgency", "y 1"), ("suppress-sound", "b true")];

pub struct Scratch {
    pub imp: Imp,
    pub root: PathBuf,
    /// Extra variables for every command.
    vars: std::cell::RefCell<Vec<(String, String)>>,
}

/// A shell script that writes its arguments to `log`, each ended by \x1f, and the run by \x1e.
fn recorder(log: &Path, then: &str) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\037' \"$0\" \"$@\" >>'{}'\nprintf '\\036' >>'{}'\n{then}\n",
        log.display(),
        log.display()
    )
}

impl Scratch {
    pub fn new(test: &str, imp: Imp) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("parity")
            .join(format!("{test}-{imp:?}"));
        let _ = fs::remove_dir_all(&root);
        for dir in ["home", "config", "cache", "state", "run", "bin", "logs"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        symlink(imp.program(), root.join("bin/agent-notify")).unwrap();
        let scratch = Scratch {
            imp,
            root,
            vars: Default::default(),
        };
        // The script talks to the bus through busctl; the binary records instead. A
        // notification's id is how many have been sent, as the binary's record numbers them.
        let log = scratch.root.join("logs/busctl");
        let answer = format!(
            r#"case " $* " in
    *" Notify "*) echo "u $(tr '\036' '\n' <'{}' | grep -c "$(printf '\037')Notify$(printf '\037')")" ;;
    *GetServerInformation*) echo 'ssss "Fake server" "Fake" "1" "1.2"' ;;
    *GetCapabilities*) echo 'as 2 "body" "actions"' ;;
esac"#,
            log.display()
        );
        scratch.tool("busctl", &recorder(&log, &answer));
        scratch
    }

    /// Sets a variable for every command run from now on.
    pub fn set_var(&self, key: &str, value: &str) {
        self.vars.borrow_mut().push((key.into(), value.into()));
    }

    /// Writes an executable into the scratch PATH.
    pub fn tool(&self, name: &str, script: &str) {
        self.tool_at(&self.root.join("bin").join(name), script);
    }

    /// Writes an executable.
    pub fn tool_at(&self, path: &Path, script: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake agent (or tool) that records how it was run, then runs `then`.
    pub fn fake(&self, name: &str, then: &str) {
        self.tool(
            name,
            &recorder(&self.root.join(format!("logs/{name}")), then),
        );
    }

    /// How the fake called `name` was run, one list of arguments per run.
    pub fn runs(&self, name: &str) -> Vec<Vec<String>> {
        let log = fs::read_to_string(self.root.join(format!("logs/{name}"))).unwrap_or_default();
        log.split('\x1e')
            .filter(|run| !run.is_empty())
            .map(|line| {
                let mut args: Vec<String> = line.split('\x1f').map(str::to_owned).collect();
                args.pop();
                args.remove(0);
                args
            })
            .collect()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            contents.replace("@ROOT@", &self.root.display().to_string()),
        )
        .unwrap();
    }

    pub fn read(&self, relative: &str) -> Option<String> {
        fs::read_to_string(self.path(relative)).ok()
    }

    /// The folder a desktop that shows agents at work makes for their notes.
    pub fn desktop(&self) {
        fs::create_dir_all(self.path("run/slipstream/working")).unwrap();
    }

    pub fn note(&self, pid: &str) -> Option<String> {
        self.read(&format!("run/slipstream/working/{pid}"))
    }

    pub fn config(&self, text: &str) {
        self.write("config/agent-notify/config", text);
    }

    /// A task that started `ago` seconds before now, by the clock the hooks use (seconds since
    /// boot), or at boot on a machine that has been up for less.
    pub fn started(&self, agent: &str, session: &str, ago: u64) {
        let uptime = fs::read_to_string("/proc/uptime").unwrap();
        let now: u64 = uptime.split('.').next().unwrap().parse().unwrap();
        self.write(
            &format!("run/agent-notify/{agent}-{session}.started"),
            &format!("{}\n", now.saturating_sub(ago)),
        );
    }

    pub fn command(&self, args: &[&str]) -> Command {
        self.command_running(self.path("bin/agent-notify"), args)
    }

    /// A command run in the scratch machine's environment.
    pub fn command_running(&self, program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("LANG", "C.UTF-8")
            .env("AGENT_NOTIFY_RECORD", self.path("logs/record"))
            .current_dir(self.path("home"));
        for (key, value) in self.vars.borrow().iter() {
            command.env(key, value);
        }
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// Runs a hook with `payload` (a fixture file's contents, or JSON) on stdin, as the agent
    /// with process ID AGENT_PID, and checks it says nothing and succeeds.
    pub fn hook(&self, agent: &str, event: &str, payload: &str) {
        let mut command = self.command(&["hook", agent, event]);
        command.env("AGENT_NOTIFY_PID", AGENT_PID);
        self.hook_with(command, payload);
    }

    pub fn hook_with(&self, mut command: Command, payload: &str) {
        let payload = payload.replace("@ROOT@", &self.root.display().to_string());
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{:?}: a hook never fails the agent",
            self.imp
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{:?}: a hook prints nothing",
            self.imp
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "",
            "{:?}: a hook prints nothing",
            self.imp
        );
    }

    /// The notification calls made so far, with how long a task took written as `<duration>`,
    /// since a clock that ticks during a run can add a second to it.
    pub fn calls(&self) -> Vec<Call> {
        let mut calls: Vec<Call> = match self.imp {
            Imp::Binary => recorded(&self.path("logs/record")),
            Imp::Script => self
                .runs("busctl")
                .iter()
                .filter_map(|args| from_busctl(args))
                .collect(),
        };
        for call in &mut calls {
            if let Call::Notify { body, .. } = call {
                *body = unduration(body);
            }
        }
        calls
    }

    /// Every file the run left, by path, with its mode, and the root written as $ROOT.
    pub fn files(&self) -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        for top in ["home", "config", "cache", "run"] {
            walk(&self.root, &self.root.join(top), &mut files);
        }
        files
    }
}

fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let name = path.strip_prefix(root).unwrap().display().to_string();
        if meta.is_dir() {
            walk(root, &path, files);
        } else {
            let text = fs::read_to_string(&path).unwrap_or_else(|_| "<binary>".into());
            let text = text.replace(&root.display().to_string(), "$ROOT");
            // Clock readings, which no two runs share.
            let text = match path.extension().and_then(|extension| extension.to_str()) {
                Some("started") => "<uptime>".to_owned(),
                Some("asked" | "retry") => "<time>".to_owned(),
                _ => text,
            };
            files.insert(
                name,
                format!("[{:o}] {text}", meta.permissions().mode() & 0o777),
            );
        }
    }
}

/// Whether `text` is how long a task took: 42s, 7m 12s, 1h 05m or 5d 23h.
fn is_duration(text: &str) -> bool {
    let unit = |part: &str, units: &str| {
        part.len() > 1
            && part[..part.len() - 1]
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            && units.contains(&part[part.len() - 1..])
    };
    match text.split_once(' ') {
        None => unit(text, "s"),
        Some((big, small)) => unit(big, "mhd") && unit(small, "smh") && small.len() == 3,
    }
}

/// The body with the duration on its first line, after the project or "Resets in", as
/// `<duration>`.
fn unduration(body: &str) -> String {
    if body.strip_prefix("Resets in ").is_some_and(is_duration) {
        return "Resets in <duration>".into();
    }
    let (first, rest) = body
        .split_once('\n')
        .map_or((body, None), |(first, rest)| (first, Some(rest)));
    let (project, last) = first
        .rsplit_once(" · ")
        .map_or(("", first), |(project, last)| (project, last));
    if !is_duration(last) {
        return body.to_owned();
    }
    let mut out = if project.is_empty() {
        String::new()
    } else {
        format!("{project} · ")
    };
    out.push_str("<duration>");
    if let Some(rest) = rest {
        out.push('\n');
        out.push_str(rest);
    }
    out
}

fn recorded(path: &Path) -> Vec<Call> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|line| {
            let call: serde_json::Value = serde_json::from_str(line).unwrap();
            match call["call"].as_str().unwrap() {
                "Notify" => {
                    assert_eq!(call["icon"], "utilities-terminal");
                    assert_eq!(call["timeout"], -1);
                    let hints = call["hints"].as_object().unwrap();
                    Call::Notify {
                        app: call["app"].as_str().unwrap().into(),
                        replaces: call["replaces"].as_u64().unwrap() as u32,
                        summary: call["summary"].as_str().unwrap().into(),
                        body: call["body"].as_str().unwrap().into(),
                        hints: hints
                            .iter()
                            .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                            .collect(),
                    }
                }
                _ => Call::Close(call["id"].as_u64().unwrap() as u32),
            }
        })
        .collect()
}

/// A call from busctl's command line: `--user --timeout=5 -- call DEST PATH IFACE METHOD SIG ARGS…`.
fn from_busctl(args: &[String]) -> Option<Call> {
    let call = args.iter().position(|arg| arg == "call")?;
    let args = &args[call + 4..];
    match args[0].as_str() {
        "Notify" => {
            assert_eq!(args[1], "susssasa{sv}i");
            assert_eq!(args[4], "utilities-terminal");
            assert_eq!(args[7], "0", "no actions");
            let count: usize = args[8].parse().unwrap();
            let mut hints = BTreeMap::new();
            for hint in 0..count {
                let at = 9 + hint * 3;
                hints.insert(
                    args[at].clone(),
                    format!("{} {}", args[at + 1], args[at + 2]),
                );
            }
            assert_eq!(args[9 + count * 3], "-1");
            Some(Call::Notify {
                app: args[2].clone(),
                replaces: args[3].parse().unwrap(),
                summary: args[5].clone(),
                body: args[6].clone(),
                hints,
            })
        }
        "CloseNotification" => Some(Call::Close(args[2].parse().unwrap())),
        _ => None,
    }
}

/// Runs `test` on a fresh scratch machine for each implementation, then checks that the script
/// left what the binary left, after `normalise` has taken out what is allowed to differ.
pub fn parity(name: &str, test: impl Fn(&Scratch)) {
    parity_except(name, test, |_, _| {});
}

pub fn parity_except(
    name: &str,
    test: impl Fn(&Scratch),
    normalise: impl Fn(Imp, &mut BTreeMap<String, String>),
) {
    let mut seen = Vec::new();
    for imp in implementations() {
        let scratch = Scratch::new(name, imp);
        test(&scratch);
        let mut files = scratch.files();
        // The marker a session waiting on you leaves for `agent-notify bar` is new.
        files.retain(|name, _| !name.ends_with(".waiting"));
        normalise(imp, &mut files);
        let mut calls = scratch.calls();
        for call in &mut calls {
            if let Call::Notify { hints, .. } = call {
                if let Some(pid) = hints.get_mut("sender-pid") {
                    *pid = "<pid>".into();
                }
            }
        }
        seen.push((imp, files, calls));
    }
    if let [
        (_, binary_files, binary_calls),
        (_, script_files, script_calls),
    ] = seen.as_slice()
    {
        assert_eq!(
            script_files, binary_files,
            "{name}: the script and the binary left different files"
        );
        assert_eq!(
            script_calls, binary_calls,
            "{name}: the script and the binary notified differently"
        );
    }
}
