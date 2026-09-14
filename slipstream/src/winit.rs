use std::time::Duration;

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    },
    utils::{Clock, Monotonic, Rectangle, Transform},
    wayland::presentation::Refresh,
};

use crate::{Slipstream, render};

pub fn init_winit(
    event_loop: &mut EventLoop<Slipstream>,
    state: &mut Slipstream,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit) = winit::init()?;

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
    let scale = std::env::var("SLIPSTREAM_SCALE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|scale| (0.5..=4.0).contains(scale))
        .map(Scale::Fractional);
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

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
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
                    let screenshots = state.take_screenshots();

                    let size = backend.window_size();
                    let damage = Rectangle::from_size(size);

                    let (elements, states) = {
                        let (renderer, mut framebuffer) = backend.bind().unwrap();
                        let elements = render::output_elements(state, renderer, &output, None);
                        let states = damage_tracker
                            .render_output(
                                renderer,
                                &mut framebuffer,
                                0,
                                &elements,
                                render::BACKGROUND,
                            )
                            .unwrap()
                            .states;
                        (elements, states)
                    };
                    backend.submit(Some(&[damage])).unwrap();
                    // Nested, the host's swap is as close to the screen as can be known.
                    state.presentation_feedback(&output, &states).presented(
                        Clock::<Monotonic>::new().now(),
                        output
                            .current_mode()
                            .map(|mode| {
                                Refresh::fixed(Duration::from_secs_f64(
                                    1_000.0 / mode.refresh as f64,
                                ))
                            })
                            .unwrap_or(Refresh::Unknown),
                        0,
                        wp_presentation_feedback::Kind::Vsync,
                    );
                    if state.lock.is_some() {
                        state.lock_frame_shown(&output.name());
                    }

                    // After the swap: reading pixels back first leaves the window's surface
                    // unbound, and the swap then fails.
                    let scale = output.current_scale().fractional_scale();
                    state.serve_captures(backend.renderer(), &output, &elements, size, scale);
                    state.serve_screenshots(backend.renderer(), &output, &elements, size, scale);
                    for path in screenshots {
                        match render::save_png(backend.renderer(), &elements, size, scale, &path) {
                            Ok(()) => tracing::info!(path, "saved screenshot"),
                            Err(err) => tracing::warn!(path, "screenshot failed: {err}"),
                        }
                    }

                    state.send_frames(&output);

                    state.space.refresh();
                    state.popups.cleanup();
                    let _ = state.display_handle.flush_clients();

                    // Ask for redraw to schedule new frame.
                    backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }
                _ => (),
            };
        })?;

    Ok(())
}
