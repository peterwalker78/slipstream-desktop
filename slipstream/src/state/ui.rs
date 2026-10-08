//! The desktop's own surfaces: the fade to the wallpaper, toasts and the on-screen display, volume,
//! brightness and media keys, settings arriving, and the app explorer and what it starts.

use super::*;

impl Slipstream {
    /// Any input. Returns true if the UI was faded out, so the input only brings it back.
    pub fn wake_ui(&mut self) -> bool {
        // Other programs waiting on `ext-idle-notify` hear about the same input we do, from the
        // one place every input path already passes through, once the pass is done.
        self.input_since_notified = true;
        if self.screens_off {
            self.set_screens_off(false);
        }
        self.scope.touched();
        let now = self.clock.tick();
        self.idle.input(now)
    }

    /// Reads which agents say they are working, once a second.
    pub fn tick_agents(&mut self) {
        let now = std::time::Instant::now();
        if self
            .agents_read
            .is_some_and(|read| now.duration_since(read) < std::time::Duration::from_secs(1))
        {
            return;
        }
        self.agents_read = Some(now);
        self.scope.follow(&crate::working::at_work());
    }

    /// Tells `ext-idle-notify` about the input this pass brought, if any.
    pub fn notify_idle_listeners(&mut self) {
        if std::mem::take(&mut self.input_since_notified) {
            let seat = self.seat.clone();
            self.idle_notifier_state.notify_activity(&seat);
        }
    }

    /// Keeps `ext-idle-notify` in step with the idle inhibitors an app holds: while one is held
    /// nothing may be told the seat has gone quiet, which is what the inhibitor is for.
    pub fn follow_idle_inhibitors(&mut self) {
        let inhibited = self.awake || self.idle.inhibitors.iter().any(|s| s.alive());
        self.idle_notifier_state.set_is_inhibited(inhibited);
    }

    /// Faces for characters the embedded fonts lack have landed: everything painted with text
    /// is painted again, now that those characters can be drawn.
    pub fn fallback_fonts_landed(&mut self) {
        if !crate::fallback::take_landed() {
            return;
        }
        for chrome in self.chromes.values_mut() {
            chrome.forget_painted_text();
        }
        self.notices.forget_painted_text();
        self.centre.forget_painted_text();
        self.explorer.forget_painted_text();
        self.quick.forget_painted_text();
        self.sheet.forget_painted_text();
        self.toast.forget_painted_text();
        self.rain.forget_painted_text();
        if let Some(exit) = self.exit.as_mut() {
            exit.forget_painted_text();
        }
        if let Some(offer) = self.offer.as_mut() {
            offer.forget_painted_text();
        }
        if let Some(share) = self.share.as_mut() {
            share.forget_painted_text();
        }
    }

    /// How visible the UI is now. It fades after a while with no input, unless Awake is on,
    /// a window fills the screen, or the explorer is open.
    pub fn ui_opacity(&mut self, now: f64) -> f32 {
        let keep_up = self.awake
            || self.fullscreen_on_screen()
            // A launcher or picker drawn by another program, while it has the keyboard.
            || self.keyboard_layer().is_some()
            || self.explorer.is_open()
            || self.quick.is_open()
            || self.centre.is_open()
            || self.bullet.is_some()
            // The ask card waits as long as it takes; the wallpaper mustn't swallow it. The
            // offer at login is the same, and so is a layout still coming back.
            || self.exit.is_some()
            || self.offer.is_some()
            || self.share.is_some()
            || self.snip.is_some()
            || self.battery.card.is_some()
            || self.history.is_open()
            || self.restoring.is_some()
            || self.lock.is_some()
            || self.unlocking.is_some();
        let opacity = self.idle.update(now, keep_up) as f32;
        // Pop-ups that arrived while it was faded show now it's coming back, and ones that
        // arrived while you typed show at the pause.
        self.show_waiting_popups(now);
        opacity
    }

    pub fn show_toast(&mut self, title: &str, body: &str) {
        let now = self.clock.tick();
        // The toast and the volume and brightness display share a place: the newer one shows.
        self.osd.hide();
        self.toast.show(title, body, now);
    }

