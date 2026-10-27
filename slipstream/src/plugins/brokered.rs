//! What Slipstream does for a plugin, by its own hand, rather than open a way out of the sandbox
//! to it: tell it which windows are open, play its sounds, and do what its card's buttons ask,
//! opening a web address, copying text or controlling what's playing, only when one is pressed.

use slipstream_config::plugins::OpenWindow;
use smithay::{desktop::Window, utils::IsAlive};

use super::Brokered;
use crate::{Slipstream, launch};

impl Slipstream {
    /// Button `button` on plugin `plugin`'s card was pressed.
    pub fn plugin_press(&mut self, plugin: &str, button: &str) {
        match self.plugins.press(plugin, button) {
            None => {}
            Some(Brokered::Media(transport)) => self.media_key(transport),
            Some(Brokered::Open(address)) => {
                tracing::info!("opening a web address from plugin {plugin}");
                let done: launch::Done = Box::new(|state, result| {
                    if let Err(err) = result {
                        state.show_toast("Couldn’t open it", &err.to_string());
                    }
                });
                launch::run(&["xdg-open".to_string(), address], done);
            }
            Some(Brokered::Copy(text)) => {
                self.put_on_clipboard(crate::history::Clip::Text(text.into()), false);
            }
        }
    }

    /// The windows open, for the plugins that need to see them, a look a second at most. Not
    /// while the screen is locked, whose windows aren't anyone's to see.
    pub fn plugins_see_windows(&mut self, windows: &[Window]) {
        if self.lock.is_some() || !self.plugins.windows_due(std::time::Instant::now()) {
            return;
        }
        let focused = self.focused_window();
        let open: Vec<OpenWindow> = windows
            .iter()
            .filter(|window| window.alive() && !crate::state::unseen(window))
            .map(|window| OpenWindow {
                app: crate::state::window_app_id(window).unwrap_or_default(),
                title: crate::state::window_title(window),
                focused: focused.as_ref() == Some(window),
            })
            .collect();
        self.plugins.windows(&open);
    }

    /// A sound a plugin's card played without a notification, as one of its notifications would
    /// sound: not with sounds off, nor under do not disturb.
    pub fn sound_from_plugin(&mut self, name: String) {
        let settings = &self.settings.notifications;
        if crate::alert::wanted(
            settings.sounds,
            false,
            false,
            false,
            settings.do_not_disturb,
        ) {
            crate::alert::play(crate::alert::Sound::Name(name), &settings.sound_theme);
        }
    }
}
