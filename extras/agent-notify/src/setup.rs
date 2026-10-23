//! `agent-notify setup [AGENT…]` and `setup --remove`: puts the hooks into each agent's settings,
//! or takes them out, beside whatever else is there.
//!
//! A settings file is rewritten whole and moved into place, keeping its mode, the order of its
//! keys and every hook that isn't this tool's, after a one-time backup beside it
//! (`FILE.agent-notify.bak`). A missing file starts empty; one that isn't JSON is left alone.

use std::fs::{self, FileTimes};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value, json};

use crate::agents::{self, Wiring, codex};
use crate::config::{self, Config};
use crate::env::Env;
use crate::payload::present;
use crate::state;
use crate::text::shell_quote;

pub fn run(env: &Env, args: &[String]) -> ExitCode {
    let (remove, named) = match args {
        [first, rest @ ..] if first == "--remove" => (true, rest),
        _ => (false, args),
    };
    write_default_config(env);
    let mut config = Config::load(env.config_file());
    if !named.is_empty() && !remove {
        if let Some(unknown) = named
            .iter()
            .find(|name| !agents::KNOWN.contains(&name.as_str()))
        {
            eprintln!(
                "unknown agent: {unknown} (known: {})",
                agents::KNOWN.join(" ")
            );
            return ExitCode::FAILURE;
        }
        if let Err(err) = config.set_agents(named) {
            eprintln!(
                "couldn't save the agents in {}: {err}",
                config.path().display()
            );
        }
    }
    let mut failed = false;
    for agent in agents::KNOWN {
        let name = agents::display_name(agent);
        // Removing, or an agent no longer listed: take its hooks out if it has any.
        if remove || !config.wants(agent) {
            if is_wired(env, agent) {
                if wire(env, agent, false) {
                    println!("{name}: hooks removed");
                } else {
                    failed = true;
                }
            }
        } else {
            if !env.have(agent) {
                println!("{name}: not installed, wiring it anyway");
            }
            if wire(env, agent, true) {
                if agent == "codex" {
                    println!("{}", codex::TRUST_HINT);
                }
                println!("{name}: hooks in place");
            } else {
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Writes the config the first time, with the agents that are installed.
fn write_default_config(env: &Env) {
    let path = env.config_file();
    if path.exists() {
        return;
    }
    let detected: Vec<&str> = agents::KNOWN
        .into_iter()
        .filter(|agent| env.have(agent))
        .collect();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match fs::write(&path, config::default_text(&detected)) {
        Ok(()) => {
            let found = if detected.is_empty() {
                "none found".to_owned()
            } else {
                detected.join(" ")
            };
            println!("Wrote {} (agents: {found})", path.display());
        }
        Err(err) => eprintln!("couldn't write {}: {err}", path.display()),
    }
}

/// Whether an agent's settings hold any of this tool's hooks.
pub fn is_wired(env: &Env, agent: &str) -> bool {
    let Some(text) = agents::hooks_file(env, agent).and_then(|file| fs::read_to_string(file).ok())
    else {
        return false;
    };
    text.lines().any(|line| {
        line.find("agent-notify")
            .is_some_and(|at| line[at..].contains(" hook "))
    })
}

/// Puts an agent's hooks in place (`install`) or takes them out, saying what went wrong.
fn wire(env: &Env, agent: &str, install: bool) -> bool {
    let quoted = shell_quote(&env.self_path().to_string_lossy());
    let command = |event: &str| format!("{quoted} hook {agent} {event}");
    let (Some(file), Some(wiring)) = (
        agents::hooks_file(env, agent),
        agents::wiring(agent, &command),
    ) else {
        return false;
    };
    merge(&file, |settings| match wiring {
        Wiring::Groups(groups) => {
            let groups = if install { groups } else { json!({}) };
            merge_groups(
                settings,
                groups.as_object().expect("hook groups are an object"),
            )
        }
        Wiring::Entry(hooks) => {
            let settings = settings.as_object_mut().ok_or(())?;
            if install {
                settings.insert("agent-notify".into(), hooks);
            } else {
                settings.shift_remove("agent-notify");
            }
            Ok(())
        }
    })
}

/// Rewrites a JSON settings file through `edit`.
fn merge(file: &Path, edit: impl FnOnce(&mut Value) -> Result<(), ()>) -> bool {
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut mode = 0o600;
    let mut settings = if file.exists() {
        let parsed = fs::read(file)
            .ok()
            .and_then(|text| serde_json::from_slice::<Value>(&text).ok());
        let Some(settings) = parsed.filter(|value| present(value).is_some()) else {
            eprintln!("  {} isn't valid JSON; leaving it alone", file.display());
            return false;
        };
        if let Err(err) = back_up(file) {
            eprintln!("  couldn't back up {}: {err}", file.display());
            return false;
        }
        mode = fs::metadata(file).map_or(mode, |meta| meta.permissions().mode() & 0o7777);
        settings
    } else {
        json!({})
    };
    let written = edit(&mut settings)
        .ok()
        .map(|()| pretty(&settings))
        .map(|text| state::replace_with_mode(file, text.as_bytes(), mode));
    if !matches!(written, Some(Ok(()))) {
        eprintln!("  couldn't update {}", file.display());
        return false;
    }
    true
}

/// A copy of the file as it was before this tool first changed it, with its mode and times.
fn back_up(file: &Path) -> std::io::Result<()> {
    let mut backup = file.as_os_str().to_owned();
    backup.push(".agent-notify.bak");
    let backup = Path::new(&backup);
    if backup.exists() {
        return Ok(());
    }
    fs::copy(file, backup)?;
    let meta = fs::metadata(file)?;
    let times = FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?);
    fs::File::options()
        .write(true)
        .open(backup)?
        .set_times(times)
}

/// JSON indented by two spaces, with a newline at the end.
fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("a JSON value serializes");
    text.push('\n');
    text
}

/// Whether a hook group runs this tool's hooks.
fn is_ours(group: &Value) -> Result<bool, ()> {
    let hooks: Vec<&Value> = match group.get("hooks") {
        Some(Value::Array(hooks)) => hooks.iter().collect(),
        Some(Value::Object(hooks)) => hooks.values().collect(),
        _ => return Ok(false),
    };
    for hook in hooks {
        match hook.get("command").and_then(present) {
            None => {}
            Some(Value::String(command))
                if command.contains("agent-notify") && command.contains(" hook ") =>
            {
                return Ok(true);
            }
            Some(Value::String(_)) => {}
            Some(_) => return Err(()),
        }
    }
    Ok(false)
}

/// Takes every hook group whose commands are this tool's out of `settings.hooks`, then adds
/// `groups`, an object of event name to one group each. Events left with no groups are dropped,
/// and `hooks` itself if nothing is left in it.
fn merge_groups(settings: &mut Value, groups: &Map<String, Value>) -> Result<(), ()> {
    let settings = settings.as_object_mut().ok_or(())?;
    let old = match settings.get("hooks") {
        None | Some(Value::Null | Value::Bool(false)) => Map::new(),
        Some(Value::Object(hooks)) => hooks.clone(),
        Some(_) => return Err(()),
    };
    let mut hooks = Map::new();
    for (event, list) in old {
        let Value::Array(list) = list else {
            return Err(());
        };
        let mut kept = Vec::new();
        for group in list {
            if !is_ours(&group)? {
                kept.push(group);
            }
        }
        hooks.insert(event, Value::Array(kept));
    }
    for (event, group) in groups {
        let added = match group {
            Value::Array(list) => list.clone(),
            other => vec![other.clone()],
        };
        match hooks.get_mut(event) {
            Some(Value::Array(list)) => list.extend(added),
            _ => {
                hooks.insert(event.clone(), Value::Array(added));
            }
        }
    }
    hooks.retain(|_, list| list.as_array().is_some_and(|list| !list.is_empty()));
    if hooks.is_empty() {
        settings.shift_remove("hooks");
    } else {
        settings.insert("hooks".into(), Value::Object(hooks));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(event: &str) -> Value {
        json!({"hooks": [{"type": "command", "command": format!("'/opt/x/agent-notify' hook claude {event}")}]})
    }

    #[test]
    fn the_person_s_own_hooks_stay_where_they_were() {
        let mut settings = json!({
            "theme": "dark",
            "hooks": {
                "PostToolUse": [{"matcher": "Edit", "hooks": [{"type": "command", "command": "fmt-on-save"}]}, ours("working")],
                "Stop": [ours("finish")],
            },
            "model": "big",
        });
        let groups = json!({"Stop": ours("finish"), "PostToolUse": ours("working"), "SessionStart": ours("session_start")});
        merge_groups(&mut settings, groups.as_object().unwrap()).unwrap();
        let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["theme", "hooks", "model"]);
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(
            hooks.keys().collect::<Vec<_>>(),
            ["PostToolUse", "Stop", "SessionStart"]
        );
        assert_eq!(
            hooks["PostToolUse"][0]["hooks"][0]["command"],
            "fmt-on-save"
        );
        assert_eq!(hooks["PostToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(hooks["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn removing_drops_emptied_events_and_an_empty_hooks() {
        let mut settings = json!({"hooks": {"Stop": [ours("finish")]}, "theme": "dark"});
        merge_groups(&mut settings, &Map::new()).unwrap();
        assert_eq!(settings, json!({"theme": "dark"}));
        let mut kept =
            json!({"hooks": {"Stop": [ours("finish"), {"hooks": [{"command": "say done"}]}]}});
        merge_groups(&mut kept, &Map::new()).unwrap();
        assert_eq!(kept["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn settings_of_an_unexpected_shape_are_not_rewritten() {
        assert!(merge_groups(&mut json!([1, 2]), &Map::new()).is_err());
        assert!(merge_groups(&mut json!({"hooks": "none"}), &Map::new()).is_err());
        assert!(
            merge_groups(
                &mut json!({"hooks": {"Stop": [{"hooks": [{"command": 7}]}]}}),
                &Map::new()
            )
            .is_err()
        );
    }

    #[test]
    fn written_settings_are_indented_by_two_spaces() {
        assert_eq!(
            pretty(&json!({"a": [], "b": {}, "c": [1]})),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1\n  ]\n}\n"
        );
    }
}