    /// The volume keys. The level is set outright rather than stepped, so what the display says
    /// and what the machine does can't drift apart, and the new level is shown at once — the
    /// reading catches up when the change has been made.
    pub fn change_volume(&mut self, step: i8) {
        let (level, muted) = {
            let mut reading = self.status.lock().unwrap();
            let level = (reading.volume as i16 + step as i16).clamp(0, 100) as u8;
            reading.volume = level;
            (level, reading.muted)
        };
        status::change(status::set_volume(level));
        self.show_osd(osd::Kind::Volume { muted }, level);
        // The click is played through the speakers after the change, so it sounds at the level
        // that was just set. Muted, there would be nothing to hear.
        if self.settings.sound.volume_blip && !muted {
            crate::sound::blip();
        }
    }

    /// The mute keys, for the speakers or the microphone.
    pub fn toggle_mute(&mut self, microphone: bool) {
        let (muted, level) = {
            let mut reading = self.status.lock().unwrap();
            let muted = if microphone {
                reading.mic_muted = !reading.mic_muted;
                reading.mic_muted
            } else {
                reading.muted = !reading.muted;
                reading.muted
            };
            (muted, reading.volume)
        };
        status::change(status::set_mute_on(microphone, muted));
        let kind = if microphone {
            osd::Kind::Microphone { muted }
        } else {
            osd::Kind::Volume { muted }
        };
        self.show_osd(kind, level);
    }

    /// The brightness keys. Without a backlight there is nothing to show, so a toast says so.
    pub fn change_brightness(&mut self, step: i8) {
        let Some((command, level)) = crate::launch::brightness(step) else {
            self.show_toast(
                "No brightness control",
                "This screen’s brightness can’t be set from here.",
            );
            return;
        };
        self.status.lock().unwrap().brightness = Some(level);
        status::change(command);
        self.show_osd(osd::Kind::Brightness, level);
    }

    /// A media key: to whichever player is playing, and what it's doing afterwards on the display.
    /// Nested, the players belong to the desktop around this window, so the key is only logged.
    pub fn media_key(&mut self, transport: crate::media::Transport) {
        if launch::machine_commands_held() {
            tracing::info!(?transport, "nested: media key logged, not sent");
            return;
        }
        if let Some(reply) = self.media_replies.clone() {
            crate::media::send(transport, reply);
        }
    }

    /// What a player is doing after a media key.
    pub fn media_played(&mut self, playing: crate::media::Playing) {
        match playing {
            crate::media::Playing::Nobody => self.show_toast(
                "Nothing to play",
                "No music or video app is open to take the media keys.",
            ),
            crate::media::Playing::Track { playing, title } => {
                if self.quick.is_open() {
                    return;
                }
                let now = self.clock.tick();
                self.toast.clear();
                self.osd.show_with(osd::Kind::Media { playing }, title, now);
            }
        }
    }

    /// The display for a key that changed the volume or the brightness, unless quick settings is
    /// open: it shows the same levels on its own sliders, and two displays of one thing is one
    /// too many.
    pub fn show_osd(&mut self, kind: osd::Kind, level: u8) {
        if self.quick.is_open() {
            self.osd.hide();
            return;
        }
        let now = self.clock.tick();
        self.toast.clear();
        self.osd.show(kind, level, now);
    }

    /// As `show_osd`, with a line under the label.
    /// Turns Awake on or off, and says so on a card.
    pub fn set_awake(&mut self, on: bool) {
        tracing::info!("Awake {}", if on { "on" } else { "off" });
        self.awake = on;
        self.follow_idle_inhibitors();
        self.show_osd_with(osd::Kind::Awake { on }, osd::awake_note(on));
    }

    /// Awake ends with the sitting: locking on purpose, suspending, or shutting the lid. No
    /// card; the pill going is enough, and there may be no screen to show one on.
    pub fn end_awake(&mut self) {
        if self.awake {
            tracing::info!("Awake off");
            self.awake = false;
            self.follow_idle_inhibitors();
        }
    }

