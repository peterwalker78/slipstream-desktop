//! What an agent is working on, in a few words, for the desktop's trace: the title the agent has
//! given the session where it keeps one, or else how the prompt begins, from the hook or the
//! transcript.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use crate::payload::{Key, Payload, at, first, to_text};
use crate::text;

/// How much of the end of a transcript is read: titles are written again as a session goes on,
/// so the newest is near the end.
const TAIL_BYTES: u64 = 400_000;

/// The task, at most 60 characters on one line; empty if nothing says.
pub fn task(payload: &Payload) -> String {
    let tail = payload
        .text(&[&[Key("transcript_path")], &[Key("transcriptPath")]])
        .and_then(|path| read_tail(Path::new(&path)));
    let mut task = tail.as_deref().and_then(title_in).unwrap_or_default();
    if task.is_empty() {
        task = payload.text(&[&[Key("prompt")]]).unwrap_or_default();
        task.truncate(text::chomp(&task).len());
    }
    if task.is_empty() {
        task = tail.as_deref().and_then(request_in).unwrap_or_default();
    }
    let task = text::strip_controls(&task.replace('\n', " "), false);
    text::cut(&task, 60).to_owned()
}

fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The newest line of the transcript that has `"type": "TYPE"` in it, as JSON.
fn newest(tail: &str, types: &[&str]) -> Option<Value> {
    let line = tail.lines().rev().find(|line| {
        types.iter().any(|kind| {
            line.contains(&format!("\"type\":\"{kind}\""))
                || line.contains(&format!("\"type\": \"{kind}\""))
        })
    })?;
    serde_json::from_str(line).ok()
}

/// The session's newest title, given by the person or the agent.
fn title_in(tail: &str) -> Option<String> {
    let entry = newest(tail, &["ai-title", "custom-title"])?;
    let title = first(
        &entry,
        &[&[Key("customTitle")], &[Key("aiTitle")], &[Key("title")]],
    )?;
    Some(text::chomp(&to_text(title)).to_owned())
}

/// Antigravity sends no prompt, but its transcript keeps each one in a USER_REQUEST tag.
fn request_in(tail: &str) -> Option<String> {
    let entry = newest(tail, &["USER_INPUT"])?;
    let content = at(&entry, &[Key("content")])?.as_str()?;
    let (_, request) = content.split_once("<USER_REQUEST>")?;
    let request = request.trim_start_matches(char::is_whitespace);
    let request = request.split('<').next().unwrap_or_default();
    Some(text::chomp(request.trim_end_matches(char::is_whitespace)).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(name: &str, lines: &[&str]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("agent-notify-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn payload(json: serde_json::Value) -> Payload {
        Payload::parse(&json.to_string())
    }

    #[test]
    fn the_newest_title_wins_over_the_prompt() {
        let path = transcript(
            "titles",
            &[
                r#"{"type":"ai-title","aiTitle":"Old name"}"#,
                r#"{"type":"user","message":"hello"}"#,
                r#"{"type": "custom-title", "customTitle": "Kiln timer\nrework"}"#,
            ],
        );
        let task = task(&payload(
            serde_json::json!({"transcript_path": path, "prompt": "ignored"}),
        ));
        assert_eq!(task, "Kiln timer rework");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn without_a_title_the_prompt_begins_the_task() {
        let prompt = format!("Make the timer count down\n\n{}", "x".repeat(80));
        let task = task(&payload(serde_json::json!({"prompt": prompt})));
        assert_eq!(task.chars().count(), 60);
        assert!(task.starts_with("Make the timer count down  x"));
        let trailing = super::task(&payload(serde_json::json!({"prompt": "Fix it\n\n"})));
        assert_eq!(
            trailing, "Fix it",
            "newlines at the end go, as a command's output loses them"
        );
    }

    #[test]
    fn antigravity_s_request_is_read_from_its_transcript() {
        let path = transcript(
            "agy",
            &[
                r#"{"type":"USER_INPUT","content":"<USER_REQUEST>\n  first one\n</USER_REQUEST>"}"#,
                r#"{"type":"USER_INPUT","content":"ctx <USER_REQUEST> Tidy the ledger\t\n</USER_REQUEST><meta/>"}"#,
            ],
        );
        let task = task(&payload(serde_json::json!({"transcriptPath": path})));
        assert_eq!(task, "Tidy the ledger");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn nothing_to_go_on_is_no_task() {
        assert_eq!(task(&Payload::parse("{}")), "");
        assert_eq!(task(&Payload::parse("garbage")), "");
    }
}
