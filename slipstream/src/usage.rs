//! How hard one minimised app is working, for its own stream: one figure from its CPU, its
//! memory and its network traffic, sampled once a second.
//!
//! One quantity per app, shown three ways at once — the length of the meter in its header, and
//! the colour and the speed of its rain — which is what an ambient animation can carry. Three
//! different quantities in one column (memory in the brightness, CPU in the speed, and so on)
//! don't read.
//!
//! Each of the three is scored on what the app is doing, never on what it holds: processor time
//! spent, memory taken or given back, bytes sent and received. An app sitting on two gigabytes
//! and doing nothing is at rest. The busiest of the three is the reading, which rises the moment
//! work starts and drains away over a few seconds once it stops, so work that comes in bursts
//! reads as work and a finished job is seen winding down.
//!
//! The reading is taken on a thread of its own, since walking a browser's processes and their
//! open files is no work for the one that draws; that thread sleeps while nothing is minimised.

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, PoisonError, Weak,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::tcp;

/// How much of an app's meter its first busy core takes. Most apps work in one thread, so one
/// core flat out has to read as real work on any machine; the rest of the meter is shared evenly
/// between the machine's other processors, so each further core is the same step up.
const APP_CPU_FIRST_CORE: f32 = 0.3;
/// The curve work within that first core is read on, so that light work shows: a tenth of a
/// core is nearly half way to a whole one.
const APP_CPU_CURVE: f32 = 1.0 / 3.0;
/// Memory taken or given back each second, on a log scale between these. Every running app's
/// memory wanders by a megabyte or two, which is not work.
const APP_MEMORY_STILL_MB: f32 = 4.0;
const APP_MEMORY_FULL_MB: f32 = 512.0;
/// Bytes sent and received each second, on a log scale between these: a keep-alive is quiet, a
/// download on a good line is flat out, and what lies between spans three orders of magnitude.
const APP_NETWORK_QUIET_KB: f32 = 1.0;
const APP_NETWORK_FULL_KB: f32 = 4096.0;
/// The most traffic alone can read: level with one busy core. Moving bytes is the machine being
/// used, not the machine being worked, so a download at full tilt shows without passing for a
/// build.
const APP_NETWORK_MOST: f32 = 0.3;
/// How long a reading takes to fall half way to a lower one.
const HALF_LIFE_SECS: f32 = 3.0;
/// How often every watched app is read.
const EVERY: Duration = Duration::from_secs(1);
/// Two looks closer together than this say nothing reliable about a rate: processor time comes
/// in hundredths of a second.
const SHORTEST: Duration = Duration::from_millis(500);

/// Where one app's numbers come from: its own cgroup when the desktop gave it one (so helpers
/// and tabs in other processes count too), else the process and its descendants.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Cgroup(PathBuf),
    Process(u32),
}

/// How hard one app is working, 0 to 1, and the three scores behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Reading {
    load: f32,
    cpu: f32,
    memory: f32,
    network: f32,
}

/// One app's running totals, as the kernel has them now.
struct Counters {
    /// Bytes of memory held.
    memory: u64,
    /// Processor time used, in microseconds.
    cpu: u64,
    /// The processes they were added up from.
    pids: Vec<u32>,
}

/// How hard one app is working, 0 to 1, for whoever draws it.
///
/// The GPU is left out: it has no per-process counter worth reading here.
#[derive(Debug)]
pub struct Meter {
    /// The reading as the sampling thread last left it. None for a window with no process to
    /// read.
    shared: Option<Arc<Mutex<Reading>>>,
    /// 0 to 1.
    pub load: f32,
}

impl Meter {
    pub fn new(pid: Option<u32>) -> Self {
        Self {
            shared: pid.map(|pid| watch(Probe::new(pid))),
            load: 0.0,
        }
    }

    /// Picks up the latest reading.
    pub fn refresh(&mut self) {
        if let Some(shared) = &self.shared {
            self.load = shared.lock().unwrap_or_else(PoisonError::into_inner).load;
        }
    }
}

