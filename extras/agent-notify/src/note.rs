//! The working note: what a desktop that shows agents at work reads about each session.
//!
//! The note is a file named for the agent's process ID. Its first line names the trace (the agent,
//! and what it is working on); the second says `idle`, or nothing while the agent works; each line
//! after is a subagent it has running, by its task. It stays while the session is open, and the
//! desktop cleans up the notes of processes that have gone.

use std::io;
use std::path::Path;

use crate::state;

pub struct Note {
    pub name: String,
    pub idle: bool,
    pub helpers: Vec<String>,
}

impl Note {
    pub fn render(&self) -> String {
        let mut text = format!("{}\n{}\n", self.name, if self.idle { "idle" } else { "" });
        for helper in &self.helpers {
            text.push_str(helper);
            text.push('\n');
        }
        text
    }

    pub fn parse(text: &str) -> Note {
        let mut lines = text.lines();
        let name = lines.next().unwrap_or_default().to_owned();
        let idle = lines.next() == Some("idle");
        let helpers = lines
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        Note {
            name,
            idle,
            helpers,
        }
    }

    /// Writes the note whole and moves it into place, so the desktop never reads half of one,
    /// and a subagent's hook running alongside writes its own whole note rather than into this.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        state::write_whole(path, self.render().as_bytes(), 0o666)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_reads_back_as_it_was_written() {
        let note = Note {
            name: "claude: fix the kiln".into(),
            idle: false,
            helpers: vec!["reading the tests".into(), "fixing the docs".into()],
        };
        let text = note.render();
        assert_eq!(
            text,
            "claude: fix the kiln\n\nreading the tests\nfixing the docs\n"
        );
        let back = Note::parse(&text);
        assert_eq!(
            (back.name.as_str(), back.idle, back.helpers.len()),
            ("claude: fix the kiln", false, 2)
        );
        let idle = Note {
            name: "gemini: ledger".into(),
            idle: true,
            helpers: vec![],
        };
        assert_eq!(idle.render(), "gemini: ledger\nidle\n");
    }
}
