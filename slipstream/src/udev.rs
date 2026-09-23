//! Real hardware: KMS output, libinput and a libseat session, so Slipstream can run as a login
//! session. Adapted from Smithay's anvil (MIT, see `LICENSE-smallvil-MIT.txt`), trimmed to one
//! renderer per GPU.

use std::{collections::HashMap, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            dmabuf::Dmabuf,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDevice, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, ImportMemWl, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, primary_gpu},
    },
    desktop::utils::OutputPresentationFeedback,
    input::keyboard::LedState,
    output::{Mode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::{
            EventLoop, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as _, ModeTypeFlags, connector, crtc},
        input::{self as libinput, DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::backend::GlobalId,
    },
    utils::{Clock, DeviceFd, Monotonic, Transform},
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
    },
};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{DrmScanEvent, DrmScanner},
};

use crate::{Slipstream, cursor::Cursor, render};

/// Tried in order for each screen's buffers: 8 bits per channel, which every driver scans out.
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Abgr8888, Fourcc::Argb8888];

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;

pub struct UdevData {
    pub session: LibSeatSession,
    libinput: Libinput,
    /// The GPU whose renderer imports client buffers: the one the firmware booted on.
    primary: Option<DrmNode>,
    gpus: HashMap<DrmNode, Gpu>,
    cursor: Cursor,
    keyboards: Vec<libinput::Device>,
    /// Screens that are connected but deliberately not lit: the laptop's panel while the lid is
    /// shut and another screen is carrying the desktop. Kept so the panel can be lit again
    /// without waiting for a hotplug that will never come — the connector never went anywhere.
    sleeping: Vec<(DrmNode, connector::Info, crtc::Handle)>,
}

struct Gpu {
    manager: DrmOutputManager<Allocator, Exporter, Option<OutputPresentationFeedback>, DrmDeviceFd>,
    scanner: DrmScanner,
    renderer: GlesRenderer,
    screens: HashMap<crtc::Handle, Screen>,
    token: RegistrationToken,
}

/// One lit connector.
struct Screen {
    output: Output,
    /// Kept so the screen can be lit again after the lid puts it out.
    connector: connector::Info,
    global: GlobalId,
    /// Each queued frame carries who to tell when it reaches the screen.
    drm_output: DrmOutput<Allocator, Exporter, Option<OutputPresentationFeedback>, DrmDeviceFd>,
    /// A frame is queued and the display hasn't shown it yet.
    waiting_for_vblank: bool,
    /// A retry is scheduled because the last frame had nothing new.
    retry_scheduled: bool,
    /// The frame waiting for its vblank was drawn with the lock up.
    lock_frame_queued: bool,
}

