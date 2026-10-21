//! How light Slipstream is, on the About page: its Featherweight score against other desktops, a
//! strip placing each of them from 0 to 100, a map of where its code goes, and the figures behind
//! the score. The figures are measured for each release, so the score is fixed for a version.

use gtk::cairo;
use slipstream_config::weight::{self, OTHERS, SLIPSTREAM, SLIPSTREAM_REDUCED};

use super::*;

include!(concat!(env!("OUT_DIR"), "/code_parts.rs"));

type Rgb = (f64, f64, f64);
const ACCENT: Rgb = (0.259, 0.827, 1.0);
const MUTED: Rgb = (0.545, 0.569, 0.627);
const TEXT: Rgb = (0.902, 0.910, 0.933);
const FONT: &str = "Atkinson Hyperlegible";

/// The strip's height, and its line's distance from the top.
const STRIP_H: i32 = 58;
const LINE_Y: f64 = 26.0;
/// Room at each end of the strip's line for its "0" and "100".
const STRIP_PAD: f64 = 30.0;
const MAP_H: i32 = 170;

fn code_mb() -> f64 {
    CODE_BYTES as f64 / 1e6
}

pub(super) fn section(page: &gtk::Box) {
    let card = group(page, "Featherweight score");
    let score = SLIPSTREAM.score();
    let place = weight::place(score);
    let of = OTHERS.len() + 1;
    let verdict = label(
        &if place == 1 {
            format!("{score:.0} out of 100, the lightest of {of}")
        } else {
            format!(
                "{score:.0} out of 100, the {} lightest of {of}",
                ordinal(place)
            )
        },
        "row-title",
    );
    let heading = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    heading.append(&verdict);
    heading.append(&label(
        &format!(
            "How light Slipstream is in real use, next to {} other desktops: the space it takes, \
             the memory it uses once you're logged in, and how much of a typical computer it \
             keeps busy while you're away. Lower is lighter. With reduced motion it scores {:.0}.",
            OTHERS.len(),
            SLIPSTREAM_REDUCED.score()
        ),
        "row-sub",
    ));
    card.append(&full_row(&heading));

    let strip = gtk::DrawingArea::builder()
        .content_height(STRIP_H)
        .hexpand(true)
        .has_tooltip(true)
        .build();
    strip.set_draw_func(move |_, cr, width, _| draw_strip(cr, width as f64, score));
    strip.connect_query_tooltip(move |area, x, _, _, tooltip| {
        let near = strip_tooltip(area.width() as f64, x as f64, score);
        tooltip.set_text(near.as_deref());
        near.is_some()
    });
    strip.update_property(&[gtk::accessible::Property::Description(&strip_description(
        score,
    ))]);
    card.append(&full_row(&strip));

    let map = gtk::DrawingArea::builder()
        .content_height(MAP_H)
        .hexpand(true)
        .has_tooltip(true)
        .build();
    map.set_draw_func(|_, cr, width, height| draw_map(cr, width as f64, height as f64));
    map.connect_query_tooltip(|area, x, y, _, tooltip| {
        let rects = squarify(
            &shares(),
            (0.0, 0.0, area.width() as f64, area.height() as f64),
        );
        let hit = rects.iter().zip(CODE_PARTS).find(|((rx, ry, rw, rh), _)| {
            let (x, y) = (x as f64, y as f64);
            x >= *rx && x < rx + rw && y >= *ry && y < ry + rh
        });
        let Some((_, (name, lines))) = hit else {
            return false;
        };
        tooltip.set_text(Some(&format!("{name}: {}", percent(*lines))));
        true
    });
    map.update_property(&[gtk::accessible::Property::Description(
        &CODE_PARTS
            .iter()
            .map(|(name, lines)| format!("{name}, {}", percent(*lines)))
            .collect::<Vec<_>>()
            .join("; "),
    )]);
    let code = gtk::Box::new(gtk::Orientation::Vertical, 8);
    code.append(&label("Where its code goes", "row-title"));
    code.append(&label(
        &format!(
            "{:.1} MB of source, by part. Point at a part for its share.",
            code_mb()
        ),
        "row-sub",
    ));
    code.append(&map);
    card.append(&full_row(&code));

    let facts = gtk::Box::builder()
        .homogeneous(true)
        .spacing(10)
        .css_classes(["row"])
        .build();
    fact(&facts, &format!("{:.0} MB", SLIPSTREAM.disk_mb), "On disk")
        .1
        .set_tooltip_text(Some(
            "What installing Slipstream adds, everything it needs included.",
        ));
    fact(&facts, &format!("{:.0} MB", SLIPSTREAM.memory_mb), "Memory")
        .1
        .set_tooltip_text(Some(
            "What Slipstream and the services it starts use once you're logged in.",
        ));
    fact(
        &facts,
        &format!("{:.1}%", SLIPSTREAM.share_at_rest()),
        "Busy while you're away",
    )
    .1
    .set_tooltip_text(Some(&format!(
        "The share of a typical {}-thread computer the moving wallpaper keeps busy.",
        weight::TYPICAL_THREADS
    )));
    fact(&facts, &SLIPSTREAM.packages.to_string(), "Packages")
        .1
        .set_tooltip_text(Some(
            "What installing Slipstream adds, counting what those need.",
        ));
    card.append(&facts);
}

