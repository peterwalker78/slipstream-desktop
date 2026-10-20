//! Gravity: weighing windows, and Super+T's strip of arrangements.

use super::*;

impl Slipstream {
    /// The focused window moves one rung heavier or lighter along
    /// gravity's ladder of layouts.
    pub fn weigh(&mut self, heavier: bool) {
        if let Some(window) = self.focused_window() {
            self.weigh_window(&window, heavier);
        }
    }

    pub fn weigh_window(&mut self, window: &Window, heavier: bool) {
        let Some(index) = self.workspaces.find(window) else {
            return;
        };
        let history = self.focus_history.clone();
        let ws = self.workspaces.get_mut(index);
        let mut others: Vec<Window> = ws
            .layout
            .windows()
            .into_iter()
            .filter(|other| other != window)
            .collect();
        others.sort_by_key(|other| recency(&history, other));
        let step = ws.gravity.step(window, heavier, &others);
        if matches!(step, Step::Moved(_)) {
            // Gravity takes over the sizes, so a maximise ends rather than coming back after it.
            ws.maximised = None;
        }
        self.report_step(window, step);
        self.retile();
    }

    /// Super+T. A quick press turns gravity on, at the arrangement last kept, or back to tiling;
    /// with Super held the arrangements appear side by side, and each further T moves along
    /// them (`arrange.rs`).
    pub fn toggle_gravity(&mut self) {
        if self.arrange.is_some() {
            self.step_arrangement(true);
            return;
        }
        let Some(window) = self.focused_window() else {
            return;
        };
        let Some(index) = self.workspaces.find(&window) else {
            return;
        };
        if self.workspaces.is_floating(&window) {
            return;
        }
        let ws = self.workspaces.get_mut(index);
        // One window looks the same in every arrangement.
        if ws.layout.windows().len() < 2 && !ws.gravity.is_on() {
            return;
        }
        let to = if ws.gravity.is_on() {
            Rung::Tiling
        } else {
            self.arrangement
        };
        let before = (ws.gravity.clone(), ws.maximised.clone());
        self.arrange = Some(crate::arrange::Arrange::start(
            to,
            window.clone(),
            index,
            before,
            self.wall(),
            self.clock.reduced_motion,
        ));
        self.arrange_to(to);
    }

    /// The next arrangement along the strip, or the one before, put in force at once.
    pub fn step_arrangement(&mut self, forward: bool) {
        if let Some(rung) = self.arrange.as_mut().map(|arrange| arrange.step(forward)) {
            self.arrange_to(rung);
        }
    }

    /// A tile of the strip clicked: that arrangement, kept.
    pub fn pick_arrangement(&mut self, index: usize) {
        if let Some(rung) = self
            .arrange
            .as_mut()
            .and_then(|arrange| arrange.select(index))
        {
            self.arrange_to(rung);
            self.finish_arrangement();
        }
    }

    pub(super) fn arrange_to(&mut self, rung: Rung) {
        let Some((window, index)) = self
            .arrange
            .as_ref()
            .map(|arrange| (arrange.window.clone(), arrange.workspace))
        else {
            return;
        };
        if !window.alive() || self.workspaces.find(&window) != Some(index) {
            self.arrange = None;
            return;
        }
        let ws = self.workspaces.get_mut(index);
        ws.gravity.arrange(&window, rung);
        if rung != Rung::Tiling {
            // Gravity takes over the sizes, so a maximise ends rather than coming back after it.
            ws.maximised = None;
        }
        self.tag(window, rung);
        self.retile();
    }

    /// Super let go: the arrangement on the strip stays, and a quick Super+T turns gravity on at
    /// it next time.
    pub fn finish_arrangement(&mut self) {
        let Some(arrange) = self.arrange.take() else {
            return;
        };
        let rung = self
            .workspaces
            .get_mut(arrange.workspace)
            .gravity
            .rung(&arrange.window);
        if matches!(
            rung,
            Rung::Grid | Rung::Centre | Rung::Wide | Rung::Spotlight
        ) {
            self.arrangement = rung;
        }
    }

    /// Esc with Super held: the workspace goes back to how it was before Super+T.
    pub fn cancel_arrangement(&mut self) {
        let Some(arrange) = self.arrange.take() else {
            return;
        };
        let (gravity, maximised) = arrange.before;
        let ws = self.workspaces.get_mut(arrange.workspace);
        ws.gravity = gravity;
        ws.maximised = maximised;
        let rung = ws.gravity.rung(&arrange.window);
        if arrange.window.alive() {
            self.tag(arrange.window, rung);
        }
        self.retile();
    }

    /// The strip's element, once Super has been held long enough, for a screen `screen` big.
    pub fn arrange_element<R>(
        &mut self,
        renderer: &mut R,
        screen: smithay::utils::Size<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Option<smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>>
    where
        R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let now = self.wall();
        let mut arrange = self.arrange.take()?;
        let element = if arrange.visible(now) {
            let pictures = self.arrangement_pictures(&arrange.window, arrange.workspace);
            let ring = self.panel_ring();
            let key = self.key_name(crate::keys::Action::ToggleGravity);
            let frame = crate::panel::Frame {
                size: screen,
                scale,
                time: now,
            };
            arrange.element(renderer, frame, ring, pictures, key)
        } else {
            None
        };
        self.arrange = Some(arrange);
        element
    }

    /// Each arrangement on the strip as it would place this workspace's windows, `window` marked.
    pub(super) fn arrangement_pictures(
        &mut self,
        window: &Window,
        index: usize,
    ) -> Vec<Vec<crate::arrange::Pane>> {
        let Some(area) = self.area_for_workspace(index) else {
            return Vec::new();
        };
        let ws = self.workspaces.get_mut(index);
        let windows = ws.layout.windows();
        let (outer, inner) = (ws.layout.outer_gap, ws.layout.inner_gap);
        crate::arrange::CHOICES
            .iter()
            .map(|rung| {
                let rects = if *rung == Rung::Tiling {
                    ws.layout.rects_within(area, &min_size)
                } else {
                    let mut gravity = ws.gravity.clone();
                    gravity.arrange(window, *rung);
                    gravity.rects(&windows, area, outer, inner)
                };
                crate::arrange::picture(&rects, area, Some(window))
            })
            .collect()
    }

    /// Tags the window with its new rung. The tag is the whole answer: a step past either end of
    /// the ladder, or with nothing else to arrange, changes nothing and says nothing.
    pub(super) fn report_step(&mut self, window: &Window, step: Step) {
        if let Step::Moved(rung) = step {
            self.tag(window.clone(), rung);
        }
    }

    /// Shows `window`'s new rung on it for a moment, and logs the change.
    pub(super) fn tag(&mut self, window: Window, rung: Rung) {
        tracing::info!(window = logged_app(&window), rung = rung.label(), "gravity");
        let now = self.clock.tick();
        self.tags.retain(|(tagged, ..)| *tagged != window);
        self.tags.push((window, rung, now));
    }
}
