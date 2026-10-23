//! `usage`: plan limits read the way each agent's own usage screen reads them, cached, and
//! announced once per window as they pass each level.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::fixtures::usage;
use crate::support::{Call, Imp, Scratch, parity_except};

const CREDENTIALS: &str = "home/.claude/.credentials.json";
const CACHE: &str = "cache/agent-notify/usage";

/// Takes the clock out of text: every `"updated":` time but 0, and the time of day a rate limit
/// ends.
fn unclock(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("\"updated\":") {
        let (before, after) = rest.split_at(at + "\"updated\":".len());
        out.push_str(before);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        out.push_str(if &after[..digits] == "0" { "0" } else { "NOW" });
        rest = &after[digits..];
    }
    out.push_str(rest);
    match out.find("asking again at ") {
        Some(at) => {
            let at = at + "asking again at ".len();
            format!("{}HH:MM{}", &out[..at], &out[at + 5..])
        }
        None => out,
    }
}

fn unclock_files(_: Imp, files: &mut BTreeMap<String, String>) {
    for text in files.values_mut() {
        *text = unclock(text);
    }
}

/// curl, answering with what `$ROOT/curl/` holds: `code`, `headers` and `body`.
fn fake_curl(s: &Scratch, code: &str, headers: &str, body: &str) {
    s.write("curl/code", code);
    s.write("curl/headers", headers);
    s.write("curl/body", body);
    let root = s.root.display();
    s.fake(
        "curl",
        &format!(
            r#"cat >'{root}/logs/curl-stdin'
while [ $# -gt 0 ]; do
    case $1 in
        -D) headers=$2; shift ;;
        -o) body=$2; shift ;;
    esac
    shift
done
cat '{root}/curl/headers' >"$headers"
cat '{root}/curl/body' >"$body"
cat '{root}/curl/code'"#
        ),
    );
}