    pub fn show_osd_with(&mut self, kind: osd::Kind, detail: &str) {
        if self.quick.is_open() {
            self.osd.hide();
            return;
        }
        let now = self.clock.tick();
        self.toast.clear();
        self.osd.show_with(kind, detail.to_string(), now);
    }

    /// The keyboard's ring inside a panel: the window ring's colour at the same four fifths'
    /// opacity it is drawn with, packed as `0xrrggbbaa` for the painter.
    pub fn panel_ring(&self) -> u32 {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        let [r, g, b] = self.ring_rgb;
        (channel(r) << 24) | (channel(g) << 16) | (channel(b) << 8) | 0xcc
    }

    /// Applies the settings file's new contents straight away (`watch.rs` follows it).
    pub fn apply_settings(&mut self, settings: slipstream_config::Settings) {
        if settings == self.settings {
            return;
        }
        tracing::info!(?settings, "settings changed");
        self.idle.set_fade_after(settings.wallpaper.fade_after_secs);
        self.idle.set_lock_after(settings.lock.after_idle_mins);
        self.idle
            .set_screen_off_after(settings.display.screen_off_mins);
        // Night light follows on the next pass; a new schedule is looked at afresh.
        let (new, old) = (&settings.display, &self.settings.display);
        if (
            new.night_light_schedule,
            &new.night_light_from,
            &new.night_light_to,
        ) != (
            old.night_light_schedule,
            &old.night_light_from,
            &old.night_light_to,
        ) {
            self.night.schedule_changed();
        }
        self.set_reduced_motion(settings.motion.reduced);
        // Apps hear about dark and light over the settings portal, and change without restarting.
        crate::appearance::set(settings.appearance.colour_scheme);
        meter::configure(&settings.meter);
        let now = self.clock.tick();
        self.wallpaper = settings.wallpaper.clone();
        for saver in self.savers.values_mut() {
            saver.set_wallpaper(&settings.wallpaper, now);
        }
        self.ring_rgb = settings.borders.selected_tile_rgb();
        self.rain.set_ring_colour(self.ring_rgb);
        self.rain.set_effects(settings.motion.effects);
        self.bullet_rgb = settings.borders.bullet_time_rgb();
        if settings.workspaces != self.settings.workspaces {
            let entries = workspace_entries(&settings);
            self.set_workspaces(&entries, self.screens.len());
        }
        let rearrange = settings.display.screens != self.settings.display.screens;
        if !settings.clipboard.history && self.settings.clipboard.history {
            self.history.close();
            self.history.clear();
        }
        if settings.session.remember != self.settings.session.remember {
            if settings.session.remember {
                // Write what's open now rather than waiting for the next thing to move: the
                // switch was just turned on, and a layout is what it promises.
                self.record_due = Some(self.wall());
            } else {
                // Off means no record sitting in the state directory at all, not a stale one.
                self.record_due = None;
                self.saved_record = None;
                let path = session::path();
                // A record in a folder another Slipstream owns isn't this one's to delete.
                if self.owns_state
                    && let Err(err) = session::forget(&path)
                {
                    tracing::warn!(path = %path.display(), "couldn't forget the layout: {err}");
                }
            }
        }
        self.settings = settings;
        // The screens were moved about in Settings: put them where they are now said to be, and
        // re-tile, since every workspace's area has changed with them.
        if rearrange {
            self.arrange_screens();
            for screen in self.screens.iter() {
                self.motion
                    .jump_camera(&screen.output.name(), screen.workspace);
            }
            self.retile();
            tracing::info!("the screens were rearranged in Settings");
        }
    }

    pub fn toggle_explorer(&mut self) {
        if self.explorer.is_open() {
            self.explorer.close();
        } else {
            self.close_panels();
            let now = self.clock.tick();
            self.explorer.open(now);
        }
    }

    pub fn explorer_key(&mut self, sym: Keysym, ch: Option<char>, mods: Mods) {
        match self.explorer.key(sym, ch, mods) {
            Outcome::Nothing => {}
            Outcome::Close => self.explorer.close(),
            Outcome::Activate(item, fresh) => self.explorer_activate(item, fresh),
        }
    }

