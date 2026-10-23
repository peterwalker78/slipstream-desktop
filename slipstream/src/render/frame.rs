//! One screen's frame, front to back: the pointer, the cards and pop-ups, the bar, the windows
//! (tilted in bullet time), the glass the UI fades through, and the living wallpaper behind it
//! all. Each layer has a function of its own; `output_elements` puts them in order.

use super::*;

/// What every part of one screen's frame works from: where the screen is, how far the UI has
/// faded, and how far bullet time has zoomed out and tilted.
pub(super) struct View {
    pub name: String,
    /// The screen's place in the space.
    pub geo: Rectangle<i32, Logical>,
    pub scale: Scale<f64>,
    pub physical: Size<i32, Physical>,
    pub now: f64,
    pub wall: f64,
    /// How visible the UI is: it fades after a while with no input, leaving the wallpaper.
    pub ui: f32,
    /// Whether the UI fades as a pane of glass passing through the wallpaper (`glass.rs`), in
    /// which case everything in the pane is drawn at full strength. Reduced motion keeps a plain
    /// fade.
    pub glass: bool,
    /// What the UI is drawn at: `ui`, or full strength inside the glass.
    pub ui_alpha: f32,
    /// Bullet time's overview: 0 normally, 1 zoomed right out. It runs on wall time, so opening
    /// and closing it is never slowed.
    pub zoomed_out: f64,
    /// How large the overview draws the workspaces.
    pub zoom: f64,
    pub centre: (f64, f64),
    pub screen_index: Option<usize>,
    /// Bullet time, the panels and the cards are drawn where the keyboard is.
    pub first: bool,
    /// Every screen tiles windows, draws a bar and runs the wallpaper.
    pub tiling: bool,
    /// The code rain stays on the leftmost screen, where the tiling area gives up the width
    /// for it.
    pub rain: bool,
    /// The workspace this screen is showing, which is not the same as the one the keyboard is on.
    pub active: usize,
    /// The workspace bullet time is looking at.
    pub viewed: usize,
    /// Workspaces sit side by side, a screen and a gap apart, and the view slides between them.
    pub step: f64,
    pub camera: f64,
    /// How far along the slide the workspace on screen is drawn.
    pub active_dx: f64,
    /// The tiling area, in this screen's own coordinates.
    pub area: Option<Rectangle<f64, Logical>>,
    /// Whether the overview is tilted this frame.
    pub tilting: bool,
    /// How much denser than the screen the tilted overview is built.
    pub dense: f64,
    /// The density everything inside the tilt is built at.
    pub world_scale: Scale<f64>,
    pub tilt: Tilt,
    pub focused: Option<Window>,
    pub docked: Option<Window>,
    /// The bar is nearer the eye than the windows once a pane hangs from it in front of them, so
    /// while the two fade it and its pane are drawn into a glass of their own, which trails the
    /// windows' on the way out and leads it back.
    pub bar_apart: bool,
    pub selected: Option<Target<Window>>,
    pub labels: Vec<(Target<Window>, String)>,
}

impl View {
    fn new(
        state: &mut Slipstream,
        renderer: &mut GlesRenderer,
        chrome: &mut Chrome,
        output: &Output,
        geo: Rectangle<i32, Logical>,
        now: f64,
    ) -> Self {
        let name = output.name();
        let screen_index = state.screens.index_of(output);
        let scale = Scale::from(output.current_scale().fractional_scale());
        let ui = state.ui_opacity(now);
        let wall = state.wall();
        let zoomed_out = state.overview.value(wall)[0].max(0.0);
        let zoom = 1.0 - 0.32 * zoomed_out;
        let centre = (geo.size.w as f64 / 2.0, geo.size.h as f64 / 2.0);
        let area = state.screen_area(screen_index.unwrap_or(0)).map(|area| {
            Rectangle::<f64, Logical>::new(
                ((area.x - geo.loc.x) as f64, (area.y - geo.loc.y) as f64).into(),
                (area.w as f64, area.h as f64).into(),
            )
        });
        let (viewed, selected, labels) = match state.bullet.as_ref() {
            Some(mode) => (mode.view, mode.selected.clone(), mode.labels.clone()),
            None => (state.active_workspace(), None, Vec::new()),
        };
        let first = state.screens.focused_output().as_ref() == Some(output);
        let active = screen_index
            .and_then(|index| state.screens.get(index))
            .map(|screen| screen.workspace)
            .unwrap_or_else(|| state.active_workspace());
        let step = (geo.size.w + motion::WORKSPACE_GAP) as f64;
        let camera = state.motion.camera(&name, wall);
        let tiling = screen_index.is_some();
        let rain = screen_index == Some(0);
        let glass = tiling
            && ui > 0.0
            && ui < 1.0
            && !state.clock.reduced_motion
            && chrome.glass.ready(renderer);
        let bar_apart = glass && rain && zoomed_out <= 0.0 && state.docked_on_show().is_some();
        // Bullet time leans the overview back like a floor, in a near perspective, and turns it a
        // little while gliding between workspaces, unless fingers are moving it.
        let velocity = if state.clock.reduced_motion || !state.motion.turns(&name) {
            0.0
        } else {
            (camera - state.motion.camera(&name, wall - 1.0 / 60.0)) * 60.0
        };
        let tilting = first && zoomed_out > 0.0 && chrome.stage.ready(renderer);
        let physical = output
            .current_mode()
            .map(|mode| mode.size)
            .unwrap_or_else(|| geo.size.to_f64().to_physical(scale).to_i32_round());
        // Tilted, the overview is drawn into a texture denser than the screen and sampled back
        // down, which is what keeps a zoomed-out window's text sharp. Everything inside the tilt
        // is built at that density; the bar, the labels in front and the rest of the screen keep
        // the screen's own.
        let dense = if tilting {
            tilt::supersample(physical)
        } else {
            1.0
        };
        let tilt = if tilting {
            Tilt {
                pitch: (15.0 * zoomed_out).to_radians(),
                yaw: (velocity * 2.0).clamp(-6.0, 6.0).to_radians() * zoomed_out.min(1.0),
                distance: 1.15 * geo.size.w as f64,
                centre,
                lift: 27.0 * zoomed_out,
                // A slight fish-eye lens over the tilted overview.
                lens: 0.05 * zoomed_out.min(1.0),
            }
        } else {
            Tilt {
                centre,
                ..Tilt::default()
            }
        };
        Self {
            name,
            geo,
            scale,
            physical,
            now,
            wall,
            ui,
            glass,
            ui_alpha: if glass { 1.0 } else { ui },
            zoomed_out,
            zoom,
            centre,
            screen_index,
            first,
            tiling,
            rain,
            active,
            viewed,
            step,
            camera,
            active_dx: (active as f64 - camera) * step,
            area,
            tilting,
            dense,
            world_scale: Scale::from((scale.x * dense, scale.y * dense)),
            tilt,
            focused: state.focused_window(),
            docked: state
                .docking
                .dock
                .as_ref()
                .map(|docked| docked.window.clone()),
            bar_apart,
            selected,
            labels,
        }
    }

    /// `r` as bullet time's overview draws it, smaller about the screen's centre.
    fn to_overview(&self, r: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        let (centre, zoom) = (self.centre, self.zoom);
        Rectangle::new(
            (
                centre.0 + (r.loc.x - centre.0) * zoom,
                centre.1 + (r.loc.y - centre.1) * zoom,
            )
                .into(),
            (r.size.w * zoom, r.size.h * zoom).into(),
        )
    }

    /// Where the tilted overview's reflection starts, the bottom of the workspace frames, and how
    /// far below that it shows.
    fn mirror_line(&self) -> Option<(f32, f32)> {
        self.area.map(|area| {
            let bottom =
                self.centre.1 + (area.loc.y + area.size.h + 10.0 - self.centre.1) * self.zoom;
            (bottom as f32, (area.size.h * self.zoom * 0.35) as f32)
        })
    }

    /// This screen's sizes, for what is drawn off screen.
    pub(super) fn sizes(&self) -> tilt::Sizes {
        tilt::Sizes {
            logical: self.geo.size,
            physical: self.physical,
            scale: self.scale.x,
        }
    }

    /// This screen, for a card drawn at `time`.
    fn frame(&self, time: f64) -> crate::panel::Frame {
        crate::panel::Frame {
            size: self.geo.size,
            scale: self.scale.x,
            time,
        }
    }

    /// How strongly bullet time's own marks show: with the overview, and with the UI.
    fn bullet_fade(&self) -> f32 {
        self.zoomed_out.min(1.0) as f32 * self.ui_alpha
    }

