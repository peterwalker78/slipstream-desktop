//! A notification drawn as a card: the pop-ups at the top right, and the cards the notification
//! centre lists.

use super::*;

// Sizes in design pixels.
pub const ICON: f32 = 36.0;
pub(super) const PAD_X: f32 = 14.0;
pub(super) const PAD_Y: f32 = 12.0;
pub(super) const META_H: f32 = 16.0;
pub(super) const TITLE_H: f32 = 20.0;
pub(super) const LINE_H: f32 = 18.0;
pub(super) const POPUP_W: f32 = 440.0;
pub(super) const POPUP_RIGHT: f32 = 14.0;
/// Pop-ups hang from the same line as the panels.
pub(super) const POPUP_TOP: f32 = crate::panel::TOP;
/// A pop-up's fade as its time runs out.
pub(super) const POP_OUT: f64 = 0.25;
pub(super) const POPUP_GAP: f32 = 10.0;
pub(super) const POPUP_LINES: usize = 3;
/// Room around a pop-up for its shadow.
pub(super) const SHADOW: f32 = 48.0;
pub(super) const POP_IN: f64 = 0.3;

pub(super) struct Card {
    pub(super) version: u64,
    pub(super) scale: f64,
    pub(super) height: f32,
    pub(super) painted: Painted,
    /// Its close cross, in design pixels from the painted area's corner.
    pub(super) cross: Option<[f32; 4]>,
    /// Its action buttons, in design pixels from the painted area's corner.
    pub(super) buttons: Vec<[f32; 4]>,
}

impl Notices {
    /// The pop-ups for a screen `width` logical pixels wide, and the transient notifications that
    /// expired since the last frame.
    pub fn popup_elements<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        scale: f64,
        now: f64,
        alpha: f32,
    ) -> (Vec<MemoryRenderBufferRenderElement<R>>, Vec<u32>)
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let before = self.popups.len();
        self.popups.retain(|popup| now - popup.at < popup.stay);
        let expired = if self.popups.len() != before {
            self.sweep()
        } else {
            Vec::new()
        };
        self.hits.clear();
        self.cross_hits.clear();
        self.button_hits.clear();
        let left = width as f32 / DESIGN_PX - POPUP_RIGHT - POPUP_W;
        let mut top = POPUP_TOP;
        let mut elements = Vec::new();
        let Self {
            list,
            popups,
            cards,
            hits,
            cross_hits,
            button_hits,
            reduced_motion,
            ..
        } = self;
        for popup in popups.iter() {
            let Some(notice) = list.iter().find(|notice| notice.id == popup.id) else {
                continue;
            };
            let fresh = cards
                .get(&popup.id)
                .is_some_and(|card| card.version == notice.version && card.scale == scale);
            if !fresh {
                let Some(card) = paint_popup(notice, scale) else {
                    continue;
                };
                cards.insert(popup.id, card);
            }
            let card = &cards[&popup.id];
            let since = now - popup.at;
            let (fade, slide) = if *reduced_motion {
                ((since / REDUCED_FADE).clamp(0.0, 1.0), 0.0)
            } else {
                let eased = HYPR.at((since / POP_IN).clamp(0.0, 1.0));
                (eased.clamp(0.0, 1.0), 40.0 * (1.0 - eased))
            };
            // A pop-up whose time runs out fades rather than vanishing; one that stays until it's
            // dismissed never reaches this.
            let fade = if popup.stay.is_finite() && !*reduced_motion {
                fade.min(((popup.stay - since) / POP_OUT).clamp(0.0, 1.0))
            } else {
                fade
            };
            let at = Point::from((
                ((left - SHADOW) as f64 + slide) * DESIGN_PX as f64,
                ((top - SHADOW) * DESIGN_PX) as f64,
            ));
            elements.extend(card.painted.element(renderer, at, fade as f32 * alpha));
            hits.push((
                popup.id,
                Rectangle::new(
                    ((left * DESIGN_PX) as f64, (top * DESIGN_PX) as f64).into(),
                    (
                        (POPUP_W * DESIGN_PX) as f64,
                        (card.height * DESIGN_PX) as f64,
                    )
                        .into(),
                ),
            ));
            // The controls are placed where the card comes to rest, so a click lands the same
            // whether or not it has finished sliding in.
            let control = |[bx, by, bw, bh]: [f32; 4]| {
                Rectangle::new(
                    (
                        ((left + bx - SHADOW) * DESIGN_PX) as f64,
                        ((top + by - SHADOW) * DESIGN_PX) as f64,
                    )
                        .into(),
                    ((bw * DESIGN_PX) as f64, (bh * DESIGN_PX) as f64).into(),
                )
            };
            if let Some(cross) = card.cross {
                cross_hits.push((popup.id, control(cross)));
            }
            for (index, button) in card.buttons.iter().copied().enumerate() {
                button_hits.push((popup.id, index, control(button)));
            }
            top += card.height + POPUP_GAP;
        }
        cards.retain(|id, _| popups.iter().any(|popup| popup.id == *id));
        (elements, expired)
    }
}

pub(super) fn paint_popup(notice: &Notice, scale: f64) -> Option<Card> {
    let height = card_height(notice, POPUP_W, POPUP_LINES);
    let mut cross = None;
    let mut buttons = Vec::new();
    let logical = Size::<i32, Logical>::from((
        ((POPUP_W + 2.0 * SHADOW) * DESIGN_PX).ceil() as i32,
        ((height + 2.0 * SHADOW) * DESIGN_PX).ceil() as i32,
    ));
    let painted = Painted::new(logical, scale, |p| {
        p.f *= DESIGN_PX;
        crate::panel::notice(p, SHADOW, SHADOW, POPUP_W, height);
        let hits = paint_card(p, notice, SHADOW, SHADOW, POPUP_W, POPUP_LINES);
        cross = Some(hits.cross);
        buttons = hits.buttons;
    })?;
    Some(Card {
        version: notice.version,
        scale,
        height,
        painted,
        cross,
        buttons,
    })
}