/// What is remembered about one app between looks.
struct Probe {
    source: Source,
    /// The last look: when, and the totals then.
    last: Option<(Instant, u64, u64)>,
    /// What each of the app's connections had carried at the last look, by socket inode.
    connections: HashMap<u64, u64>,
    reading: Reading,
}

impl Probe {
    fn new(pid: u32) -> Self {
        Self {
            source: source(pid),
            last: None,
            connections: HashMap::new(),
            reading: Reading::default(),
        }
    }

    /// Reads the app's totals and scores what it did since the last look.
    fn sample(&mut self, now: Instant, tcp: &HashMap<u64, u64>) {
        if self
            .last
            .is_some_and(|(then, ..)| now.duration_since(then) < SHORTEST)
        {
            return;
        }
        let Some(counters) = counters(&self.source) else {
            // The app has gone; so has whatever it was doing.
            self.last = None;
            self.reading = Reading::default();
            return;
        };
        // No connections anywhere, or a kernel that didn't list them this time: no files to
        // walk, and what is known of the app's connections stands until there is a list again.
        let carried = if tcp.is_empty() {
            0
        } else {
            let totals = counters
                .pids
                .iter()
                .flat_map(|&pid| tcp::sockets_of(pid))
                .filter_map(|inode| Some((inode, *tcp.get(&inode)?)));
            carried(&mut self.connections, self.last.is_none(), totals)
        };
        self.score(now, counters.cpu, counters.memory, carried);
    }

    /// Turns totals into a reading: the busiest of the three, risen to at once and fallen from
    /// slowly. The first look only notes where the totals stand.
    fn score(&mut self, now: Instant, cpu: u64, memory: u64, carried: u64) {
        if let Some((then, last_cpu, last_memory)) = self.last {
            let seconds = now.duration_since(then).as_secs_f32().max(0.001);
            let cores = cpu.saturating_sub(last_cpu) as f32 / 1e6 / seconds;
            let moved_mb = memory.abs_diff(last_memory) as f32 / (1024.0 * 1024.0) / seconds;
            let carried_kb = carried as f32 / 1024.0 / seconds;
            let reading = &mut self.reading;
            reading.cpu = cpu_share(cores, processors());
            reading.memory = log_share(moved_mb, APP_MEMORY_STILL_MB, APP_MEMORY_FULL_MB);
            reading.network =
                APP_NETWORK_MOST * log_share(carried_kb, APP_NETWORK_QUIET_KB, APP_NETWORK_FULL_KB);
            let busiest = reading.cpu.max(reading.memory).max(reading.network);
            reading.load = settle(reading.load, busiest, seconds);
        }
        self.last = Some((now, cpu, memory));
    }
}

/// An app's CPU as a share of what the machine has: the first core's worth takes the first
/// `APP_CPU_FIRST_CORE` of the meter, on a curve, and every core after it an equal step of what
/// is left. Of twelve, one core reads 0.30, four about a half, nine about four fifths.
fn cpu_share(cores: f32, processors: f32) -> f32 {
    let processors = processors.max(1.0);
    let cores = cores.clamp(0.0, processors);
    // With one processor there are no further cores to leave room for.
    let first = if processors > 1.0 {
        APP_CPU_FIRST_CORE
    } else {
        1.0
    };
    if cores <= 1.0 {
        return first * cores.powf(APP_CPU_CURVE);
    }
    first + (1.0 - first) * (cores - 1.0) / (processors - 1.0)
}

/// How many processors there are to be busy on.
fn processors() -> f32 {
    static PROCESSORS: OnceLock<f32> = OnceLock::new();
    *PROCESSORS.get_or_init(|| thread::available_parallelism().map_or(1.0, |n| n.get() as f32))
}

/// A rate as a share, on a log scale from `quiet` to `full`.
fn log_share(rate: f32, quiet: f32, full: f32) -> f32 {
    if rate <= quiet {
        return 0.0;
    }
    ((rate / quiet).log10() / (full / quiet).log10()).clamp(0.0, 1.0)
}

