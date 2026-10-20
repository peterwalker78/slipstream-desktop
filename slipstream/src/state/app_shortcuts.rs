//! Shortcuts apps keep while another app has the keyboard: what the portal asks, the card's
//! answers, and the file they are written to (`shortcuts/`).

use std::time::Duration;

use smithay::{
    input::keyboard::Keysym,
    utils::{Clock, Logical, Monotonic, Point},
};

use slipstream_config::shortcuts as file;

use crate::{
    Slipstream,
    keys::Mods,
    shortcuts::{
        Answer, Pending,
        portal::{self, Event},
    },
};

impl Slipstream {
    /// Something an app asked through the shortcuts portal.
    pub fn shortcut_event(&mut self, event: Event) {
        match event {
            Event::Opened { session, app } => {
                // The name its desktop entry gives, else its app ID.
                let name = self
                    .explorer
                    .app_name(&app)
                    .or_else(|| (!app.is_empty()).then(|| app.clone()))
                    .unwrap_or_else(|| "An app".to_string());
                self.shortcuts.opened(session, app, name);
            }
            Event::Bind {
                session,
                wanted,
                reply,
            } => {
                let now = self.wall();
                let pending = Pending::new(session, wanted, reply);
                if self.shortcuts.bind(pending, now, self.clock.reduced_motion) {
                    self.wake_ui();
                }
            }
            Event::List { session, reply } => {
                let _ = reply.try_send(self.shortcuts.list(&session));
            }
            Event::Closed { session } => {
                self.shortcuts.closed(&session);
            }
        }
    }

    /// A key while the card asking about an app's shortcuts is up.
    pub fn shortcuts_key(&mut self, key: Keysym, mods: Mods) {
        let now = self.wall();
        let answer = self
            .shortcuts
            .key(key, mods, now, self.clock.reduced_motion);
        self.shortcuts_answered(answer);
    }

    pub fn shortcuts_click(&mut self, x: f64, y: f64) {
        let now = self.wall();
        let answer = self.shortcuts.click(x, y, now, self.clock.reduced_motion);
        self.shortcuts_answered(answer);
    }

    pub fn shortcuts_hover(&mut self, pos: Point<f64, Logical>) {
        let Some((_, screen)) = self.overlay_screen() else {
            return;
        };
        let pos = pos - Point::from((screen.x as f64, screen.y as f64));
        if let Some(ask) = self.shortcuts.ask_mut() {
            ask.hover(pos.x, pos.y);
        }
    }

    fn shortcuts_answered(&mut self, answer: Answer) {
        if answer == (Answer::Answered { written: true })
            && let Err(err) = file::write(&file::path(), self.shortcuts.granted())
        {
            tracing::warn!("couldn't write down the shortcuts apps were given: {err}");
        }
        // Another request may have been waiting behind the card.
        if self.shortcuts.ask().is_some() {
            self.wake_ui();
        }
    }

    /// The shortcuts file changed, most likely because one was taken away in Settings.
    pub fn shortcuts_file_changed(&mut self) {
        let time = Duration::from(Clock::<Monotonic>::new().now()).as_millis() as u64;
        let granted = file::read(&file::path());
        for signal in self.shortcuts.reload(granted, time) {
            portal::emit(signal);
        }
    }
}
