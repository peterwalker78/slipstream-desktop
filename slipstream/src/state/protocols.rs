//! The Wayland protocols' global state, which Smithay's handler traits hand back to it, made
//! along with the globals that need no state of their own.

use super::*;

pub struct Protocols {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub viewporter_state: ViewporterState,
    /// Portal dialogs parented to the app that opened them.
    pub xdg_foreign_state: XdgForeignState,
    /// Other programs' panels, launchers and overlays (`layers.rs`).
    pub layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState,
    /// Apps asking for one of their windows to be brought forward, with proof of the click or key
    /// that asked for it (`handlers/mod.rs`).
    pub xdg_activation_state: XdgActivationState,
    pub seat_state: SeatState<Slipstream>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    /// Clipboard control for `wl-copy`, `wl-paste` and clipboard managers, in both protocols'
    /// forms, filtered down to the clients `clipboard::may_control` allows.
    pub ext_data_control: ext_data_control::DataControlState,
    pub wlr_data_control: wlr_data_control::DataControlState,
    pub xwayland_shell_state: XWaylandShellState,
    /// Set up by the hardware backend once it knows the GPU's buffer formats.
    pub dmabuf: Option<(DmabufState, DmabufGlobal)>,
    /// Explicit sync (`linux-drm-syncobj-v1`), offered by the hardware backend when the GPU can
    /// wake us on a timeline point.
    pub syncobj: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// Screen sharing: the capture protocols the portal talks to, and the list of windows it
    /// picks from. Every global is filtered down to the clients `capture::may_capture` allows, so
    /// no app but the portal can see them at all.
    pub image_capture_source: ImageCaptureSourceState,
    pub output_capture_source: OutputCaptureSourceState,
    pub toplevel_capture_source: ToplevelCaptureSourceState,
    pub image_copy_capture: ImageCopyCaptureState,
    pub foreign_toplevel_list: ForeignToplevelListState,
    pub idle_inhibit_state: IdleInhibitManagerState,
    /// `ext-idle-notify`: what tells other programs the seat has gone quiet.
    pub idle_notifier_state: IdleNotifierState<Slipstream>,
    /// Virtual machines and remote desktops asking for every key (`takeback.rs`).
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
}

impl Protocols {
    pub fn new(dh: &DisplayHandle, event_loop: &LoopHandle<'static, Slipstream>) -> Self {
        let compositor_state = CompositorState::new::<Slipstream>(dh);
        let xdg_shell_state = XdgShellState::new::<Slipstream>(dh);
        let xdg_decoration_state = XdgDecorationState::new::<Slipstream>(dh);
        let shm_state = ShmState::new::<Slipstream>(dh, vec![]);

        let output_manager_state = OutputManagerState::new_with_xdg_output::<Slipstream>(dh);
        // Sharp text at 1.25× for apps that support it (Qt 6, GTK 4, Firefox).
        let fractional_scale_state = FractionalScaleManagerState::new::<Slipstream>(dh);
        let viewporter_state = ViewporterState::new::<Slipstream>(dh);
        // Apps name the cursor they want from the theme (GTK 4, Qt 6, Chromium) instead of
        // drawing one of their own at the wrong size.
        CursorShapeManagerState::new::<Slipstream>(dh);
        // When each frame reached the screen, so video players and games pace themselves.
        PresentationState::new::<Slipstream>(dh, Clock::<Monotonic>::new().id() as u32);
        // One-colour buffers, which toolkits use for backgrounds and fades.
        SinglePixelBufferState::new::<Slipstream>(dh);
        let xdg_foreign_state = XdgForeignState::new::<Slipstream>(dh);
        // Typing other scripts through IBus or fcitx5, and on-screen keyboards.
        crate::ime::init(dh);
        let layer_shell_state = crate::layers::state(dh);
        let xdg_activation_state = XdgActivationState::new::<Slipstream>(dh);

        // Clipboard, drag-and-drop, and middle-click paste.
        let data_device_state = DataDeviceState::new::<Slipstream>(dh);
        let primary_selection_state = PrimarySelectionState::new::<Slipstream>(dh);
        // Both selections for clipboard tools and managers, without a window of their own, and
        // only for unsandboxed clients.
        let clipboard_globals = clipboard::state(dh, &primary_selection_state);
        let xwayland_shell_state = XWaylandShellState::new::<Slipstream>(dh);

        let seat_state = SeatState::new();

        // Screen sharing: the portal's own globals, offered to the portal alone.
        let capture_globals = capture::state(dh);

        // `wlr-gamma-control`: wlsunset and the like drive a screen's ramps themselves, and the
        // night light stands aside for whichever screens they have taken.
        crate::gamma::state(dh);
        // `wlr-screencopy`: grim, wf-recorder and the screenshot scripts people already have.
        // Offered to every client, and refused unless the program has been allowed (`screencopy.rs`).
        crate::screencopy::state(dh);
        // Video players ask for the screen to stay awake, which keeps the UI from fading.
        let idle_inhibit_state = IdleInhibitManagerState::new::<Slipstream>(dh);
        // `ext-idle-notify`: swayidle and anything else that waits for the seat to go quiet.
        // Slipstream does its own fading and locking on its own timers; this only tells other
        // programs what the seat is doing, and honours the same inhibitors we do.
        let idle_notifier_state = IdleNotifierState::<Slipstream>::new(dh, event_loop.clone());

        // Games: relative motion for mouse look, and the pointer locked or confined to their
        // window. Touchpad swipes and pinches reach apps too (zooming a page or a picture).
        RelativePointerManagerState::new::<Slipstream>(dh);
        PointerConstraintsState::new::<Slipstream>(dh);
        PointerGesturesState::new::<Slipstream>(dh);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Slipstream>(dh);

        Self {
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            output_manager_state,
            fractional_scale_state,
            viewporter_state,
            xdg_foreign_state,
            layer_shell_state,
            xdg_activation_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            ext_data_control: clipboard_globals.ext,
            wlr_data_control: clipboard_globals.wlr,
            xwayland_shell_state,
            dmabuf: None,
            syncobj: None,
            image_capture_source: capture_globals.source,
            output_capture_source: capture_globals.output_source,
            toplevel_capture_source: capture_globals.toplevel_source,
            image_copy_capture: capture_globals.copy,
            foreign_toplevel_list: capture_globals.toplevel_list,
            idle_inhibit_state,
            idle_notifier_state,
            keyboard_shortcuts_inhibit_state,
        }
    }
}
