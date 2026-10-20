//! Notifications arriving from apps and from the desktop itself, and what is done with them.

use super::*;

/// You count as watching a window when it has the keyboard and there has been input this recently.
/// Reading a long page can take a while without a keypress, but past this a sound is more use
/// than not.
pub(super) const PRESENT: std::time::Duration = std::time::Duration::from_secs(60);

/// A picture sent as pixels, drawn into a `px`-pixel square keeping its proportions.
pub(super) fn picture(image: &Image, px: u32) -> Option<Pixmap> {
    let mut source = Pixmap::new(image.width, image.height)?;
    for (dst, src) in source
        .data_mut()
        .chunks_exact_mut(4)
        .zip(image.rgba.chunks_exact(4))
    {
        let alpha = src[3] as u16;
        let premultiply = |channel: u8| ((channel as u16 * alpha + 127) / 255) as u8;
        dst.copy_from_slice(&[
            premultiply(src[0]),
            premultiply(src[1]),
            premultiply(src[2]),
            src[3],
        ]);
    }
    let mut out = Pixmap::new(px, px)?;
    let (w, h, side) = (image.width as f32, image.height as f32, px as f32);
    let k = side / w.max(h);
    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    let transform = Transform::from_row(k, 0.0, 0.0, k, (side - w * k) / 2.0, (side - h * k) / 2.0);
    out.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
    Some(out)
}

impl Slipstream {
    /// A notification from the desktop itself, for what mustn't be missed: it waits in the
    /// notification centre as an app's would, where a toast would be gone in seconds. A critical
    /// one stays up until it's dismissed.
    pub fn notify_self(&mut self, summary: &str, body: &str, critical: bool) {
        self.notification_arrived(Incoming {
            id: notify::next_id(),
            app_name: "Slipstream".to_string(),
            summary: summary.to_string(),
            body: body.to_string(),
            urgency: if critical { 2 } else { 1 },
            expire_timeout: -1,
            ..Incoming::default()
        });
    }

    pub fn notification_event(&mut self, event: notify::Event) {
        match event {
            notify::Event::Notify(incoming) => self.notification_arrived(*incoming),
            notify::Event::Close(id) => {
                if self.notices.remove(id) {
                    notify::closed(vec![id], Reason::Closed);
                }
            }
        }
    }

