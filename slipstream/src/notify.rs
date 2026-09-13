//! The session's notification server: `org.freedesktop.Notifications` on the session bus, as the
//! Desktop Notifications spec (1.2) describes it. Apps call `Notify`; each notification goes to
//! the main loop, which pops it up and keeps it for the notification centre (`notices.rs`).
//!
//! zbus serves the bus from its own thread, so a slow client never holds up a frame.

use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{
    interface,
    zvariant::{OwnedValue, Value},
};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Notification IDs, shared by the bus and debug steps. Never 0, which the spec uses for "none".
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// The connection, once the name is ours, for sending signals.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

pub fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed).max(1)
}

/// A notification as an app sent it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Incoming {
    pub id: u32,
    pub app_name: String,
    /// An icon name, or a file's path or `file://` URI.
    pub app_icon: String,
    /// The `desktop-entry` hint: the app's desktop entry ID.
    pub desktop_entry: Option<String>,
    pub summary: String,
    pub body: String,
    /// It has a `default` action, invoked by clicking it.
    pub default_action: bool,
    /// Its other actions, identifier and label, shown as buttons.
    pub actions: Vec<(String, String)>,
    /// The `image-data` hint's picture.
    pub image: Option<Image>,
    /// The `image-path` hint: an icon name, or a file's path or URI.
    pub image_path: Option<String>,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    /// Pops up, but isn't kept.
    pub transient: bool,
    /// Stays after its action is invoked.
    pub resident: bool,
    /// Milliseconds: -1 for the server's choice, 0 for never.
    pub expire_timeout: i32,
}

/// A picture sent as pixels: straight RGBA, 8 bits per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum Event {
    Notify(Box<Incoming>),
    /// The app took its notification back.
    Close(u32),
}

/// Why a notification closed, numbered as the spec numbers them.
#[derive(Debug, Clone, Copy)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

struct Server {
    events: Mutex<Sender<Event>>,
}

impl Server {
    fn send(&self, event: Event) {
        let _ = self.events.lock().unwrap().send(event);
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    async fn get_capabilities(&self) -> Vec<String> {
        ["actions", "body", "icon-static", "persistence"]
            .map(String::from)
            .to_vec()
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            next_id()
        };
        let text = |key: &str| {
            hints
                .get(key)
                .and_then(|value| <&str>::try_from(&**value).ok())
                .map(str::to_string)
        };
        let flag = |key: &str| {
            hints
                .get(key)
                .and_then(|value| bool::try_from(&**value).ok())
                .unwrap_or(false)
        };
        let incoming = Incoming {
            id,
            app_name,
            app_icon,
            desktop_entry: text("desktop-entry"),
            summary: cut(summary, SUMMARY_CHARS),
            body: cut(body, BODY_CHARS),
            default_action: has_default(&actions),
            actions: buttons(&actions),
            // Older apps use the spec's earlier names for the picture.
            image: ["image-data", "image_data", "icon_data"]
                .iter()
                .find_map(|key| hints.get(*key).and_then(image)),
            image_path: text("image-path").or_else(|| text("image_path")),
            urgency: urgency(hints.get("urgency")),
            transient: flag("transient"),
            resident: flag("resident"),
            expire_timeout,
        };
        self.send(Event::Notify(Box::new(incoming)));
        id
    }

    async fn close_notification(&self, id: u32) {
        self.send(Event::Close(id));
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "Slipstream".into(),
            "Slipstream".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }
}

/// Takes the notification server's name on the session bus and serves it, sending what arrives to
/// `events`. If another notification server has the name, it says so in the log and gives up.
pub fn serve(events: Sender<Event>) {
    std::thread::spawn(move || {
        let server = Server {
            events: Mutex::new(events),
        };
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|builder| builder.name(NAME))
            .and_then(|builder| builder.serve_at(PATH, server))
            .and_then(|builder| builder.build());
        match connection {
            Ok(connection) => {
                tracing::info!("serving notifications on the session bus");
                let _ = CONNECTION.set(connection);
            }
            Err(err) => tracing::warn!(
                "couldn't serve notifications (another notification server may have the name): \
                 {err}"
            ),
        }
    });
}

