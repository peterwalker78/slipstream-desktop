mod capture;

use std::{cell::RefCell, rc::Rc, time::Duration};

use smithay::{
    backend::{
        input::{ButtonState, InputEvent, KeyState, KeyboardKeyEvent, PointerButtonEvent},
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    },
    utils::{Clock, Monotonic, Transform},
    wayland::presentation::Refresh,
};

use crate::{Slipstream, render};

/// How long a recording waits after each frame for clients to draw their next picture.
const RECORDING_GRACE: Duration = Duration::from_millis(15);

pub fn init_winit(
    event_loop: &mut EventLoop<Slipstream>,
    state: &mut Slipstream,
) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, winit) = winit::init()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<Slipstream>(&state.display_handle);
    // SLIPSTREAM_SCALE previews fractional scaling nested, as on a laptop panel.
    let scale = crate::screen::scale_from_env().map(Scale::Fractional);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        scale,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));
    state.screen_connected(&output, 0);

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let mut capture = capture::Capture::new(backend.window());
    // Shared with the idle callback that asks for each next frame.
    let shared = Rc::new(RefCell::new(backend));
    state.keys_let_go = !capture.held();

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            let mut held = shared.borrow_mut();
            let backend = &mut *held;
            capture.dispatch();
            match &event {
                WinitEvent::Focus(true) => capture.take(backend.window()),
                WinitEvent::Focus(false) => capture.let_go(backend.window()),
                WinitEvent::Input(InputEvent::Keyboard { event }) => capture.key(
                    backend.window(),
                    event.key_code().raw(),
                    event.state() == KeyState::Pressed,
                ),
                WinitEvent::Input(InputEvent::PointerButton { event })
                    if event.state() == ButtonState::Pressed =>
                {
                    capture.take(backend.window());
                }
                _ => (),
            }
            state.keys_let_go = !capture.held();
            match event {
                WinitEvent::Resized { size, .. } => {
                    output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                    // A resized window is a resized output: tiles follow it, and so does the
                    // size anything sharing the screen is capturing at.
                    state.retile();
                    state.capture_size_changed(&output);
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    state.run_due_debug_steps();
                    // Off, the window keeps its last picture and nothing is drawn, as a real
                    // screen would show nothing; the debug steps and clients keep being served.
                    if state.screens_off {
                        let _ = state.display_handle.flush_clients();
                        backend.window().request_redraw();
                        return;
                    }
                    let screenshots = state.take_screenshots();

                    let size = backend.window_size();
                    let view = state.zoom_view(&output);
                    // Only what changed since the back buffer was last drawn is drawn again, as
                    // on a real screen; an unknown age draws everything.
                    let age = backend.buffer_age().unwrap_or(0);

                    let drawn = match backend.bind() {
                        Ok((renderer, mut framebuffer)) => {
                            let elements = render::output_elements(state, renderer, &output, None);
                            damage_tracker
                                .render_output(
                                    renderer,
                                    &mut framebuffer,
                                    age,
                                    &render::zoomed(&elements, view),
                                    render::BACKGROUND,
                                )
                                .map(|result| (result.damage.cloned(), result.states))
                                .map(|(damage, states)| (elements, damage, states))
                                .map_err(|err| format!("{err:?}"))
                        }
                        Err(err) => Err(format!("{err:?}")),
                    };
                    // Nothing new is nothing to swap: the window keeps the picture it has.
                    let submitted = drawn.and_then(|(elements, damage, states)| match damage {
                        Some(damage) => backend
                            .submit(Some(&damage))
                            .map(|()| (elements, true, states))
                            .map_err(|err| format!("{err:?}")),
                        None => Ok((elements, false, states)),
                    });
                    // A frame that fails is skipped, and the next one tried, rather than taking
                    // the session down with it.
                    let (elements, swapped, states) = match submitted {
                        Ok(drawn) => drawn,
                        Err(err) => {
                            tracing::warn!("couldn't draw the window: {err}");
                            backend.window().request_redraw();
                            return;
                        }
                    };
                    let period = output
                        .current_mode()
                        .filter(|mode| mode.refresh > 0)
                        .map(|mode| Duration::from_secs_f64(1_000.0 / mode.refresh as f64));
                    state.update_scanout_outputs(&output, &states);
                    // Nested, the host's swap is as close to the screen as can be known.
                    if swapped {
                        state.presentation_feedback(&output, &states).presented(
                            Clock::<Monotonic>::new().now(),
                            period.map_or(Refresh::Unknown, Refresh::fixed),
                            0,
                            wp_presentation_feedback::Kind::Vsync,
                        );
                    }
                    if state.lock.is_some() {
                        state.lock_frame_shown(&output.name());
                    }

                    // After the swap: reading pixels back first leaves the window's surface
                    // unbound, and the swap then fails.
                    let scale = output.current_scale().fractional_scale();
                    state.serve_captures(backend.renderer(), &output, &elements, size, scale);
                    state.serve_screenshots(backend.renderer(), &output, &elements, size, scale);
                    // Debug frames are what the screen showed, zoom and all.
                    let shown = render::zoomed(&elements, view);
                    for path in screenshots {
                        match render::save_png(backend.renderer(), &shown, size, scale, &path) {
                            Ok(()) => tracing::info!(path, "saved screenshot"),
                            Err(err) => tracing::warn!(path, "screenshot failed: {err}"),
                        }
                    }

                    state.send_frames(&output);

                    state.space.refresh();
                    state.popups.cleanup();
                    let _ = state.display_handle.flush_clients();
                    // The next frame is asked for once this pass of the event loop is over, so
                    // what clients sent while this one was drawn is read before it. Asked for at
                    // once, a slow frame let several more go by between reads, and windows only
                    // moved on every few frames. A recording also waits a moment for clients to
                    // answer this frame, so every window has drawn its next picture in time.
                    // With nothing new swapped, there is no swap to pace the next frame, so it
                    // is asked for a frame later.
                    let next = shared.clone();
                    let ask = move || next.borrow().window().request_redraw();
                    if !swapped {
                        let _ = state.loop_handle.insert_source(
                            Timer::from_duration(period.unwrap_or(Duration::from_millis(16))),
                            move |_, _, _| {
                                ask();
                                TimeoutAction::Drop
                            },
                        );
                    } else if state.clock.stepped() {
                        let _ = state.loop_handle.insert_source(
                            Timer::from_duration(RECORDING_GRACE),
                            move |_, _, _| {
                                ask();
                                TimeoutAction::Drop
                            },
                        );
                    } else {
                        state.loop_handle.insert_idle(move |_| ask());
                    }
                }
                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }
                _ => (),
            };
        })?;

    Ok(())
}