    pub fn notification_arrived(&mut self, incoming: Incoming) {
        let scale = self.output_scale().unwrap_or(1.0);
        let px = (ICON as f64 * DESIGN_PX as f64 * scale).round() as u32;
        let icon = self.notice_icon(&incoming, px);
        let named = |name: &str| Some(name.to_string()).filter(|name| !name.is_empty());
        let app = incoming
            .desktop_entry
            .as_deref()
            .and_then(|entry| self.explorer.app_name(entry))
            .or_else(|| named(&incoming.app_name))
            .unwrap_or_else(|| "Notification".to_string());
        let app_id = incoming
            .desktop_entry
            .clone()
            .or_else(|| named(&incoming.app_name))
            .map(|id| id.to_lowercase());
        // Where it came from: the window of the nearest process up the sender's tree that has one.
        let origin = self.window_for_pids(&incoming.origin);
        let origin_pid = origin.as_ref().and_then(|window| self.window_pid(window));
        // Attention isn't demanded from someone already giving it: while you're at the window a
        // notification came from, it makes no sound, doesn't wake anything, and a critical one
        // passes like any other rather than staying until dismissed.
        let watching = origin.is_some()
            && origin == self.focused_window()
            && self.lock.is_none()
            && !self.idle.is_faded()
            && self.idle.since_input() < PRESENT;
        let critical = incoming.urgency >= 2 && !watching;
        // A critical notification brings the faded UI back to show itself, though not the lock's
        // screen, which shows nothing of what arrives.
        if critical && self.screens_off {
            self.set_screens_off(false);
        }
        if critical && self.lock.is_none() && self.idle.is_faded() {
            let now = self.clock.tick();
            self.idle.wake(now);
        }
        let settings = &self.settings.notifications;
        if let Some(sound) = incoming.sound.clone()
            && crate::alert::wanted(
                settings.sounds,
                incoming.suppress_sound,
                watching,
                critical,
                settings.do_not_disturb,
            )
        {
            crate::alert::play(sound, &settings.sound_theme);
        }
        let pop = Pop::for_arrival(
            critical,
            incoming.transient,
            incoming.expire_timeout,
            self.centre.is_open(),
            self.settings.notifications.do_not_disturb,
            // Nothing pops up over the lock; it waits for the desktop to come back.
            self.idle.is_faded() || self.lock.is_some(),
            self.settings.notifications.wait_while_typing
                && self.concentration.typing(std::time::Instant::now()),
        );
        let notice = Notice {
            id: incoming.id,
            app,
            app_id,
            origin: origin_pid,
            icon,
            summary: notify::plain(&incoming.summary),
            body: notify::plain(&incoming.body),
            time: self.status.lock().unwrap().time.clone(),
            default_action: incoming.default_action,
            actions: incoming.actions.clone(),
            resident: incoming.resident,
            transient: incoming.transient,
            version: 0,
            digest: false,
        };
        tracing::info!(
            id = notice.id,
            app = notice.app,
            ?pop,
            origin = ?origin.as_ref().map(crate::state::logged_app),
            watching,
            "notification"
        );
        let now = self.clock.tick();
        let wall = self.wall();
        let unread = !self.centre.is_open();
        let dropped = self.notices.arrive(notice, pop, now, wall, unread);
        notify::closed(dropped, Reason::Expired);
        let gone = self.notices.sweep();
        notify::closed(gone, Reason::Expired);
    }

    /// A notification's icon: the picture it sent, else its image or icon by name or file, else
    /// its app's icon.
    fn notice_icon(&mut self, incoming: &Incoming, px: u32) -> Option<Pixmap> {
        if let Some(found) = incoming.image.as_ref().and_then(|image| picture(image, px)) {
            return Some(found);
        }
        let sources = [incoming.image_path.as_deref(), Some(&incoming.app_icon)];
        for source in sources.into_iter().flatten().filter(|s| !s.is_empty()) {
            let source = source.strip_prefix("file://").unwrap_or(source);
            let found = if source.starts_with('/') {
                paint::load_image(Path::new(source), px)
            } else {
                self.explorer.named_icon(source, px)
            };
            if found.is_some() {
                return found;
            }
        }
        [
            incoming.desktop_entry.clone(),
            Some(incoming.app_name.to_lowercase()),
        ]
        .into_iter()
        .flatten()
        .find_map(|id| self.explorer.app_icon(&id, px))
    }

    /// Waiting pop-ups that can show now: the UI is in view, and you've paused typing or they
    /// have waited as long as the settings allow. One shows as itself; several as one card.
    pub fn show_waiting_popups(&mut self, now: f64) {
        if self.idle.is_faded() || self.lock.is_some() || !self.notices.has_waiting() {
            return;
        }
        let wall = self.wall();
        let settings = &self.settings.notifications;
        let typing =
            settings.wait_while_typing && self.concentration.typing(std::time::Instant::now());
        let overdue = self
            .notices
            .oldest_at_pause()
            .is_some_and(|arrived| wall - arrived >= settings.longest_wait_secs());
        if typing && !overdue {
            return;
        }
        let due = self.notices.take_waiting(wall);
        tracing::info!(count = due.len(), overdue, "waiting pop-ups shown");
        match due.as_slice() {
            [] => {}
            [one] => self.notices.pop_up(one.id, one.stay, now),
            several => {
                let digest = self.digest(several);
                let stay = several
                    .iter()
                    .map(|due| due.stay)
                    .fold(POPUP_SHOWN, f64::max);
                self.notices.add(digest, now, Some(stay), false);
            }
        }
    }