    fn label_for(&self, window: &Window) -> Option<&str> {
        self.labels
            .iter()
            .find(|(target, _)| target.window() == window)
            .map(|(_, label)| label.as_str())
    }
}

/// The windows and everything drawn with them, front to back, kept apart from the rest of the
/// frame until they have all been drawn, since bullet time tilts them as one.
#[derive(Default)]
struct World {
    /// What the overview tilts.
    elements: Vec<OutputElement>,
    /// The overview's upright labels, in front of the tilt.
    labels: Vec<OutputElement>,
    /// A window dragged in bullet time: where its ghost goes, in front of the rest of the world.
    ghost: Option<Rectangle<f64, Logical>>,
    /// How much of the front of `elements` is the docked pane's, bay and flight included:
    /// whatever else is put in front of the windows goes in behind that.
    pane_front: usize,
    /// The docked pane's own elements while the overview is open: upright, just behind the bar.
    pane_layer: Vec<OutputElement>,
    /// The bar and what hangs from it, a pane of their own while they fade (`View::bar_apart`).
    bar_pane: Vec<OutputElement>,
    /// How many distant windows have been shaded so far.
    dimmed: usize,
}

/// One window as this frame draws it.
struct Drawn {
    window: Window,
    /// Where it is drawn on this screen.
    shown: Rectangle<f64, Logical>,
    alpha: f32,
    /// The density it is built at: the overview's, or the screen's own for the docked pane.
    scale: Scale<f64>,
    /// The workspace it belongs to.
    home: Option<usize>,
}

/// Everything on `output`, front to back. `cursor` is `None` when a host desktop draws the
/// pointer (nested).
pub fn output_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    cursor: Option<&mut Cursor>,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    // A dragged gap's tiles follow it once a frame, before anything is drawn from them.
    state.retile_for_drag();
    let Some(geo) = state.space.output_geometry(output) else {
        return elements;
    };
    // This screen's own drawing buffers and wallpaper, put back at the end. Taking them out
    // keeps every use below a plain local, and keeps the screens from sharing a buffer sized for
    // one of them.
    let name = output.name();
    let mut chrome = state.drawn.chromes.remove(&name).unwrap_or_default();
    let mut saver = state
        .savers
        .remove(&name)
        .unwrap_or_else(|| state.new_saver());
    let now = state.clock.frame();
    // While locked, the lock is all there is: nothing below reads a window.
    if state.lock.is_some() {
        let elements = lock_elements(
            state,
            renderer,
            output,
            geo,
            &mut chrome,
            &mut saver,
            cursor,
        );
        chrome.unlock = None;
        state.drawn.chromes.insert(name.clone(), chrome);
        state.savers.insert(name, saver);
        return elements;
    }
    if state
        .unlocking
        .as_ref()
        .is_some_and(|unlocking| unlocking.done(state.wall()))
    {
        state.unlocking = None;
    }
    if state.unlocking.is_none() {
        chrome.unlock = None;
    }
    let v = View::new(state, renderer, &mut chrome, output, geo, now);
    chrome.overview.begin();
    chrome.panes.begin();
    chrome.overview.set_accent(state.bullet_rgb);
    if v.first && state.bullet.is_some() {
        state.drawn.overview_hits.clear();
        state.drawn.frame_hits.clear();
    }
    if let Some(cursor) = cursor.filter(|_| v.ui > 0.5) {
        elements.extend(pointer_elements(state, renderer, &v, cursor));
    }
    let used: Vec<bool> = (0..state.workspaces.count())
        .map(|i| !state.workspaces.get(i).is_empty())
        .collect();

    // Fading between the UI and the wallpaper, everything from here down to the wallpaper is
    // drawn into the glass. The pointer, already added, stays in front.
    let pane_start = elements.len();
    let mut world = World::default();
    let layers_shown = overlay_elements(state, renderer, output, &v, &mut chrome, &mut elements);
    if v.first {
        state.drawn.overview_tilt = v.tilt;
        // Ready before the explorer first opens.
        state.explorer.warm_icons(v.scale.x);
    }
    // Alt+Tab's deck of glass panes, drawn with the windows below; the flat card stands in where
    // panes can't be drawn.
    let deck_shown = v.first
        && state.deck.is_some()
        && !state.clock.reduced_motion
        && chrome.panes.ready(renderer);
    if v.first && !deck_shown {
        state.deck = None;
    }
    if v.first {
        elements.extend(card_elements(
            state, renderer, geo.size, v.scale.x, now, v.wall, deck_shown,
        ));
    }
    elements.extend(popup_elements(state, renderer, &v));
    if v.first && v.zoomed_out > 0.0 && v.ui > 0.0 {
        elements.extend(bullet_border_elements(state, renderer, &v, &mut chrome));
    }
    // The bar hides when a window on screen fills it.
    if v.tiling && v.ui > 0.0 && !state.fullscreen_on(v.active) {
        world.bar_pane = bar_elements(state, renderer, &v, &mut chrome, &used);
        // Fading with a window docked, the bar and what hangs from it are a pane of their own.
        if !v.bar_apart {
            elements.append(&mut world.bar_pane);
        }
    }
    // The deck goes just behind the bar, in front of everything else of the desktop's.
    let behind_bar = elements.len();
    if (v.first || v.rain) && v.zoomed_out > 0.0 && v.ui > 0.0 {
        elements.extend(stream_hint_elements(state, renderer, &v, &mut chrome));
    }

    // The code rain, down the right-hand side.
    if v.rain && v.ui > 0.0 && !state.rain.is_empty() && !state.fullscreen_on(v.active) {
        elements.extend(
            state
                .rain
                .elements(renderer, geo.size, bar::HEIGHT, v.scale.x, now, v.ui_alpha)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    window_world(state, renderer, &v, &mut chrome, &mut world, deck_shown);

    // Alt+Tab's deck, just behind the bar.
    let mut deck_elements: Vec<OutputElement> = Vec::new();
    if deck_shown && let Some(mut deck) = state.deck.take() {
        if deck.visible(v.wall) {
            deck_elements = deck_elements_for(state, &mut chrome, renderer, &mut deck, &v);
        }
        if !deck.done(v.wall) {
            state.deck = Some(deck);
        }
    }
    elements.splice(behind_bar..behind_bar, deck_elements);
    elements.splice(
        behind_bar..behind_bar,
        std::mem::take(&mut world.pane_layer),
    );

    if v.first && v.ui > 0.0 && v.zoomed_out > 0.0 {
        workspace_frame_elements(state, renderer, &v, &mut chrome, &mut world);
    }
    ghost_elements(state, renderer, &v, &mut chrome, &mut world);
    // The upright labels go in front, then the windows and frames: tilted while bullet time is
    // zoomed out, as they are otherwise.
    elements.append(&mut world.labels);
    let tilted = if v.tilting && !world.elements.is_empty() {
        let fade = v.zoomed_out.min(1.0) as f32;
        let look = tilt::Look {
            fog: 0.4 * fade,
            mirror: v.mirror_line().map_or((0.0, 0.0, 0.0), |(y, depth)| {
                (y, depth, 0.2 * fade * v.ui_alpha)
            }),
        };
        chrome.stage.element(
            renderer,
            &world.elements,
            v.sizes(),
            v.dense,
            &v.tilt,
            &look,
        )
    } else {
        None
    };
    match tilted {
        Some(element) => elements.push(OutputElement::Shaded(element)),
        None => elements.append(&mut world.elements),
    }

    // The keyboard-tips prompt, unless the explorer or bullet time is open over it.
    if v.tiling
        && v.ui > 0.0
        && v.zoomed_out <= 0.0
        && !used[v.active]
        && !state.explorer.is_open()
        && !state.sheet.is_open()
    {
        elements.extend(
            chrome
                .tips(
                    renderer,
                    &v,
                    (!state.clock.reduced_motion).then_some(v.wall),
                    &state.key_label(crate::keys::Action::ShortcutSheet),
                )
                .map(OutputElement::Memory),
        );
    }
    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::BACK,
            v.scale,
            v.ui_alpha,
        ));
    }

    let fallen = if v.glass {
        fade_into_glass(
            renderer,
            &v,
            &mut chrome,
            &mut elements,
            world.bar_pane,
            pane_start,
        )
    } else {
        Fallen::default()
    };
    unlock_elements(state, renderer, &v, &mut chrome, &mut elements, pane_start);
    if v.tiling {
        wallpaper_elements(
            state,
            renderer,
            &v,
            &mut chrome,
            &mut saver,
            &mut elements,
            fallen,
        );
    }
    curtain_elements(state, &v, &mut chrome, &mut elements);
    chrome.panes.finish();
    state.drawn.chromes.insert(name.clone(), chrome);
    state.savers.insert(name, saver);
    elements
}

