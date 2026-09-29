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
#[allow(clippy::too_many_arguments)]
pub(super) fn lock_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geo: Rectangle<i32, Logical>,
    chrome: &mut Chrome,
    saver: &mut crate::saver::Saver,
    cursor: Option<&mut Cursor>,
    scale: Scale<f64>,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    let now = state.clock.now();
    let wall = state.wall();
    if let Some(cursor) = cursor {
        let pointer = state.pointer_location();
        if output_geo.to_f64().contains(pointer) {
            let pos = pointer - output_geo.loc.to_f64();
            let (buffer, hotspot) = cursor.image(
                CursorIcon::Default.name(),
                scale.x,
                state.start_time.elapsed(),
            );
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (pos - hotspot).to_physical(scale),
                &buffer,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                Ok(element) => elements.push(OutputElement::Memory(element)),
                Err(err) => tracing::warn!("couldn't draw the pointer: {err:?}"),
            }
        }
    }
    if state.screens.focused_output().as_ref() == Some(output) {
        let ring = state.panel_ring();
        elements.extend(
            state
                .osd
                .element(renderer, output_geo.size, scale.x, ring, now)
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
        let readings = {
            let status = state.status.lock().unwrap();
            crate::saver::Readings {
                time: status.time.clone(),
                date: status.date.clone(),
                place: state
                    .screens
                    .get(index)
                    .map(|screen| state.workspaces.label(screen.workspace))
                    .unwrap_or_default(),
                battery: status.battery,
            }
        };
        saver.set_readings(&readings);
        elements.extend(
            saver
                .element(
                    renderer,
                    output_geo.size,
                    scale.x,
                    now,
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
#[allow(clippy::too_many_arguments)]
pub(super) fn unlock_zoom(
    renderer: &mut GlesRenderer,
    chrome: &mut Chrome,
    pane: &[OutputElement],
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
    scale: f64,
    zoom: f64,
    opacity: f32,
) -> Option<TextureRenderElement<GlesTexture>> {
    if chrome.unlock_broken {
        return None;
    }
    tilt::draw_offscreen(
        renderer,
        &mut chrome.unlock,
        &mut chrome.unlock_broken,
        pane,
        logical,
        physical,
        scale,
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
