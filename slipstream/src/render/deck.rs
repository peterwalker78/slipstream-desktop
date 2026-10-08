//! Alt+Tab's deck of glass panes, and the label under the front one.

use super::*;

/// Alt+Tab's deck on the screen `output`: each window's pane, nearest first, the name of the one
/// at the front, and the dark behind them all. The panes lift from where their windows are drawn
/// and fly back there.
#[allow(clippy::too_many_arguments)]
pub(super) fn deck_elements_for(
    state: &mut Slipstream,
    chrome: &mut Chrome,
    renderer: &mut GlesRenderer,
    deck: &mut crate::deck::Deck<Window>,
    output: &Output,
    output_geo: Rectangle<i32, Logical>,
    scale: f64,
    now: f64,
    wall: f64,
    camera: f64,
    step: f64,
    rain_output: bool,
    ui_alpha: f32,
) -> Vec<OutputElement> {
    use crate::pane::{Look, Pose};
    let _ = output;
    let screen = (output_geo.size.w as f64, output_geo.size.h as f64);
    let eye = crate::deck::camera(screen);
    let off_screen = Pose {
        z: 3.0 * screen.0,
        ..Pose::flat([
            screen.0 * 0.25,
            screen.1 * 0.25,
            screen.0 * 0.5,
            screen.1 * 0.5,
        ])
    };
    // Where each window is drawn now, which is where its pane lifts from and lands.
    let live: Vec<Pose> = deck
        .windows
        .iter()
        .map(|window| {
            // The docked pane lifts from where it hangs.
            if rain_output
                && state.is_docked(window)
                && let Some(rect) = state.docked_rect()
            {
                return Pose::flat([
                    (rect.x - output_geo.loc.x) as f64,
                    (rect.y - output_geo.loc.y) as f64,
                    rect.w as f64,
                    rect.h as f64,
                ]);
            }
            if rain_output && let Some(stream) = state.rain.index_of(window) {
                let screen_rect = Rect {
                    x: 0,
                    y: 0,
                    w: output_geo.size.w,
                    h: output_geo.size.h,
                };
                let column = Rain::column(stream, screen_rect, bar::HEIGHT);
                let w = column.w as f64;
                return Pose::flat([column.x as f64, column.y as f64, w, w * 0.75]);
            }
            let (Some(index), Some(frame)) = (
                state.workspaces.find(window),
                state.motion.frame(window, now),
            ) else {
                return off_screen;
            };
            let origin = state
                .screen_rect_for_workspace(index)
                .map_or(output_geo.loc.x, |rect| rect.x);
            let dx = shift_for_workspace(index, camera, step, output_geo.loc.x, origin);
            let own = window.geometry().size;
            let [x, y, w, h] = frame.rect;
            let (w, h) = if frame.moving {
                (w, h)
            } else {
                (own.w as f64, own.h as f64)
            };
            Pose::flat([
                x + dx - output_geo.loc.x as f64,
                y - output_geo.loc.y as f64,
                w,
                h,
            ])
        })
        .collect();
    let releasing = deck.releasing();
    let mut panes: Vec<(usize, Pose, f32, f32)> = Vec::new();
    for (i, window) in deck.windows.iter().enumerate() {
        if !window.alive() {
            continue;
        }
        let own = window.geometry().size;
        let (pose, alpha, shade) = deck.pose(
            i,
            live[i],
            (own.w.max(1) as f64, own.h.max(1) as f64),
            screen,
            wall,
        );
        panes.push((i, pose, alpha, shade));
    }
    if !releasing {
        for (i, pose, ..) in &panes {
            deck.last[*i] = Some(*pose);
        }
    }
    // Nearest first, as elements go front to back. The docked pane hangs in front of the
    // windows: level with them it is drawn over them, as it is at rest, and flying home it is
    // in front all the way, since the others land beneath it.
    let docked = deck
        .windows
        .iter()
        .position(|window| state.is_docked(window));
    let depth = |(i, pose, ..): &(usize, Pose, f32, f32)| match docked {
        Some(docked) if docked == *i && releasing => f64::MIN,
        Some(docked) if docked == *i => pose.z - 1.0,
        _ => pose.z,
    };
    panes.sort_by(|a, b| depth(a).total_cmp(&depth(b)));
    let mut elements = Vec::new();
    let mut hits = Vec::new();
    for (i, pose, alpha, shade) in &panes {
        let ring = (!releasing && *i == deck.front()).then_some((state.ring_rgb, 0.9));
        let look = Look {
            alpha: alpha * ui_alpha,
            ring,
            shade: *shade,
            ..Default::default()
        };
        if let Some(element) = chrome.panes.element(
            renderer,
            &deck.windows[*i],
            pose,
            &eye,
            output_geo.size,
            scale,
            &look,
        ) {
            elements.push(OutputElement::Shaded(element));
        }
        if *alpha > 0.5
            && let Some(bounds) = crate::pane::bounds(pose, &eye, 0.0)
        {
            hits.push((*i, bounds));
        }
    }
    let dim = deck.dim(wall) as f32;
    // The front pane's name, under it.
    if !releasing
        && let Some((_, front, ..)) = panes.iter().find(|(i, ..)| *i == deck.front())
        && let Some((x, y)) = crate::pane::project(front, &eye, (0.5, 1.0))
    {
        let window = &deck.windows[deck.front()];
        let place = if state.rain.contains(window) {
            "in the rain".to_string()
        } else if state.is_docked(window) {
            "on the bar".to_string()
        } else {
            state
                .workspaces
                .find(window)
                .map(|index| format!("workspace {}", state.workspaces.label(index)))
                .unwrap_or_default()
        };
        let label = format!("{}  ·  {place}", state.stream_name(window));
        if chrome
            .deck_label
            .as_ref()
            .is_none_or(|(said, painted)| *said != label || painted.scale != scale)
        {
            chrome.deck_label = paint_deck_label(&label, scale).map(|painted| (label, painted));
        }
        if let Some((_, painted)) = chrome.deck_label.as_ref() {
            // The painted area reaches past the card by its shadow's margin.
            let m = (panel::MARGIN * DESIGN_PX) as f64;
            let at = Point::<f64, Logical>::from((
                (x - painted.logical.w as f64 / 2.0).max(8.0 - m),
                y + 22.0 - m,
            ));
            elements.splice(
                0..0,
                painted
                    .element(renderer, at, dim * ui_alpha)
                    .map(OutputElement::Memory),
            );
        }
    }
    chrome.deck_dim.update(output_geo.size, BLACK);
    elements.push(OutputElement::Solid(SolidColorRenderElement::from_buffer(
        &chrome.deck_dim,
        Point::<i32, Physical>::from((0, 0)),
        Scale::from(scale),
        0.62 * dim * ui_alpha,
        Kind::Unspecified,
    )));
    if let Some(switcher) = state.switcher.as_mut() {
        switcher.set_hits(hits);
    }
    elements
}

/// The deck's name tag: the front window's name and where it lives, on the panels' glass.
/// The painted area holds the card's shadow around it: `panel::MARGIN` design pixels at the
/// sides and top, and `panel::BELOW` more underneath.
pub(super) fn paint_deck_label(label: &str, scale: f64) -> Option<paint::Painted> {
    let style = Style {
        tracking: 0.02,
        ..Style::new(Face::Display, 27.0, panel::BRIGHT)
    };
    let width = text::width(label, &style) + 2.0 * DECK_LABEL_PAD;
    let m = panel::MARGIN;
    let logical = Size::<i32, Logical>::from((
        ((width + 2.0 * m) * DESIGN_PX).ceil() as i32,
        ((DECK_LABEL_H + 2.0 * m + panel::BELOW) * DESIGN_PX).ceil() as i32,
    ));
    paint::Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        panel::glass(p, m, m, width, DECK_LABEL_H);
        p.text(label, m + DECK_LABEL_PAD, m + DECK_LABEL_H / 2.0, &style);
    })
}

/// The deck's name tag's height and the room at its ends, in design pixels.
pub(super) const DECK_LABEL_H: f32 = 56.0;

pub(super) const DECK_LABEL_PAD: f32 = 24.0;