/// The password card, a snip being chosen, and other programs' launchers, pickers and panels, in
/// front of everything of the desktop's. Returns whether other programs' layers are shown at all.
fn overlay_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    output: &Output,
    v: &View,
    chrome: &mut Chrome,
    elements: &mut Vec<OutputElement>,
) -> bool {
    // The password card in front of everything but the pointer, other programs' overlays
    // included, so nothing can be drawn over what it says. The rest of the screen is shaded behind
    // it, the bar and every other card too, and every other screen.
    let ring = state.panel_ring();
    if let Some(prompt) = state.polkit.prompt.as_mut() {
        if v.first {
            let card = prompt.element(renderer, v.frame(v.wall), ring);
            elements.extend(card.map(OutputElement::Memory));
        }
        let shade = prompt.shade(&v.name, v.geo.size, v.scale.x, v.wall);
        elements.push(OutputElement::Solid(shade));
    }
    // A snip being chosen is in front of everything but the pointer; fragments of one just taken
    // fly in front of that.
    elements.extend(state.snip_flight_elements(renderer, output));
    let before_snip = elements.len();
    let mut snip = state.snip_elements(renderer, output, &mut chrome.overview, v.scale.x);
    elements.append(&mut snip);
    let snipping = elements.len() > before_snip;
    // Other programs' launchers, pickers and panels in front of everything of the desktop's, and
    // their docks and wallpapers behind the windows, all inside the pane that fades. Not in
    // bullet time, which is the desktop's own.
    let layers_shown = v.tiling && v.ui > 0.0 && v.zoomed_out <= 0.0 && !snipping;
    if layers_shown {
        elements.extend(state.layer_elements(
            renderer,
            output,
            &crate::layers::FRONT,
            v.scale,
            v.ui_alpha,
        ));
    }
    layers_shown
}

/// The pointer, when it is on this screen: the app's own cursor, or a named one from the theme.
/// Slipstream's own shows over a gap or during a drag, never over an overlay or the lock.
fn pointer_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    cursor: &mut Cursor,
) -> Vec<OutputElement> {
    let pointer = state.pointer_location();
    if !v.geo.to_f64().contains(pointer) {
        return Vec::new();
    }
    let pos = pointer - v.geo.loc.to_f64();
    if let CursorImageStatus::Surface(surface) = &state.cursor_status {
        if !surface.alive() {
            state.cursor_status = CursorImageStatus::default_named();
        }
    }
    let status = match state.cursor_override {
        // Over the password card only the plain cursor: one a client drew could cover it.
        _ if state.polkit.is_up() => CursorImageStatus::default_named(),
        Some(icon) if state.drag.is_some() || !state.pointer_blocked() => {
            CursorImageStatus::Named(icon)
        }
        _ => state.cursor_status.clone(),
    };
    match &status {
        CursorImageStatus::Hidden => Vec::new(),
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|data| data.lock().unwrap().hotspot)
                    .unwrap_or_else(|| (0, 0).into())
            });
            let location: Point<i32, Physical> =
                (pos - hotspot.to_f64()).to_physical(v.scale).to_i32_round();
            render_elements_from_surface_tree(
                renderer,
                surface,
                location,
                v.scale,
                1.0,
                Kind::Cursor,
            )
        }
        CursorImageStatus::Named(icon) => {
            let time = state.start_time.elapsed();
            themed_pointer(renderer, cursor, icon.name(), pos, v.scale, time)
                .into_iter()
                .collect()
        }
    }
}

/// The notifications popping up at the top right, unless a window fills the screen, the toast,
/// and the volume and brightness display. Where they are is kept for the pointer.
fn popup_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    let mut over_windows = Vec::new();
    let (size, scale) = (v.geo.size, v.scale);
    if v.first && v.ui > 0.0 && !state.fullscreen_on(v.active) {
        let (popups, expired) = state
            .notices
            .popup_elements(renderer, size.w, scale.x, v.now, v.ui_alpha);
        over_windows.extend(popups.iter().map(|popup| popup.geometry(scale)));
        elements.extend(popups.into_iter().map(OutputElement::Memory));
        crate::notify::closed(expired, crate::notify::Reason::Expired);
    }
    if v.first && v.ui > 0.0 {
        let toast = state.toast.element(renderer, size, scale.x, v.now);
        over_windows.extend(toast.iter().map(|toast| toast.geometry(scale)));
        elements.extend(toast.map(OutputElement::Memory));
        // The volume and brightness display sits low on the screen, over a fullscreen window as
        // well: a film is exactly when the volume keys are used.
        let ring = state.panel_ring();
        let osd = state.osd.element(renderer, size, scale.x, ring, v.now);
        over_windows.extend(osd.iter().map(|osd| osd.geometry(scale)));
        elements.extend(osd.map(OutputElement::Memory));
    }
    if v.first {
        state.drawn.chrome_areas = over_windows
            .into_iter()
            .map(|area| {
                let mut area = area.to_f64().to_logical(scale);
                area.loc += v.geo.loc.to_f64();
                area
            })
            .collect();
    }
    elements
}

/// Bullet time's amber border round the whole screen, in front of the bar: the bar is part of
/// the slowed world while it's open, not something outside the frame. And its keys, along the
/// foot of the screen, fading with the overview.
fn bullet_border_elements(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
) -> Vec<OutputElement> {
    let fade = v.bullet_fade();
    let (w, h) = (v.geo.size.w as f64, v.geo.size.h as f64);
    let edge = Rectangle::new((2.0, 2.0).into(), (w - 4.0, h - 4.0).into());
    let border = overview::border(state.bullet_rgb);
    let mut elements: Vec<OutputElement> = chrome
        .overview
        .outline(edge, 0.0, 2, border, v.scale, fade)
        .into_iter()
        .map(OutputElement::Solid)
        .collect();
    if chrome
        .legend
        .as_ref()
        .is_none_or(|legend| legend.scale != v.scale.x)
    {
        chrome.legend = paint_legend(v.scale.x);
    }
    if let Some(legend) = chrome.legend.as_ref() {
        let at = Point::<f64, Logical>::from((
            ((v.geo.size.w - legend.logical.w) / 2) as f64,
            h - 28.0 * 0.8 - legend.logical.h as f64,
        ));
        elements.extend(
            legend
                .element(renderer, at, fade)
                .map(OutputElement::Memory),
        );
    }
    elements
}

/// The bar, with the docked window's slot alive on top of it where the bar kept room for it. On
/// the screen bullet time is drawn on it follows the overview: the workspace being looked at is
/// highlighted, the one it started on is ringed, and the title names what's chosen.
fn bar_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    used: &[bool],
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    let content = bar_content(state, v.active, v.screen_index, v.first, used);
    let docked = content.dock.is_some();
    // The tooltip of the module the pointer has rested on, if it is on this screen's bar.
    let tooltip = state
        .module_hover
        .showing(std::time::Instant::now())
        .filter(|(name, _)| *name == v.name)
        .and_then(|(_, id)| {
            let (_, shown) = content.modules.iter().find(|(shown, _)| shown == id)?;
            Some((*id, shown.tooltip.clone()?))
        });
    let bar = chrome.bar.element(
        renderer,
        content,
        v.geo.size.w,
        v.scale.x,
        v.ui_alpha,
        v.wall,
        state.clock.reduced_motion,
    );
    if let Some((id, tooltip)) = tooltip
        && let Some(area) = chrome.bar.area_of(bar::Target::Module(id))
    {
        elements.extend(
            chrome
                .module_tip
                .element(
                    renderer,
                    &tooltip,
                    area,
                    (v.geo.size.w, bar::HEIGHT),
                    v.scale.x,
                    v.ui_alpha,
                )
                .map(OutputElement::Memory),
        );
    }
    let (ring, reduced_motion, demand) = (
        state.ring_rgb,
        state.clock.reduced_motion,
        state.rain.demand(),
    );
    // The slot opens, lights and flashes in step with its pane's flight (`dock::SlotLook`).
    let mut slot = None;
    if docked
        && let Some((x, width)) = chrome.bar.dock_slot()
        && let Some(window) = state.docked_on_show()
    {
        chrome.slot_at = Some((x, width));
        let keyboard = state.focused_window().as_ref() == Some(&window);
        let look = state
            .docking
            .dock
            .as_ref()
            .map_or(crate::dock::SlotLook::AT_REST, |docked| {
                crate::dock::SlotLook::docked(
                    docked.flight.as_ref(),
                    docked.lit_from,
                    docked.seated,
                    v.now,
                )
            });
        slot = Some((x, width, look, keyboard, false));
    } else if v.rain
        && let Some((x, width)) = chrome.slot_at
        && let Some((_, flight)) = state.docking.dock_leaving.as_ref()
    {
        // The slot of a pane on its way down: its light runs out and it closes behind it.
        slot = Some((
            x,
            width,
            crate::dock::SlotLook::left(flight, v.now),
            false,
            true,
        ));
    }
    if let Some((x, width, look, keyboard, leaving)) = slot {
        let light = state.rain.light();
        let drawn = if leaving {
            state.docking.dock_slot_leaving.as_mut()
        } else {
            state.docking.dock_slot.as_mut()
        };
        if let Some(drawn) = drawn {
            let appearance = crate::slot::Appearance {
                alpha: v.ui_alpha * look.card,
                keyboard,
                ring,
                reduced_motion,
                demand,
                light,
                lit: look.lit,
                flash: look.flash,
            };
            let at = Point::from((x, crate::slot::TOP as f64));
            let element = drawn.element(
                renderer,
                at,
                width.round() as i32,
                v.scale.x,
                v.now,
                appearance,
            );
            elements.extend(element.map(OutputElement::Memory));
        }
    }
    elements.extend(bar.map(OutputElement::Memory));
    elements
}