/// A row in the card's style holding `child` across its whole width.
fn full_row(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::builder().css_classes(["row"]).build();
    row.append(child);
    row
}

/// A figure over its caption, added to `facts`.
fn fact(facts: &gtk::Box, value: &str, caption: &str) -> (gtk::Label, gtk::Label) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let value = label(value, "kv");
    let caption = label(caption, "row-sub");
    column.append(&value);
    column.append(&caption);
    facts.append(&column);
    (value, caption)
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Each desktop on the strip: where along it, its name, its score, and whether it's Slipstream.
fn strip_points(width: f64, ours: f64) -> Vec<(f64, &'static str, f64, bool)> {
    let along = |score: f64| STRIP_PAD + (width - 2.0 * STRIP_PAD) * score / 100.0;
    let mut points: Vec<_> = OTHERS
        .iter()
        .map(|other| {
            let score = other.score();
            (along(score), other.name, score, false)
        })
        .collect();
    points.push((along(ours), "Slipstream", ours, true));
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points
}

fn strip_tooltip(width: f64, x: f64, ours: f64) -> Option<String> {
    let lines: Vec<String> = strip_points(width, ours)
        .into_iter()
        .filter(|(at, ..)| (at - x).abs() <= 7.0)
        .map(|(_, name, score, _)| format!("{name}: {score:.0}"))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn strip_description(ours: f64) -> String {
    strip_points(0.0, ours)
        .into_iter()
        .map(|(_, name, score, _)| format!("{name} {score:.0}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn set(cr: &cairo::Context, (r, g, b): Rgb, alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

fn text_width(cr: &cairo::Context, text: &str) -> f64 {
    cr.text_extents(text).map(|e| e.x_advance()).unwrap_or(0.0)
}

fn draw_strip(cr: &cairo::Context, width: f64, ours: f64) {
    cr.select_font_face(FONT, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.5);
    set(cr, MUTED, 0.35);
    cr.rectangle(STRIP_PAD, LINE_Y - 0.5, width - 2.0 * STRIP_PAD, 1.0);
    let _ = cr.fill();
    set(cr, MUTED, 1.0);
    let ends = [
        ("0", STRIP_PAD - 8.0 - text_width(cr, "0")),
        ("100", width - STRIP_PAD + 8.0),
    ];
    for (text, x) in ends {
        cr.move_to(x, LINE_Y + 4.0);
        let _ = cr.show_text(text);
    }

    // Labels go under the line where there's room, otherwise over it; a name with no room for
    // either is left to its tooltip.
    let points = strip_points(width, ours);
    let mut taken: Vec<(f64, f64, bool)> = Vec::new();
    let mut place = |cr: &cairo::Context, at: f64, text: &str, prefer_above: bool| {
        let w = text_width(cr, text);
        let x = (at - w / 2.0).clamp(0.0, width - w);
        let free = |above: bool| {
            !taken
                .iter()
                .any(|(l, r, a)| *a == above && x < r + 8.0 && x + w + 8.0 > *l)
        };
        let above = [prefer_above, !prefer_above]
            .into_iter()
            .find(|&a| free(a))?;
        taken.push((x, x + w, above));
        Some((x, if above { LINE_Y - 12.0 } else { LINE_Y + 22.0 }))
    };
    // Slipstream's label first, so it's never the one crowded out.
    for (at, name, score, ours) in points
        .iter()
        .filter(|p| p.3)
        .chain(points.iter().filter(|p| !p.3))
    {
        let text = if *ours {
            format!("{name} {score:.0}")
        } else {
            name.split(" + ").next().unwrap_or(name).to_string()
        };
        if *ours {
            cr.select_font_face(FONT, cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            cr.set_font_size(12.5);
        }
        if let Some((x, y)) = place(cr, *at, &text, *ours) {
            set(cr, if *ours { ACCENT } else { MUTED }, 1.0);
            cr.move_to(x, y);
            let _ = cr.show_text(&text);
        }
        if *ours {
            cr.select_font_face(FONT, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            cr.set_font_size(11.5);
        }
    }
    for (at, _, _, ours) in &points {
        let (colour, radius) = if *ours { (ACCENT, 6.0) } else { (MUTED, 4.0) };
        set(cr, colour, 1.0);
        cr.arc(*at, LINE_Y, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

fn shares() -> Vec<f64> {
    CODE_PARTS.iter().map(|(_, lines)| *lines as f64).collect()
}

fn percent(lines: u64) -> String {
    let total: u64 = CODE_PARTS.iter().map(|(_, lines)| lines).sum();
    let share = 100.0 * lines as f64 / total.max(1) as f64;
    if share < 1.0 {
        "under 1%".to_string()
    } else {
        format!("{share:.0}%")
    }
}

fn draw_map(cr: &cairo::Context, width: f64, height: f64) {
    cr.select_font_face(FONT, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.5);
    let rects = squarify(&shares(), (0.0, 0.0, width, height));
    for (rank, ((x, y, w, h), (name, lines_of_code))) in
        rects.into_iter().zip(CODE_PARTS).enumerate()
    {
        // Largest brightest, in the one colour, so no part reads as more important than its size.
        set(cr, ACCENT, 0.55 - 0.032 * rank as f64);
        cr.rectangle(x + 1.0, y + 1.0, (w - 2.0).max(0.0), (h - 2.0).max(0.0));
        let _ = cr.fill();
        // The name, wrapped to the block, and its share under it, where they fit.
        let mut lines = wrap(cr, name, w - 14.0);
        lines.push(percent(*lines_of_code));
        let room = ((h - 8.0) / 15.0).floor().max(0.0) as usize;
        if lines.len() - 1 > room || lines.iter().any(|line| text_width(cr, line) > w - 14.0) {
            continue;
        }
        let last = lines.len() - 1;
        for (row, line) in lines.iter().enumerate().take(room) {
            set(cr, TEXT, if row == last { 0.7 } else { 1.0 });
            cr.move_to(x + 7.0, y + 17.0 + 15.0 * row as f64);
            let _ = cr.show_text(line);
        }
    }
}

/// `text` broken between words into lines no wider than `width`.
fn wrap(cr: &cairo::Context, text: &str, width: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if text_width(cr, &format!("{line} {word}")) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

type Rect = (f64, f64, f64, f64);

/// A squarified treemap: `values` (largest first) laid out in `bounds` as rectangles in the same
/// order, each with an area in proportion, kept as near square as the rest allow.
fn squarify(values: &[f64], bounds: Rect) -> Vec<Rect> {
    let (mut x, mut y, mut w, mut h) = bounds;
    let total: f64 = values.iter().sum();
    if total <= 0.0 || w <= 0.0 || h <= 0.0 {
        return vec![(x, y, 0.0, 0.0); values.len()];
    }
    let areas: Vec<f64> = values.iter().map(|v| v * w * h / total).collect();
    let worst = |row: &[f64], side: f64| {
        let sum: f64 = row.iter().sum();
        let most = row.iter().copied().fold(0.0, f64::max);
        let least = row.iter().copied().fold(f64::INFINITY, f64::min);
        (side * side * most / (sum * sum)).max(sum * sum / (side * side * least))
    };
    let mut out = Vec::with_capacity(values.len());
    let mut start = 0;
    while start < areas.len() {
        let side = w.min(h);
        let mut end = start + 1;
        while end < areas.len()
            && worst(&areas[start..=end], side) <= worst(&areas[start..end], side)
        {
            end += 1;
        }
        let row = &areas[start..end];
        let thick = row.iter().sum::<f64>() / side;
        let mut offset = 0.0;
        for area in row {
            let length = area / thick;
            if w >= h {
                out.push((x, y + offset, thick, length));
            } else {
                out.push((x + offset, y, length, thick));
            }
            offset += length;
        }
        if w >= h {
            x += thick;
            w -= thick;
        } else {
            y += thick;
            h -= thick;
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_compositor_module_is_placed_in_a_part() {
        assert!(
            UNPLACED.is_empty(),
            "place these in a part in slipstream-settings/build.rs: {UNPLACED:?}"
        );
        assert!(
            STALE.is_empty(),
            "no module goes by these any more, so take them out of build.rs: {STALE:?}"
        );
    }

    #[test]
    fn the_code_map_holds_every_part_largest_first() {
        assert_eq!(CODE_PARTS.len(), 12);
        assert!(CODE_PARTS.windows(2).all(|pair| pair[0].1 >= pair[1].1));
        assert!(CODE_PARTS.iter().all(|(_, lines)| *lines > 0));
    }

    #[test]
    fn a_treemap_fills_its_bounds_in_proportion() {
        let values = [50.0, 25.0, 12.0, 8.0, 5.0];
        let rects = squarify(&values, (10.0, 20.0, 400.0, 150.0));
        assert_eq!(rects.len(), values.len());
        let area: f64 = rects.iter().map(|(_, _, w, h)| w * h).sum();
        assert!((area - 400.0 * 150.0).abs() < 1e-6);
        for ((x, y, w, h), value) in rects.iter().zip(values) {
            assert!(*x >= 10.0 - 1e-9 && x + w <= 410.0 + 1e-9);
            assert!(*y >= 20.0 - 1e-9 && y + h <= 170.0 + 1e-9);
            assert!((w * h / (400.0 * 150.0) - value / 100.0).abs() < 1e-9);
        }
    }
}
