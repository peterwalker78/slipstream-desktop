//! The config file, `~/.config/agent-notify/config`: one `key = value` per line, the last one for
//! a key winning, and `#` starting a comment line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::agents;
use crate::state;
use crate::text::is_number;

pub struct Config {
    path: PathBuf,
    text: String,
}

/// The value on `line` if it sets `key`.
fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(key)?;
    let rest = rest.trim_start().strip_prefix('=')?;
    Some(rest.trim())
}

impl Config {
    pub fn load(path: PathBuf) -> Config {
        let text = fs::read_to_string(&path).unwrap_or_default();
        Config { path, text }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value set for `key`, if it has one.
    pub fn get(&self, key: &str) -> Option<&str> {
        let value = self
            .text
            .lines()
            .rev()
            .find_map(|line| value_of(line, key))?;
        (!value.is_empty()).then_some(value)
    }

    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    /// A whole number for `key`, or `default` if it isn't one.
    pub fn number(&self, key: &str, default: u64) -> u64 {
        self.get(key)
            .filter(|v| is_number(v))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// The agents to wire up and report on.
    pub fn agents(&self) -> Vec<&str> {
        self.get("agents")
            .map(|v| v.split_whitespace().collect())
            .unwrap_or_default()
    }

    pub fn wants(&self, agent: &str) -> bool {
        self.agents().contains(&agent)
    }

    pub fn long_task_secs(&self) -> u64 {
        self.number("long-task-secs", 120)
    }

    pub fn sound_finished(&self) -> &str {
        self.get_or("sound-finished", "complete")
    }

    pub fn sound_attention(&self) -> &str {
        self.get_or("sound-attention", "window-attention")
    }

    /// Saves `agents` as the ones to wire up, in place of the line that names them.
    pub fn set_agents(&mut self, agents: &[String]) -> io::Result<()> {
        let line = format!("agents = {}", agents.join(" "));
        let mut found = false;
        let mut text = String::with_capacity(self.text.len());
        for old in self.text.lines() {
            if value_of(old, "agents").is_some() {
                text.push_str(&line);
                found = true;
            } else {
                text.push_str(old);
            }
            text.push('\n');
        }
        if !found {
            text.push_str(&line);
            text.push('\n');
        }
        state::write_private(&self.path, text.as_bytes())?;
        self.text = text;
        Ok(())
    }
}

/// The config written the first time, naming the agents that are installed.
pub fn default_text(detected: &[&str]) -> String {
    format!(
        "# agent-notify. Run \"agent-notify setup\" after changing agents.

# The agents to wire up, any of: {known}
agents = {detected}

# A task that runs this long, in seconds, is announced with a sound, and as critical if it's waiting
# on you. Shorter ones get a quiet notification.
long-task-secs = 120

# Sound theme names (freedesktop sound naming), for the long tasks.
sound-finished = complete
sound-attention = window-attention
",
        known = agents::KNOWN.join(" "),
        detected = detected.join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config {
            path: PathBuf::from("/nonexistent"),
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_last_line_for_a_key_wins_and_comments_do_not_count() {
        let config = config("agents = claude\n# agents = codex\n  agents=gemini agy  \n");
        assert_eq!(config.agents(), ["gemini", "agy"]);
    }

    #[test]
    fn a_key_is_matched_whole() {
        let config = config("long-task-secs-extra = 5\nlong-task-secs = 300 \n");
        assert_eq!(config.long_task_secs(), 300);
        assert_eq!(config.get("long-task"), None);
    }

    #[test]
    fn an_empty_or_unreadable_value_falls_back_to_the_default() {
        let config = config("long-task-secs = soon\nsound-finished =\n");
        assert_eq!(config.long_task_secs(), 120);
        assert_eq!(config.sound_finished(), "complete");
    }

    #[test]
    fn the_default_config_names_what_was_found() {
        let text = default_text(&["claude", "codex"]);
        let config = config(&text);
        assert_eq!(config.agents(), ["claude", "codex"]);
        assert_eq!(config.sound_attention(), "window-attention");
    }
}