pub(super) fn body_style() -> Style {
    Style::new(Face::Body, 14.0, 0xbcc3cfff)
}

/// The body in lines no wider than `width`, at most `max`; the last is cut short if there's more.
/// Its own line breaks are kept.
pub(super) fn body_lines(notice: &Notice, width: f32, max: usize) -> Vec<String> {
    let style = body_style();
    let mut lines: Vec<String> = notice
        .body
        .lines()
        .flat_map(|line| text::wrap(line, &style, width))
        .collect();
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.pop() {
            lines.push(text::ellipsize(
                &format!("{}…", last.trim_end()),
                &style,
                width,
            ));
        }
    }
    lines
}

/// How tall a notification's card is, `width` wide with at most `max_lines` of body.
pub fn card_height(notice: &Notice, width: f32, max_lines: usize) -> f32 {
    let text_w = width - 2.0 * PAD_X - ICON - 12.0;
    let lines = body_lines(notice, text_w, max_lines).len();
    let body_h = if lines > 0 {
        2.0 + lines as f32 * LINE_H
    } else {
        0.0
    };
    let buttons = if notice.actions.is_empty() {
        0.0
    } else {
        BUTTON_GAP + BUTTON_H
    };
    2.0 * PAD_Y + (META_H + 1.0 + TITLE_H + body_h).max(ICON) + buttons
}

/// The action buttons along the bottom of a card.
pub(super) const BUTTON_H: f32 = 28.0;
pub(super) const BUTTON_GAP: f32 = 10.0;

/// Where a card's controls were painted, each box as x, y, w, h: its cross, and its action
/// buttons in order.
#[derive(Debug, Default)]
pub struct CardHits {
    pub cross: [f32; 4],
    pub buttons: Vec<[f32; 4]>,
}

/// Paints a card's contents (not its background) with its corner at (`x`, `y`): the app's icon,
/// its name and the time with a cross after it, the summary and the body, and its action
/// buttons. Says where the cross and the buttons are.
pub fn paint_card(
    p: &mut Painter,
    notice: &Notice,
    x: f32,
    y: f32,
    width: f32,
    max_lines: usize,
) -> CardHits {
    match &notice.icon {
        Some(icon) => p.image(icon, x + PAD_X, y + PAD_Y),
        None => placeholder(p, &notice.app, x + PAD_X, y + PAD_Y),
    }
    let text_x = x + PAD_X + ICON + 12.0;
    let text_w = width - (text_x - x) - PAD_X;
    let meta_centre = y + PAD_Y + META_H / 2.0;
    let meta = Style::new(Face::Body, 12.5, panel::HINT);
    let mut right = x + width - PAD_X - 14.0;
    p.icon(
        icons::CLOSE,
        right,
        meta_centre - 7.0,
        14.0,
        Some(panel::PLACEHOLDER),
    );
    let cross = [right - 8.0, y + PAD_Y - 6.0, 30.0, META_H + 12.0];
    right -= 10.0;
    right -= text::width(&notice.time, &meta);
    p.text(&notice.time, right, meta_centre, &meta);
    p.text(
        &text::ellipsize(&notice.app, &meta, right - 10.0 - text_x),
        text_x,
        meta_centre,
        &meta,
    );
    let title = Style::new(Face::BodyBold, 15.0, INK);
    p.text(
        &text::ellipsize(&notice.summary, &title, text_w),
        text_x,
        y + PAD_Y + META_H + 1.0 + TITLE_H / 2.0,
        &title,
    );
    let body = body_style();
    let first = y + PAD_Y + META_H + 1.0 + TITLE_H + 2.0 + LINE_H / 2.0;
    for (i, line) in body_lines(notice, text_w, max_lines).iter().enumerate() {
        p.text(line, text_x, first + i as f32 * LINE_H, &body);
    }
    let mut buttons = Vec::new();
    if !notice.actions.is_empty() {
        let height = card_height(notice, width, max_lines);
        let top = y + height - PAD_Y - BUTTON_H;
        let label = Style::new(Face::Body, 13.5, INK);
        let mut bx = text_x;
        for (_, text) in &notice.actions {
            let w = (text::width(text, &label) + 24.0).min(x + width - PAD_X - bx);
            if w < 40.0 {
                break;
            }
            panel::quiet_button(p, bx, top, w, BUTTON_H, panel::ROW_RADIUS, false);
            p.text(
                &text::ellipsize(text, &label, w - 24.0),
                bx + 12.0,
                top + BUTTON_H / 2.0,
                &label,
            );
            buttons.push([bx, top, w, BUTTON_H]);
            bx += w + 8.0;
        }
    }
    CardHits { cross, buttons }
}

/// An app with no icon gets a coloured square with its initial, as the explorer draws one.
pub(super) fn placeholder(p: &mut Painter, name: &str, x: f32, y: f32) {
    panel::app_placeholder(p, name, x, y, ICON, 9.0, 17.0);
}

/// The pop-up under `pointer`, in the space's coordinates, when the pop-ups were drawn on a
/// screen whose corner is at `origin` and their areas (`hits`) are kept in that screen's own.
pub fn popup_hit(
    pointer: (f64, f64),
    origin: (f64, f64),
    hits: &[(u32, Rectangle<f64, Logical>)],
) -> Option<u32> {
    let local = (pointer.0 - origin.0, pointer.1 - origin.1);
    hits.iter()
        .find(|(_, area)| area.contains(local))
        .map(|(id, _)| *id)
}
