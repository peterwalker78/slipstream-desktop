//! The UI's icons, 24×24 stroke drawings, and the Slipstream logo, rasterised with resvg.

use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg,
};

pub const WIFI: &str = r#"<path d="M2.5 9a14 14 0 0 1 19 0M5.5 12.5a9.5 9.5 0 0 1 13 0M8.8 16a4.8 4.8 0 0 1 6.4 0"/><circle cx="12" cy="19" r=".9" fill="currentColor"/>"#;
pub const WIFI_OFF: &str = r#"<path d="M2.5 9a14 14 0 0 1 19 0M5.5 12.5a9.5 9.5 0 0 1 13 0" opacity=".35"/><path d="M4 4l16 16"/>"#;
pub const VOLUME: &str = r#"<path d="M4 9.5h3.5L12 6v12l-4.5-3.5H4z"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>"#;
/// Not in the mockup, which never mutes: the speaker with a cross instead of waves.
pub const VOLUME_MUTED: &str =
    r#"<path d="M4 9.5h3.5L12 6v12l-4.5-3.5H4z"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>"#;
/// Not in the mockup either: the microphone key gets the same display as the volume keys.
pub const MIC: &str = r#"<rect x="9" y="2.5" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3.5"/>"#;
pub const MIC_MUTED: &str = r#"<rect x="9" y="2.5" width="6" height="11" rx="3" opacity=".35"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3.5" opacity=".35"/><path d="M4 4l16 16"/>"#;
pub const BELL: &str =
    r#"<path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z"/><path d="M10 20.5a2 2 0 0 0 4 0"/>"#;
pub const BLUETOOTH: &str = r#"<path d="M7 7.5l10 9-5 4.5v-18l5 4.5-10 9"/>"#;
pub const SEARCH: &str = r#"<circle cx="10.5" cy="10.5" r="6.5"/><path d="M15.5 15.5l5 5"/>"#;
pub const FILE: &str = r#"<path d="M6 3.5h8l4.5 4.5v12.5H6z"/><path d="M14 3.5V8h4.5"/>"#;
/// A padlock, as the mockup draws it.
pub const LOCK: &str = r#"<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/>"#;
pub const POWER: &str = r#"<path d="M12 3.5v8"/><path d="M6.8 6.8a7.5 7.5 0 1 0 10.4 0"/>"#;
/// The mockup's `motion` arrow: running a command, and reduced motion.
pub const RUN: &str = r#"<path d="M3 12h7M5 8h9M5 16h9"/><path d="M15 6l6 6-6 6"/>"#;
/// Do not disturb: replaces the bell while it's on.
pub const DND: &str = r#"<circle cx="12" cy="12" r="8.5"/><path d="M7.5 12h9"/>"#;
pub const MOON: &str = r#"<path d="M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z"/>"#;
/// Power mode.
pub const GAUGE: &str = r#"<path d="M4.5 16a8 8 0 1 1 15 0"/><path d="M12 13l4-4"/>"#;
/// Brightness.
pub const SUN: &str = r#"<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2.5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8"/>"#;
/// A tick, for a switch that's on.
pub const TICK: &str = r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#;

/// A keyboard: the shortcut sheet.
/// Caps Lock: an arrow pointing up over a bar, as the key is marked.
pub const CAPS_LOCK: &str = r#"<path d="M12 3.5l7.5 7.5H15v4.5H9V11H4.5z"/><path d="M9 19.5h6"/>"#;
pub const KEYBOARD: &str = r#"<rect x="2.5" y="6" width="19" height="12" rx="2"/><path d="M6 10h.5M9 10h.5M12 10h.5M15 10h.5M18 10h.5M8 14h8"/>"#;

/// Two overlapping frames: bullet time's overview, on the bar.
pub const OVERVIEW: &str = r#"<rect x="3" y="7" width="13" height="10" rx="1.5"/><path d="M8 7V5.5A1.5 1.5 0 0 1 9.5 4h10A1.5 1.5 0 0 1 21 5.5V13a1.5 1.5 0 0 1-1.5 1.5H16"/>"#;

