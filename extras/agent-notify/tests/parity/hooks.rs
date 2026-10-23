//! What each agent's hooks leave: the working note, the state, and the notifications.

use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;

use crate::fixtures::{agy, claude, codex, gemini};
use crate::support::{AGENT_PID, Call, Imp, QUIET, Scratch, notify, parity, parity_except};

/// Longer than a long task (120 seconds unless the config says otherwise), or since boot.
const LONG_AGO: u64 = 5 * 3600 + 30 * 60;
/// How a notification's body reads the time a task took, in the tests (see `Scratch::calls`).
const LONG_AGO_TEXT: &str = "<duration>";

fn sound(name: &str, urgency: &str) -> Vec<(String, String)> {
    vec![
        ("urgency".into(), format!("y {urgency}")),
        ("sound-name".into(), format!("s {name}")),
    ]
}

#[test]
fn a_new_session_leaves_an_idle_note_named_for_its_folder() {
    parity("session_start", |s| {
        s.desktop();
        s.hook("claude", "session_start", claude::SESSION_START);
        assert_eq!(s.note(AGENT_PID).as_deref(), Some("claude: kiln\nidle\n"));
        assert!(s.calls().is_empty());
    });
}

#[test]
fn a_session_that_already_has_a_note_keeps_it() {
    parity("session_start_again", |s| {
        s.desktop();
        s.write("run/slipstream/working/4242", "claude: earlier work\n\n");
        s.hook("claude", "session_start", claude::SESSION_START);
        assert_eq!(
            s.note(AGENT_PID).as_deref(),
            Some("claude: earlier work\n\n")
        );
    });
}

#[test]
fn a_prompt_names_the_note_and_withdraws_the_last_notification() {
    parity("prompt", |s| {
        s.desktop();
        s.write(
            &format!("run/agent-notify/claude-{}.id", claude::SESSION),
            "3\n",
        );
        s.hook("claude", "start", claude::PROMPT);
        assert_eq!(
            s.note(AGENT_PID).as_deref(),
            Some("claude: Make the kiln timer count down from the firing schedule inst\n\n")
        );
        assert_eq!(s.calls(), [Call::Close(3)]);
        assert!(
            s.read(&format!("run/agent-notify/claude-{}.id", claude::SESSION))
                .is_none()
        );
        assert!(
            s.read(&format!(
                "run/agent-notify/claude-{}.started",
                claude::SESSION
            ))
            .is_some()
        );
    });
}

#[test]
fn the_newest_title_in_the_transcript_names_the_note() {
    parity("title", |s| {
        s.desktop();
        s.write(claude::TRANSCRIPT_PATH, claude::TRANSCRIPT);
        s.hook("claude", "working", claude::TOOL_USED);
        assert_eq!(
            s.note(AGENT_PID).as_deref(),
            Some("claude: Kiln timer: count downfrom the schedule\n\n")
        );
    });
}

#[test]
fn a_subagent_s_hook_keeps_the_session_s_name() {
    parity("subagent_tool", |s| {
        s.desktop();
        s.write(
            "run/slipstream/working/4242",
            "claude: Kiln timer countdown\nidle\n",
        );
        s.hook("claude", "working", claude::SUBAGENT_TOOL_USED);
        assert_eq!(
            s.note(AGENT_PID).as_deref(),
            Some("claude: Kiln timer countdown\n\n")
        );
    });
}

#[test]
fn a_background_session_is_named_by_its_record_with_its_subagents() {
    parity("job_record", |s| {
        s.desktop();
        s.write("home/.claude/sessions/4242.json", claude::SESSION_ENTRY);
        s.write("home/.claude/jobs/5c1d7e2a/state.json", claude::JOB_STATE);
        s.hook("claude", "working", claude::TOOL_USED);
        let working = "claude: Kiln timer rework\n\nReading the firing schedule\nChecking the header layout\n";
        assert_eq!(s.note(AGENT_PID).as_deref(), Some(working));
        // Finished, but still working through its subagents.
        s.hook("claude", "finish", claude::STOP);
        assert_eq!(s.note(AGENT_PID).as_deref(), Some(working));
    });
}

