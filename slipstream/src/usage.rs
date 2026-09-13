//! How hard one minimised app is working, for its own stream of code rain: one figure from its CPU
//! and its memory, sampled once a second.
//!
//! One quantity per app, shown three ways at once — the length of the meter in its header, and
//! the colour and the speed of its rain — which is what an ambient animation can carry. Three
//! different quantities in one column (memory in the brightness, CPU in the speed, network in
//! the rests) were tried first and measured as unreadable.
//!
//! The machine's own demand, from CPU, memory, GPU and network together, lived here between the
//! two and is in the history if it is ever wanted again: `git show 8545f65:slipstream/src/usage.rs`.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

/// One core's worth of CPU fills an app's meter: desktop apps work in one thread most of the
/// time, so a whole core is "flat out" to a reader, and a build on twelve of them is no more
/// readable for being twelve times past the end of the bar.
const APP_CPU_FULL: f32 = 1.0;
/// An app's memory, on a log scale between these: everything holds some, and the difference
/// between a file manager and a browser is two orders of magnitude, not a fraction.
const APP_MEMORY_QUIET_MB: f32 = 64.0;
const APP_MEMORY_FULL_MB: f32 = 4096.0;

/// Where one app's numbers come from: its own cgroup when the desktop gave it one (so helpers
/// and tabs in other processes count too), else the process and its descendants.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Cgroup(PathBuf),
    Process(u32),
}

/// How hard one app is working, 0 to 1, on the same scale and by the same rule as the machine's
/// demand: the busier of its CPU and its memory decides.
///
/// Only CPU and memory. The GPU has no per-process counter worth reading here, and Linux keeps
/// no per-process network counters at all without a privileged eBPF helper — a number the whole
/// system shares is exactly what this meter exists to stop showing under an app's name.
#[derive(Debug)]
pub struct Meter {
    source: Option<Source>,
    /// The last sample: when, and CPU microseconds.
    last: Option<(Instant, u64)>,
    /// 0 to 1.
    pub load: f32,
    /// The two behind it, for the log and for tests.
    pub cpu: f32,
    pub memory: f32,
}

impl Meter {
    pub fn new(pid: Option<u32>) -> Self {
        let mut meter = Self {
            source: pid.map(source),
            last: None,
            load: 0.0,
            cpu: 0.0,
            memory: 0.0,
        };
        // Memory is true straight away; CPU needs an interval, so it reads zero for a second.
        meter.sample();
        meter
    }

    /// Samples again if a second has passed. Returns whether the reading changed.
    pub fn refresh(&mut self) -> bool {
        if self
            .last
            .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(1))
        {
            return false;
        }
        let before = self.load;
        self.sample();
        before != self.load
    }

    fn sample(&mut self) {
        let now = Instant::now();
        let Some((memory, cpu)) = self.source.as_ref().and_then(memory_and_cpu) else {
            self.load = 0.0;
            return;
        };
        if let Some((then, last_cpu)) = self.last {
            let seconds = now.duration_since(then).as_secs_f32().max(0.001);
            let cores = cpu.saturating_sub(last_cpu) as f32 / 1e6 / seconds;
            self.cpu = (cores / APP_CPU_FULL).clamp(0.0, 1.0);
        }
        self.memory = memory_share_mb(memory as f32 / (1024.0 * 1024.0));
        self.load = self.cpu.max(self.memory);
        self.last = Some((now, cpu));
    }
}

/// An app's memory as a share, on a log scale from `APP_MEMORY_QUIET_MB` to `APP_MEMORY_FULL_MB`.
fn memory_share_mb(mb: f32) -> f32 {
    if mb <= APP_MEMORY_QUIET_MB {
        return 0.0;
    }
    let span = (APP_MEMORY_FULL_MB / APP_MEMORY_QUIET_MB).log10();
    ((mb / APP_MEMORY_QUIET_MB).log10() / span).clamp(0.0, 1.0)
}