/// Takes the seat, opens every GPU and input device, and hooks them into the event loop.
pub fn run(
    event_loop: &mut EventLoop<'static, Slipstream>,
    state: &mut Slipstream,
) -> Result<(), Box<dyn std::error::Error>> {
    let (session, notifier) = LibSeatSession::new().map_err(|err| {
        format!("couldn't take the seat (start Slipstream from the login screen or a text console): {err}")
    })?;
    let seat = session.seat();
    tracing::info!(seat, "session opened");

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| "libinput couldn't open the seat's input devices")?;

    let udev = UdevBackend::new(&seat)?;
    state.udev = Some(UdevData {
        session,
        libinput: libinput.clone(),
        primary: None,
        gpus: HashMap::new(),
        cursor: Cursor::load(),
        keyboards: Vec::new(),
        sleeping: Vec::new(),
    });

    let handle = event_loop.handle();
    handle
        .insert_source(
            LibinputInputBackend::new(libinput),
            |mut event, _, state| {
                match &mut event {
                    InputEvent::DeviceAdded { device } => {
                        configure_input_device(device);
                        if device.has_capability(DeviceCapability::Keyboard) {
                            if let Some(udev) = state.udev.as_mut() {
                                udev.keyboards.push(device.clone());
                            }
                        }
                    }
                    InputEvent::DeviceRemoved { device } => {
                        if let Some(udev) = state.udev.as_mut() {
                            udev.keyboards.retain(|keyboard| keyboard != device);
                        }
                    }
                    _ => {}
                }
                state.process_input_event(event);
            },
        )
        .map_err(|err| err.error)?;

    handle
        .insert_source(notifier, |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused (switched to another virtual terminal)");
                state.end_drag();
                if let Some(udev) = state.udev.as_mut() {
                    udev.libinput.suspend();
                    for gpu in udev.gpus.values_mut() {
                        gpu.manager.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session active again");
                let Some(udev) = state.udev.as_mut() else {
                    return;
                };
                if let Err(err) = udev.libinput.resume() {
                    tracing::error!("libinput didn't resume: {err:?}");
                }
                let mut screens = Vec::new();
                for (node, gpu) in udev.gpus.iter_mut() {
                    // Another program may have changed the modes while we were away: reset them.
                    if let Err(err) = gpu.manager.lock().activate(true) {
                        tracing::error!(%node, "couldn't reactivate the GPU: {err:?}");
                    }
                    for (crtc, screen) in gpu.screens.iter_mut() {
                        screen.waiting_for_vblank = false;
                        screen.lock_frame_queued = false;
                        screens.push((*node, *crtc));
                    }
                }
                for (node, crtc) in screens {
                    state
                        .loop_handle
                        .insert_idle(move |state| state.render_screen(node, crtc));
                }
                state.set_night_light(state.night.current);
            }
        })
        .map_err(|err| err.error)?;

    // The boot GPU first, so its renderer is the one that imports client buffers.
    let primary = primary_gpu(&seat)
        .ok()
        .flatten()
        .and_then(|path| DrmNode::from_path(path).ok());
    let mut devices: Vec<_> = udev
        .device_list()
        .map(|(id, path)| (id, path.to_path_buf()))
        .collect();
    devices.sort_by_key(|(id, _)| primary.is_none_or(|p| p.dev_id() != *id));
    for (id, path) in devices {
        match DrmNode::from_dev_id(id) {
            Ok(node) => {
                if let Err(err) = state.add_gpu(node, &path) {
                    tracing::warn!(?path, "skipping GPU: {err}");
                }
            }
            Err(err) => tracing::warn!(?path, "skipping GPU: {err}"),
        }
    }
    if state.udev.as_ref().is_none_or(|udev| udev.gpus.is_empty()) {
        return Err("no GPU could drive a screen".into());
    }

    handle
        .insert_source(udev, |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    if let Err(err) = state.add_gpu(node, &path) {
                        tracing::warn!(?path, "skipping GPU: {err}");
                    }
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    state.gpu_changed(node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    state.remove_gpu(node);
                }
            }
        })
        .map_err(|err| err.error)?;

    Ok(())
}

impl Slipstream {
    fn add_gpu(&mut self, node: DrmNode, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let udev = self.udev.as_mut().ok_or("no session")?;
        if udev.gpus.contains_key(&node) {
            return Ok(());
        }
        let fd = udev.session.open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
        let gbm = GbmDevice::new(fd)?;

        let display = unsafe { EGLDisplay::new(gbm.clone())? };
        let egl_device = EGLDevice::device_for_display(&display)?;
        if egl_device.is_software() {
            return Err("only software rendering is available".into());
        }
        let render_node = egl_device
            .try_get_render_node()
            .ok()
            .flatten()
            .unwrap_or(node);
        let renderer = unsafe { GlesRenderer::new(EGLContext::new(&display)?)? };

        let render_formats: Vec<_> = renderer
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect();
        let manager = DrmOutputManager::new(
            drm,
            GbmAllocator::new(
                gbm.clone(),
                GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
            ),
            GbmFramebufferExporter::new(gbm.clone(), render_node.into()),
            Some(gbm),
            COLOR_FORMATS,
            render_formats,
        );

        let token = self
            .loop_handle
            .insert_source(
                notifier,
                move |event, metadata: &mut Option<DrmEventMetadata>, state: &mut Slipstream| {
                    match event {
                        DrmEvent::VBlank(crtc) => state.frame_shown(node, crtc, metadata.as_ref()),
                        DrmEvent::Error(err) => tracing::error!(%node, "DRM error: {err:?}"),
                    }
                },
            )
            .map_err(|err| err.error)?;

        if udev.primary.is_none() {
            udev.primary = Some(node);
            self.shm_state.update_formats(renderer.shm_formats());
            let feedback =
                DmabufFeedbackBuilder::new(render_node.dev_id(), renderer.dmabuf_formats())
                    .build()?;
            let mut dmabuf_state = DmabufState::new();
            let global = dmabuf_state
                .create_global_with_default_feedback::<Slipstream>(&self.display_handle, &feedback);
            self.dmabuf = Some((dmabuf_state, global));
            tracing::info!(%render_node, "primary GPU");
        }

        udev.gpus.insert(
            node,
            Gpu {
                manager,
                scanner: DrmScanner::new(),
                renderer,
                screens: HashMap::new(),
                token,
            },
        );
        self.gpu_changed(node);
        Ok(())
    }