    /// A click while the explorer is open, in logical pixels from the output's corner: on an item
    /// it opens it (a `middle` click starts another of an open app). Outside it closes, and on
    /// another of the bar's buttons that button then does its own thing, as quick settings and
    /// the centre hand them on.
    pub fn explorer_click(&mut self, x: f64, y: f64, middle: bool) {
        match self.explorer.click(x, y, middle) {
            Outcome::Activate(item, fresh) => self.explorer_activate(item, fresh),
            Outcome::Close => {
                self.explorer.close();
                let target = (!self.fullscreen_on_screen())
                    .then(|| self.focused_bar_target(x, y))
                    .flatten()
                    .filter(|target| *target != bar::Target::Apps);
                if let Some(target) = target {
                    self.bar_clicked(target);
                }
            }
            Outcome::Nothing => {}
        }
    }

    /// Opens what was chosen in the explorer. An app that's already open is focused, on
    /// whichever workspace it's on, rather than started again, unless `fresh` asks for a new
    /// one (Shift+Enter or a middle click).
    pub fn explorer_activate(&mut self, item: Item, fresh: bool) {
        self.explorer.close();
        match item {
            Item::App(app) if fresh => {
                tracing::info!(app = app.id, command = ?app.exec, "starting another from the explorer");
                self.start_app(&app);
            }
            Item::App(app) => {
                let names = [
                    Some(app.id.to_lowercase()),
                    app.wm_class.as_ref().map(|class| class.to_lowercase()),
                ];
                let open = self.all_open_windows().into_iter().find(|window| {
                    window.alive()
                        && window_app_id(window).is_some_and(|id| names.contains(&Some(id)))
                });
                match open {
                    Some(window) if self.rain.contains(&window) => self.restore(&window),
                    Some(window) => {
                        self.activate_window(&window);
                        tracing::info!(app = app.id, "focused an open app from the explorer");
                    }
                    None => {
                        tracing::info!(app = app.id, command = ?app.exec, "launching from the explorer");
                        self.start_app(&app);
                    }
                }
            }
            Item::File(path) => self.open_path(&path),
            Item::Run(query) => self.run_query(&query),
            Item::Shortcuts => self.toggle_sheet(),
            Item::Answer(answer) => {
                tracing::info!("copied an answer from the explorer");
                let clip = crate::history::Clip::Text(answer.value.clone().into());
                self.put_on_clipboard(clip, false);
                self.show_toast("Copied", &answer.shown);
            }
            Item::Emoji(emoji, name) => {
                tracing::info!(name, "typing an emoji from the explorer");
                self.put_on_clipboard(crate::history::Clip::Text(emoji.into()), true);
            }
            Item::LogOut => {
                tracing::info!("log out chosen in the explorer");
                self.begin_exit(Intent::LogOut);
            }
        }
    }

    /// Starts an app from its desktop entry, and says so if it can't be.
    pub(super) fn start_app(&mut self, app: &crate::apps::App) {
        self.concentration.asked(std::time::Instant::now());
        match launch::app(&app.exec, app.terminal) {
            Ok(()) => {}
            Err(launch::Failure::NothingInstalled) => self.no_terminal(),
            Err(launch::Failure::Spawn(err)) => {
                self.show_toast("Couldn’t start it", &format!("{}: {err}", app.name))
            }
        }
    }

    /// Opens a file or folder in its default app.
    pub(super) fn open_path(&mut self, path: &std::path::Path) {
        self.concentration.asked(std::time::Instant::now());
        let command = ["xdg-open".to_string(), path.to_string_lossy().into_owned()];
        if let Err(err) = launch::spawn(&command) {
            self.show_toast("Couldn’t open it", &err.to_string());
        }
    }

