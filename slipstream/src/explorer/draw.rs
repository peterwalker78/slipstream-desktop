//! Drawing the explorer: its grid of apps, the side column, and the caret.

use super::*;

/// The painted explorer, its size in logical pixels, and its size in buffer pixels.
pub(super) type Painted = (MemoryRenderBuffer, Size<i32, Logical>, (i32, i32));

/// The caret's size in design pixels. It's an element of its own, so blinking never repaints the
/// explorer.
pub(super) const CARET: (f32, f32) = (3.0, 29.7);
pub(super) const CARET_COLOUR: u32 = panel::AMBER;

/// Everything the painted explorer depends on.
#[derive(Clone, PartialEq)]
pub(super) struct Look {
    pub(super) query: String,
    pub(super) selected: usize,
    pub(super) scroll: usize,
    pub(super) version: u64,
    pub(super) icons_landed: u64,
    pub(super) running: Vec<String>,
    /// The keyboard's ring colour, which marks the selection.
    pub(super) ring: u32,
    pub(super) width: i32,
    pub(super) scale: f64,
    /// How far the answer's decode is, in thirtieths of a second, while it runs.
    pub(super) decode_step: Option<u64>,
    /// The keys named in it.
    pub(super) keys: Keys,
}

impl Explorer {
    /// The explorer's elements, front first (the caret, then the painted explorer), for a screen
    /// `width` logical pixels wide at `scale`, repainted only when what it shows changes.
    /// `running` holds the app IDs and X11 classes of open windows, lowercased.
    #[allow(clippy::too_many_arguments)]
    pub fn element<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
        running: Vec<String>,
        ring: u32,
        keys: Keys,
    ) -> Vec<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if !self.open {
            return Vec::new();
        }
        self.warm_icons(scale);
        let since = now - self.opened_at;
        // A new answer decodes out of rain glyphs, on wall time.
        let answer = self.answer().map(|answer| answer.value);
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        match (&answer, &self.answer_since) {
            (Some(value), Some((shown, _))) if value == shown => {}
            (Some(value), _) => self.answer_since = Some((value.clone(), wall)),
            (None, _) => self.answer_since = None,
        }
        self.answer_age = self
            .answer_since
            .as_ref()
            .map_or(f64::MAX, |(_, at)| wall - at);
        let decode_step = (!self.reduced_motion && self.answer_age < ANSWER_DECODE)
            .then_some((self.answer_age * 30.0) as u64);
        let look = {
            let catalog = self.catalog.lock().unwrap();
            Look {
                query: self.query.clone(),
                selected: self.selected,
                scroll: self.scroll,
                version: catalog.version,
                icons_landed: catalog.tile_icons_landed,
                running,
                ring,
                width,
                scale,
                decode_step,
                keys,
            }
        };
        if self.shown.as_ref() != Some(&look) {
            if self.repaint(&look).is_none() {
                return Vec::new();
            }
            self.shown = Some(look);
        }
        let (alpha, rise) = if self.reduced_motion {
            ((since / REDUCED_FADE).clamp(0.0, 1.0), 0.0)
        } else {
            let eased = HYPR.at(since / OPEN);
            (eased.clamp(0.0, 1.0), -10.0 * (1.0 - eased))
        };
        let Some((buffer, logical, device)) = self.buffer.as_ref() else {
            return Vec::new();
        };
        // Whole screen pixels, or the text blurs.
        let location =
            Point::<f64, Logical>::from((self.origin.x, self.origin.y + rise * DESIGN_PX as f64))
                .to_physical(scale)
                .to_i32_round::<i32>();
        let mut elements = Vec::with_capacity(2);
        // The caret blinks, half a second on and half off, unless motion is reduced: then it
        // stays on.
        let caret_on = self.reduced_motion || (since * 2.0).rem_euclid(2.0) < 1.0;
        if caret_on {
            if self.caret.as_ref().is_none_or(|(at, _, _)| *at != scale) {
                self.caret = caret_buffer(scale);
            }
            if let Some((_, caret, size)) = &self.caret {
                let at = location + self.caret_at.to_physical(scale).to_i32_round::<i32>();
                elements.extend(
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        at.to_f64(),
                        caret,
                        Some(alpha as f32),
                        None,
                        Some(*size),
                        Kind::Unspecified,
                    )
                    .ok(),
                );
            }
        }
        elements.extend(
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                buffer,
                Some(alpha as f32),
                Some(Rectangle::from_size(
                    (device.0 as f64, device.1 as f64).into(),
                )),
                Some(*logical),
                Kind::Unspecified,
            )
            .ok(),
        );
        elements
    }

    pub(super) fn repaint(&mut self, look: &Look) -> Option<()> {
        let results = self.results();
        let screen_w = look.width as f32 / DESIGN_PX;
        let width = WIDTH.min(screen_w - 40.0).max(480.0);
        let logical = Size::<i32, Logical>::from((
            ((width + 2.0 * MARGIN) * DESIGN_PX).ceil() as i32,
            ((HEIGHT + 2.0 * MARGIN + 40.0) * DESIGN_PX).ceil() as i32,
        ));
        let device = (
            (logical.w as f64 * look.scale).round() as i32,
            (logical.h as f64 * look.scale).round() as i32,
        );
        let f = look.scale as f32 * DESIGN_PX;
        let mut p = Painter::new(device.0 as u32, device.1 as u32, f)?;

        // The frame's corner inside the pixmap, and its left edge on screen.
        let (fx, fy) = (MARGIN, MARGIN);
        let left = (screen_w - width) / 2.0;
        let on_screen = |x: f32, y: f32, w: f32, h: f32| {
            Rectangle::<f64, Logical>::new(
                (
                    ((left + x - fx) * DESIGN_PX) as f64,
                    ((TOP + y - fy) * DESIGN_PX) as f64,
                )
                    .into(),
                ((w * DESIGN_PX) as f64, (h * DESIGN_PX) as f64).into(),
            )
        };
        let mut targets = Vec::new();

        // Glass, nearly solid: with no blur behind it, text under a more see-through pane
        // would show through.
        panel::glass(&mut p, fx, fy, width, HEIGHT);

        // The search row.
        let centre = fy + SEARCH_H / 2.0;
        p.icon(
            icons::SEARCH,
            fx + panel::HEAD_INSET,
            centre - 14.0,
            28.0,
            Some(panel::HINT),
        );
        let mut x = fx + panel::HEAD_INSET + 28.0 + 16.0;
        x += p.text(
            &look.query,
            x,
            centre,
            &Style::new(Face::Body, 27.0, panel::BRIGHT),
        );
        // In logical pixels from the painted area's corner.
        self.caret_at = Point::from((
            ((x + 2.0) * DESIGN_PX) as f64,
            ((centre - 13.0) * DESIGN_PX) as f64,
        ));
        if look.query.is_empty() {
            p.text(
                "Apps, files, settings, sums, :emoji",
                x + 9.0,
                centre,
                &Style::new(Face::Body, 27.0, panel::PLACEHOLDER),
            );
        }
        panel::key_hint(
            &mut p,
            fx + width - panel::HEAD_INSET,
            centre,
            &[&look.keys.explorer],
            "",
        );
        p.fill(fx, fy + SEARCH_H, width, 1.0, 0.0, panel::DIVIDER);

        // The apps grid.
        let body_y = fy + SEARCH_H + 1.0;
        let apps_w = width * 1.6 / 2.6;
        let tile_w = (apps_w - 40.0 - TILE_GAP * (COLUMNS - 1) as f32) / COLUMNS as f32;
        let answering = results.answer.is_some() || look.query.trim_start().starts_with(':');
        if results.apps.is_empty() && !answering {
            let message = if look.query.trim().is_empty() {
                "Looking for apps…".to_string()
            } else {
                format!("No apps match “{}”.", look.query.trim())
            };
            p.text(
                &message,
                fx + 30.0,
                body_y + 62.0,
                &Style::new(Face::Body, 16.0, panel::HINT),
            );
        }
        let icon_px = (ICON * f).round() as u32;
        let first = look.scroll * COLUMNS;
        for (index, app) in results
            .apps
            .iter()
            .enumerate()
            .skip(first)
            .take(ROWS * COLUMNS)
        {
            let (row, column) = ((index - first) / COLUMNS, index % COLUMNS);
            let x = fx + 20.0 + column as f32 * (tile_w + TILE_GAP);
            let y = body_y + 20.0 + row as f32 * (TILE_H + TILE_GAP);
            let selected = index == look.selected;
            if selected {
                p.fill(
                    x,
                    y,
                    tile_w,
                    TILE_H,
                    panel::TILE_RADIUS,
                    panel::selection_fill(look.ring),
                );
                p.border(x, y, tile_w, TILE_H, panel::TILE_RADIUS, 2.0, look.ring);
            }
            let icon_x = x + (tile_w - ICON) / 2.0;
            match self.tile_icon(app, icon_px) {
                Some(Some(icon)) => p.image(&icon, icon_x, y + 16.0),
                Some(None) => placeholder(&mut p, &app.name, icon_x, y + 16.0),
                // Still being rasterised: the tile repaints when it lands.
                None => {}
            }
            let style = Style::new(
                Face::Body,
                15.0,
                if selected { 0xffffffff } else { panel::INK },
            );
            let name = text::ellipsize(&app.name, &style, tile_w - 12.0);
            let name_w = text::width(&name, &style);
            p.text(
                &name,
                x + (tile_w - name_w) / 2.0,
                y + 16.0 + ICON + 10.0 + 11.0,
                &style,
            );
            let running = [Some(&app.id), app.wm_class.as_ref()]
                .into_iter()
                .flatten()
                .any(|name| look.running.contains(&name.to_lowercase()));
            if running {
                p.fill(
                    x + tile_w / 2.0 - 2.5,
                    y + TILE_H - 10.0,
                    5.0,
                    5.0,
                    2.5,
                    0x7fe3ffff,
                );
            }
            targets.push((Item::App(app.clone()), on_screen(x, y, tile_w, TILE_H)));
        }
        let rows = results.apps.len().div_ceil(COLUMNS);
        if rows > ROWS {
            let track = ROWS as f32 * (TILE_H + TILE_GAP) - TILE_GAP;
            let thumb = track * ROWS as f32 / rows as f32;
            let top = body_y + 20.0 + track * look.scroll as f32 / rows as f32;
            p.fill(fx + apps_w - 9.0, top, 3.0, thumb, 1.5, 0xffffff26);
        }

        // The side column: answer, emoji, settings, files, run, system.
        let side_x = fx + apps_w;
        p.fill(side_x, body_y, 1.0, BODY_H, 0.0, panel::DIVIDER);
        let (row_x, row_w) = (side_x + 17.0, width - apps_w - 33.0);
        let mut y = body_y + 14.0;
        let mut index = results.apps.len();
        if let Some(answer) = &results.answer {
            y += header(&mut p, row_x, y, "Answer");
            let selected = index == look.selected;
            answer_row(
                &mut p,
                row_x,
                y,
                row_w,
                answer,
                selected,
                look.ring,
                look.decode_step.map(|_| self.answer_age),
            );
            targets.push((
                Item::Answer(answer.clone()),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_STEP;
            index += 1;
        }
        if !results.emoji.is_empty() {
            y += header(&mut p, row_x, y, "Emoji");
        } else if look.query.trim_start().starts_with(':') {
            y += header(&mut p, row_x, y, "Emoji");
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::SEARCH,
                    label: if look.query.trim().len() > 1 {
                        "No emoji match".into()
                    } else {
                        "Type a word: :fire, :thumbs, :party".into()
                    },
                    trailing: None,
                    keycap: false,
                    selected: false,
                    dim: true,
                    glyph: None,
                },
                look.ring,
            );
            y += ROW_STEP;
        }
        for (emoji, name) in &results.emoji {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: "",
                    label: name.to_string(),
                    trailing: (index == look.selected).then(|| "⏎".to_string()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: Some(emoji),
                },
                look.ring,
            );
            targets.push((Item::Emoji(emoji, name), on_screen(row_x, y, row_w, ROW_H)));
            y += ROW_STEP;
            index += 1;
        }
        if !results.settings.is_empty() {
            y += header(&mut p, row_x, y, "Settings");
        }
        for found in &results.settings {
            // A setting found on a page says which page it's on.
            let (label, trailing) = match found.matched {
                Some(setting) => (setting, Some(found.page.title.to_string())),
                None => (found.page.title, None),
            };
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::COG,
                    label: label.to_string(),
                    trailing,
                    keycap: false,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((
                Item::Setting(found.page.id),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_STEP;
            index += 1;
        }
        let files_title = if look.query.trim().is_empty() {
            "Recent files"
        } else {
            "Files"
        };
        // A sum or an emoji search has no use for an empty files list.
        let files_shown = !results.files.is_empty() || !answering;
        if files_shown {
            y += header(&mut p, row_x, y, files_title);
        }
        if results.files.is_empty() && files_shown {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: icons::FILE,
                    label: if look.query.trim().is_empty() {
                        "No recent files".into()
                    } else {
                        "No files match".into()
                    },
                    trailing: None,
                    keycap: false,
                    selected: false,
                    dim: true,
                    glyph: None,
                },
                look.ring,
            );
            y += ROW_STEP;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        for found in &results.files {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: if found.folder {
                        icons::FOLDER
                    } else {
                        icons::FILE
                    },
                    label: file_name(&found.path),
                    trailing: folder_name(&found.path, home.as_deref()),
                    keycap: false,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((
                Item::File(found.path.clone()),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_STEP;
            index += 1;
        }
        let mut run_opens = false;
        if let Some(command) = &results.run {
            y += header(&mut p, row_x, y, "Run");
            let (opens, label) = run_label(command, self.run_is_program(command));
            run_opens = opens;
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: if opens { icons::FILE } else { icons::RUN },
                    label,
                    // Enter's key, as on an emoji, only while Enter would run it.
                    trailing: (index == look.selected).then(|| "⏎".to_string()),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((
                Item::Run(command.clone()),
                on_screen(row_x, y, row_w, ROW_H),
            ));
            y += ROW_STEP;
            index += 1;
        }
        if !results.system.is_empty() {
            y += header(&mut p, row_x, y, "System");
        }
        for action in &results.system {
            side_row(
                &mut p,
                row_x,
                y,
                row_w,
                &Row {
                    icon: action.icon(),
                    label: action.label().into(),
                    trailing: action.key(&look.keys),
                    keycap: true,
                    selected: index == look.selected,
                    dim: false,
                    glyph: None,
                },
                look.ring,
            );
            targets.push((Item::System(*action), on_screen(row_x, y, row_w, ROW_H)));
            y += ROW_STEP;
            index += 1;
        }

        // The footer: what the keys do, with Enter's word for what's chosen.
        let foot_y = body_y + BODY_H;
        p.fill(fx, foot_y, width, 1.0, 0.0, panel::DIVIDER);
        let mut right = fx + width - 22.0;
        let chosen = results.items().into_iter().nth(look.selected);
        let enter = match &chosen {
            Some(Item::Answer(_)) => "copy",
            Some(Item::Emoji(..)) => "type",
            Some(Item::Run(_)) if !run_opens => "run",
            _ => "open",
        };
        let mut keys: Vec<(&[&str], &str)> = vec![(&["Esc"], "clear, close")];
        if matches!(chosen, Some(Item::App(_))) {
            keys.push((&["Shift+⏎"], "new window"));
        }
        keys.extend([
            (&["⏎"][..], enter),
            (&["Tab"], "next section"),
            (&["← → ↑ ↓"], "choose"),
        ]);
        for (keys, label) in keys {
            right = panel::key_hint(&mut p, right, foot_y + FOOT_H / 2.0, keys, label)
                - panel::HINT_GAP;
        }

        self.origin = on_screen(0.0, 0.0, 0.0, 0.0).loc;
        self.frame = on_screen(fx, fy, width, HEIGHT);
        self.targets = targets;
        self.buffer = Some((paint::buffer(&p.pixmap), logical, device));
        Some(())
    }
}

/// The Run row for `command`: whether it opens rather than runs, and its label. URLs and paths
/// open; a path whose first word is a program (`program`) runs, with the rest as arguments.
pub(super) fn run_label(command: &str, program: bool) -> (bool, String) {
    let opens = !program
        && !matches!(
            apps::run_query(command, std::path::Path::new("/home"), |_| false),
            apps::Run::Command(_)
        );
    let label = if opens {
        format!("Open “{command}”")
    } else {
        format!("Run “{command}”")
    };
    (opens, label)
}

/// The caret painted at `scale`, with its size in logical pixels. Sized as the painted explorer
/// is, in whole logical pixels first, so the renderer scales it by the same amount.
pub(super) fn caret_buffer(scale: f64) -> Option<(f64, MemoryRenderBuffer, Size<i32, Logical>)> {
    let size = Size::<i32, Logical>::from((
        ((CARET.0 * DESIGN_PX).round() as i32).max(1),
        ((CARET.1 * DESIGN_PX).round() as i32).max(1),
    ));
    let mut pixmap = Pixmap::new(
        ((size.w as f64 * scale).round() as u32).max(1),
        ((size.h as f64 * scale).round() as u32).max(1),
    )?;
    let [r, g, b, a] = CARET_COLOUR.to_be_bytes();
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(r, g, b, a));
    Some((scale, paint::buffer(&pixmap), size))
}