/// The reading after `seconds`: up to a busier score at once, down towards a quieter one by half
/// the distance every `HALF_LIFE_SECS`.
fn settle(was: f32, busiest: f32, seconds: f32) -> f32 {
    if busiest >= was {
        return busiest;
    }
    busiest + (was - busiest) * 0.5_f32.powf(seconds / HALF_LIFE_SECS)
}

/// The bytes an app's connections have carried since the last look, given each one's total now.
/// `known` is left holding those totals for next time.
///
/// A connection not seen before is counted from nothing, since it was opened since the last
/// look, except on the first look of all, when every connection is new and none of what they
/// have carried is news.
fn carried(
    known: &mut HashMap<u64, u64>,
    first: bool,
    totals: impl Iterator<Item = (u64, u64)>,
) -> u64 {
    let mut now = HashMap::new();
    let mut bytes = 0u64;
    for (inode, total) in totals {
        // A connection shared between processes turns up once for each of them.
        if now.insert(inode, total).is_some() {
            continue;
        }
        let before = if first {
            total
        } else {
            known.get(&inode).copied().unwrap_or(0)
        };
        bytes = bytes.saturating_add(total.saturating_sub(before));
    }
    *known = now;
    bytes
}

/// An app on its way to the sampling thread: what to read, and where to leave the reading. The
/// thread stops reading an app once nothing holds the other end.
type Watched = (Probe, Weak<Mutex<Reading>>);

/// Hands an app to the sampling thread, starting the thread if this is the first.
fn watch(probe: Probe) -> Arc<Mutex<Reading>> {
    static SAMPLER: OnceLock<Sender<Watched>> = OnceLock::new();
    let shared = Arc::new(Mutex::new(Reading::default()));
    let sender = SAMPLER.get_or_init(|| {
        let (sender, arrivals) = mpsc::channel();
        let started = thread::Builder::new()
            .name("usage".into())
            .spawn(move || sample_all(&arrivals));
        if let Err(error) = started {
            tracing::warn!(%error, "no thread to read apps' usage on; streams will read as idle");
        }
        sender
    });
    let _ = sender.send((probe, Arc::downgrade(&shared)));
    shared
}