/// Bullet time's hints over the code rain's streams, on the screen the rain is drawn on, and the
/// vignette, behind the bar.
fn stream_hint_elements(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
) -> Vec<OutputElement> {
    let mut elements = Vec::new();
    let fade = v.bullet_fade();
    if v.rain && state.bullet.is_some() {
        let screen = Rect {
            x: 0,
            y: 0,
            w: v.geo.size.w,
            h: v.geo.size.h,
        };
        for (index, stream) in state.rain.streams.iter().enumerate() {
            let window = &stream.window;
            let column = Rain::column(index, screen, bar::HEIGHT);
            let middle = Point::from((
                column.x as f64 + column.w as f64 / 2.0,
                column.y as f64 + 200.0,
            ));
            if let Some(label) = v.label_for(window) {
                elements.extend(
                    chrome
                        .overview
                        .hint(renderer, label, None, middle, v.scale.x, fade)
                        .map(OutputElement::Memory),
                );
            }
            if matches!(&v.selected, Some(Target::Stream(chosen)) if chosen == window) {
                let around = Rectangle::new(
                    (column.x as f64, column.y as f64 + 4.0).into(),
                    (column.w as f64, column.h as f64 - 8.0).into(),
                );
                let ring = overview::ring(state.bullet_rgb);
                elements.extend(
                    chrome
                        .overview
                        .outline(around, 3.0, 3, ring, v.scale, fade)
                        .into_iter()
                        .map(OutputElement::Solid),
                );
            }
        }
    }
    elements.extend(
        chrome
            .overview
            .vignette(renderer, v.geo.size, v.scale.x, fade)
            .map(OutputElement::Memory),
    );
    elements
}

/// The windows this screen draws, front to back, each with how far along its workspace it is
/// drawn. The space lists the workspace on screen bottom to top. During a switch, the workspace
/// sliding away is drawn too: its windows are unmapped but still alive. A window's place is on
/// the screen its workspace is laid out for; it's drawn where it sits on that screen, as far from
/// this screen's view as its workspace is. The ones on a workspace another screen is showing land
/// off the side of this one, which is where they belong.
fn windows_in_view(
    state: &Slipstream,
    v: &View,
    coming_down: Option<&Window>,
    dock_flying: bool,
) -> Vec<(Window, f64)> {
    let offset = |index: usize| {
        let origin = state
            .screen_rect_for_workspace(index)
            .map_or(v.geo.loc.x, |rect| rect.x);
        shift_for_workspace(index, v.camera, v.step, v.geo.loc.x, origin)
    };
    let mut windows: Vec<(Window, Option<usize>)> = state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            if v.docked.as_ref() == Some(window) || coming_down == Some(window) {
                return None;
            }
            let index = state.workspaces.find(window).unwrap_or(v.active);
            // Outside bullet time another screen's windows are that screen's alone: a wider
            // screen's workspace is wider than the gap between this screen's workspaces, so its
            // edge would reach in here.
            let elsewhere = state
                .screens
                .showing(index)
                .is_some_and(|showing| Some(showing) != v.screen_index);
            (!elsewhere || v.zoomed_out > 0.0).then(|| (window.clone(), Some(index)))
        })
        .collect();
    for index in
        (0..state.workspaces.count()).filter(|index| state.screens.showing(*index).is_none())
    {
        // Zoomed out, neighbouring workspaces come into view.
        if ((index as f64 - v.camera) * v.step).abs() * v.zoom < v.step {
            // Front to back: the floating windows, front-most first, then the tiles.
            let ws = state.workspaces.get(index);
            let floating = ws.floating.windows().into_iter().rev();
            windows.extend(
                floating
                    .chain(ws.layout.windows())
                    .map(|window| (window, Some(index))),
            );
        }
    }
    // Windows just minimised, still pouring into their streams.
    windows.extend(
        state
            .rain
            .arriving(v.now)
            .into_iter()
            .map(|window| (window, None)),
    );
    // Fully faded, only the wallpaper is drawn.
    if v.ui <= 0.0 {
        windows.clear();
    }
    let mut windows: Vec<(Window, f64)> = windows
        .into_iter()
        .map(|(window, index)| (window, index.map_or(v.active_dx, offset)))
        .collect();
    if v.rain
        && v.ui > 0.0
        && !dock_flying
        && let Some(window) = v
            .docked
            .as_ref()
            .filter(|window| state.space.element_geometry(window).is_some())
    {
        windows.insert(0, (window.clone(), 0.0));
    }
    windows
}

/// While the docked pane is out in the deck its bay stays under the bar, for the pane to leave
/// and come home to.
fn dock_bay_elements(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    in_deck: &[Window],
) {
    let Some(pane) = v
        .docked
        .as_ref()
        .filter(|window| in_deck.contains(window))
        .and_then(|_| state.docked_rect())
    else {
        return;
    };
    let at = Point::from(((pane.x - v.geo.loc.x) as f64, (pane.y - v.geo.loc.y) as f64));
    world.elements.extend(
        chrome
            .dock_frame(renderer, (pane.w, pane.h), v.scale.x, at, v.ui_alpha)
            .map(OutputElement::Memory),
    );
    let inside =
        Rectangle::<i32, Logical>::new((at.x as i32, at.y as i32).into(), (pane.w, pane.h).into());
    world.elements.push(OutputElement::Solid(
        chrome.overview.solid(inside, DOCK_BAY, v.scale, v.ui_alpha),
    ));
}