/// Tells apps their notifications closed. Sent from a thread of its own, so the main loop never
/// waits on the bus.
pub fn closed(ids: Vec<u32>, reason: Reason) {
    let Some(connection) = CONNECTION.get().cloned() else {
        return;
    };
    if ids.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for id in ids {
            let body = (id, reason as u32);
            if let Err(err) =
                connection.emit_signal(None::<&str>, PATH, NAME, "NotificationClosed", &body)
            {
                tracing::warn!(id, "couldn't send NotificationClosed: {err}");
            }
        }
    });
}

/// Tells an app one of its notification's actions was chosen.
pub fn invoked(id: u32, action: &str) {
    let Some(connection) = CONNECTION.get().cloned() else {
        return;
    };
    let action = action.to_string();
    std::thread::spawn(move || {
        let body = (id, action.as_str());
        if let Err(err) = connection.emit_signal(None::<&str>, PATH, NAME, "ActionInvoked", &body) {
            tracing::warn!(id, "couldn't send ActionInvoked: {err}");
        }
    });
}

/// The longest summary and body kept, in characters. Any app on the session bus can send a
/// notification, and every paint wraps its body, so a runaway one can't stall frames.
const SUMMARY_CHARS: usize = 256;
const BODY_CHARS: usize = 4096;

/// `text` cut to at most `chars` characters, at a character's boundary.
fn cut(mut text: String, chars: usize) -> String {
    if let Some((at, _)) = text.char_indices().nth(chars) {
        text.truncate(at);
    }
    text
}

/// The most buttons a notification shows, and the longest label.
const BUTTONS: usize = 3;
const LABEL_CHARS: usize = 24;

/// The actions besides `default`, as identifier and label, for buttons: at most `BUTTONS`, each
/// label cut short, and none without a label.
fn buttons(actions: &[String]) -> Vec<(String, String)> {
    actions
        .chunks_exact(2)
        .filter(|pair| pair[0] != "default" && !pair[1].trim().is_empty())
        .take(BUTTONS)
        .map(|pair| {
            (
                pair[0].clone(),
                cut(pair[1].trim().to_string(), LABEL_CHARS),
            )
        })
        .collect()
}

/// Whether the actions, identifier and label in turn, include `default`. An identifier left
/// without its label at the end still counts.
fn has_default(actions: &[String]) -> bool {
    actions.iter().step_by(2).any(|action| action == "default")
}

/// The `image-data` hint, `(iiibiiay)`: width, height, bytes per row, alpha, bits per sample,
/// channels, and the pixels.
fn image(value: &OwnedValue) -> Option<Image> {
    let value = Value::from(value.try_clone().ok()?);
    let (width, height, rowstride, _alpha, bits, channels, data): (
        i32,
        i32,
        i32,
        bool,
        i32,
        i32,
        Vec<u8>,
    ) = value.try_into().ok()?;
    decode(width, height, rowstride, bits, channels, &data)
}