pub(super) fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The name of the folder a file is in, shown beside it: Home for the home folder itself.
pub(super) fn folder_name(
    path: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Option<String> {
    let folder = path.parent()?;
    if Some(folder) == home {
        return Some("Home".into());
    }
    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// An app with no icon gets a coloured square with its initial.
pub(super) fn placeholder(p: &mut Painter, name: &str, x: f32, y: f32) {
    panel::app_placeholder(p, name, x + 4.0, y + 4.0, ICON - 8.0, 15.0, 30.0);
}

/// A section title in the side column. Returns the height it takes.
pub(super) fn header(p: &mut Painter, x: f32, y: f32, title: &str) -> f32 {
    p.text(
        &title.to_uppercase(),
        x + 10.0,
        y + 19.0,
        &panel::section_style(),
    );
    HEADER_H
}

pub(super) struct Row<'a> {
    pub(super) icon: &'a str,
    pub(super) label: String,
    /// A folder name, or a key shown as a keycap.
    pub(super) trailing: Option<String>,
    pub(super) keycap: bool,
    pub(super) selected: bool,
    pub(super) dim: bool,
    /// Text drawn in the icon's place: an emoji.
    pub(super) glyph: Option<&'a str>,
}

/// The answer to a sum or a conversion: what was understood, then the value, which decodes out of
/// rain glyphs for `decoding` seconds after it changes. Enter copies it.
#[allow(clippy::too_many_arguments)]
pub(super) fn answer_row(
    p: &mut Painter,
    x: f32,
    y: f32,
    w: f32,
    answer: &crate::calc::Answer,
    selected: bool,
    ring: u32,
    decoding: Option<f64>,
) {
    if selected {
        panel::selected_row(p, x, y, w, ROW_H, ring);
    }
    let centre = y + ROW_H / 2.0;
    p.icon(
        icons::RUN,
        x + 12.0,
        centre - 11.0,
        22.0,
        Some(panel::TERTIARY),
    );
    let right = x + w - 12.0;
    let hint = "copy";
    let hint_style = Style::new(Face::Body, 12.0, panel::HINT);
    let hint_w = text::width(hint, &hint_style);
    p.text(hint, right - hint_w, centre, &hint_style);
    let room = w - 46.0 - hint_w - 24.0;
    let colour = if selected { 0xffffffff } else { panel::INK };
    let style = Style::new(Face::Body, 15.0, colour);
    // What was understood, the value, and any unit after it: `5 km = ` `3.107` ` mi`.
    let (lead, value, tail) = match answer.shown.rfind(&answer.value) {
        Some(at) => (
            &answer.shown[..at],
            answer.value.as_str(),
            &answer.shown[at + answer.value.len()..],
        ),
        None => (answer.shown.as_str(), "", ""),
    };
    let value_style = Style::new(Face::BodyBold, 15.0, colour);
    let full = text::width(&answer.shown, &style) + 2.0;
    if full > room || decoding.is_none() {
        let mut at = x + 46.0;
        at += p.text(
            &text::ellipsize(lead, &style, room * 0.6),
            at,
            centre,
            &style,
        );
        let left = (x + 46.0 + room - at).max(0.0);
        at += p.text(
            &text::ellipsize(value, &value_style, left),
            at,
            centre,
            &value_style,
        );
        let left = (x + 46.0 + room - at).max(0.0);
        p.text(&text::ellipsize(tail, &style, left), at, centre, &style);
        return;
    }
    let mut at = x + 46.0;
    at += p.text(lead, at, centre, &style);
    let [r, g, b, _] = ring.to_be_bytes();
    let glyph_style = Style::new(Face::MonoBold, 15.0, u32::from_be_bytes([r, g, b, 0xff]));
    let age = decoding.unwrap_or(f64::MAX);
    let masks = crate::history::decoded(value.chars().count(), age, value.len() as u64);
    for (ch, mask) in value.chars().zip(masks) {
        let shown = mask.unwrap_or(ch).to_string();
        let style = if mask.is_some() {
            &glyph_style
        } else {
            &value_style
        };
        at += p.text(&shown, at, centre, style);
    }
    p.text(tail, at, centre, &style);
}

pub(super) fn side_row(p: &mut Painter, x: f32, y: f32, w: f32, row: &Row, ring: u32) {
    if row.selected {
        panel::selected_row(p, x, y, w, ROW_H, ring);
    }
    let centre = y + ROW_H / 2.0;
    let ink = if row.dim {
        panel::HINT
    } else {
        panel::TERTIARY
    };
    match row.glyph {
        Some(glyph) => {
            let style = Style::new(Face::Body, 20.0, panel::BRIGHT);
            let glyph_w = text::width(glyph, &style);
            p.text(glyph, x + 12.0 + (22.0 - glyph_w) / 2.0, centre, &style);
        }
        None => p.icon(row.icon, x + 12.0, centre - 11.0, 22.0, Some(ink)),
    }
    let mut room = w - 12.0 - 22.0 - 12.0 - 12.0;
    if let Some(trailing) = &row.trailing {
        let right = x + w - 12.0;
        if row.keycap {
            let tw = paint::keycap_width(trailing);
            p.keycap(trailing, right - tw, centre);
            room -= tw + 12.0;
        } else {
            let style = Style::new(Face::Body, 12.0, panel::HINT);
            let tw = text::width(trailing, &style);
            p.text(trailing, right - tw, centre, &style);
            room -= tw + 12.0;
        }
    }
    let colour = if row.dim {
        panel::HINT
    } else if row.selected {
        0xffffffff
    } else {
        panel::INK
    };
    let style = Style::new(Face::Body, 15.0, colour);
    p.text(
        &text::ellipsize(&row.label, &style, room.max(0.0)),
        x + 46.0,
        centre,
        &style,
    );
}
