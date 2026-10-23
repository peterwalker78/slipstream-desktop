//! `setup`, `setup --remove` and `status`: the hooks put into each agent's settings beside the
//! person's own, and taken out again.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Output;

use crate::support::{Imp, Scratch, parity};

/// Every agent installed, as fakes on the scratch PATH.
fn install_all(s: &Scratch) {
    for agent in ["claude", "gemini", "codex", "agy"] {
        s.fake(agent, "");
    }
}

fn says(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn with_root(s: &Scratch, text: &str) -> String {
    text.replace("$ROOT", &s.root.display().to_string())
}

const CLAUDE_SETTINGS: &str = "home/.claude/settings.json";

/// Settings of the person's own: a hook of theirs, an old one of this tool's from somewhere else.
const OWN_SETTINGS: &str = r#"{
  "model": "opus",
  "hooks": {
    "PostToolUse": [
      {"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "cargo fmt"}]},
      {"hooks": [{"type": "command", "command": "'/old/place/agent-notify' hook claude working", "timeout": 5}]}
    ],
    "PreCompact": [{"hooks": [{"type": "command", "command": "notify-send compacting"}]}]
  },
  "permissions": {"allow": ["Bash(cargo test:*)"]}
}
"#;

#[test]
fn the_first_setup_writes_the_config_and_wires_every_agent_installed() {
    parity("setup_first", |s| {
        install_all(s);
        let output = s.run(&["setup"]);
        assert!(output.status.success());
        let expected = "Wrote $ROOT/config/agent-notify/config (agents: claude gemini codex agy)\n\
            Claude Code: hooks in place\nGemini CLI: hooks in place\n\
            \x20 Codex runs new hooks only once you trust them: open /hooks in Codex.\n\
            Codex: hooks in place\nAntigravity: hooks in place\n";
        assert_eq!(says(&output), (with_root(s, expected), String::new()));
        let claude = s.read(CLAUDE_SETTINGS).unwrap();
        assert!(claude.contains(&with_root(
            s,
            r#""command": "'$ROOT/bin/agent-notify' hook claude working""#
        )));
        let mode = fs::metadata(s.path(CLAUDE_SETTINGS))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a new settings file is the person's alone");
        for file in [
            "home/.gemini/settings.json",
            "home/.codex/hooks.json",
            "home/.gemini/config/hooks.json",
        ] {
            assert!(s.read(file).unwrap().contains("agent-notify"), "{file}");
        }
    });
}

#[test]
fn setup_merges_beside_the_person_s_own_hooks_and_backs_up_once() {
    parity("setup_merge", |s| {
        s.fake("claude", "");
        s.config("agents = claude\n");
        s.write(CLAUDE_SETTINGS, OWN_SETTINGS);
        fs::set_permissions(s.path(CLAUDE_SETTINGS), fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(says(&s.run(&["setup"])).0, "Claude Code: hooks in place\n");
        let once = s.read(CLAUDE_SETTINGS).unwrap();
        assert_eq!(says(&s.run(&["setup"])).0, "Claude Code: hooks in place\n");
        assert_eq!(
            s.read(CLAUDE_SETTINGS).unwrap(),
            once,
            "setting up again changes nothing"
        );
        assert_eq!(
            s.read("home/.claude/settings.json.agent-notify.bak")
                .as_deref(),
            Some(OWN_SETTINGS)
        );
        assert!(once.contains("cargo fmt") && once.contains("notify-send compacting"));
        assert!(!once.contains("/old/place"), "the old hook is replaced");
        let keys: Vec<usize> = ["\"model\"", "\"hooks\"", "\"permissions\""]
            .iter()
            .map(|key| once.find(key).unwrap())
            .collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] < pair[1]),
            "keys keep their order:\n{once}"
        );
        let mode = fs::metadata(s.path(CLAUDE_SETTINGS))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640, "the file keeps its mode");
    });
}

#[test]
fn naming_agents_saves_them_and_unwires_the_rest() {
    parity("setup_named", |s| {
        install_all(s);
        s.config("# mine\nagents = claude codex\nlong-task-secs = 60\n");
        s.run(&["setup"]);
        let output = s.run(&["setup", "gemini"]);
        let expected =
            "Claude Code: hooks removed\nGemini CLI: hooks in place\nCodex: hooks removed\n";
        assert_eq!(says(&output).0, expected);
        assert_eq!(
            s.read("config/agent-notify/config").as_deref(),
            Some("# mine\nagents = gemini\nlong-task-secs = 60\n")
        );
        assert!(!s.read(CLAUDE_SETTINGS).unwrap().contains("agent-notify"));
    });
}

#[test]
fn setup_remove_takes_every_hook_back_out() {
    parity("setup_remove", |s| {
        install_all(s);
        s.run(&["setup"]);
        s.write(
            "home/.gemini/settings.json",
            &s.read("home/.gemini/settings.json").unwrap().replacen(
                '{',
                "{\"theme\": \"dark\",",
                1,
            ),
        );
        let output = s.run(&["setup", "--remove", "claude"]);
        let expected = "Claude Code: hooks removed\nGemini CLI: hooks removed\nCodex: hooks removed\nAntigravity: hooks removed\n";
        assert_eq!(says(&output).0, expected);
        assert_eq!(s.read(CLAUDE_SETTINGS).as_deref(), Some("{}\n"));
        assert_eq!(
            s.read("home/.gemini/settings.json").as_deref(),
            Some("{\n  \"theme\": \"dark\"\n}\n")
        );
        assert_eq!(
            s.read("home/.gemini/config/hooks.json").as_deref(),
            Some("{}\n")
        );
        assert_eq!(
            says(&s.run(&["setup", "--remove"])).0,
            "",
            "nothing left to take out"
        );
    });
}