fn report(s: &Scratch, args: &[&str]) -> Value {
    let output = s.run(args);
    assert!(
        output.status.success(),
        "{:?}: {}",
        s.imp,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn labels(section: &Value) -> Vec<(String, i64)> {
    section["meters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|meter| {
            (
                meter["label"].as_str().unwrap().to_owned(),
                meter["percent"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn summaries(s: &Scratch) -> Vec<(String, String)> {
    s.calls()
        .into_iter()
        .map(|call| match call {
            Call::Notify { summary, hints, .. } => (summary, hints["urgency"].clone()),
            Call::Close(id) => panic!("closed {id}"),
        })
        .collect()
}

#[test]
fn claude_s_limits_are_read_with_its_own_login_and_announced_once() {
    parity_except(
        "usage_claude",
        |s| {
            s.config("agents = claude\n");
            s.write(CREDENTIALS, usage::CLAUDE_LOGIN);
            fake_curl(s, "200", "HTTP/2 200\r\n", usage::CLAUDE_REPLY);
            let report = report(s, &["usage", "--refresh"]);
            let claude = &report["sections"][0];
            assert_eq!(
                (claude["name"].as_str(), claude["detail"].as_str()),
                (Some("Claude"), Some("Max"))
            );
            let expected = [
                ("Session", 76),
                ("Weekly", 12),
                ("Weekly · Opus", 91),
                ("Extra usage", 0),
            ];
            assert_eq!(
                labels(claude),
                expected.map(|(label, percent)| (label.to_owned(), percent))
            );
            assert_eq!(
                claude["meters"][0]["resets"], 4_070_926_800_i64,
                "fractions and offsets read"
            );
            assert_eq!(claude["meters"][2]["resets"], 4_071_254_400_i64);
            assert_eq!(claude["stale"], false);
            // The token went to curl on its stdin, never on its command line.
            assert_eq!(
                s.read("logs/curl-stdin").as_deref(),
                Some("Authorization: Bearer sk-test-not-a-real-token\n")
            );
            assert!(!s.runs("curl")[0].iter().any(|arg| arg.contains("sk-test")));
            let alerts = [
                ("Claude Code · Session: 76% used", "y 1"),
                ("Claude Code · Weekly · Opus: 91% used", "y 1"),
            ];
            assert_eq!(
                summaries(s),
                alerts.map(|(summary, urgency)| (summary.to_owned(), urgency.to_owned()))
            );
            // Asked again in the same windows: nothing new to say.
            s.write(&format!("{CACHE}/claude.asked"), "0\n");
            report_again(s);
            assert_eq!(summaries(s).len(), 2);
            assert_eq!(s.runs("curl").len(), 2);
        },
        unclock_files,
    );
}

fn report_again(s: &Scratch) {
    let output = s.run(&["usage", "--refresh"]);
    assert!(output.status.success());
}

#[test]
fn an_expired_claude_login_is_never_renewed_and_the_numbers_go_stale() {
    parity_except(
        "usage_claude_expired",
        |s| {
            s.config("agents = claude\n");
            s.write(CREDENTIALS, usage::CLAUDE_EXPIRED);
            fake_curl(s, "200", "", usage::CLAUDE_REPLY);
            let report = report(s, &["usage"]);
            let note = "Login expired: Claude Code renews it when it next runs";
            assert_eq!(report["sections"][0]["note"], note);
            assert_eq!(report["sections"][0]["stale"], true);
            assert!(s.runs("curl").is_empty());
        },
        unclock_files,
    );
}

#[test]
fn claude_refusing_keeps_the_last_numbers_with_the_reason() {
    for (code, headers, note) in [
        (
            "429",
            "HTTP/2 429\r\nretry-after: 1200\r\n",
            "Rate limited: asking again at HH:MM",
        ),
        (
            "401",
            "",
            "Login refused: Claude Code renews it when it next runs",
        ),
        ("000", "", "Couldn't reach Anthropic"),
        ("503", "", "Anthropic answered HTTP 503"),
    ] {
        parity_except(
            &format!("usage_claude_{code}"),
            |s| {
                s.config("agents = claude\n");
                s.write(CREDENTIALS, usage::CLAUDE_LOGIN);
                s.write(&format!("{CACHE}/claude.json"), r#"{"name":"Claude","detail":"Max","meters":[{"label":"Session","percent":20,"resets":null}],"note":null,"stale":false,"updated":1}"#);
                fake_curl(s, code, headers, "{}");
                let report = report(s, &["usage", "--refresh"]);
                let section = &report["sections"][0];
                assert_eq!(unclock(section["note"].as_str().unwrap()), note, "{code}");
                assert_eq!(section["stale"], true);
                assert_eq!(labels(section), [("Session".to_owned(), 20)]);
                assert_eq!(
                    s.read(&format!("{CACHE}/claude.retry")).is_some(),
                    code == "429"
                );
                assert!(s.read(&format!("{CACHE}/claude.body")).is_none());
            },
            unclock_files,
        );
    }
}

#[test]
fn codex_s_limits_come_from_its_app_server() {
    parity_except(
        "usage_codex",
        |s| {
            s.config("agents = codex\nusage-alerts = 50 100\n");
            s.write("codex/replies", usage::CODEX_REPLIES);
            let root = s.root.display();
            s.fake(
                "codex",
                &format!(
                    r#"while IFS= read -r line; do
    case $line in *'"id":1,'*) cat '{root}/codex/replies' ;; esac
done"#
                ),
            );
            let report = report(s, &["usage", "--refresh"]);
            let codex = &report["sections"][0];
            assert_eq!(codex["detail"], "Plus");
            let expected = [("5-hour", 43), ("Weekly", 100), ("Spark · Daily", 3)];
            assert_eq!(
                labels(codex),
                expected.map(|(label, percent)| (label.to_owned(), percent))
            );
            assert_eq!(
                s.runs("codex"),
                [["-s", "read-only", "-a", "never", "app-server"]]
            );
            let alerts = [("Codex · Weekly: limit reached", "y 2")];
            assert_eq!(
                summaries(s),
                alerts.map(|(summary, urgency)| (summary.to_owned(), urgency.to_owned()))
            );
        },
        unclock_files,
    );
}

#[test]
fn antigravity_s_limits_come_from_its_own_readout() {
    parity_except(
        "usage_agy",
        |s| {
            s.config("agents = agy\n");
            s.write("agy/readout", usage::AGY_READOUT);
            let root = s.root.display();
            s.fake(
                "agy",
                &format!("pwd >'{root}/logs/agy-cwd'\ncat '{root}/agy/readout'"),
            );
            let report = report(s, &["usage", "--refresh"]);
            let agy = &report["sections"][0];
            let expected = [("Daily · Gemini Pro", 80), ("Weekly · Claude", 5)];
            assert_eq!(
                labels(agy),
                expected.map(|(label, percent)| (label.to_owned(), percent))
            );
            assert_eq!(agy["meters"][0]["resets"], 4_070_995_200_i64);
            let cwd = s.read("logs/agy-cwd").unwrap();
            assert_eq!(
                std::fs::canonicalize(cwd.trim()).unwrap(),
                std::fs::canonicalize(s.path(CACHE)).unwrap()
            );
            assert_eq!(
                summaries(s),
                [(
                    "Antigravity · Daily · Gemini Pro: 80% used".to_owned(),
                    "y 1".to_owned()
                )]
            );
        },
        unclock_files,
    );
}

#[test]
fn cached_numbers_are_shown_straight_away_and_marked_stale_once_old() {
    parity_except(
        "usage_cached",
        |s| {
            s.config("agents = claude\n");
            s.write(CREDENTIALS, usage::CLAUDE_LOGIN);
            fake_curl(s, "200", "", usage::CLAUDE_REPLY);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            s.write(&format!("{CACHE}/claude.asked"), &format!("{now}\n"));
            s.write(
                &format!("{CACHE}/claude.json"),
                &format!(
                    r#"{{"name":"Claude","meters":[],"note":null,"stale":false,"updated":{}}}"#,
                    now - 1000
                ),
            );
            let report = report(s, &["usage"]);
            assert_eq!(
                report["sections"][0]["stale"], true,
                "older than three refreshes"
            );
            assert!(s.runs("curl").is_empty(), "asked too recently to ask again");
        },
        unclock_files,
    );
}

#[test]
fn with_nothing_to_report_the_report_is_empty() {
    parity_except(
        "usage_none",
        |s| {
            s.config("agents = gemini\n");
            assert_eq!(report(s, &["usage"]), serde_json::json!({"sections": []}));
            let output = s.run(&["usage", "--sideways"]);
            assert!(!output.status.success());
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "usage: agent-notify usage [--refresh]\n"
            );
        },
        unclock_files,
    );
}