/// Straight RGBA from 8-bit RGB or RGBA rows, `rowstride` bytes apart. Nothing for a picture that
/// doesn't add up, or one far bigger than an icon needs.
fn decode(
    width: i32,
    height: i32,
    rowstride: i32,
    bits: i32,
    channels: i32,
    data: &[u8],
) -> Option<Image> {
    if bits != 8 || !(3..=4).contains(&channels) || !(1..=1024).contains(&width) {
        return None;
    }
    if !(1..=1024).contains(&height) {
        return None;
    }
    if rowstride <= 0 {
        return None;
    }
    let (w, h, stride, channels) = (
        width as usize,
        height as usize,
        rowstride as usize,
        channels as usize,
    );
    let row = w * channels;
    let needed = (h - 1)
        .checked_mul(stride)
        .and_then(|rows| rows.checked_add(row))?;
    if stride < row || data.len() < needed {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let start = row * stride;
        for pixel in data[start..start + w * channels].chunks_exact(channels) {
            let alpha = if channels == 4 { pixel[3] } else { 255 };
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], alpha]);
        }
    }
    Some(Image {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

/// The `urgency` hint: 0 low, 1 normal, 2 critical. The spec says a byte, but some apps send an
/// `i` or a `u`, and a critical notification mustn't be missed for that. Anything else is normal.
fn urgency(value: Option<&OwnedValue>) -> u8 {
    let Some(value) = value.map(|value| &**value) else {
        return 1;
    };
    let level = u8::try_from(value)
        .map(i64::from)
        .or_else(|_| i32::try_from(value).map(i64::from))
        .or_else(|_| u32::try_from(value).map(i64::from));
    level.map_or(1, |level| level.clamp(0, 2) as u8)
}

/// Plain text from a summary or body. Slipstream doesn't offer markup, but some apps send it
/// anyway: its tags go (a line or paragraph break leaving a space, so words don't run together),
/// entities are decoded, and runs of white space become one space. A `<` that doesn't start one
/// of those tags stays as it is.
pub fn plain(text: &str) -> String {
    const TAGS: [&str; 9] = ["a", "b", "br", "i", "img", "p", "s", "span", "u"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let tag_end = after.find('>').filter(|end| *end <= 512).filter(|end| {
            let name: String = after[..*end]
                .trim_start_matches('/')
                .chars()
                .take_while(|ch| ch.is_ascii_alphabetic())
                .collect();
            TAGS.contains(&name.to_ascii_lowercase().as_str())
        });
        match tag_end {
            Some(end) => {
                let name: String = after[..end]
                    .trim_start_matches('/')
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphabetic())
                    .collect();
                if name.eq_ignore_ascii_case("br") || name.eq_ignore_ascii_case("p") {
                    out.push(' ');
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with its entities decoded in one pass, so `&amp;lt;` is `&lt;` and not `<`: the named
/// ones markup uses, `&nbsp;`, and numeric ones in decimal or hex. One that isn't valid stays as
/// it was written.
fn entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let decoded = after
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| Some((entity(&after[..end])?, end)));
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "amp" => Some('&'),
        "nbsp" => Some(' '),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code).filter(|ch| *ch != '\0')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_dropped_but_a_plain_less_than_stays() {
        assert_eq!(
            plain("<b>Sam</b> said: <i>see you</i> at 10 &amp; bring\n tea"),
            "Sam said: see you at 10 & bring tea"
        );
        assert_eq!(plain("5 < 6 and 7 > 3"), "5 < 6 and 7 > 3");
        assert_eq!(plain("<a href=\"https://example.com\">link</a>"), "link");
    }

    #[test]
    fn rgb_rows_with_padding_decode_to_rgba() {
        // Two pixels per row, three bytes each, padded to a stride of 8.
        let data = [
            255, 0, 0, 0, 255, 0, 9, 9, //
            0, 0, 255, 1, 2, 3,
        ];
        let image = decode(2, 2, 8, 8, 3, &data).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.rgba[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&image.rgba[8..], &[0, 0, 255, 255, 1, 2, 3, 255]);
        assert_eq!(decode(2, 2, 8, 8, 3, &data[..10]), None, "too short");
        assert_eq!(decode(2, 2, 8, 16, 3, &data), None, "16 bits a sample");
    }

    #[test]
    fn plain_breaks_become_spaces() {
        assert_eq!(plain("a<br>b"), "a b");
        assert_eq!(plain("a<br/>b<BR />c"), "a b c");
        assert_eq!(plain("<p>First</p><p>Second</p>"), "First Second");
        assert_eq!(plain("keep<b>together</b>"), "keeptogether");
    }

    #[test]
    fn numeric_entities_decode() {
        assert_eq!(plain("it&#8217;s"), "it’s");
        assert_eq!(plain("&#x1F389; party&#X21;"), "🎉 party!");
        assert_eq!(plain("a&nbsp;b"), "a b");
        assert_eq!(plain("&amp;lt; stays &lt;"), "&lt; stays <");
        assert_eq!(
            plain("&#xZZ; &#99999999; &#0; &bogus; AT&T"),
            "&#xZZ; &#99999999; &#0; &bogus; AT&T",
            "invalid ones are left as written"
        );
    }

    #[test]
    fn urgency_as_i32_is_critical() {
        let value = |v: Value<'static>| OwnedValue::try_from(v).unwrap();
        assert_eq!(urgency(Some(&value(Value::from(2u8)))), 2);
        assert_eq!(urgency(Some(&value(Value::from(2i32)))), 2);
        assert_eq!(urgency(Some(&value(Value::from(2u32)))), 2);
        assert_eq!(urgency(Some(&value(Value::from(0i32)))), 0);
        assert_eq!(
            urgency(Some(&value(Value::from(7i32)))),
            2,
            "past critical is critical"
        );
        assert_eq!(urgency(Some(&value(Value::from(-3i32)))), 0);
        assert_eq!(urgency(Some(&value(Value::from("high")))), 1);
        assert_eq!(urgency(None), 1);
    }

    #[test]
    fn a_negative_rowstride_is_refused() {
        assert_eq!(decode(2, 2, -1, 8, 3, &[0; 16]), None);
    }

    #[test]
    fn a_stride_that_overflows_is_refused() {
        assert_eq!(decode(1024, 1024, i32::MAX, 8, 4, &[]), None);
    }

    /// An `image-data` hint as it comes off the bus.
    fn hint(width: i32, height: i32, stride: i32, channels: i32, data: Vec<u8>) -> OwnedValue {
        let alpha = channels == 4;
        OwnedValue::try_from(Value::from((
            width, height, stride, alpha, 8i32, channels, data,
        )))
        .unwrap()
    }

    #[test]
    fn image_data_from_a_real_variant() {
        let rgb = vec![
            255, 0, 0, 0, 255, 0, 9, 9, //
            0, 0, 255, 1, 2, 3, 9, 9,
        ];
        let image = super::image(&hint(2, 2, 8, 3, rgb)).expect("RGB with padding");
        assert_eq!(
            image.rgba,
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 255]
        );
        let rgba = super::image(&hint(1, 2, 4, 4, vec![1, 2, 3, 4, 5, 6, 7, 8])).expect("RGBA");
        assert_eq!((rgba.width, rgba.height), (1, 2));
        assert_eq!(rgba.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
        let huge = hint(2000, 2000, 8000, 4, vec![0; 2000 * 2000 * 4]);
        assert_eq!(super::image(&huge), None, "far bigger than an icon");
    }

    #[test]
    fn an_odd_action_list_is_fine() {
        assert!(has_default(&["default".to_string()]));
        assert!(has_default(&[
            "reply".to_string(),
            "Reply".to_string(),
            "default".to_string()
        ]));
        assert!(!has_default(&["reply".to_string()]));
        assert!(!has_default(&[]));
    }

    #[test]
    fn long_text_is_cut_at_a_character() {
        let body = cut("é".repeat(10_000), BODY_CHARS);
        assert_eq!(body.chars().count(), 4096);
        assert_eq!(cut("short".to_string(), SUMMARY_CHARS), "short");
        let summary = cut("ab".repeat(200), SUMMARY_CHARS);
        assert_eq!(summary.len(), 256);
    }

    #[test]
    fn actions_besides_default_become_a_few_buttons() {
        let actions: Vec<String> = [
            "default",
            "Open",
            "reply",
            "Reply",
            "mute",
            "",
            "later",
            "Remind me in an hour or two",
            "archive",
            "Archive",
            "spam",
            "Spam",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let buttons = buttons(&actions);
        assert_eq!(
            buttons
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["reply", "later", "archive"],
            "no default, none without a label, three at most"
        );
        assert_eq!(buttons[1].1.chars().count(), LABEL_CHARS);
    }
}