/// Media: playing, and paused.
pub const PLAY: &str = r#"<path d="M8 5.5v13l10.5-6.5z"/>"#;
pub const PAUSE: &str = r#"<path d="M8.5 5.5v13M15.5 5.5v13"/>"#;

/// Dismisses a notification.
pub const CLOSE: &str = r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#;
/// A chevron pointing right (›): more behind this, such as a tile's settings page.
pub const CHEVRON: &str = r#"<path d="M9.5 6l6 6-6 6"/>"#;
/// Two chevrons, amber and mint. Brings its own fills: draw it without ink.
pub const LOGO: &str = r##"<path d="M2.5 5.5h9l5.5 6.5-5.5 6.5h-9L8 12z" fill="#ffb547"/><path d="M14 5.5h2.5l5.5 6.5-5.5 6.5H14l5.5-6.5z" fill="#3cf0c0"/>"##;

/// The battery, filled to `percent`. The mockup's is fixed at 78%.
pub fn battery(percent: u8, charging: bool) -> String {
    let fill = if charging { "#3cf0c0" } else { "currentColor" };
    let level = 13.0 * percent.min(100) as f32 / 100.0;
    format!(
        r#"<rect x="2.5" y="7.5" width="17" height="9" rx="2"/><path d="M21.5 10.5v3"/><rect x="4.5" y="9.5" width="{level:.2}" height="5" rx="1" fill="{fill}" stroke="none"/>"#
    )
}

/// Draws the 24×24 icon `body` into `pixmap` at (`x`, `y`), `size` pixels square. With `ink`
/// (0xRRGGBBAA), it's stroked in that colour as the mockup's `.ico` class does; without, the body
/// brings its own paint.
pub fn draw(pixmap: &mut Pixmap, body: &str, x: f32, y: f32, size: f32, ink: Option<u32>) {
    let paint = match ink {
        Some(rgba) => {
            let [r, g, b, a] = rgba.to_be_bytes();
            format!(
                r##"fill="none" stroke="currentColor" color="#{r:02x}{g:02x}{b:02x}" opacity="{:.3}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round""##,
                a as f32 / 255.0
            )
        }
        None => String::new(),
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24" {paint}>{body}</svg>"#
    );
    match usvg::Tree::from_str(&svg, &usvg::Options::default()) {
        Ok(tree) => {
            let k = size / 24.0;
            resvg::render(
                &tree,
                Transform::from_row(k, 0.0, 0.0, k, x, y),
                &mut pixmap.as_mut(),
            );
        }
        Err(err) => tracing::warn!("couldn't draw an icon: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(body: &str, ink: Option<u32>) -> usize {
        let mut pixmap = Pixmap::new(48, 48).unwrap();
        draw(&mut pixmap, body, 0.0, 0.0, 48.0, ink);
        pixmap
            .data()
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .count()
    }

    #[test]
    fn every_icon_draws_something() {
        for body in [
            WIFI,
            WIFI_OFF,
            VOLUME,
            VOLUME_MUTED,
            BELL,
            BLUETOOTH,
            SEARCH,
            FILE,
            POWER,
            RUN,
            DND,
            MOON,
            GAUGE,
            SUN,
            CLOSE,
            MIC,
            MIC_MUTED,
        ] {
            assert!(coverage(body, Some(0xdfe4ecff)) > 20, "{body}");
        }
        assert!(coverage(LOGO, None) > 200);
        assert!(coverage(&battery(50, false), Some(0xdfe4ecff)) > 20);
    }

    #[test]
    fn a_fuller_battery_covers_more() {
        let ink = Some(0xffffffff);
        assert!(coverage(&battery(90, false), ink) > coverage(&battery(10, false), ink));
    }
}