/// The app's cgroup if it has its own, else the process itself.
fn source(pid: u32) -> Source {
    let ours = fs::read_to_string("/proc/self/cgroup").ok();
    let theirs = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    match theirs.as_deref().and_then(cgroup_path) {
        Some(path)
            if is_app_cgroup(path) && Some(path) != ours.as_deref().and_then(cgroup_path) =>
        {
            Source::Cgroup(PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/')))
        }
        _ => Source::Process(pid),
    }
}

/// The unified hierarchy's path from `/proc/<pid>/cgroup`.
fn cgroup_path(text: &str) -> Option<&str> {
    text.lines().find_map(|line| line.strip_prefix("0::"))
}

/// A scope or service the desktop made for one app, rather than the session's own cgroup.
fn is_app_cgroup(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|leaf| {
        leaf.starts_with("app-") && (leaf.ends_with(".scope") || leaf.ends_with(".service"))
    })
}

/// Memory in bytes and CPU time in microseconds.
fn memory_and_cpu(source: &Source) -> Option<(u64, u64)> {
    match source {
        Source::Cgroup(dir) => {
            let memory = fs::read_to_string(dir.join("memory.current"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let cpu = fs::read_to_string(dir.join("cpu.stat"))
                .ok()?
                .lines()
                .find_map(|line| line.strip_prefix("usage_usec "))?
                .trim()
                .parse()
                .ok()?;
            Some((memory, cpu))
        }
        Source::Process(pid) => {
            let (mut memory, mut ticks) = (0, 0);
            for pid in process_tree(*pid) {
                let resident: u64 = fs::read_to_string(format!("/proc/{pid}/statm"))
                    .ok()
                    .and_then(|statm| statm.split_whitespace().nth(1)?.parse().ok())
                    .unwrap_or(0);
                memory += resident * 4096;
                ticks += fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .and_then(|stat| cpu_ticks(&stat))
                    .unwrap_or(0);
            }
            // Clock ticks are 1/100 s on Linux.
            (memory > 0).then_some((memory, ticks * 10_000))
        }
    }
}

/// A process and all its descendants, from the kernel's own children lists.
fn process_tree(pid: u32) -> Vec<u32> {
    let mut tree = vec![pid];
    let mut i = 0;
    while i < tree.len() && tree.len() < 512 {
        let tasks = fs::read_dir(format!("/proc/{}/task", tree[i]));
        for task in tasks.into_iter().flatten().flatten() {
            if let Ok(children) = fs::read_to_string(task.path().join("children")) {
                tree.extend(
                    children
                        .split_whitespace()
                        .filter_map(|c| c.parse::<u32>().ok()),
                );
            }
        }
        i += 1;
    }
    tree
}

/// User plus system time from `/proc/<pid>/stat`, in clock ticks.
fn cpu_ticks(stat: &str) -> Option<u64> {
    // The command name can contain spaces and brackets, so count fields from its end.
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some(user + system)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_meter_reads_memory_on_a_log_scale() {
        assert_eq!(memory_share_mb(16.0), 0.0, "less than a browser tab");
        assert_eq!(memory_share_mb(APP_MEMORY_QUIET_MB), 0.0);
        // Half way up the scale in orders of magnitude, not in megabytes.
        let half = memory_share_mb(512.0);
        assert!((half - 0.5).abs() < 0.01, "512 MB should be half: {half}");
        assert_eq!(memory_share_mb(APP_MEMORY_FULL_MB), 1.0);
        assert_eq!(memory_share_mb(64_000.0), 1.0, "clamped, not past the end");
    }

    #[test]
    fn app_cgroups_are_told_apart_from_the_session() {
        let text = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.kate@55f8.service\n";
        let path = cgroup_path(text).unwrap();
        assert!(is_app_cgroup(path));
        assert!(is_app_cgroup(
            "/user.slice/x/app.slice/app-flatpak-org.mozilla.firefox-12.scope"
        ));
        assert!(!is_app_cgroup(
            "/user.slice/user-1000.slice/session-5.scope"
        ));
    }

    #[test]
    fn cpu_time_survives_a_command_name_with_spaces() {
        let stat = "4242 (Web Content) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0 100 1000 200";
        assert_eq!(cpu_ticks(stat), Some(300));
    }

    #[test]
    fn an_app_with_no_process_behind_it_reads_as_nothing() {
        let mut meter = Meter::new(None);
        assert_eq!(meter.load, 0.0);
        meter.refresh();
        assert_eq!(meter.load, 0.0);
    }

    #[test]
    fn this_process_can_be_measured() {
        let meter = Meter::new(Some(std::process::id()));
        // Whatever it is using, memory is readable and the reading is in range.
        assert!((0.0..=1.0).contains(&meter.load));
        assert!((0.0..=1.0).contains(&meter.memory));
    }
}