#[test]
fn settings_that_are_not_json_are_left_alone() {
    parity("setup_not_json", |s| {
        s.fake("claude", "");
        s.config("agents = claude\n");
        s.write(CLAUDE_SETTINGS, "{ \"model\": \"opus\", }\n");
        let output = s.run(&["setup"]);
        let expected = with_root(
            s,
            "  $ROOT/home/.claude/settings.json isn't valid JSON; leaving it alone\n",
        );
        assert_eq!(says(&output), (String::new(), expected));
        assert_eq!(
            s.read(CLAUDE_SETTINGS).as_deref(),
            Some("{ \"model\": \"opus\", }\n")
        );
    });
}

#[test]
fn an_unknown_agent_is_refused() {
    parity("setup_unknown", |s| {
        s.config("agents = claude\n");
        let output = s.run(&["setup", "claude", "bard"]);
        assert!(!output.status.success());
        assert_eq!(
            says(&output).1,
            "unknown agent: bard (known: claude gemini codex agy)\n"
        );
        assert_eq!(
            s.read("config/agent-notify/config").as_deref(),
            Some("agents = claude\n")
        );
    });
}

#[test]
fn an_agent_not_installed_is_wired_anyway() {
    parity("setup_missing", |s| {
        s.config("agents = agy\n");
        assert_eq!(
            says(&s.run(&["setup"])).0,
            "Antigravity: not installed, wiring it anyway\nAntigravity: hooks in place\n"
        );
    });
}

#[test]
fn codex_s_hooks_follow_codex_home() {
    parity("setup_codex_home", |s| {
        s.fake("codex", "");
        s.config("agents = codex\n");
        let mut command = s.command(&["setup"]);
        command.env("CODEX_HOME", s.path("home/codex-elsewhere"));
        command.output().unwrap();
        assert!(
            s.read("home/codex-elsewhere/hooks.json")
                .unwrap()
                .contains("hook codex working")
        );
        assert!(s.read("home/.codex/hooks.json").is_none());
    });
}

#[test]
fn status_says_what_is_configured_wired_and_open() {
    let mut seen = Vec::new();
    for imp in crate::support::implementations() {
        let s = Scratch::new("status", imp);
        s.fake("claude", "");
        s.config("agents = claude\nlong-task-secs = 90\n");
        s.run(&["setup"]);
        s.desktop();
        let mut open = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        s.write(
            &format!("run/slipstream/working/{}", open.id()),
            "claude: Kiln timer\nidle\n",
        );
        s.write(
            "run/slipstream/working/999999999",
            "gone: its process has ended\n\n",
        );
        let output = s.run(&["status"]);
        open.kill().unwrap();
        open.wait().unwrap();
        let text = "config: $ROOT/config/agent-notify/config\nagents: claude\nlong task: 90s\n";
        let printed = says(&output)
            .0
            .replace(&open.id().to_string(), "PID")
            .replace(&s.root.display().to_string(), "$ROOT");
        assert!(printed.starts_with(text), "{printed}");
        assert!(
            printed.contains("  Claude Code  installed: yes  hooks: yes\n"),
            "{printed}"
        );
        assert!(printed.contains("  Codex        installed: "), "{printed}");
        assert!(
            printed.contains("sessions on the desktop:\n  PID"),
            "{printed}"
        );
        assert!(
            printed.contains(" idle    claude: Kiln timer\n"),
            "{printed}"
        );
        assert!(!printed.contains("its process has ended"), "{printed}");
        // The script listed the tools it needed, and asked the bus through busctl.
        let lines: Vec<String> = printed
            .lines()
            .filter(|line| !line.starts_with("notification server:"))
            .filter(|line| {
                !["  jq", "  busctl", "  notify-send"]
                    .iter()
                    .any(|tool| line.starts_with(tool))
            })
            .filter(|line| !line.starts_with("  Gemini CLI") && !line.starts_with("  Codex "))
            .map(str::to_owned)
            .collect();
        seen.push(lines);
    }
    if let [binary, script] = seen.as_slice() {
        assert_eq!(script, binary);
    }
}

/// The script saved named agents only over an `agents` line already in the config.
#[test]
fn naming_agents_adds_the_line_to_a_config_without_one() {
    let s = Scratch::new("setup_no_agents_line", Imp::Binary);
    s.fake("gemini", "");
    s.config("long-task-secs = 60\n");
    assert_eq!(
        says(&s.run(&["setup", "gemini"])).0,
        "Gemini CLI: hooks in place\n"
    );
    let config = s.read("config/agent-notify/config");
    assert_eq!(
        config.as_deref(),
        Some("long-task-secs = 60\nagents = gemini\n")
    );
}

#[test]
fn help_lists_the_commands_and_an_unknown_one_is_refused() {
    let s = Scratch::new("help", Imp::Binary);
    let help = s.run(&["--help"]);
    assert!(help.status.success());
    let help = says(&help).0;
    let commands = [
        "setup [AGENT...]",
        "status",
        "test [--long]",
        "usage [--refresh]",
        "bar",
    ];
    for command in commands.iter().chain(&["keep claude", "hook AGENT EVENT"]) {
        assert!(
            help.contains(&format!("agent-notify {command}")),
            "{command}"
        );
    }
    let unknown = s.run(&["frobnicate"]);
    assert!(!unknown.status.success());
    assert!(
        says(&unknown)
            .1
            .starts_with("agent-notify: no command \"frobnicate\"")
    );
}