/// The windows, front to back, and what moves among them: the docked pane's bay and flights, and
/// two tiles passing through each other. The deck's windows are drawn in the deck instead.
fn window_world(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    deck_shown: bool,
) {
    // A window on its way down from the bar is drawn as its pane until it lands.
    let coming_down: Option<Window> = state
        .docking
        .dock_leaving
        .as_ref()
        .filter(|(_, flight)| {
            v.rain && v.zoomed_out <= 0.0 && !flight.done(v.now) && chrome.panes.ready(renderer)
        })
        .map(|(window, _)| window.clone());
    // The pane docked to the bar is no workspace's: it is drawn on the screen the streams are on
    // wherever the workspaces slide to, in front of every window. While it flies up from its
    // tile, between sizes or back down, it is one rigid pane, drawn after the rest (`dock.rs`).
    let dock_flying = v.rain
        && v.zoomed_out <= 0.0
        && state
            .docking
            .dock
            .as_ref()
            .and_then(|docked| docked.flight)
            .is_some_and(|flight| !flight.done(v.now))
        && chrome.panes.ready(renderer);
    let windows = windows_in_view(state, v, coming_down.as_ref(), dock_flying);
    state
        .tags
        .retain(|(window, _, at)| v.now - at < TAG_SHOWN && window.alive());
    // Two tiles passing through each other, drawn as panes after the rest (`pane::Pass`).
    let passing = state
        .pass
        .as_ref()
        .filter(|pass| !pass.done(v.now) && v.zoomed_out <= 0.0 && !state.clock.reduced_motion)
        .filter(|pass| state.workspaces.find(&pass.front) == Some(v.active))
        .map(|pass| (pass.front.clone(), pass.back.clone()));
    let passing = passing.filter(|_| chrome.panes.ready(renderer));
    let in_deck: Vec<Window> = if deck_shown {
        state
            .deck
            .as_ref()
            .filter(|deck| deck.visible(v.wall))
            .map(|deck| deck.windows.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Windows drawn here that the space doesn't hold get frame callbacks too (`drawn_off_space`),
    // the deck's among them: they are drawn wherever they live, so they keep drawing.
    for window in windows.iter().map(|(window, _)| window).chain(&in_deck) {
        if state.space.element_geometry(window).is_none()
            && !state.drawn.drawn_off_space.contains(window)
        {
            state.drawn.drawn_off_space.push(window.clone());
        }
    }
    if v.rain && v.ui > 0.0 && v.zoomed_out <= 0.0 {
        dock_bay_elements(state, renderer, v, chrome, world, &in_deck);
    }
    world.pane_front = world.elements.len();
    for (window, dx) in windows {
        let passes = passing
            .as_ref()
            .is_some_and(|(front, back)| *front == window || *back == window);
        if !in_deck.contains(&window) && !passes {
            draw_window(state, renderer, v, chrome, world, window, dx);
        }
    }
    if let Some(passing) = passing.as_ref() {
        passing_elements(state, renderer, v, chrome, world, passing);
    }
    if state.pass.as_ref().is_some_and(|pass| pass.done(v.now)) {
        state.pass = None;
    }
    dock_flight_elements(state, renderer, v, chrome, world);
}

/// One window, `dx` along its workspace, with everything drawn on and around it.
fn draw_window(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    window: Window,
    dx: f64,
) {
    let own = window.geometry();
    // A window is kept inside the tiling area of the screen its own workspace is on, which
    // is not this screen's when it belongs to the one next door.
    let home = state.workspaces.find(&window);
    let docked = v.docked.as_ref() == Some(&window);
    let mark = world.elements.len();
    // The docked pane hangs from the bar, which stays upright in bullet time: it is drawn on the
    // screen itself, at the screen's own density, not in the overview's world.
    let world_scale = if docked { v.scale } else { v.world_scale };
    // An X11 window that places itself is drawn where it put itself, under the bar or not.
    let area = if docked {
        state.screen_area(0)
    } else if crate::state::places_itself(&window) {
        None
    } else {
        home.and_then(|index| state.area_for_workspace(index))
            .or_else(|| state.output_area())
    }
    .map(|area| [area.x as f64, area.y as f64, area.w as f64, area.h as f64]);
    let frame = match state.motion.frame(&window, v.now) {
        Some(frame) => frame,
        // X11 menus and tooltips place themselves and don't animate.
        None => match state.space.element_geometry(&window) {
            Some(geo) => motion::Frame::still([
                geo.loc.x as f64,
                geo.loc.y as f64,
                geo.size.w as f64,
                geo.size.h as f64,
            ]),
            None => return,
        },
    };
    let ([x, y, w, h], fitted) = drawn_rect(state, v, &window, &frame, own, area);
    let s = frame.scale;
    let shown = Rectangle::<f64, Logical>::new(
        (
            x + dx - v.geo.loc.x as f64 + w * (1.0 - s) / 2.0,
            y - v.geo.loc.y as f64 + h * (1.0 - s) / 2.0,
        )
            .into(),
        (w * s, h * s).into(),
    );
    // In bullet time windows are drawn smaller around the screen's centre, each also shrunk a
    // little about its own centre so the gaps between them open up inside their frame.
    let shown = if v.zoomed_out > 0.0 && !docked {
        let k = 0.06 * v.zoomed_out;
        v.to_overview(Rectangle::new(
            (
                shown.loc.x + shown.size.w * k / 2.0,
                shown.loc.y + shown.size.h * k / 2.0,
            )
                .into(),
            (shown.size.w * (1.0 - k), shown.size.h * (1.0 - k)).into(),
        ))
    } else {
        shown
    };
    let drawn = Drawn {
        window,
        shown,
        alpha: frame.alpha as f32 * v.ui_alpha,
        scale: world_scale,
        home,
    };
    if v.zoomed_out > 0.0 && v.first && !docked {
        overview_marks(state, renderer, v, chrome, world, &drawn);
    }
    window_marks(state, renderer, v, chrome, world, &drawn);
    if docked {
        docked_marks(state, renderer, v, chrome, world, &drawn);
    }
    // In bullet time the selection ring takes over from the focus ring.
    if v.focused.as_ref() == Some(&drawn.window) {
        let alpha = drawn.alpha * (1.0 - v.zoomed_out.min(1.0) as f32);
        let colour = overview::ring(state.ring_rgb);
        world.elements.extend(
            chrome
                .ring(shown.to_i32_round(), colour, world_scale, alpha)
                .into_iter()
                .map(OutputElement::Solid),
        );
    }
    let window = drawn.window;
    let location = (shown.loc - own.loc.to_f64())
        .to_physical(world_scale)
        .to_i32_round();
    // The window's own see-through-ness is its content's alone: its ring and shadow stay.
    let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        AsRenderElements::<GlesRenderer>::render_elements(
            &window,
            renderer,
            location,
            world_scale,
            drawn.alpha * crate::state::own_opacity(&window),
        );
    // A handle on what it was just drawn from, so it can fade out if it closes.
    if let Some(picture) = crate::ghost::Picture::of(&window, &renderer.context_id()) {
        match state
            .drawn
            .pictures
            .iter_mut()
            .find(|(drawn, _)| *drawn == window)
        {
            Some((_, kept)) => *kept = picture,
            None => state.drawn.pictures.push((window.clone(), picture)),
        }
    }
    if frame.waiting && !surfaces.is_empty() {
        state.motion.shown(&window, v.now);
    }
    // Only a window at rest on the screen it is on takes the pointer where it is drawn.
    if fitted && s == 1.0 && v.zoomed_out <= 0.0 && dx == 0.0 {
        let global = Rectangle::new(shown.loc + v.geo.loc.to_f64(), shown.size);
        match state.drawn.fitted.iter_mut().find(|(w, _)| *w == window) {
            Some((_, drawn)) => *drawn = global,
            None => state.drawn.fitted.push((window.clone(), global)),
        }
    } else if !fitted {
        state.drawn.fitted.retain(|(w, _)| *w != window);
    }
    if (frame.moving || fitted || s != 1.0 || v.zoom != 1.0) && own.size.w > 0 && own.size.h > 0 {
        let pivot = shown.loc.to_physical(world_scale).to_i32_round();
        let stretch = Scale::from((
            shown.size.w / own.size.w as f64,
            shown.size.h / own.size.h as f64,
        ));
        world.elements.extend(surfaces.into_iter().map(|surface| {
            OutputElement::Rescaled(RescaleRenderElement::from_element(surface, pivot, stretch))
        }));
    } else {
        world
            .elements
            .extend(surfaces.into_iter().map(OutputElement::Surface));
    }
    // A floating window casts a soft shadow on what's beneath it.
    if !docked && home.is_some() && state.workspaces.is_floating(&window) {
        let zoom = if v.zoomed_out > 0.0 { v.zoom } else { 1.0 };
        world.elements.extend(
            chrome
                .window_shadow(renderer, shown, (world_scale.x, zoom), drawn.alpha)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    // And round it, the bar's material carried on down its sides and under its foot, with the
    // shadow the whole piece casts on the desktop.
    if docked {
        let size = (shown.size.w.round() as i32, shown.size.h.round() as i32);
        world.elements.extend(
            chrome
                .dock_frame(renderer, size, v.scale.x, shown.loc, drawn.alpha)
                .map(OutputElement::Memory),
        );
        // With the overview open, everything of the pane's goes in front of it; fading, it goes
        // with the bar it hangs from.
        if v.zoomed_out > 0.0 {
            world.pane_layer.extend(world.elements.drain(mark..));
        } else if v.bar_apart {
            world.bar_pane.extend(world.elements.drain(mark..));
        }
        // The docked window is drawn first, so all of the world so far is its pane.
        world.pane_front = world.elements.len();
    }
}

/// Where `window` is drawn, in the space's coordinates, and whether that is at other than its
/// own size while at rest. At rest a window is drawn at its own size. While its tile moves it's
/// stretched to the tile's animated size, since the client redraws at the new size only a frame
/// or two in.
fn drawn_rect(
    state: &Slipstream,
    v: &View,
    window: &Window,
    frame: &motion::Frame,
    own: Rectangle<i32, Logical>,
    area: Option<[f64; 4]>,
) -> ([f64; 4], bool) {
    let tile = frame.rect;
    if frame.moving || awaiting_size(window) {
        // A client that hasn't yet answered the size it was last sent is still catching up (a
        // slow app, a gap being dragged, a hidden workspace coming back): it's stretched to its
        // tile, as while moving, rather than drawn at its old size with a band of the tile empty
        // or spilling over a neighbour.
        return (tile, false);
    }
    let at_rest = [tile[0], tile[1], own.size.w as f64, own.size.h as f64];
    match area {
        // Zoomed out, a window bigger than its tile (a client with a minimum size, or a
        // fullscreen one) shrinks to fit, so it stays inside its workspace's frame.
        Some(area) if v.zoomed_out > 0.0 => {
            let fit = fit_within(at_rest, tile, area);
            let t = v.zoomed_out.min(1.0);
            (
                std::array::from_fn(|i| at_rest[i] + (fit[i] - at_rest[i]) * t),
                false,
            )
        }
        // A client that won't go as small as its tile — an app with a minimum width wider than
        // the tiling could give it, or one squeezed by the code rain taking width from the right
        // — would otherwise overlap its neighbour or be drawn under the rain. It shrinks into the
        // tile instead, so it and its focus ring stay inside. A fullscreen window is not in the
        // area at all.
        Some(area) if !state.is_fullscreen(window) => {
            let scaled = fit_within(at_rest, tile, area);
            // Rounding at a fractional scale leaves a window a pixel or two over an edge.
            // Rescaling the whole window for that is far more visible than the overhang — it
            // flickers every time the client redraws at a size of its own — so only a real
            // refusal to shrink counts.
            if at_rest[2] - scaled[2] > SLACK || at_rest[3] - scaled[3] > SLACK {
                tracing::debug!(
                    own = ?at_rest,
                    ?area,
                    drawn = ?scaled,
                    "a window too big for its tile is drawn scaled into it"
                );
                (scaled, true)
            } else {
                (at_rest, false)
            }
        }
        _ => (at_rest, false),
    }
}

/// A window's marks in bullet time's overview: its letter, its close button under the pointer,
/// the ghost a drag leaves, the selection ring, and the shade over workspaces not being looked
/// at.
fn overview_marks(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    drawn: &Drawn,
) {
    let (window, shown, alpha) = (&drawn.window, drawn.shown, drawn.alpha);
    if state.bullet.is_some() {
        if let Some(label) = v.label_for(window) {
            let name = state.window_name(window);
            // A floating window's letter sits just above it, so a dialog centred over its
            // parent doesn't cover the parent's.
            let down = if state.workspaces.is_floating(window) {
                -30.0
            } else {
                shown.size.h / 2.0
            };
            let flat = (shown.loc.x + shown.size.w / 2.0, shown.loc.y + down);
            let middle = Point::from(v.tilt.project(flat).unwrap_or(flat));
            world.labels.extend(
                chrome
                    .overview
                    .hint(renderer, label, Some(&name), middle, v.scale.x, alpha)
                    .map(OutputElement::Memory),
            );
        }
        let pointer = state.bullet.as_ref().map(|mode| &mode.pointer);
        // The window under the pointer shows its close button, in the flat overview so it tilts
        // with its window.
        let hovered = pointer
            .filter(|pointer| pointer.drag.is_none() && pointer.hover.as_ref() == Some(window))
            .map(|pointer| pointer.on_close);
        if let Some(hot) = hovered {
            let button = bullet::close_button(shown);
            world.elements.extend(
                chrome
                    .overview
                    .close_button(renderer, button, hot, drawn.scale.x, alpha)
                    .map(OutputElement::Memory),
            );
        }
        // A window being dragged leaves a ghost of itself, half its size, at the pointer.
        if let Some(drag) = pointer
            .and_then(|pointer| pointer.drag.as_ref())
            .filter(|drag| drag.window == *window)
        {
            let (w, h) = (shown.size.w / 2.0, shown.size.h / 2.0);
            world.ghost = Some(Rectangle::new(
                (drag.at.x - w / 2.0, drag.at.y - h / 2.0).into(),
                (w, h).into(),
            ));
        }
        if matches!(&v.selected, Some(Target::Window(chosen)) if chosen == window) {
            // Heavier than the viewed frame's 3 px, with a faint second line further out for a
            // glow, so the two never read as one double line.
            let ring = overview::ring(state.bullet_rgb);
            let outline = &mut chrome.overview;
            let near = outline.outline(shown, 6.0, 6, ring, drawn.scale, alpha);
            let glow = outline.outline(shown, 14.0 + 6.0, 1, ring, drawn.scale, alpha * 0.25);
            world
                .elements
                .extend(near.into_iter().chain(glow).map(OutputElement::Solid));
        }
        state.drawn.overview_hits.push((window.clone(), shown));
    }
    // Workspaces not being looked at are dimmed.
    if drawn.home != Some(v.viewed) {
        let dim = alpha * 0.55 * v.zoomed_out.min(1.0) as f32;
        let shade = chrome
            .overview
            .solid(shown.to_i32_round(), overview::SHADE, drawn.scale, dim);
        world.elements.push(OutputElement::Solid(shade));
    }
}

/// Gravity's tag and the shade over a distant window, and the outline of the tile a window
/// dragged with Super held would swap with.
fn window_marks(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    drawn: &Drawn,
) {
    let (window, shown, alpha) = (&drawn.window, drawn.shown, drawn.alpha);
    // Gravity: the rung's tag in front, then the distant shade.
    if let Some((_, rung, at)) = state.tags.iter().find(|(w, ..)| w == window).cloned() {
        world.elements.extend(
            chrome
                .tag(
                    renderer,
                    rung,
                    shown,
                    v.now - at,
                    drawn.scale,
                    state.clock.reduced_motion,
                )
                .map(OutputElement::Memory),
        );
    }
    let rung = drawn
        .home
        .map(|index| state.workspaces.get(index).gravity.rung(window));
    if rung == Some(Rung::Distant) {
        let shade = chrome.dim(world.dimmed, shown.to_i32_round(), drawn.scale, alpha);
        world.elements.push(OutputElement::Solid(shade));
        world.dimmed += 1;
    }
    if state.swap_target() == Some(window) {
        let colour = overview::ring(state.ring_rgb);
        world.elements.extend(
            chrome
                .overview
                .outline(shown, 0.0, 2, colour, drawn.scale, alpha * 0.6)
                .into_iter()
                .map(OutputElement::Solid),
        );
    }
}

/// The docked pane's marks: in bullet time its letter and, when chosen, the ring, as any window
/// has; and always the fine light edge of its glass, whoever has the keyboard, with the seam
/// where its frame meets the bar lit for a moment after it lands.
fn docked_marks(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    drawn: &Drawn,
) {
    let (window, shown, alpha) = (&drawn.window, drawn.shown, drawn.alpha);
    if v.zoomed_out > 0.0 && state.bullet.is_some() {
        let fade = alpha * v.zoomed_out.min(1.0) as f32;
        if let Some(label) = v.label_for(window) {
            let name = state.window_name(window);
            let middle = Point::from((
                shown.loc.x + shown.size.w / 2.0,
                shown.loc.y + shown.size.h / 2.0,
            ));
            world.elements.extend(
                chrome
                    .overview
                    .hint(renderer, label, Some(&name), middle, v.scale.x, fade)
                    .map(OutputElement::Memory),
            );
        }
        if matches!(&v.selected, Some(Target::Window(chosen)) if chosen == window) {
            let ring = overview::ring(state.bullet_rgb);
            world.elements.extend(
                chrome
                    .overview
                    .outline(shown, 0.0, 3, ring, drawn.scale, fade)
                    .into_iter()
                    .map(OutputElement::Solid),
            );
        }
    }
    let seated = state.docking.dock.as_ref().and_then(|docked| docked.seated);
    let seam = crate::dock::seam(seated, v.now);
    if seam > 0.0 {
        // White along the bar's edge, and round the glass as it locks in.
        let edge = crate::dock::FRAME;
        let line = Rectangle::<i32, Logical>::new(
            (
                shown.loc.x.round() as i32 - edge,
                shown.loc.y.round() as i32 - edge,
            )
                .into(),
            (shown.size.w.round() as i32 + 2 * edge, 2).into(),
        );
        world
            .elements
            .push(OutputElement::Solid(chrome.overview.solid(
                line,
                WHITE,
                drawn.scale,
                alpha * seam,
            )));
        world.elements.extend(
            chrome
                .overview
                .outline(shown, 0.0, 2, WHITE, drawn.scale, alpha * seam * 0.8)
                .into_iter()
                .map(OutputElement::Solid),
        );
    }
    world.elements.extend(
        chrome
            .overview
            .outline(shown, 0.0, 1, WHITE, drawn.scale, alpha * DOCK_RIM)
            .into_iter()
            .map(OutputElement::Solid),
    );
}

/// Two tiles passing through each other, in front of the other windows.
fn passing_elements(
    state: &Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
    (front_window, back_window): &(Window, Window),
) {
    let Some(pass) = state.pass.as_ref() else {
        return;
    };
    let screen = (v.geo.size.w as f64, v.geo.size.h as f64);
    let camera = crate::pane::Camera::for_screen(screen.0, screen.1);
    let origin = (v.geo.loc.x as f64, v.geo.loc.y as f64);
    let (front, back, deep) = pass.poses(v.now, screen.0, origin);
    let (axis, forward) = pass.leading();
    let end = if forward { 1.0 } else { 0.0 };
    let edge_at = match axis {
        crate::pane::Axis::Across => {
            crate::pane::project(&front, &camera, (end, 0.5)).map(|(x, _)| x)
        }
        crate::pane::Axis::Down => {
            crate::pane::project(&front, &camera, (0.5, end)).map(|(_, y)| y)
        }
    };
    let deep = deep as f32;
    let front_look = crate::pane::Look {
        alpha: v.ui_alpha,
        ring: Some((state.ring_rgb, RING_ALPHA)),
        edge: Some((axis, forward, 0.95 * deep)),
        ..Default::default()
    };
    let back_look = crate::pane::Look {
        alpha: v.ui_alpha,
        shade: 0.25 * deep,
        band: edge_at.map(|at| (axis, at, 150.0, 0.34 * deep)),
        ..Default::default()
    };
    let screen = v.sizes();
    let pieces: Vec<OutputElement> = [
        chrome
            .panes
            .element(renderer, front_window, &front, &camera, screen, &front_look),
        chrome
            .panes
            .element(renderer, back_window, &back, &camera, screen, &back_look),
    ]
    .into_iter()
    .flatten()
    .map(OutputElement::Shaded)
    .collect();
    world
        .elements
        .splice(world.pane_front..world.pane_front, pieces);
}

/// The docked pane in flight, and one just undocked on its way down, in front of every window.
/// Flights that have landed are let go.
fn dock_flight_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
) {
    if v.rain && v.ui > 0.0 && v.zoomed_out <= 0.0 && chrome.panes.ready(renderer) {
        let arriving = state
            .docking
            .dock
            .as_ref()
            .and_then(|docked| Some((docked.window.clone(), docked.flight?)));
        let flights: Vec<(Window, crate::dock::Flight)> = state
            .docking
            .dock_leaving
            .clone()
            .into_iter()
            .chain(arriving)
            .filter(|(_, flight)| !flight.done(v.now))
            .collect();
        let origin = (v.geo.loc.x as f64, v.geo.loc.y as f64);
        let local = crate::pane::Camera::for_screen(v.geo.size.w as f64, v.geo.size.h as f64);
        // The stops are in the space's coordinates, so the eye is too, and the pane is brought
        // on to this screen afterwards.
        let eye = crate::pane::Camera {
            x: local.x + origin.0,
            y: local.y + origin.1,
            ..local
        };
        let mut pieces = Vec::new();
        for (window, flight) in flights {
            let (mut pose, lit) = flight.at(v.now, &eye);
            pose.x -= origin.0;
            pose.y -= origin.1;
            // The focus ring if the keyboard is in it, and the glass's own light edge if not.
            let ring = if v.focused.as_ref() == Some(&window) {
                (state.ring_rgb, RING_ALPHA)
            } else {
                ([1.0; 3], DOCK_RIM)
            };
            let (band_at, band_half, band_light) = lit.band;
            let look = crate::pane::Look {
                alpha: v.ui_alpha,
                ring: Some(ring),
                shade: 0.0,
                edge: Some(lit.edge),
                reach: lit.reach,
                band: Some((
                    crate::pane::Axis::Down,
                    band_at - origin.1,
                    band_half,
                    band_light,
                )),
            };
            pieces.extend(
                chrome
                    .panes
                    .element(renderer, &window, &pose, &local, v.sizes(), &look)
                    .map(OutputElement::Shaded),
            );
            // Behind the pane, the bay in the bar's material that it is heading for or has
            // left: the frame, and dark glass where the window will be.
            if let Some((bay, shown)) = flight.bay(v.now).filter(|(_, shown)| *shown > 0.0) {
                let at = Point::from((bay[0] - origin.0, bay[1] - origin.1));
                let size = (bay[2].round() as i32, bay[3].round() as i32);
                let alpha = v.ui_alpha * shown;
                pieces.extend(
                    chrome
                        .dock_frame(renderer, size, v.scale.x, at, alpha)
                        .map(OutputElement::Memory),
                );
                let inside = Rectangle::<i32, Logical>::new(
                    (at.x.round() as i32, at.y.round() as i32).into(),
                    size.into(),
                );
                pieces.push(OutputElement::Solid(
                    chrome.overview.solid(inside, DOCK_BAY, v.scale, alpha),
                ));
            }
        }
        world.pane_front += pieces.len();
        world.elements.splice(0..0, pieces);
    }
    if let Some(docked) = state.docking.dock.as_mut()
        && docked.flight.is_some_and(|flight| flight.done(v.now))
    {
        docked.flight = None;
    }
    if state
        .docking
        .dock_leaving
        .as_ref()
        .is_some_and(|(_, flight)| flight.done(v.now))
    {
        state.docking.dock_leaving = None;
        state.docking.dock_slot_leaving = None;
    }
}

/// Bullet time: every workspace in view gets its frame, number and caption, behind its windows.
fn workspace_frame_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
) {
    let Some(area) = v.area else {
        return;
    };
    let fade = v.bullet_fade();
    let home = state.bullet.as_ref().map(|mode| mode.home);
    for index in 0..state.workspaces.count() {
        let dx = (index as f64 - v.camera) * v.step;
        if dx.abs() * v.zoom > v.geo.size.w as f64 {
            continue;
        }
        let frame = v.to_overview(Rectangle::new(
            (area.loc.x + dx - 10.0, area.loc.y - 10.0).into(),
            (area.size.w + 20.0, area.size.h + 20.0).into(),
        ));
        let viewed = index == v.viewed;
        // The frame a dragged window would land in lights up.
        let drop_target = state
            .bullet
            .as_ref()
            .and_then(|mode| mode.pointer.drag.as_ref())
            .is_some_and(|drag| drag.over == Some(index));
        let (colour, thickness) = if viewed || drop_target {
            (overview::frame_viewed(state.bullet_rgb), 3)
        } else {
            (overview::FRAME, 2)
        };
        world.elements.extend(
            chrome
                .overview
                .outline(frame, 0.0, thickness, colour, v.world_scale, fade)
                .into_iter()
                .map(OutputElement::Solid),
        );
        let count = state.workspaces.get(index).len();
        let mut caption = bullet::window_count(count);
        if home == Some(index) {
            caption.push_str(" · you started here");
        }
        let at = Point::from((frame.loc.x + 4.0, frame.loc.y - 46.0));
        let label = state.workspaces.label(index);
        world.elements.extend(
            chrome
                .overview
                .workspace_label(
                    renderer,
                    (&label, &caption),
                    viewed,
                    at,
                    v.world_scale.x,
                    fade,
                )
                .map(OutputElement::Memory),
        );
        if count == 0 {
            // High in the frame, clear of the wallpaper's logo in the middle of the screen.
            let middle = Point::from((
                frame.loc.x + frame.size.w / 2.0,
                frame.loc.y + frame.size.h * 0.15,
            ));
            world.elements.extend(
                chrome
                    .overview
                    .empty_note(renderer, index + 1, middle, v.world_scale.x, fade)
                    .map(OutputElement::Memory),
            );
        }
        if state.bullet.is_some() {
            state.drawn.frame_hits.push((index, frame));
        }
    }
}

/// Closed windows fade in front of the windows retiling into their space, and behind the docked
/// pane as they were. Not in bullet time's overview, which draws windows where no ghost was
/// captured. Then the ghost a window dragged in bullet time leaves at the pointer, in front.
fn ghost_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    world: &mut World,
) {
    let now = v.now;
    if v.zoomed_out <= 0.0 {
        let context = renderer.context_id();
        let screen = v.geo.to_f64();
        let falling =
            state.ghosts.iter().any(|ghost| ghost.rain.is_some()) && chrome.fx.ready(renderer);
        let mut pieces: Vec<OutputElement> = Vec::new();
        for ghost in state
            .ghosts
            .iter_mut()
            .filter(|ghost| ghost.overlaps(screen))
        {
            // Falling away in the chosen effects, when closing into the rain is on.
            if let Some(effects) = ghost.rain
                && falling
            {
                let still: Vec<OutputElement> = ghost
                    .still_elements(&context, v.scale)
                    .into_iter()
                    .map(OutputElement::Texture)
                    .collect();
                let rect = Rectangle::new(ghost.rect.loc - screen.loc, ghost.rect.size);
                let fall = crate::fx::Fall {
                    effects,
                    rect,
                    bottom: screen.size.h,
                    scale: v.scale.x,
                    t: now - ghost.started,
                };
                if let Some(element) = chrome.fx.fall(renderer, &mut ghost.texture, still, &fall) {
                    pieces.push(OutputElement::Shaded(element));
                }
                continue;
            }
            pieces.extend(
                ghost
                    .elements(&context, screen.loc, v.scale, now)
                    .into_iter()
                    .map(OutputElement::Texture),
            );
        }
        world
            .elements
            .splice(world.pane_front..world.pane_front, pieces);
    }
    state.ghosts.retain(|ghost| !ghost.done(now));
    state.drawn.pictures.retain(|(window, _)| window.alive());
    if let Some(ghost) = world.ghost {
        let alpha = 0.8 * v.bullet_fade();
        let ring = overview::ring(state.bullet_rgb);
        let edges = chrome
            .overview
            .outline(ghost, 0.0, 2, ring, v.world_scale, alpha);
        world
            .elements
            .splice(0..0, edges.into_iter().map(OutputElement::Solid));
    }
}

/// Where the UI's turning pages sit in the frame, for the wallpaper to lie over them behind the
/// glass.
#[derive(Default, Clone, Copy)]
struct Fallen {
    /// The windows' pane.
    windows: Option<usize>,
    /// The bar's own pane, when it has one.
    bar: Option<usize>,
}

/// Fading between the UI and the wallpaper: everything from `pane_start` on is drawn into a pane
/// of glass that passes through the wallpaper, with the bar's own pane in front of it when it has
/// one.
fn fade_into_glass(
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    elements: &mut Vec<OutputElement>,
    mut bar_pane: Vec<OutputElement>,
    pane_start: usize,
) -> Fallen {
    let mut fallen = Fallen::default();
    let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
    let screen = v.sizes();
    let Some(element) = chrome
        .glass
        .element(renderer, &pane, screen, 1.0 - v.ui, 0.0)
    else {
        // Drawn plainly this once; a glass that's broken stays off from now on.
        elements.append(&mut bar_pane);
        elements.extend(pane);
        return fallen;
    };
    // The bar's pane first: it is in front.
    if !bar_pane.is_empty() {
        let gone = crate::glass::nearer(1.0 - v.ui);
        match chrome
            .glass_bar
            .element(renderer, &bar_pane, screen, gone, crate::glass::NEARER)
        {
            Some(bar) => {
                fallen.bar = Some(elements.len());
                elements.push(OutputElement::Shaded(bar));
            }
            None => elements.append(&mut bar_pane),
        }
    }
    fallen.windows = Some(elements.len());
    elements.push(OutputElement::Shaded(element));
    fallen
}

/// Coming back from the lock, the desktop is drawn whole into a texture that grows into place
/// and brightens, while the lock's card and veil fade in front of it.
fn unlock_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    elements: &mut Vec<OutputElement>,
    pane_start: usize,
) {
    if v.tiling
        && !v.glass
        && let Some((zoom, opacity)) = state.unlocking.as_ref().map(|u| u.desktop(v.wall))
    {
        let pane: Vec<OutputElement> = elements.drain(pane_start..).collect();
        match unlock_zoom(renderer, chrome, &pane, v, zoom, opacity) {
            Some(element) => elements.push(OutputElement::Texture(element)),
            None => elements.extend(pane),
        }
    }
    let Some(unlocking) = state.unlocking.as_mut() else {
        return;
    };
    let (alpha, _) = unlocking.card_state(v.wall);
    let mut lock: Vec<OutputElement> = Vec::new();
    if v.first {
        lock.extend(
            unlocking
                .elements(renderer, v.geo.size, v.scale.x, v.wall)
                .into_iter()
                .map(OutputElement::Memory),
        );
    }
    if v.tiling && alpha > 0.0 {
        lock.push(veil(chrome, v.geo.size, v.scale, alpha));
    }
    elements.splice(pane_start..pane_start, lock);
}

