//! agent-notify driven as the agents drive it, on scratch machines: see `support`.
//!
//! To check the binary against the shell script it replaced, name the script (it needs jq):
//! `AGENT_NOTIFY_SCRIPT=/path/to/agent-notify.sh cargo test -p agent-notify --test parity`.

mod bar;
mod fixtures;
mod hooks;
mod keep;
mod setup;
mod support;
mod usage;