#[test]
fn a_quick_task_finishing_is_quiet() {
    parity("quick_finish", |s| {
        s.desktop();
        s.config("long-task-secs = 100000\n");
        s.started("claude", claude::SESSION, LONG_AGO);
        s.hook("claude", "finish", claude::STOP);
        assert_eq!(s.note(AGENT_PID).as_deref(), Some("claude: kiln\nidle\n"));
        let body = format!(
            "kiln · {LONG_AGO_TEXT}\nThe timer now counts down. &lt;b&gt;Tests&lt;/b&gt; pass &amp; the header shows the time left."
        );
        assert_eq!(
            s.calls(),
            [notify(
                "Claude Code",
                0,
                "Claude Code finished",
                &body,
                QUIET
            )]
        );
        assert_eq!(
            s.read(&format!("run/agent-notify/claude-{}.id", claude::SESSION))
                .as_deref(),
            Some("1\n")
        );
        assert!(
            s.read(&format!(
                "run/agent-notify/claude-{}.started",
                claude::SESSION
            ))
            .is_none()
        );
    });
}

#[test]
fn a_long_task_finishing_has_a_sound_but_waits_for_you() {
    parity("long_finish", |s| {
        s.config("sound-finished = bell\n");
        s.started("claude", claude::SESSION, LONG_AGO);
        s.write(
            &format!("run/agent-notify/claude-{}.id", claude::SESSION),
            "7\n",
        );
        s.hook("claude", "finish", claude::STOP);
        let calls = s.calls();
        let [
            Call::Notify {
                replaces, hints, ..
            },
        ] = calls.as_slice()
        else {
            panic!("{calls:?}")
        };
        assert_eq!(
            *replaces, 7,
            "it takes the place of the session's last notification"
        );
        assert_eq!(hints, &sound("bell", "1").into_iter().collect());
    });
}

#[test]
fn an_agent_waiting_on_you_after_a_long_task_is_critical() {
    parity("long_attention", |s| {
        s.desktop();
        s.started("claude", claude::SESSION, LONG_AGO);
        s.hook("claude", "attention", claude::PERMISSION);
        assert_eq!(s.note(AGENT_PID).as_deref(), Some("claude: kiln\nidle\n"));
        let body = format!("kiln · {LONG_AGO_TEXT}\nClaude needs your permission to use Bash");
        let hints = sound("window-attention", "2");
        assert_eq!(
            s.calls(),
            [notify(
                "Claude Code",
                0,
                "Claude Code needs you",
                &body,
                &hints
            )]
        );
        if s.imp == Imp::Binary {
            assert!(
                s.read("run/agent-notify/4242.waiting").is_some(),
                "the bar can tell it waits on you"
            );
        }
        s.hook("claude", "start", claude::PROMPT);
        assert!(s.read("run/agent-notify/4242.waiting").is_none());
    });
}

