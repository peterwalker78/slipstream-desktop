//! What the bar and quick settings show about the machine: the time and date, battery, network,
//! Bluetooth, volume, brightness and power mode. Read off the main thread, since most of it means
//! running a program.

use std::{
    process::Stdio,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::Thread,
    time::{Duration, Instant},
};

use crate::calendar::Date;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Charge in percent and whether it's charging; `None` without a battery.
    pub battery: Option<(u8, bool)>,
    /// How long the battery lasts, or how long until it's full, e.g. `3 h 10 min left`.
    pub battery_time: Option<String>,
    /// Local time as HH:MM; empty until the first reading.
    pub time: String,
    /// The local date as the mockup writes it, e.g. `Fri 11 Sep`.
    pub date: String,
    /// Today, for the calendar.
    pub today: Option<Date>,
    pub online: bool,
    /// Wi-Fi's radio is switched on.
    pub wifi: bool,
    /// The active connection's name: the Wi-Fi network, or the wired connection.
    pub network: Option<String>,
    /// Bluetooth is switched on.
    pub bluetooth: bool,
    /// A connected Bluetooth device's name.
    pub bluetooth_device: Option<String>,
    pub muted: bool,
    /// The speakers' volume in percent.
    pub volume: u8,
    /// The microphone is muted. Only the on-screen display shows this; the bar has no mic icon.
    pub mic_muted: bool,
    /// The screen's brightness in percent; `None` without a backlight.
    pub brightness: Option<u8>,
    /// power-profiles-daemon's profile: `power-saver`, `balanced` or `performance`.
    pub power_profile: Option<String>,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            battery: None,
            battery_time: None,
            time: String::new(),
            date: String::new(),
            today: None,
            // Until NetworkManager answers, don't flash an offline icon.
            online: true,
            wifi: true,
            network: None,
            bluetooth: false,
            bluetooth_device: None,
            muted: false,
            volume: 0,
            mic_muted: false,
            brightness: None,
            power_profile: None,
        }
    }
}

/// The reading thread, so a change can ask for a fresh reading straight away.
static READER: OnceLock<Thread> = OnceLock::new();
/// Read everything on the next pass, not only what changes often.
static EVERYTHING: AtomicBool = AtomicBool::new(false);
/// Until when the reader leaves the shared reading alone, while a change shown ahead of time is
/// still being made.
static HELD_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

pub fn start() -> Arc<Mutex<Reading>> {
    let shared = Arc::new(Mutex::new(Reading::default()));
    let writer = shared.clone();
    let reader = std::thread::spawn(move || {
        let mut reading = Reading::default();
        let mut tick = 0u64;
        loop {
            let everything = EVERYTHING.swap(false, Ordering::Relaxed) || tick % 5 == 0;
            read_often(&mut reading);
            // NetworkManager, BlueZ and the power profile change rarely; ask them every 10 s.
            if everything {
                read_rarely(&mut reading);
            }
            let held = HELD_UNTIL
                .lock()
                .unwrap()
                .is_some_and(|until| Instant::now() < until);
            if !held {
                *writer.lock().unwrap() = reading.clone();
            }
            tick += 1;
            std::thread::park_timeout(Duration::from_secs(2));
        }
    });
    let _ = READER.set(reader.thread().clone());
    shared
}

/// A change was shown before it's made (a toggle flipped on the keypress): keep showing it for a
/// few seconds rather than the reading from before.
pub fn hold() {
    *HELD_UNTIL.lock().unwrap() = Some(Instant::now() + Duration::from_secs(4));
}

/// The change is made: read everything again now.
pub fn refresh() {
    *HELD_UNTIL.lock().unwrap() = None;
    EVERYTHING.store(true, Ordering::Relaxed);
    if let Some(reader) = READER.get() {
        reader.unpark();
    }
}

/// Changes waiting or being made. The reading is refreshed once the last is done, so a slider
/// moved in quick steps doesn't bounce back to an older level in between.
static PENDING: AtomicUsize = AtomicUsize::new(0);
/// Changes are made one at a time, in order, on a thread of their own.
static CHANGES: OnceLock<Mutex<mpsc::Sender<Vec<String>>>> = OnceLock::new();

