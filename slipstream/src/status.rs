//! What the bar and quick settings show about the machine: the time and date, battery, network,
//! Bluetooth, volume, brightness and power mode. Read off the main thread, since some of it means
//! waiting on other programs.

use std::{
    ffi::CStr,
    process::Stdio,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::Thread,
    time::{Duration, Instant},
};

use slipstream_config::localtime;
use zbus::{
    blocking::Connection,
    zvariant::{OwnedObjectPath, OwnedValue},
};

use crate::calendar::Date;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Charge in percent and whether a charger is plugged in, charging or not (a battery held at
    /// a charge limit, or already full, still counts); `None` without a battery.
    pub battery: Option<(u8, bool)>,
    /// How long the battery lasts, or how long until it's full, e.g. `3 h 10 min left`.
    pub battery_time: Option<String>,
    /// Local time as HH:MM; empty until the first reading.
    pub time: String,
    /// The local date, short, e.g. `Fri 11 Sep`.
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
/// `pactl subscribe` is running, so the sound is read only when it reports a change.
static FOLLOWING: AtomicBool = AtomicBool::new(false);
/// Read the speakers' volume on the next pass.
static SPEAKERS_CHANGED: AtomicBool = AtomicBool::new(true);
/// Read the microphone on the next pass.
static MIC_CHANGED: AtomicBool = AtomicBool::new(true);
/// Until when the reader leaves the shared reading alone, while a change shown ahead of time is
/// still being made.
static HELD_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// `reading` locked, even if a reader thread panicked while it held it: readers only ever store
/// whole values, so what is there is still a reading, and the desktop carries on rather than
/// panicking with it.
pub fn held<T>(reading: &Mutex<T>) -> MutexGuard<'_, T> {
    reading.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn start() -> Arc<Mutex<Reading>> {
    let shared = Arc::new(Mutex::new(Reading::default()));
    let writer = shared.clone();
    let reader = std::thread::spawn(move || {
        let mut reading = Reading::default();
        let mut estimate = slipstream_config::battery::Estimate::default();
        let started = Instant::now();
        let mut rarely: Option<Instant> = None;
        let bus = system_bus();
        loop {
            // Timed rather than counted, since a volume change wakes the reader early.
            let everything = EVERYTHING.swap(false, Ordering::Relaxed)
                || rarely.is_none_or(|then| then.elapsed() >= RARELY);
            if everything {
                rarely = Some(Instant::now());
            }
            read_often(&mut reading, &mut estimate, started.elapsed().as_secs_f64());
            // Without `pactl subscribe` to say when the sound changed, the speakers are read on
            // every pass and the microphone with the things that change rarely.
            let following = FOLLOWING.load(Ordering::Relaxed);
            if SPEAKERS_CHANGED.swap(false, Ordering::Relaxed) || !following {
                let (volume, muted) = volume().unwrap_or((0, false));
                reading.volume = volume;
                reading.muted = muted;
            }
            if (MIC_CHANGED.swap(false, Ordering::Relaxed) || (everything && !following))
                && let Some((_, muted)) = mic()
            {
                reading.mic_muted = muted;
            }
            // NetworkManager, BlueZ and the power profile change rarely; ask them every 10 s.
            if everything {
                read_rarely(&mut reading, bus.as_ref());
            }
            let held = HELD_UNTIL
                .lock()
                .unwrap()
                .is_some_and(|until| Instant::now() < until);
            if !held {
                *writer.lock().unwrap() = reading.clone();
            }
            std::thread::park_timeout(Duration::from_secs(2));
        }
    });
    let _ = READER.set(reader.thread().clone());
    std::thread::spawn(follow_sound);
    shared
}

/// How often NetworkManager, BlueZ and the power profile are asked again.
const RARELY: Duration = Duration::from_secs(10);
/// How many times `pactl subscribe` is started again after it ends, as when PipeWire restarts.
const FOLLOW_TRIES: usize = 20;

/// Volume changes made anywhere else, from Settings, another app or a headset's buttons, reach
/// the reading at once: `pactl subscribe` reports each change to the speakers or microphone in
/// use, or to which ones those are, and the reader reads that again. Nothing else reads them while
/// it runs. Without `pactl`, the reader reads them on a timer instead.
fn follow_sound() {
    use std::io::BufRead;
    for _ in 0..FOLLOW_TRIES {
        let mut command = crate::launch::command("pactl");
        crate::launch::dies_with_us(&mut command);
        let child = command
            .arg("subscribe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return;
        };
        // Anything that changed while nothing was following is read afresh.
        FOLLOWING.store(true, Ordering::Relaxed);
        SPEAKERS_CHANGED.store(true, Ordering::Relaxed);
        MIC_CHANGED.store(true, Ordering::Relaxed);
        if let Some(events) = child.stdout.take() {
            for line in std::io::BufReader::new(events).lines() {
                let Ok(line) = line else {
                    break;
                };
                let server = line.contains(" on server");
                if line.contains(" on source ") || server {
                    MIC_CHANGED.store(true, Ordering::Relaxed);
                }
                if line.contains(" on sink ") || server {
                    SPEAKERS_CHANGED.store(true, Ordering::Relaxed);
                    if let Some(reader) = READER.get() {
                        reader.unpark();
                    }
                }
            }
        }
        let _ = child.wait();
        FOLLOWING.store(false, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(5));
    }
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
    SPEAKERS_CHANGED.store(true, Ordering::Relaxed);
    MIC_CHANGED.store(true, Ordering::Relaxed);
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

/// The command that sets the speakers' volume, capped at 100% as the volume keys are.
pub fn set_volume(percent: u8) -> Vec<String> {
    let level = format!("{:.2}", percent.min(100) as f64 / 100.0);
    crate::launch::strings(&[
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
    crate::launch::strings(&["wpctl", "set-mute", device, if muted { "1" } else { "0" }])
}

pub fn set_wifi(on: bool) -> Vec<String> {
    crate::launch::strings(&["nmcli", "radio", "wifi", if on { "on" } else { "off" }])
}

/// Turning Bluetooth on unblocks its radio first: BlueZ can't power an adapter that rfkill
/// blocks.
pub fn set_bluetooth(on: bool) -> Vec<String> {
    let powered =
        format!("busctl set-property org.bluez /org/bluez/hci0 org.bluez.Adapter1 Powered b {on}");
    let script = if on {
        format!("rfkill unblock bluetooth; {powered}")
    } else {
        powered
    };
    crate::launch::strings(&["sh", "-c", &script])
}

/// The power mode after `current`, as the quick settings tile cycles them.
pub fn next_power_profile(current: &str) -> &'static str {
    match current {
        "power-saver" => "balanced",
        "balanced" => "performance",
        _ => "power-saver",
    }
}

fn read_often(
    reading: &mut Reading,
    estimate: &mut slipstream_config::battery::Estimate,
    now: f64,
) {
    if let Some((time, date, today)) = clock() {
        reading.time = time;
        reading.date = date;
        reading.today = Some(today);
    }
    reading.battery = battery();
    reading.battery_time = battery_time(estimate, now);
    reading.brightness = crate::launch::backlight()
        .map(|(_, now, max)| (now * 100 + max / 2).checked_div(max).unwrap_or(0).min(100) as u8);
}

fn read_rarely(reading: &mut Reading, bus: Option<&Connection>) {
    // Without NetworkManager there's no telling, so the bar assumes a connection.
    reading.online = property(bus, NM, NM_PATH, NM, "State").is_none_or(is_connected);
    reading.wifi = property(bus, NM, NM_PATH, NM, "WirelessEnabled").unwrap_or(true);
    reading.network = active_connection(&active_connections(bus));
    reading.bluetooth =
        property(bus, BLUEZ, ADAPTER, "org.bluez.Adapter1", "Powered") == Some(true);
    reading.bluetooth_device = if reading.bluetooth {
        connected_device(bluetooth_devices(bus))
    } else {
        None
    };
    reading.power_profile = property(
        bus,
        POWER_PROFILES,
        POWER_PROFILES_PATH,
        POWER_PROFILES,
        "ActiveProfile",
    );
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

/// How the bar writes the time and the date, as `strftime` patterns.
pub const TIME: &CStr = c"%H:%M";
pub const DATE: &CStr = c"%a %-d %b";

/// The time and date in the system's time zone and locale, following changes to either.
fn clock() -> Option<(String, String, Date)> {
    let tm = localtime::now()?;
    let today = Date {
        year: tm.tm_year + 1900,
        month: tm.tm_mon as u32 + 1,
        day: tm.tm_mday as u32,
    };
    Some((
        localtime::format(&tm, TIME)?,
        localtime::format(&tm, DATE)?,
        today,
    ))
}

/// How long NetworkManager, BlueZ or power-profiles-daemon get to answer; a missing bluetoothd
/// could otherwise keep the reader waiting.
const ANSWER: Duration = Duration::from_secs(3);

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "/org/bluez/hci0";

/// The system bus, kept for the reader's life, so a reading costs a few messages and no programs.
fn system_bus() -> Option<Connection> {
    zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(ANSWER))
        .and_then(|builder| builder.build())
        .inspect_err(|err| tracing::warn!("no system bus for network and Bluetooth: {err}"))
        .ok()
}

/// One property of an object on the system bus; `None` if the service or the object isn't there.
fn property<T: TryFrom<OwnedValue>>(
    bus: Option<&Connection>,
    service: &str,
    path: &str,
    interface: &str,
    name: &str,
) -> Option<T> {
    let reply = bus?
        .call_method(
            Some(service),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, name),
        )
        .ok()?;
    let value: OwnedValue = reply.body().deserialize().ok()?;
    T::try_from(value).ok()
}

/// NetworkManager's state: connected to the internet (70), to the site only (60) or to the local
/// network only (50) all count; connecting (40) doesn't.
fn is_connected(nm_state: u32) -> bool {
    nm_state >= 50
}

/// Each active connection's name and type, sorted by name.
fn active_connections(bus: Option<&Connection>) -> Vec<(String, String)> {
    const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
    let paths: Vec<OwnedObjectPath> =
        property(bus, NM, NM_PATH, NM, "ActiveConnections").unwrap_or_default();
    let mut connections: Vec<(String, String)> = paths
        .iter()
        .filter_map(|path| {
            let text = |name| property::<String>(bus, NM, path.as_str(), ACTIVE, name);
            Some((text("Id")?, text("Type")?))
        })
        .collect();
    connections.sort();
    connections
}

/// The first Wi-Fi connection, else the first wired one.
fn active_connection(connections: &[(String, String)]) -> Option<String> {
    let of_kind = |wanted: &str| {
        connections
            .iter()
            .find(|(_, kind)| kind == wanted)
            .map(|(name, _)| name.clone())
    };
    of_kind("802-11-wireless").or_else(|| of_kind("802-3-ethernet"))
}

/// Each Bluetooth device BlueZ knows: its object path, whether it's connected, and its name.
fn bluetooth_devices(bus: Option<&Connection>) -> Vec<(String, bool, Option<String>)> {
    let objects = bus.and_then(|bus| {
        bus.call_method(
            Some(BLUEZ),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .ok()?
        .body()
        .deserialize::<zbus::fdo::ManagedObjects>()
        .ok()
    });
    objects
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(path, interfaces)| {
            let (_, device) = interfaces
                .into_iter()
                .find(|(name, _)| name.as_str() == "org.bluez.Device1")?;
            let value = |key: &str| device.get(key)?.try_clone().ok();
            let connected = value("Connected").and_then(|v| bool::try_from(v).ok());
            let name = |key| value(key).and_then(|v| String::try_from(v).ok());
            Some((
                path.as_str().to_string(),
                connected == Some(true),
                name("Alias").or_else(|| name("Name")),
            ))
        })
        .collect()
}

/// The name of the first connected device on the adapter, in order of their object paths.
fn connected_device(mut devices: Vec<(String, bool, Option<String>)>) -> Option<String> {
    devices.sort();
    let on_adapter = format!("{ADAPTER}/");
    devices.into_iter().find_map(|(path, connected, name)| {
        (connected && path.starts_with(&on_adapter))
            .then_some(name)
            .flatten()
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
const POWER_PROFILES_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";

/// The command that switches the power profile.
pub fn set_power_profile(profile: &str) -> Vec<String> {
    [
        "busctl",
        "set-property",
        POWER_PROFILES,
        POWER_PROFILES_PATH,
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
            let read = |name: &str| std::fs::read_to_string(path.join(name)).unwrap_or_default();
            // A wireless mouse's or headset's battery has the scope "Device"; the laptop's has
            // "System", or no scope at all.
            read("type").trim() == "Battery" && read("scope").trim() != "Device"
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
    let status = read("status").unwrap_or_default();
    Some((capacity, plugged_in(&status) || charger_online()))
}

/// Whether the battery's own status means a charger is attached.
fn plugged_in(status: &str) -> bool {
    matches!(status, "Charging" | "Full" | "Not charging")
}

/// Whether any supply other than a battery (mains, or USB power delivery) is online.
fn charger_online() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    entries.flatten().map(|entry| entry.path()).any(|path| {
        let read = |name: &str| std::fs::read_to_string(path.join(name)).unwrap_or_default();
        read("type").trim() != "Battery"
            && read("scope").trim() != "Device"
            && read("online").trim() == "1"
    })
}

/// Batteries report either energy (µWh, µW) or charge (µAh, µA); the ratio is hours either way.
/// `estimate` steadies the rate across readings.
fn battery_time(estimate: &mut slipstream_config::battery::Estimate, now: f64) -> Option<String> {
    let dir = battery_dir()?;
    let read = |name: &str| -> Option<f64> {
        std::fs::read_to_string(dir.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let status = std::fs::read_to_string(dir.join("status")).ok()?;
    let status = status.trim();
    let (amount, full, rate) = if read("energy_now").is_some() {
        (read("energy_now"), read("energy_full"), read("power_now"))
    } else {
        (read("charge_now"), read("charge_full"), read("current_now"))
    };
    let hours = estimate.update(status, amount, full, rate, now)?;
    let suffix = if status == "Charging" {
        "until full"
    } else {
        "left"
    };
    Some(format!(
        "{} {suffix}",
        slipstream_config::battery::duration(hours)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_states_that_count_as_online() {
        assert!(is_connected(70), "connected");
        assert!(is_connected(60), "connected (site only)");
        assert!(is_connected(50), "connected (local only)");
        assert!(!is_connected(40), "connecting");
        assert!(!is_connected(20), "disconnected");
    }

    #[test]
    fn wifi_is_preferred_over_a_wired_connection() {
        let connection = |name: &str, kind: &str| (name.to_string(), kind.to_string());
        let lo = connection("lo", "loopback");
        let wired = connection("Wired 1", "802-3-ethernet");
        let wifi = connection("Cafe: Guest", "802-11-wireless");
        assert_eq!(
            active_connection(&[lo.clone(), wired.clone(), wifi]),
            Some("Cafe: Guest".into())
        );
        assert_eq!(
            active_connection(&[lo.clone(), wired]),
            Some("Wired 1".into())
        );
        assert_eq!(active_connection(&[lo]), None);
    }

    #[test]
    fn volume_and_mute_come_from_wpctl() {
        assert_eq!(parse_volume("Volume: 0.75 [MUTED]"), Some((75, true)));
        assert_eq!(parse_volume("Volume: 0.40"), Some((40, false)));
        assert_eq!(parse_volume("nonsense"), None);
    }

    #[test]
    fn the_clock_reads_time_date_and_today() {
        let (time, date, today) = clock().unwrap();
        assert_eq!(time.len(), 5, "{time}");
        assert!(!date.is_empty());
        assert!((1..=12).contains(&today.month) && (1..=31).contains(&today.day));
    }

    #[test]
    fn the_first_connected_device_on_the_adapter_is_named() {
        let device = |path: &str, connected: bool, name: Option<&str>| {
            (path.to_string(), connected, name.map(str::to_string))
        };
        let devices = vec![
            device("/org/bluez/hci1/dev_AA", true, Some("Other adapter")),
            device("/org/bluez/hci0/dev_CC", true, Some("Keyboard")),
            device("/org/bluez/hci0/dev_AA", false, Some("Speaker")),
            device("/org/bluez/hci0/dev_BB", true, Some("Headphones")),
        ];
        assert_eq!(connected_device(devices), Some("Headphones".into()));
        assert_eq!(
            connected_device(vec![device("/org/bluez/hci0/dev_AA", false, None)]),
            None
        );
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