#[test]
fn a_permission_request_says_what_it_wants_to_run() {
    parity("codex_permission", |s| {
        s.hook("codex", "attention", codex::PERMISSION);
        s.hook("codex", "attention", codex::TOOL_ONLY);
        s.hook(
            "codex",
            "attention",
            &format!(r#"{{"session_id":"{}"}}"#, codex::SESSION),
        );
        assert_eq!(
            s.calls(),
            [
                notify(
                    "Codex",
                    0,
                    "Codex needs you",
                    "orchard\nWants to run: cargo test -- --include-ignored",
                    QUIET
                ),
                notify(
                    "Codex",
                    1,
                    "Codex needs you",
                    "orchard\nWaiting to use apply_patch",
                    QUIET
                ),
                notify(
                    "Codex",
                    2,
                    "Codex needs you",
                    "Waiting for your answer",
                    QUIET
                ),
            ]
        );
    });
}

#[test]
fn codex_s_last_message_is_the_finish_s_detail() {
    parity("codex_finish", |s| {
        s.desktop();
        s.hook("codex", "start", codex::PROMPT);
        assert_eq!(
            s.note(AGENT_PID).as_deref(),
            Some("codex: Add pruning dates to the orchard planner\n\n")
        );
        s.started("codex", codex::SESSION, LONG_AGO);
        s.hook("codex", "finish", codex::STOP);
        let body = format!(
            "orchard · {LONG_AGO_TEXT}\nPruning dates are in. Apples &gt; pears, as asked."
        );
        assert_eq!(
            s.calls(),
            [notify(
                "Codex",
                0,
                "Codex finished",
                &body,
                &sound("complete", "1")
            )]
        );
    });
}

#[test]
fn gemini_s_answer_is_the_finish_s_detail() {
    parity("gemini", |s| {
        s.desktop();
        s.hook("gemini", "start", gemini::PROMPT);
        s.hook("gemini", "working", gemini::TOOL_USED);
        let name = "gemini: Reconcile the March ledger against the bank export";
        assert_eq!(s.note(AGENT_PID), Some(format!("{name}\n\n")));
        s.started("gemini", gemini::SESSION, LONG_AGO);
        s.hook("gemini", "finish", gemini::ANSWERED);
        assert_eq!(s.note(AGENT_PID), Some(format!("{name}\nidle\n")));
        s.hook("gemini", "attention", gemini::PERMISSION);
        let finished = format!(
            "ledger · {LONG_AGO_TEXT}\nTwo entries differ: the 4th (rounding) and the 19th (a refund)."
        );
        assert_eq!(
            s.calls(),
            [
                notify(
                    "Gemini CLI",
                    0,
                    "Gemini CLI finished",
                    &finished,
                    &sound("complete", "1")
                ),
                notify(
                    "Gemini CLI",
                    1,
                    "Gemini CLI needs you",
                    "ledger\nGemini CLI wants to run: rm -rf build/",
                    QUIET
                ),
            ]
        );
    });
}

#[test]
fn antigravity_starts_on_its_first_step_and_is_named_from_its_transcript() {
    parity("antigravity", |s| {
        s.desktop();
        s.write(agy::TRANSCRIPT_PATH, agy::TRANSCRIPT);
        s.hook("agy", "session_start", agy::SESSION_START);
        let name = "antigravity: Log the humidity every hour too";
        assert_eq!(s.note(AGENT_PID), Some(format!("{name}\nidle\n")));
        s.hook("agy", "step", agy::STEP);
        assert_eq!(s.note(AGENT_PID), Some(format!("{name}\n\n")));
        let started = format!("run/agent-notify/agy-{}.started", agy::SESSION);
        assert!(s.read(&started).is_some(), "the first step is the start");
        s.started("agy", agy::SESSION, LONG_AGO);
        s.hook("agy", "step", agy::STEP);
        assert!(
            s.read(&started).is_some_and(|text| text.trim() != ""),
            "a later step is only work"
        );
        s.hook("agy", "finish", agy::STOP);
        let body = format!("greenhouse · {LONG_AGO_TEXT}");
        assert_eq!(
            s.calls(),
            [notify(
                "Antigravity",
                0,
                "Antigravity finished",
                &body,
                &sound("complete", "1")
            )]
        );
        assert_eq!(s.note(AGENT_PID), Some(format!("{name}\nidle\n")));
    });
}

#[test]
fn without_a_desktop_s_folder_there_is_no_note_but_still_a_notification() {
    parity("no_desktop", |s| {
        s.hook("claude", "finish", claude::STOP);
        assert!(s.note(AGENT_PID).is_none());
        assert_eq!(s.calls().len(), 1);
    });
}

#[test]
fn what_isn_t_an_agent_s_json_says_nothing() {
    parity("not_json", |s| {
        s.desktop();
        s.hook("claude", "finish", "not json at all");
        s.hook("claude", "attention", "[1, 2]");
        s.hook("claude", "compact", claude::STOP);
        assert!(s.calls().is_empty());
        assert!(s.note(AGENT_PID).is_none());
        // A note is still kept for a session opening, named for the agent alone.
        s.hook("claude", "session_start", "");
        assert_eq!(s.note(AGENT_PID).as_deref(), Some("claude\nidle\n"));
    });
}

#[test]
fn a_session_without_an_id_is_the_default_one() {
    parity("default_session", |s| {
        s.hook("gemini", "start", r#"{"prompt": "hello"}"#);
        assert!(s.read("run/agent-notify/gemini-default.started").is_some());
    });
}

#[test]
fn a_kept_session_s_notification_names_the_window_attached_to_it() {
    parity_except(
        "kept_window",
        |s| {
            // A process called agent-notify, as `keep` leaves in the window.
            fs::create_dir_all(s.path("window")).unwrap();
            symlink("/usr/bin/sleep", s.path("window/agent-notify")).unwrap();
            let mut window = Command::new(s.path("window/agent-notify"))
                .arg("30")
                .spawn()
                .unwrap();
            s.write(
                "run/agent-notify/claude-5c1d7e2a.window",
                &format!("{}\n", window.id()),
            );
            s.hook("claude", "finish", claude::STOP);
            // Another process with the number left in the file is no window of the session's.
            s.write(
                "run/agent-notify/claude-5c1d7e2a.window",
                &format!("{}\n", std::process::id()),
            );
            s.hook("claude", "finish", claude::STOP);
            let calls = s.calls();
            let [
                Call::Notify { hints: first, .. },
                Call::Notify { hints: second, .. },
            ] = calls.as_slice()
            else {
                panic!("{calls:?}")
            };
            assert_eq!(first.get("sender-pid"), Some(&format!("x {}", window.id())));
            assert_eq!(second.get("sender-pid"), None);
            window.kill().unwrap();
            window.wait().unwrap();
        },
        |_, files| {
            files.remove("run/agent-notify/claude-5c1d7e2a.window");
        },
    );
}

/// Runs a hook under made-up agent processes: `$ROOT/tree/fakeagent` runs its arguments as a child
/// and records its PID, so the hook's process tree has processes called `fakeagent` above it. A
/// made-up agent, since whatever runs the tests may itself be running under a real one.
fn under_fake_agent(s: &Scratch, chain: &[&str]) {
    let fake = s.path("tree/fakeagent");
    s.tool_at(
        &fake,
        &format!(
            "#!/bin/sh\necho $$ >>'{}'\n\"$@\"\nexit $?\n",
            s.path("logs/claude-pids").display()
        ),
    );
    let mut args: Vec<String> = Vec::new();
    for step in chain {
        match *step {
            "fakeagent" => args.push(fake.display().to_string()),
            "sh" => args.extend(["sh", "-c", "\"$@\"; exit $?", "sh"].map(String::from)),
            other => panic!("no fake process {other}"),
        }
    }
    args.push(s.path("bin/agent-notify").display().to_string());
    args.extend(["hook", "fakeagent", "session_start"].map(String::from));
    let mut command = s.command_running(&args[0], &[]);
    command.args(&args[1..]);
    s.hook_with(command, claude::SESSION_START);
}

fn notes(s: &Scratch) -> Vec<String> {
    let mut notes: Vec<String> = fs::read_dir(s.path("run/slipstream/working"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    notes.sort();
    notes
}

fn fake_pids(s: &Scratch) -> Vec<String> {
    s.read("logs/claude-pids")
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_agent_is_the_nearest_process_above_the_hook_called_what_it_is() {
    for imp in crate::support::implementations() {
        let s = Scratch::new("tree_one", imp);
        s.desktop();
        under_fake_agent(&s, &["fakeagent"]);
        assert_eq!(notes(&s), fake_pids(&s), "{imp:?}");
    }
}

#[test]
fn an_agent_restarting_itself_is_one_session() {
    for imp in crate::support::implementations() {
        let s = Scratch::new("tree_restart", imp);
        s.desktop();
        under_fake_agent(&s, &["fakeagent", "fakeagent"]);
        assert_eq!(
            notes(&s),
            fake_pids(&s)[1..],
            "{imp:?}: named for the lower"
        );
    }
}

#[test]
fn a_subagent_running_as_a_process_of_its_own_leaves_no_note() {
    for imp in crate::support::implementations() {
        let s = Scratch::new("tree_subagent", imp);
        s.desktop();
        under_fake_agent(&s, &["fakeagent", "sh", "fakeagent"]);
        assert_eq!(fake_pids(&s).len(), 2);
        assert!(notes(&s).is_empty(), "{imp:?}");
    }
}

#[test]
fn a_hook_never_waits_on_a_missing_notification_server() {
    // With no recording and no session bus, a notification can't be sent: the hook still says
    // nothing and succeeds, and logs why.
    let s = Scratch::new("no_bus", Imp::Binary);
    let mut command = s.command(&["hook", "claude", "finish"]);
    command
        .env_remove("AGENT_NOTIFY_RECORD")
        .env("AGENT_NOTIFY_PID", AGENT_PID);
    command.env(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={}", s.path("run/no-bus").display()),
    );
    s.hook_with(command, claude::STOP);
    let log = s
        .read("state/agent-notify/agent-notify.log")
        .unwrap_or_default();
    assert!(log.contains("hook claude finish:"), "{log}");
}