/// Runs `command` off the main thread, after any changes still waiting, then reads everything
/// again.
pub fn change(command: Vec<String>) {
    hold();
    let changes = CHANGES.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Vec<String>>();
        std::thread::spawn(move || {
            for command in receiver {
                make(&command, crate::launch::machine_commands_held(), run);
                if PENDING.fetch_sub(1, Ordering::Relaxed) == 1 {
                    refresh();
                }
            }
        });
        Mutex::new(sender)
    });
    PENDING.fetch_add(1, Ordering::Relaxed);
    if changes.lock().unwrap().send(command).is_err() {
        PENDING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Makes one change with `runner`, unless machine commands are held, when it only says what it
/// would have run. The reading is refreshed either way, so a nested run's controls settle back to
/// the machine's real state.
fn make(command: &[String], held: bool, runner: impl FnOnce(&[String])) {
    if held {
        tracing::info!("nested: not running {command:?}");
    } else {
        runner(command);
    }
}

fn run(command: &[String]) {
    let Some((program, args)) = command.split_first() else {
        return;
    };
    match crate::launch::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if !status.success() => tracing::warn!(?command, "failed: {status}"),
        Err(err) => tracing::warn!(?command, "couldn't run: {err}"),
        Ok(_) => {}
    }
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

/// The command that sets the speakers' volume, capped at 100% as the volume keys are.
pub fn set_volume(percent: u8) -> Vec<String> {
    let level = format!("{:.2}", percent.min(100) as f64 / 100.0);
    strings(&[
        "wpctl",
        "set-volume",
        "-l",
        "1.0",
        "@DEFAULT_AUDIO_SINK@",
        &level,
    ])
}

pub fn set_mute(muted: bool) -> Vec<String> {
    set_mute_on(false, muted)
}

/// The command that mutes or unmutes the speakers, or the microphone.
pub fn set_mute_on(microphone: bool, muted: bool) -> Vec<String> {
    let device = if microphone {
        "@DEFAULT_AUDIO_SOURCE@"
    } else {
        "@DEFAULT_AUDIO_SINK@"
    };
    strings(&["wpctl", "set-mute", device, if muted { "1" } else { "0" }])
}

pub fn set_wifi(on: bool) -> Vec<String> {
    strings(&["nmcli", "radio", "wifi", if on { "on" } else { "off" }])
}

/// Turning Bluetooth on unblocks its radio first, as Plasma's and GNOME's switches do: BlueZ can't
/// power an adapter that rfkill blocks.
pub fn set_bluetooth(on: bool) -> Vec<String> {
    let powered =
        format!("busctl set-property org.bluez /org/bluez/hci0 org.bluez.Adapter1 Powered b {on}");
    let script = if on {
        format!("rfkill unblock bluetooth; {powered}")
    } else {
        powered
    };
    strings(&["sh", "-c", &script])
}

/// The power mode after `current`, as the quick settings tile cycles them.
pub fn next_power_profile(current: &str) -> &'static str {
    match current {
        "power-saver" => "balanced",
        "balanced" => "performance",
        _ => "power-saver",
    }
}

fn read_often(reading: &mut Reading) {
    if let Some((time, date, today)) = clock() {
        reading.time = time;
        reading.date = date;
        reading.today = Some(today);
    }
    reading.battery = battery();
    reading.battery_time = battery_time();
    let (volume, muted) = volume().unwrap_or((0, false));
    reading.volume = volume;
    reading.muted = muted;
    reading.brightness = crate::launch::backlight()
        .map(|(_, now, max)| (now * 100 + max / 2).checked_div(max).unwrap_or(0).min(100) as u8);
}

fn read_rarely(reading: &mut Reading) {
    reading.online = online();
    reading.wifi = output("nmcli", &["-t", "-f", "WIFI", "radio"]).is_none_or(|s| s == "enabled");
    reading.network = output(
        "nmcli",
        &["-t", "-f", "NAME,TYPE", "connection", "show", "--active"],
    )
    .and_then(|text| active_connection(&text));
    reading.bluetooth = bluetooth();
    reading.bluetooth_device = if reading.bluetooth {
        // bluetoothctl waits forever for a missing bluetoothd, hence the timeout.
        output("timeout", &["3", "bluetoothctl", "devices", "Connected"])
            .and_then(|text| connected_device(&text))
    } else {
        None
    };
    reading.power_profile = power_profile();
    // Only the microphone key and its display care, and nothing else changes it behind our back
    // often enough to read every couple of seconds.
    if let Some((_, muted)) = mic() {
        reading.mic_muted = muted;
    }
}