    /// The explorer's Run: a URL or a path is opened, anything else is run as a program with its
    /// arguments, split as a desktop entry's command line is and never through a shell.
    pub(super) fn run_query(&mut self, query: &str) {
        use crate::apps::{Run, run_query};
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let run = match run_query(query, &home, |_| false) {
            // A path is looked at on a thread, so a hung mount can hold the event loop up for a
            // moment at most. One that doesn't answer in time is handed on to be opened anyway.
            Run::Path(path) => {
                let (typed, folder) = (query.to_string(), home.clone());
                let looked = launch::within(std::time::Duration::from_millis(150), move || {
                    let run = run_query(&typed, &folder, launch::executable);
                    let missing = matches!(&run, Run::Path(path) if !path.exists());
                    (run, missing)
                });
                match looked {
                    Some((_, true)) => {
                        let (title, body) = launch::nothing_at(query);
                        self.show_toast(title, &body);
                        return;
                    }
                    Some((run, false)) => run,
                    None => Run::Path(path),
                }
            }
            run => run,
        };
        match run {
            Run::Url(url) => {
                tracing::info!("opening a URL from the explorer");
                if let Err(err) = launch::spawn(&["xdg-open".to_string(), url]) {
                    self.show_toast("Couldn’t open it", &err.to_string());
                }
            }
            Run::Path(path) => {
                tracing::info!("opening a path from the explorer");
                self.open_path(&path);
            }
            Run::Command(words) => {
                tracing::info!(command = ?words, "running a command from the explorer");
                let program = words.first().cloned().unwrap_or_default();
                if let Err(err) = launch::spawn(&words) {
                    let (title, body) = launch::couldnt_run(&program, &err);
                    self.show_toast(title, &body);
                }
            }
        }
    }

    /// Super+Return, Super+E, Super+B and All settings: starts the app, or says what's missing.
    pub fn launch_app(&mut self, app: crate::keys::App) {
        use crate::keys::App;
        self.concentration.asked(std::time::Instant::now());
        match launch::launch(app) {
            Ok(()) => {}
            Err(launch::Failure::NothingInstalled) => match app {
                App::Terminal => self.no_terminal(),
                App::Files => self.show_toast(
                    "No file manager",
                    "Install a file manager, or set a default for folders.",
                ),
                App::Settings => self.show_toast(
                    "No Settings app",
                    "slipstream-settings isn’t installed beside Slipstream or on the PATH.",
                ),
                App::Browser => self.show_toast(
                    "No browser",
                    "Choose a default browser in your settings, or set $BROWSER.",
                ),
            },
            Err(launch::Failure::Spawn(err)) => {
                let name = match app {
                    App::Terminal => "the terminal",
                    App::Files => "the file manager",
                    App::Settings => "Settings",
                    App::Browser => "the browser",
                };
                self.show_toast("Couldn’t start it", &format!("{name}: {err}"));
            }
        }
    }

    pub(super) fn no_terminal(&mut self) {
        self.show_toast("No terminal", "Install a terminal, or set $TERMINAL.");
    }

    /// Quick settings' chevrons: the system's own settings page for Wi-Fi or Bluetooth.
    /// Slipstream's own Settings app, on the page with the id `page`.
    pub fn open_slipstream_settings(&mut self, page: &str) {
        self.concentration.asked(std::time::Instant::now());
        let Some(command) = launch::settings_app_on(page) else {
            self.show_toast(
                "No Settings app",
                "slipstream-settings isn’t installed beside Slipstream or on the PATH.",
            );
            return;
        };
        tracing::info!(page, ?command, "opening a Settings page");
        if let Err(err) = launch::spawn(&command) {
            self.show_toast("Couldn’t start Settings", &err.to_string());
        }
    }

    pub fn open_settings_page(&mut self, page: launch::Page) {
        let Some(command) = launch::settings_page(page) else {
            match page {
                launch::Page::Network => self.show_toast(
                    "No network settings app",
                    "Install nm-connection-editor or plasma-nm.",
                ),
                launch::Page::Bluetooth => {
                    self.show_toast("No Bluetooth settings app", "Install bluedevil or blueman.")
                }
            }
            return;
        };
        tracing::info!(?page, ?command, "opening a system settings page");
        if let Err(err) = launch::spawn(&command) {
            self.show_toast("Couldn’t open the settings page", &err.to_string());
        }
    }
}