/// The living wallpaper, behind everything: dim while the UI is up, full once it fades. It holds
/// still under a window that fills the screen. Where the UI's pages have turned behind the glass,
/// the same wallpaper lies over them.
fn wallpaper_elements(
    state: &mut Slipstream,
    renderer: &mut GlesRenderer,
    v: &View,
    chrome: &mut Chrome,
    saver: &mut crate::saver::Saver,
    elements: &mut Vec<OutputElement>,
    fallen: Fallen,
) {
    let (size, scale, ui) = (v.geo.size, v.scale.x, v.ui);
    let paused = state.fullscreen_on(v.active) && ui >= 1.0;
    let glow = 0.45 + 0.55 * (1.0 - ui);
    // Slowed for bullet time, a wallpaper step lasts many frames, so it tweens between them.
    let tween = state.clock.in_bullet_time();
    saver.set_readings(&saver_readings(state, state.workspaces.label(v.active)));
    // A trace for each agent at work, over the wallpaper once the desktop has gone.
    elements.extend(
        state
            .scope
            .element(renderer, &v.name, size, scale, v.now, ui)
            .map(OutputElement::Memory),
    );
    elements.extend(
        saver
            .element(renderer, v.frame(v.now), glow, paused, tween)
            .map(OutputElement::Memory),
    );
    let Some(at) = fallen.windows else {
        return;
    };
    let through = saver.again(renderer, size, glow).and_then(|wallpaper| {
        chrome.glass.through(
            renderer,
            wallpaper,
            v.sizes(),
            (1.0 - ui, 0.0),
            None,
            BACKGROUND,
        )
    });
    if let Some(through) = through {
        elements.insert(at, OutputElement::Shaded(through));
    }
    // The bar's pane goes behind the wallpaper after the windows' has, so where it is behind, the
    // order is the wallpaper, the bar's pane, then the windows'.
    if let Some(bar_at) = fallen.bar {
        let windows = chrome.glass.shown(renderer, v.sizes(), 1.0 - ui, 0.0);
        let gone = crate::glass::nearer(1.0 - ui);
        let through = saver.again(renderer, size, glow).and_then(|wallpaper| {
            chrome.glass_bar.through(
                renderer,
                wallpaper,
                v.sizes(),
                (gone, crate::glass::NEARER),
                windows,
                BACKGROUND,
            )
        });
        if let Some(through) = through {
            elements.insert(bar_at, OutputElement::Shaded(through));
        }
    }
}

/// A screenshot's flash over the whole screen it was taken of, and the blackout on the way out
/// of the session, over everything the screen holds, the bar included.
fn curtain_elements(
    state: &mut Slipstream,
    v: &View,
    chrome: &mut Chrome,
    elements: &mut Vec<OutputElement>,
) {
    if let Some(alpha) = state.flash_alpha(&v.name) {
        chrome.flash.update(v.geo.size, WHITE);
        elements.insert(
            0,
            OutputElement::Solid(SolidColorRenderElement::from_buffer(
                &chrome.flash,
                (0, 0),
                v.scale,
                alpha,
                Kind::Unspecified,
            )),
        );
    }
    if !v.tiling {
        return;
    }
    let alpha = state
        .exit
        .as_ref()
        .map_or(0.0, |exit| exit.blackout(v.wall));
    if alpha > 0.0 {
        chrome.blackout.update(v.geo.size, BLACK);
        elements.insert(
            0,
            OutputElement::Solid(SolidColorRenderElement::from_buffer(
                &chrome.blackout,
                (0, 0),
                v.scale,
                alpha,
                Kind::Unspecified,
            )),
        );
    }
}
