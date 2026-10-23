//! `bar`: a line of JSON each time the agents at work change, for a bar module.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::{Imp, Scratch};

#[test]
fn the_bar_module_follows_the_working_notes() {
    let s = Scratch::new("bar", Imp::Binary);
    s.desktop();
    let mut bar = s
        .command(&["bar"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (sender, lines) = mpsc::channel();
    let stdout = bar.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender
                .send(serde_json::from_str::<Value>(&line).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let next = || {
        lines
            .recv_timeout(Duration::from_secs(5))
            .expect("a line from the bar module")
    };
    assert_eq!(
        next(),
        json!({"text": ""}),
        "nothing at work: an empty module"
    );

    // This test's own process stands in for an agent's.
    let pid = std::process::id();
    let note = format!("run/slipstream/working/{pid}");
    s.write(&note, "claude: Kiln & timer\n\nreading the tests\n");
    assert_eq!(
        next(),
        json!({"text": "1 working", "tooltip": "claude: Kiln &amp; timer\n  reading the tests", "class": "working"})
    );
    s.write(&format!("run/agent-notify/{pid}.waiting"), "");
    s.write(&note, "claude: Kiln & timer\nidle\n");
    assert_eq!(
        next(),
        json!({"text": "1 waiting", "tooltip": "claude: Kiln &amp; timer (waiting on you)", "class": "warning"})
    );
    std::fs::remove_file(s.path(&note)).unwrap();
    assert_eq!(next(), json!({"text": ""}));
    bar.kill().unwrap();
    bar.wait().unwrap();
}