    /// Lights newly plugged screens and drops unplugged ones.
    fn gpu_changed(&mut self, node: DrmNode) {
        let Some(gpu) = self.udev.as_mut().and_then(|udev| udev.gpus.get_mut(&node)) else {
            return;
        };
        let events: Vec<DrmScanEvent> = match gpu.scanner.scan_connectors(gpu.manager.device()) {
            Ok(result) => result.into_iter().collect(),
            Err(err) => {
                tracing::warn!(%node, "couldn't scan connectors: {err}");
                return;
            }
        };
        for event in events {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_connected(node, connector, crtc),
                DrmScanEvent::Disconnected {
                    crtc: Some(crtc), ..
                } => self.connector_disconnected(node, crtc),
                _ => {}
            }
        }
        self.retile();
    }

    fn connector_connected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        // The lid is shut and something else is lit: the panel stays out, and is remembered so
        // that opening the lid brings it straight back.
        if self.lid_closed && crate::state::is_internal_panel(&name) && self.lit_elsewhere() {
            tracing::info!(output = name, "the lid is shut, so the panel stays out");
            if let Some(udev) = self.udev.as_mut() {
                udev.sleeping.push((node, connector.clone(), crtc));
            }
            return;
        }
        let Some(gpu) = self.udev.as_mut().and_then(|udev| udev.gpus.get_mut(&node)) else {
            return;
        };
        let modes = connector.modes();
        let Some(drm_mode) = modes
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or(modes.first())
            .copied()
        else {
            tracing::warn!(output = name, "screen reports no modes");
            return;
        };
        let mode = Mode::from(drm_mode);

        let info = display_info::for_connector(gpu.manager.device(), connector.handle());
        let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (width_mm as i32, height_mm as i32).into(),
                subpixel: connector.subpixel().into(),
                make: info
                    .as_ref()
                    .and_then(|i| i.make())
                    .unwrap_or_else(|| "Unknown".into()),
                model: info
                    .as_ref()
                    .and_then(|i| i.model())
                    .unwrap_or_else(|| "Unknown".into()),
                serial_number: info
                    .as_ref()
                    .and_then(|i| i.serial())
                    .unwrap_or_else(|| "Unknown".into()),
            },
        );
        let scale = output_scale(mode.size.w, width_mm);
        // To the right of what's lit for now; `arrange_screens` puts a laptop panel first once it
        // has joined.
        let x = self
            .space
            .outputs()
            .filter_map(|o| self.space.output_geometry(o))
            .map(|geo| geo.loc.x + geo.size.w)
            .max()
            .unwrap_or(0);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            Some((x, 0).into()),
        );

        let drm_output = match gpu
            .manager
            .lock()
            .initialize_output::<_, render::OutputElement>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                None,
                &mut gpu.renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(drm_output) => drm_output,
            Err(err) => {
                tracing::warn!(output = name, "couldn't light the screen: {err:?}");
                return;
            }
        };
        let global = output.create_global::<Slipstream>(&self.display_handle);
        self.space.map_output(&output, (x, 0));
        tracing::info!(
            output = name,
            width = mode.size.w,
            height = mode.size.h,
            refresh_mhz = mode.refresh,
            width_mm,
            scale,
            "screen connected"
        );
        let screen_output = output.clone();
        gpu.screens.insert(
            crtc,
            Screen {
                output,
                connector: connector.clone(),
                global,
                drm_output,
                waiting_for_vblank: false,
                retry_scheduled: false,
                lock_frame_queued: false,
            },
        );
        self.screen_connected(&screen_output, x);
        self.loop_handle
            .insert_idle(move |state| state.render_screen(node, crtc));
        self.set_night_light(self.night.current);
    }

    fn connector_disconnected(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let Some(screen) = self
            .udev
            .as_mut()
            .and_then(|udev| udev.gpus.get_mut(&node))
            .and_then(|gpu| gpu.screens.remove(&crtc))
        else {
            return;
        };
        self.space.unmap_output(&screen.output);
        self.screen_disconnected(&screen.output);
        self.display_handle
            .remove_global::<Slipstream>(screen.global);
        if let Some(udev) = self.udev.as_mut() {
            udev.sleeping
                .retain(|(_, connector, _)| connector.handle() != screen.connector.handle());
        }
        tracing::info!(output = screen.output.name(), "screen disconnected");
    }

    /// Whether any screen other than the laptop's own panel is lit.
    fn lit_elsewhere(&self) -> bool {
        self.space
            .outputs()
            .any(|output| !crate::state::is_internal_panel(&output.name()))
    }

    /// The lid opened or shut (libinput's switch, which only reaches us because `inhibit.rs`
    /// holds logind's; without that the machine would have suspended instead).
    ///
    /// Shut, with a screen still lit elsewhere, the panel goes out and the desktop carries on
    /// there. Shut with nothing else lit, the lid is left alone: putting out the only screen
    /// would leave a session nobody could get back to, and logind's own setting — suspend, most
    /// likely — is the right answer on a laptop on its own.
    pub fn lid_switched(&mut self, closed: bool) {
        if self.lid_closed == closed {
            return;
        }
        self.lid_closed = closed;
        if closed {
            self.end_awake();
            self.put_the_panel_out();
        } else {
            self.light_the_panel();
        }
    }

    /// Puts the laptop's panel out, keeping what it would take to light it again.
    fn put_the_panel_out(&mut self) {
        if !self.lit_elsewhere() {
            tracing::info!("the lid shut on the only screen; leaving it to logind");
            return;
        }
        let Some(udev) = self.udev.as_mut() else {
            // Nested, there is no panel and no DRM: the leftmost screen stands in for one, so
            // the rules above it can be checked headlessly.
            let Some(output) = self
                .screens
                .get(0)
                .map(|screen| screen.output.clone())
                .filter(|_| self.screens.len() > 1)
            else {
                return;
            };
            self.space.unmap_output(&output);
            self.screen_disconnected(&output);
            self.nested_panel_name = Some(output.name());
            self.nested_panel = Some(output);
            return;
        };
        let panel: Vec<(DrmNode, crtc::Handle)> = udev
            .gpus
            .iter()
            .flat_map(|(node, gpu)| {
                gpu.screens
                    .iter()
                    .filter(|(_, screen)| crate::state::is_internal_panel(&screen.output.name()))
                    .map(|(crtc, _)| (*node, *crtc))
            })
            .collect();
        for (node, crtc) in panel {
            let Some(screen) = self
                .udev
                .as_mut()
                .and_then(|udev| udev.gpus.get_mut(&node))
                .and_then(|gpu| gpu.screens.remove(&crtc))
            else {
                continue;
            };
            // Dropping the screen releases its CRTC, which is what actually turns the panel off;
            // the connector itself is still connected, so no hotplug will ever bring it back.
            self.space.unmap_output(&screen.output);
            self.screen_disconnected(&screen.output);
            self.display_handle
                .remove_global::<Slipstream>(screen.global);
            if let Some(udev) = self.udev.as_mut() {
                udev.sleeping.push((node, screen.connector.clone(), crtc));
            }
            tracing::info!(
                output = screen.output.name(),
                "the lid is shut: the panel is out and the desktop is on the other screen"
            );
        }
    }

    /// Lights whatever the lid put out.
    fn light_the_panel(&mut self) {
        if let Some(output) = self.nested_panel.take() {
            self.space.map_output(&output, (0, 0));
            self.screen_connected(&output, 0);
            return;
        }
        let sleeping: Vec<(DrmNode, connector::Info, crtc::Handle)> = self
            .udev
            .as_mut()
            .map(|udev| std::mem::take(&mut udev.sleeping))
            .unwrap_or_default();
        for (node, connector, crtc) in sleeping {
            tracing::info!(
                connector = format!(
                    "{}-{}",
                    connector.interface().as_str(),
                    connector.interface_id()
                ),
                "the lid is open: lighting the panel again"
            );
            self.connector_connected(node, connector, crtc);
        }
    }

    fn remove_gpu(&mut self, node: DrmNode) {
        let Some(udev) = self.udev.as_mut() else {
            return;
        };
        let Some(gpu) = udev.gpus.remove(&node) else {
            return;
        };
        if udev.primary == Some(node) {
            udev.primary = None;
        }
        self.loop_handle.remove(gpu.token);
        // Each of its screens leaves the desktop as an unplugged one does, so the keyboard and
        // the workspaces never point at an output that's gone.
        for screen in gpu.screens.into_values() {
            self.space.unmap_output(&screen.output);
            self.screen_disconnected(&screen.output);
            self.display_handle
                .remove_global::<Slipstream>(screen.global);
        }
        self.retile();
        tracing::info!(%node, "GPU removed");
    }

    /// The display showed the last frame: start the next one.
    fn frame_shown(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        metadata: Option<&DrmEventMetadata>,
    ) {
        let Some(screen) = self
            .udev
            .as_mut()
            .and_then(|udev| udev.gpus.get_mut(&node))
            .and_then(|gpu| gpu.screens.get_mut(&crtc))
        else {
            return;
        };
        match screen.drm_output.frame_submitted() {
            Ok(Some(Some(mut feedback))) => {
                // The kernel's own timestamp when it has one, which is on the monotonic clock
                // presentation-time was announced with.
                let hardware = metadata.and_then(|metadata| match metadata.time {
                    DrmEventTime::Monotonic(time) if !time.is_zero() => Some(time),
                    _ => None,
                });
                let (time, flags) = match hardware {
                    Some(time) => (
                        time.into(),
                        wp_presentation_feedback::Kind::Vsync
                            | wp_presentation_feedback::Kind::HwClock
                            | wp_presentation_feedback::Kind::HwCompletion,
                    ),
                    None => (
                        Clock::<Monotonic>::new().now(),
                        wp_presentation_feedback::Kind::Vsync,
                    ),
                };
                let refresh = screen
                    .output
                    .current_mode()
                    .filter(|mode| mode.refresh > 0)
                    .map(|mode| {
                        Refresh::fixed(Duration::from_secs_f64(1_000.0 / mode.refresh as f64))
                    })
                    .unwrap_or(Refresh::Unknown);
                let sequence = metadata.map_or(0, |metadata| metadata.sequence as u64);
                feedback.presented(time, refresh, sequence, flags);
            }
            Ok(_) => {}
            Err(err) => tracing::warn!("frame wasn't shown: {err:?}"),
        }
        screen.waiting_for_vblank = false;
        if std::mem::take(&mut screen.lock_frame_queued) {
            let name = screen.output.name();
            self.lock_frame_shown(&name);
        }
        self.render_screen(node, crtc);
    }

    pub fn render_screen(&mut self, node: DrmNode, crtc: crtc::Handle) {
        // Out of `self` for the frame, so the renderer and the state borrow side by side.
        let Some(mut udev) = self.udev.take() else {
            return;
        };
        self.render_screen_with(&mut udev, node, crtc);
        self.udev = Some(udev);
    }

    fn render_screen_with(&mut self, udev: &mut UdevData, node: DrmNode, crtc: crtc::Handle) {
        if !udev.session.is_active() {
            return;
        }
        let Some(gpu) = udev.gpus.get_mut(&node) else {
            return;
        };
        let Some(screen) = gpu.screens.get_mut(&crtc) else {
            return;
        };
        if screen.waiting_for_vblank {
            return;
        }
        let output = screen.output.clone();

        self.run_due_debug_steps();
        let locked = self.lock.is_some();
        let screenshots = self.take_screenshots();
        let elements =
            render::output_elements(self, &mut gpu.renderer, &output, Some(&mut udev.cursor));
        match screen.drm_output.render_frame(
            &mut gpu.renderer,
            &elements,
            render::BACKGROUND,
            FrameFlags::DEFAULT,
        ) {
            Ok(frame) => {
                self.update_scanout_outputs(&output, &frame.states);
                if !frame.is_empty {
                    let feedback = self.presentation_feedback(&output, &frame.states);
                    match screen.drm_output.queue_frame(Some(feedback)) {
                        Ok(()) => {
                            screen.waiting_for_vblank = true;
                            screen.lock_frame_queued = locked;
                        }
                        Err(err) => tracing::warn!(
                            output = output.name(),
                            "couldn't queue the frame: {err:?}"
                        ),
                    }
                }
            }
            Err(err) => tracing::warn!(output = output.name(), "rendering failed: {err:?}"),
        }
        if let Some(size) = output.current_mode().map(|mode| mode.size) {
            let scale = output.current_scale().fractional_scale();
            // Anything recording this screen is filled from the very elements it was just drawn
            // from, so what a client gets is what was on the screen.
            self.serve_captures(&mut gpu.renderer, &output, &elements, size, scale);
            self.serve_screenshots(&mut gpu.renderer, &output, &elements, size, scale);
            for path in screenshots {
                match render::save_png(&mut gpu.renderer, &elements, size, scale, &path) {
                    Ok(()) => tracing::info!(path, "saved screenshot"),
                    Err(err) => tracing::warn!(path, "screenshot failed: {err}"),
                }
            }
        }

        self.send_frames(&output);

        // Nothing new to show: look again in a frame's time.
        if !screen.waiting_for_vblank && !screen.retry_scheduled {
            screen.retry_scheduled = true;
            let refresh_mhz = output
                .current_mode()
                .map(|mode| mode.refresh)
                .filter(|refresh| *refresh > 0)
                .unwrap_or(60_000);
            let delay = Duration::from_micros(1_000_000_000 / refresh_mhz as u64);
            let timer =
                self.loop_handle
                    .insert_source(Timer::from_duration(delay), move |_, _, state| {
                        if let Some(screen) = state
                            .udev
                            .as_mut()
                            .and_then(|udev| udev.gpus.get_mut(&node))
                            .and_then(|gpu| gpu.screens.get_mut(&crtc))
                        {
                            screen.retry_scheduled = false;
                        }
                        state.render_screen(node, crtc);
                        TimeoutAction::Drop
                    });
            if timer.is_err() {
                screen.retry_scheduled = false;
            }
        }
    }

    /// The scale a screen gets when nothing has been said about it: worked out from how many
    /// pixels it has across how many millimetres, which is what it was given when it lit up.
    pub fn automatic_scale(&self, output: &Output) -> f64 {
        let width_px = output.current_mode().map(|mode| mode.size.w).unwrap_or(0);
        let width_mm = output.physical_properties().size.w.max(0) as u32;
        output_scale(width_px, width_mm)
    }

    /// Puts a screen into `mode`, on the hardware. Whether it took.
    ///
    /// A modeset while the session is away from this virtual terminal would fail and could leave
    /// the screen dark, so it is refused rather than attempted; the mode is applied again when
    /// the session comes back and the screen is set up afresh.
    pub fn set_screen_mode(&mut self, output: &Output, want: &str) -> bool {
        let Some(udev) = self.udev.as_mut() else {
            return false;
        };
        if !udev.session.is_active() {
            tracing::info!("not changing a screen's mode while the session is away");
            return false;
        }
        let name = output.name();
        for (node, gpu) in udev.gpus.iter_mut() {
            let Some(screen) = gpu
                .screens
                .values_mut()
                .find(|screen| screen.output == *output)
            else {
                continue;
            };
            let Some(drm) = screen.connector.modes().iter().copied().find(|drm| {
                let mode = Mode::from(*drm);
                slipstream_config::screens::ScreenMode {
                    width: mode.size.w,
                    height: mode.size.h,
                    refresh: mode.refresh,
                    preferred: false,
                }
                .matches(want)
            }) else {
                tracing::warn!(screen = name, want, "this screen has no such mode");
                return false;
            };
            let mode = Mode::from(drm);
            if output.current_mode() == Some(mode) {
                return true;
            }
            let before = output.current_mode();
            match screen.drm_output.use_mode(
                drm,
                &mut gpu.renderer,
                &DrmOutputRenderElements::<_, crate::render::OutputElement>::default(),
            ) {
                Ok(()) => {
                    output.change_current_state(Some(mode), None, None, None);
                    tracing::info!(screen = name, mode = want, %node, "changed a screen's mode");
                    return true;
                }
                Err(err) => {
                    // A mode the hardware can't drive alongside the others leaves the screen
                    // where it was rather than dark.
                    tracing::warn!(screen = name, want, "couldn't change the mode: {err}");
                    if let Some(before) = before {
                        output.change_current_state(Some(before), None, None, None);
                    }
                    return false;
                }
            }
        }
        false
    }

    /// Every mode a screen offers, as its connector last reported them. Nested there is only the
    /// window's own size, which is not a mode anybody can change.
    pub fn screen_modes(&self, output: &Output) -> Vec<slipstream_config::screens::ScreenMode> {
        let Some(udev) = self.udev.as_ref() else {
            return Vec::new();
        };
        for gpu in udev.gpus.values() {
            for screen in gpu.screens.values() {
                if screen.output != *output {
                    continue;
                }
                let mut modes: Vec<_> = screen
                    .connector
                    .modes()
                    .iter()
                    .map(|drm| {
                        let mode = Mode::from(*drm);
                        slipstream_config::screens::ScreenMode {
                            width: mode.size.w,
                            height: mode.size.h,
                            refresh: mode.refresh,
                            preferred: drm.mode_type().contains(ModeTypeFlags::PREFERRED),
                        }
                    })
                    .collect();
                // Biggest first, then fastest: the order the Settings app offers them in.
                modes.sort_by_key(|mode| {
                    std::cmp::Reverse((mode.width * mode.height, mode.refresh))
                });
                modes.dedup();
                return modes;
            }
        }
        Vec::new()
    }

    /// How many entries a screen's gamma ramp has, or `None` where it has none to give.
    pub fn gamma_size(&self, output: &Output) -> Option<usize> {
        let udev = self.udev.as_ref()?;
        for gpu in udev.gpus.values() {
            for (crtc, screen) in &gpu.screens {
                if screen.output == *output {
                    let length = gpu.manager.device().get_crtc(*crtc).ok()?.gamma_length() as usize;
                    return (length >= 2).then_some(length);
                }
            }
        }
        None
    }

    /// Writes one screen's ramps outright, for a program that has taken its gamma. Returns
    /// whether the screen took them.
    pub fn set_output_gamma(
        &self,
        output: &Output,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> bool {
        let Some(udev) = self.udev.as_ref() else {
            // Nested there are no CRTCs to write to, so a program's ramps are accepted and
            // ignored rather than reported as a failure it can do nothing about.
            return true;
        };
        for (node, gpu) in &udev.gpus {
            for (crtc, screen) in &gpu.screens {
                if screen.output != *output {
                    continue;
                }
                return match gpu.manager.device().set_gamma(*crtc, red, green, blue) {
                    Ok(()) => true,
                    Err(err) => {
                        tracing::warn!(%node, "couldn't set a program's gamma: {err}");
                        false
                    }
                };
            }
        }
        false
    }

    /// Night light on every screen, or off, through each CRTC's gamma ramp. Screens that light up
    /// later, or come back from another virtual terminal, get it again. A screen whose gamma a
    /// program has taken is left alone until it gives it back.
    pub fn set_night_light(&self, strength: f64) {
        let Some(udev) = self.udev.as_ref() else {
            return;
        };
        let held = self.gamma.held_outputs();
        for (node, gpu) in &udev.gpus {
            let device = gpu.manager.device();
            for (crtc, screen) in &gpu.screens {
                if held.contains(&screen.output.name()) {
                    continue;
                }
                let length = match device.get_crtc(*crtc) {
                    Ok(info) => info.gamma_length() as usize,
                    Err(err) => {
                        tracing::warn!(%node, "couldn't read the screen's gamma size: {err}");
                        continue;
                    }
                };
                if length < 2 {
                    tracing::info!(%node, "this screen has no gamma ramp for night light");
                    continue;
                }
                let [red, green, blue] = crate::nightlight::ramps(length, strength);
                match device.set_gamma(*crtc, &red, &green, &blue) {
                    Ok(()) => tracing::debug!(%node, strength, "night light"),
                    Err(err) => tracing::warn!(%node, strength, "couldn't set night light: {err}"),
                }
            }
        }
    }

    /// Ctrl+Alt+F1–F12.
    pub fn switch_vt(&mut self, vt: i32) {
        // The pointer is never left caught by a drag on the way out.
        self.end_drag();
        if let Some(udev) = self.udev.as_mut() {
            tracing::info!(vt, "switching virtual terminal");
            if let Err(err) = udev.session.change_vt(vt) {
                tracing::error!(vt, "couldn't switch virtual terminal: {err}");
            }
        }
    }

    /// Caps Lock and Num Lock lights.
    pub fn update_leds(&mut self, leds: LedState) {
        if let Some(udev) = self.udev.as_mut() {
            for keyboard in &mut udev.keyboards {
                keyboard.led_update(leds.into());
            }
        }
    }
}