/// A program's trimmed output, if it ran and succeeded.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = crate::launch::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `date` follows the system time zone and locale, including changes to them; std knows only UTC.
fn clock() -> Option<(String, String, Date)> {
    parse_clock(&output("date", &["+%H:%M|%a %-d %b|%Y-%m-%d"])?)
}

fn parse_clock(text: &str) -> Option<(String, String, Date)> {
    let mut parts = text.split('|');
    let time = parts.next()?.to_string();
    let date = parts.next()?.to_string();
    let mut ymd = parts.next()?.split('-');
    let today = Date {
        year: ymd.next()?.parse().ok()?,
        month: ymd.next()?.parse().ok()?,
        day: ymd.next()?.parse().ok()?,
    };
    Some((time, date, today))
}

/// Without NetworkManager there's no telling, so the bar assumes a connection.
fn online() -> bool {
    output("nmcli", &["-t", "-f", "STATE", "general"]).is_none_or(|state| is_connected(&state))
}

fn is_connected(nm_state: &str) -> bool {
    // "connected (site only)" and "connected (local only)" still count; "connecting" doesn't.
    nm_state.starts_with("connected")
}

/// The first Wi-Fi connection in `nmcli -t -f NAME,TYPE connection show --active`, else the first
/// wired one. nmcli escapes colons in names as `\:`.
fn active_connection(text: &str) -> Option<String> {
    let connections: Vec<(String, &str)> = text
        .lines()
        .filter_map(|line| {
            let (name, kind) = line.rsplit_once(':')?;
            Some((name.replace("\\:", ":"), kind))
        })
        .collect();
    let of_kind = |wanted: &str| {
        connections
            .iter()
            .find(|(_, kind)| *kind == wanted)
            .map(|(name, _)| name.clone())
    };
    of_kind("802-11-wireless").or_else(|| of_kind("802-3-ethernet"))
}

fn bluetooth() -> bool {
    output(
        "busctl",
        &[
            "get-property",
            "org.bluez",
            "/org/bluez/hci0",
            "org.bluez.Adapter1",
            "Powered",
        ],
    )
    .is_some_and(|powered| powered == "b true")
}

/// The first device in `bluetoothctl devices Connected`: lines like `Device <address> <name>`.
fn connected_device(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("Device ")?;
        let (_, name) = rest.split_once(' ')?;
        Some(name.to_string())
    })
}

fn volume() -> Option<(u8, bool)> {
    parse_volume(&output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?)
}

fn mic() -> Option<(u8, bool)> {
    parse_volume(&output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"])?)
}

/// `Volume: 0.75 [MUTED]` is 75%, muted.
fn parse_volume(text: &str) -> Option<(u8, bool)> {
    let level: f64 = text
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((
        (level * 100.0).round().clamp(0.0, 255.0) as u8,
        text.contains("[MUTED]"),
    ))
}

const POWER_PROFILES: &str = "org.freedesktop.UPower.PowerProfiles";

fn power_profile() -> Option<String> {
    let path = format!("/{}", POWER_PROFILES.replace('.', "/"));
    let text = output(
        "busctl",
        &[
            "get-property",
            POWER_PROFILES,
            &path,
            POWER_PROFILES,
            "ActiveProfile",
        ],
    )?;
    Some(text.strip_prefix("s ")?.trim_matches('"').to_string())
}

/// The command that switches the power profile.
pub fn set_power_profile(profile: &str) -> Vec<String> {
    let path = format!("/{}", POWER_PROFILES.replace('.', "/"));
    [
        "busctl",
        "set-property",
        POWER_PROFILES,
        &path,
        POWER_PROFILES,
        "ActiveProfile",
        "s",
        profile,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn battery_dir() -> Option<std::path::PathBuf> {
    std::fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            std::fs::read_to_string(path.join("type")).is_ok_and(|kind| kind.trim() == "Battery")
        })
}

fn battery() -> Option<(u8, bool)> {
    let dir = battery_dir()?;
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .map(|text| text.trim().to_string())
    };
    let capacity = read("capacity")?.parse().ok()?;
    Some((capacity, read("status")? == "Charging"))
}

