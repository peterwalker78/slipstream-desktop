//! The lock screen as drawn: its veil over each screen, and the desktop growing back into place
//! as it goes.

use super::*;

/// The lock's veil, premultiplied as the renderer takes colours.
pub(super) fn veil_colour() -> Color32F {
    let [r, g, b, a] = crate::lock::VEIL;
    Color32F::new(r * a, g * a, b * a, a)
}

/// The lock's veil over a whole screen.
pub(super) fn veil(
    chrome: &mut Chrome,
    size: Size<i32, Logical>,
    scale: Scale<f64>,
    alpha: f32,
) -> OutputElement {
    chrome.veil.update(size, veil_colour());
    OutputElement::Solid(SolidColorRenderElement::from_buffer(
        &chrome.veil,
        (0, 0),
        scale,
        alpha,
        Kind::Unspecified,
    ))
}

/// Everything on a screen while it's locked, front to back: the plain pointer, then on the
/// focused screen the volume and brightness display and the lock's card, then the veil and the
/// living wallpaper. No window, pop-up, toast, bar or client cursor is read.
pub(super) fn lock_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geo: Rectangle<i32, Logical>,
    chrome: &mut Chrome,
    saver: &mut crate::saver::Saver,
    cursor: Option<&mut Cursor>,
) -> Vec<OutputElement> {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let mut elements = Vec::new();
    let now = state.clock.now();
    let wall = state.wall();
    if let Some(cursor) = cursor {
        let pointer = state.pointer_location();
        if output_geo.to_f64().contains(pointer) {
            let pos = pointer - output_geo.loc.to_f64();
            elements.extend(themed_pointer(
                renderer,
                cursor,
                CursorIcon::Default.name(),
                pos,
                scale,
                state.start_time.elapsed(),
            ));
        }
    }
    if state.screens.focused_output().as_ref() == Some(output) {
        let inks = (state.panel_ring(), state.panel_accent());
        elements.extend(
            state
                .osd
                .element(renderer, output_geo.size, scale.x, inks, now)
                .map(OutputElement::Memory),
        );
        let facts = state.lock_facts();
        if let Some(lock) = state.lock.as_mut() {
            elements.extend(
                lock.elements(renderer, output_geo.size, scale.x, &facts, wall)
                    .into_iter()
                    .map(OutputElement::Memory),
            );
        }
    }
    let shown = state.lock.as_ref().map_or(1.0, |lock| lock.shown(wall));
    elements.push(veil(chrome, output_geo.size, scale, shown));
    if let Some(index) = state.screens.index_of(output) {
        let place = state
            .screens
            .get(index)
            .map(|screen| state.workspaces.label(screen.workspace))
            .unwrap_or_default();
        saver.set_readings(&saver_readings(state, place));
        elements.extend(
            saver
                .element(
                    renderer,
                    crate::panel::Frame {
                        size: output_geo.size,
                        scale: scale.x,
                        time: now,
                        accent: state.panel_accent(),
                    },
                    crate::lock::WALLPAPER_GLOW,
                    false,
                    false,
                )
                .map(OutputElement::Memory),
        );
    }
    elements
}

/// `pane`, the desktop, drawn into the screen's unlock texture and shown `zoom` of its size about
/// the screen's centre at `opacity`. `None` when the texture can't be made, and the desktop is
/// drawn plainly instead.
pub(super) fn unlock_zoom(
    renderer: &mut GlesRenderer,
    chrome: &mut Chrome,
    pane: &[OutputElement],
    view: &View,
    zoom: f64,
    opacity: f32,
) -> Option<TextureRenderElement<GlesTexture>> {
    let screen = view.sizes();
    let (logical, physical, scale) = (screen.logical, screen.physical, screen.scale);
    if chrome.unlock_broken {
        return None;
    }
    tilt::draw_offscreen(
        renderer,
        &mut chrome.unlock,
        &mut chrome.unlock_broken,
        pane,
        screen,
        "the unlock",
    )?;
    let texture = &chrome.unlock.as_ref()?.texture;
    let shown = Size::<i32, Logical>::from((
        (logical.w as f64 * zoom).round() as i32,
        (logical.h as f64 * zoom).round() as i32,
    ));
    let at = Point::<f64, Logical>::from((
        (logical.w - shown.w) as f64 / 2.0,
        (logical.h - shown.h) as f64 / 2.0,
    ))
    .to_physical(scale);
    Some(TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        at,
        texture.clone(),
        1,
        Transform::Normal,
        Some(opacity),
        Some(Rectangle::from_size(
            (physical.w as f64, physical.h as f64).into(),
        )),
        Some(shown),
        None,
        Kind::Unspecified,
    ))
}