impl DmabufHandler for Slipstream {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self
            .dmabuf
            .as_mut()
            .expect("the dmabuf global only exists alongside its state")
            .0
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let imported = self
            .udev
            .as_mut()
            .and_then(|udev| {
                let primary = udev.primary?;
                udev.gpus.get_mut(&primary)
            })
            .is_some_and(|gpu| gpu.renderer.import_dmabuf(&dmabuf, None).is_ok());
        if imported {
            let _ = notifier.successful::<Slipstream>();
        } else {
            notifier.failed();
        }
    }
}

/// Touchpads tap to click, as on Windows and in Plasma 6.
fn configure_input_device(device: &mut libinput::Device) {
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(true);
    }
}

/// `SLIPSTREAM_SCALE` if it's set, otherwise a guess from pixel density.
fn output_scale(width_px: i32, width_mm: u32) -> f64 {
    std::env::var("SLIPSTREAM_SCALE")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|scale| (0.5..=4.0).contains(scale))
        .unwrap_or_else(|| scale_for_density(width_px, width_mm))
}

/// What a screen gets when nothing in Settings says otherwise: 1.25 on a 14" 1920×1200 laptop
/// panel, 2 on 4K laptops, 1 on ordinary desktop monitors. Screens that don't report a size
/// (projectors, some VMs) get 1.
fn scale_for_density(width_px: i32, width_mm: u32) -> f64 {
    if width_mm == 0 {
        return 1.0;
    }
    let dpi = width_px as f64 * 25.4 / width_mm as f64;
    if dpi >= 220.0 {
        2.0
    } else if dpi >= 180.0 {
        1.5
    } else if dpi >= 140.0 {
        1.25
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::scale_for_density;

    #[test]
    fn scale_follows_pixel_density() {
        assert_eq!(
            scale_for_density(1920, 301),
            1.25,
            "14-inch 1920×1200 laptop"
        );
        assert_eq!(scale_for_density(3440, 800), 1.0, "34-inch ultrawide");
        assert_eq!(scale_for_density(3840, 344), 2.0, "15.6-inch 4K laptop");
        assert_eq!(scale_for_density(1920, 0), 1.0, "unknown size");
    }
}