/// Batteries report either energy (µWh, µW) or charge (µAh, µA); the ratio is hours either way.
fn battery_time() -> Option<String> {
    let dir = battery_dir()?;
    let read = |name: &str| -> Option<f64> {
        std::fs::read_to_string(dir.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let status = std::fs::read_to_string(dir.join("status")).ok()?;
    let (now, full, rate) = match (read("energy_now"), read("energy_full"), read("power_now")) {
        (Some(now), Some(full), Some(rate)) => (now, full, rate),
        _ => (
            read("charge_now")?,
            read("charge_full")?,
            read("current_now")?,
        ),
    };
    match status.trim() {
        "Discharging" => time_text(now / rate, "left"),
        "Charging" => time_text((full - now) / rate, "until full"),
        _ => None,
    }
}

/// `hours` as `3 h 10 min left`; nothing for a rate of zero or a nonsense estimate.
fn time_text(hours: f64, suffix: &str) -> Option<String> {
    if !hours.is_finite() || hours <= 0.0 || hours > 48.0 {
        return None;
    }
    let minutes = (hours * 60.0).round() as u64;
    Some(match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min {suffix}"),
        (h, 0) => format!("{h} h {suffix}"),
        (h, m) => format!("{h} h {m} min {suffix}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_states_that_count_as_online() {
        assert!(is_connected("connected"));
        assert!(is_connected("connected (site only)"));
        assert!(!is_connected("connecting"));
        assert!(!is_connected("disconnected"));
    }

    #[test]
    fn wifi_is_preferred_and_escaped_colons_are_undone() {
        let text = "lo:loopback\nWired 1:802-3-ethernet\nCafe\\: Guest:802-11-wireless";
        assert_eq!(active_connection(text), Some("Cafe: Guest".into()));
        assert_eq!(
            active_connection("lo:loopback\nWired 1:802-3-ethernet"),
            Some("Wired 1".into())
        );
        assert_eq!(active_connection("lo:loopback"), None);
    }

    #[test]
    fn volume_and_mute_come_from_wpctl() {
        assert_eq!(parse_volume("Volume: 0.75 [MUTED]"), Some((75, true)));
        assert_eq!(parse_volume("Volume: 0.40"), Some((40, false)));
        assert_eq!(parse_volume("nonsense"), None);
    }

    #[test]
    fn the_clock_reads_time_date_and_today() {
        let (time, date, today) = parse_clock("17:24|Fri 11 Sep|2026-09-11").unwrap();
        assert_eq!((time.as_str(), date.as_str()), ("17:24", "Fri 11 Sep"));
        assert_eq!(
            today,
            Date {
                year: 2026,
                month: 9,
                day: 11
            }
        );
    }

    #[test]
    fn bluetooth_devices_are_named_after_their_address() {
        assert_eq!(
            connected_device("Device 00:11:22:33:44:55 WH-1000XM4\n"),
            Some("WH-1000XM4".into())
        );
        assert_eq!(connected_device(""), None);
    }

    #[test]
    fn battery_times_read_naturally() {
        assert_eq!(
            time_text(3.0 + 10.0 / 60.0, "left"),
            Some("3 h 10 min left".into())
        );
        assert_eq!(time_text(0.75, "left"), Some("45 min left".into()));
        assert_eq!(time_text(2.0, "until full"), Some("2 h until full".into()));
        assert_eq!(time_text(f64::INFINITY, "left"), None);
    }

    #[test]
    fn nested_changes_are_logged_not_run() {
        for command in [
            set_wifi(false),
            set_bluetooth(true),
            set_volume(30),
            set_mute_on(true, true),
            set_power_profile("performance"),
        ] {
            make(&command, true, |command| {
                panic!("{command:?} ran while machine commands were held")
            });
        }
        let mut ran = Vec::new();
        make(&set_wifi(true), false, |command| ran.push(command.to_vec()));
        assert_eq!(ran, [set_wifi(true)], "the session runs them");
    }

    #[test]
    fn volume_is_set_as_a_fraction_and_power_modes_cycle() {
        assert_eq!(set_volume(62).last().map(String::as_str), Some("0.62"));
        assert_eq!(set_volume(140).last().map(String::as_str), Some("1.00"));
        assert_eq!(next_power_profile("power-saver"), "balanced");
        assert_eq!(next_power_profile("balanced"), "performance");
        assert_eq!(next_power_profile("performance"), "power-saver");
    }
}