    /// One card for several pop-ups that waited: how many, and the newest few by app and title.
    fn digest(&mut self, due: &[Due]) -> Notice {
        let kept: Vec<&Notice> = due
            .iter()
            .filter_map(|due| self.notices.get(due.id))
            .collect();
        let reason = if due.iter().all(|due| due.at_pause) {
            "while you were typing"
        } else {
            "while you were away"
        };
        let line = |notice: &Notice| {
            if notice.summary.is_empty() {
                notice.app.clone()
            } else {
                format!("{}: {}", notice.app, notice.summary)
            }
        };
        let mut lines: Vec<String> = if kept.len() > POPUP_LINES {
            let mut first: Vec<String> =
                kept.iter().take(POPUP_LINES - 1).map(|n| line(n)).collect();
            first.push(format!("and {} more", kept.len() - (POPUP_LINES - 1)));
            first
        } else {
            kept.iter().map(|n| line(n)).collect()
        };
        lines.retain(|line| !line.trim().is_empty());
        let scale = self.output_scale().unwrap_or(1.0);
        let px = (ICON as f64 * DESIGN_PX as f64 * scale).round() as u32;
        let icon = Pixmap::new(px, px).map(|mut pixmap| {
            icons::draw(&mut pixmap, icons::LOGO, 0.0, 0.0, px as f32, None);
            pixmap
        });
        Notice {
            id: notify::next_id(),
            app: "Slipstream".into(),
            app_id: None,
            origin: None,
            icon,
            summary: format!("{} {reason}", due.len()),
            body: lines.join("\n"),
            time: kept.first().map(|n| n.time.clone()).unwrap_or_default(),
            default_action: false,
            actions: Vec::new(),
            resident: false,
            transient: true,
            version: 0,
            digest: true,
        }
    }

    /// A notification clicked, in a pop-up or the centre: its app hears, its window comes
    /// forward, and it goes unless it asked to stay. A digest opens the centre, which lists what
    /// it stands for.
    pub fn open_notice(&mut self, id: u32) {
        let Some(notice) = self.notices.get(id).cloned() else {
            return;
        };
        if notice.digest {
            if !self.centre.is_open() {
                self.toggle_notification_centre();
            }
            return;
        }
        tracing::info!(id, app = notice.app, "notification opened");
        if notice.default_action {
            notify::invoked(id, "default");
        }
        let window = notice
            .origin
            .and_then(|pid| self.window_for_pids(&[pid]))
            .or_else(|| {
                notice
                    .app_id
                    .as_deref()
                    .and_then(|app| self.window_for_app(app))
            });
        if let Some(window) = window {
            self.activate_window(&window);
        }
        if notice.resident {
            let gone = self.notices.dismiss_popup(id);
            notify::closed(gone, Reason::Expired);
        } else {
            self.dismiss_notice(id);
        }
    }

    /// One of a notification's action buttons, from a pop-up or the centre: its app hears which,
    /// and the notification goes unless it asked to stay.
    pub fn invoke_notice_action(&mut self, id: u32, index: usize) {
        let Some(notice) = self.notices.get(id).cloned() else {
            return;
        };
        let Some((action, _)) = notice.actions.get(index) else {
            return;
        };
        tracing::info!(id, app = notice.app, index, "notification action chosen");
        notify::invoked(id, action);
        if notice.resident {
            let gone = self.notices.dismiss_popup(id);
            notify::closed(gone, Reason::Expired);
        } else {
            self.dismiss_notice(id);
        }
    }

    pub fn dismiss_notice(&mut self, id: u32) {
        if self.notices.remove(id) {
            notify::closed(vec![id], Reason::Dismissed);
        }
    }

    pub fn clear_notices(&mut self) {
        let ids = self.notices.clear();
        notify::closed(ids, Reason::Dismissed);
    }
}