/// Reads every watched app once a second, for as long as there are any.
fn sample_all(arrivals: &Receiver<Watched>) {
    let mut watched: Vec<Watched> = Vec::new();
    let mut next = Instant::now();
    loop {
        let arrival = if watched.is_empty() {
            // Nothing to read, so nothing to wake for.
            arrivals.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            arrivals.recv_timeout(next.saturating_duration_since(Instant::now()))
        };
        match arrival {
            Ok((mut probe, shared)) => {
                // Note where its totals stand now, so its first reading comes a second from
                // here rather than whenever the round after next happens to fall.
                if watched.is_empty() {
                    next = Instant::now() + EVERY;
                }
                probe.sample(Instant::now(), &tcp::bytes_by_inode());
                watched.push((probe, shared));
            }
            Err(RecvTimeoutError::Timeout) => {
                next = Instant::now() + EVERY;
                let tcp = tcp::bytes_by_inode();
                let now = Instant::now();
                watched.retain_mut(|(probe, shared)| {
                    let Some(shared) = shared.upgrade() else {
                        return false;
                    };
                    probe.sample(now, &tcp);
                    let reading = probe.reading;
                    tracing::debug!(
                        source = ?probe.source,
                        cpu = reading.cpu,
                        memory = reading.memory,
                        network = reading.network,
                        load = reading.load,
                        "usage"
                    );
                    *shared.lock().unwrap_or_else(PoisonError::into_inner) = reading;
                    true
                });
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
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

fn counters(source: &Source) -> Option<Counters> {
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
            let pids = fs::read_to_string(dir.join("cgroup.procs"))
                .unwrap_or_default()
                .lines()
                .filter_map(|pid| pid.parse().ok())
                .collect();
            Some(Counters { memory, cpu, pids })
        }
        Source::Process(pid) => {
            let pids = process_tree(*pid);
            let (mut memory, mut ticks) = (0, 0);
            for pid in &pids {
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
            (memory > 0).then_some(Counters {
                memory,
                cpu: ticks * 10_000,
                pids,
            })
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

    const MB: u64 = 1024 * 1024;
    const SECOND: Duration = Duration::from_secs(1);

    /// The processor time, in microseconds, of every processor busy for `seconds`.
    fn flat_out(seconds: u64) -> u64 {
        (processors() * 1e6) as u64 * seconds
    }

    /// A probe that has had its first look, with `memory` held and nothing done yet.
    fn probe_holding(memory: u64, at: Instant) -> Probe {
        let mut probe = Probe::new(std::process::id());
        probe.score(at, 0, memory, 0);
        probe
    }

    #[test]
    fn a_large_app_at_rest_reads_as_quiet() {
        let start = Instant::now();
        let mut probe = probe_holding(2048 * MB, start);
        for second in 1..=5 {
            probe.score(start + SECOND * second, 0, 2048 * MB, 0);
            assert_eq!(probe.reading.load, 0.0, "holding memory is not work");
        }
    }

    #[test]
    fn light_work_shows() {
        assert_eq!(cpu_share(0.0, 12.0), 0.0);
        let light = cpu_share(0.1, 12.0);
        assert!((0.12..0.16).contains(&light), "a tenth of a core: {light}");
        assert_eq!(cpu_share(1.0, 12.0), APP_CPU_FIRST_CORE);
        assert_eq!(cpu_share(12.0, 12.0), 1.0);
        assert_eq!(cpu_share(40.0, 12.0), 1.0, "clamped, not past the end");
        // One busy core reads the same whatever the machine, unless it is all the machine has.
        assert_eq!(cpu_share(1.0, 4.0), cpu_share(1.0, 64.0));
        assert_eq!(cpu_share(1.0, 1.0), 1.0);
    }

    #[test]
    fn every_core_past_the_first_is_the_same_step() {
        let step = cpu_share(2.0, 12.0) - cpu_share(1.0, 12.0);
        assert!(step > 0.06, "a step too small to see: {step}");
        for cores in 2..=12 {
            let rise = cpu_share(cores as f32, 12.0) - cpu_share(cores as f32 - 1.0, 12.0);
            assert!(
                (rise - step).abs() < 1e-4,
                "core {cores} adds {rise}, not {step}"
            );
        }
    }

    #[test]
    fn memory_counts_while_it_moves() {
        let start = Instant::now();
        let mut probe = probe_holding(500 * MB, start);
        // Taking 45 MB in a second is about half way up the scale.
        probe.score(start + SECOND, 0, 545 * MB, 0);
        assert!((probe.reading.memory - 0.5).abs() < 0.01);
        // Giving it back is work too.
        probe.score(start + SECOND * 2, 0, 500 * MB, 0);
        assert!((probe.reading.memory - 0.5).abs() < 0.01);
        // A megabyte or two of wander isn't.
        probe.score(start + SECOND * 3, 0, 502 * MB, 0);
        assert_eq!(probe.reading.memory, 0.0);
    }

    #[test]
    fn network_is_read_on_a_log_scale() {
        let share = |kb: f32| log_share(kb, APP_NETWORK_QUIET_KB, APP_NETWORK_FULL_KB);
        assert_eq!(share(0.2), 0.0, "a keep-alive");
        assert_eq!(share(APP_NETWORK_QUIET_KB), 0.0);
        // Half way up in orders of magnitude, not in kilobytes.
        assert!((share(64.0) - 0.5).abs() < 0.01);
        assert_eq!(share(APP_NETWORK_FULL_KB), 1.0);
        assert_eq!(share(900_000.0), 1.0, "clamped, not past the end");
    }

    #[test]
    fn traffic_alone_never_reads_as_flat_out() {
        let start = Instant::now();
        let mut probe = probe_holding(500 * MB, start);
        // A gigabyte in a second, and not a tick of processor time.
        probe.score(start + SECOND, 0, 500 * MB, 1 << 30);
        assert_eq!(probe.reading.load, APP_NETWORK_MOST);
        assert_eq!(
            probe.reading.load,
            cpu_share(1.0, 12.0),
            "level with one busy core"
        );
    }

    #[test]
    fn the_busiest_of_the_three_decides() {
        let start = Instant::now();
        let mut probe = probe_holding(500 * MB, start);
        // An idle processor and still memory, but 64 KB a second coming down the line: half way
        // up what traffic can read.
        probe.score(start + SECOND, 0, 500 * MB, 64 * 1024);
        assert!((probe.reading.load - 0.5 * APP_NETWORK_MOST).abs() < 0.01);
        assert_eq!(probe.reading.cpu, 0.0);

        let mut probe = probe_holding(500 * MB, start);
        // Every processor flat out outweighs a trickle on the network.
        probe.score(start + SECOND, flat_out(1), 500 * MB, 4 * 1024);
        assert_eq!(probe.reading.load, 1.0);
    }

    #[test]
    fn a_reading_rises_at_once_and_falls_slowly() {
        let start = Instant::now();
        let mut probe = probe_holding(500 * MB, start);
        probe.score(start + SECOND, flat_out(1), 500 * MB, 0);
        assert_eq!(probe.reading.load, 1.0, "flat out within the second");
        // The work stops. Half the reading is left a half-life later, a quarter after two.
        let mut at = start + SECOND;
        for _ in 0..3 {
            at += SECOND;
            probe.score(at, flat_out(1), 500 * MB, 0);
        }
        assert!((probe.reading.load - 0.5).abs() < 0.01);
        for _ in 0..3 {
            at += SECOND;
            probe.score(at, flat_out(1), 500 * MB, 0);
        }
        assert!((probe.reading.load - 0.25).abs() < 0.01);
        // And it comes straight back up when the work does.
        probe.score(at + SECOND, flat_out(2), 500 * MB, 0);
        assert_eq!(probe.reading.load, 1.0);
    }

    #[test]
    fn a_reading_falls_to_lighter_work_not_past_it() {
        assert_eq!(settle(0.2, 0.6, 1.0), 0.6);
        let after = settle(1.0, 0.4, HALF_LIFE_SECS);
        assert!(
            (after - 0.7).abs() < 1e-4,
            "half way from 1.0 to 0.4: {after}"
        );
        assert!(settle(1.0, 0.4, 600.0) >= 0.4);
    }

    #[test]
    fn connections_are_counted_from_where_they_were_last_seen() {
        let mut known = HashMap::new();
        // The first look: two connections that have carried plenty already, none of it news.
        let first = carried(&mut known, true, [(10, 5_000), (11, 80_000)].into_iter());
        assert_eq!(first, 0);
        // One carries on, one has closed, one has opened since and is counted from nothing.
        let second = carried(&mut known, false, [(10, 6_500), (12, 300)].into_iter());
        assert_eq!(second, 1_800);
        // A connection held by two processes of the app is counted once.
        let third = carried(
            &mut known,
            false,
            [(10, 7_000), (10, 7_000), (12, 300)].into_iter(),
        );
        assert_eq!(third, 500);
        assert_eq!(known.len(), 2);
    }

    #[test]
    fn a_short_gap_between_looks_is_not_scored() {
        let start = Instant::now();
        let mut probe = Probe::new(std::process::id());
        probe.sample(start, &HashMap::new());
        let noted = probe.last;
        probe.sample(start + Duration::from_millis(100), &HashMap::new());
        assert_eq!(probe.last, noted, "the first look still stands");
        assert_eq!(probe.reading.load, 0.0);
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
        let start = Instant::now();
        let mut probe = Probe::new(std::process::id());
        probe.sample(start, &tcp::bytes_by_inode());
        assert!(probe.last.is_some(), "its totals are readable");
        probe.sample(start + SECOND, &tcp::bytes_by_inode());
        let reading = probe.reading;
        for score in [reading.load, reading.cpu, reading.memory, reading.network] {
            assert!((0.0..=1.0).contains(&score));
        }
    }
}
