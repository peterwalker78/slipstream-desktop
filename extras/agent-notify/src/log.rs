//! Where hooks report what went wrong, since the agent must see nothing from them:
//! `$XDG_STATE_HOME/agent-notify/agent-notify.log`, kept short by starting it again (with the
//! previous one as `.old`) once it passes a size.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::env::Env;

const MAX_BYTES: u64 = 256 * 1024;

pub fn error(env: &Env, context: &str, message: &dyn std::fmt::Display) {
    let path = env.log_file();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = fs::rename(&path, path.with_extension("log.old"));
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{secs} {context}: {message}");
    }
}
