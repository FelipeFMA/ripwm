use std::{collections::HashMap, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmNode, NodeType,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDevice, EGLDisplay, context::ContextPriority},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportAll, ImportMem,
            element::surface::WaylandSurfaceRenderElement,
            element::{AsRenderElements, memory::MemoryRenderBuffer},
            gles::GlesRenderer,
            multigpu::{GpuManager, MultiRenderer, gbm::GbmGlesBackend},
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{EventLoop, LoopHandle, RegistrationToken},
        drm::control::{ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, IsAlive, Scale, Transform},
    wayland::compositor,
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use crate::{Smallvil, drawing::PointerElement};

smithay::backend::renderer::element::render_elements! {
    pub UdevOutputRenderElements<R, E> where R: ImportAll + ImportMem;
    Space=smithay::desktop::space::SpaceRenderElements<R, E>,
    Pointer=crate::drawing::PointerRenderElement<R>,
}

type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

type DrmOutputType = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    smithay::backend::drm::exporter::gbm::GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

#[derive(Debug, PartialEq, Eq)]
pub struct UdevOutputId {
    pub device_id: DrmNode,
    pub crtc: crtc::Handle,
}

pub struct SurfaceData {
    pub output: Output,
    pub drm_output: DrmOutputType,
}

pub struct BackendData {
    pub drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        smithay::backend::drm::exporter::gbm::GbmFramebufferExporter<DrmDeviceFd>,
        (),
        DrmDeviceFd,
    >,
    pub drm_scanner: DrmScanner,
    pub surfaces: HashMap<crtc::Handle, SurfaceData>,
    pub registration_token: RegistrationToken,
    pub render_node: Option<DrmNode>,
}

pub struct UdevData {
    pub handle: LoopHandle<'static, Smallvil>,
    pub session: LibSeatSession,
    pub primary_gpu: DrmNode,
    pub gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    pub backends: HashMap<DrmNode, BackendData>,
    pub pointer_image: crate::cursor::Cursor,
    pub pointer_images: Vec<(xcursor::parser::Image, MemoryRenderBuffer)>,
    pub pointer_element: PointerElement,
}

fn u32_to_i32_saturating(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

pub fn run_udev() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new()?;
    let display = smithay::reexports::wayland_server::Display::new()?;

    let mut state = Smallvil::new(&mut event_loop, display);

    let (session, notifier) = LibSeatSession::new()?;

    let primary_gpu = if let Ok(var) = std::env::var("SMALLVIL_DRM_DEVICE") {
        DrmNode::from_path(var)?
    } else {
        primary_gpu(session.seat())?
            .and_then(|x| DrmNode::from_path(x).ok()?.node_with_type(NodeType::Render)?.ok())
            .unwrap_or_else(|| {
                all_gpus(session.seat())
                    .unwrap()
                    .into_iter()
                    .find_map(|x| DrmNode::from_path(x).ok())
                    .expect("No GPU found")
            })
    };

    let gpus = GpuManager::new(GbmGlesBackend::with_factory(|display| {
        let context = EGLContext::new_with_priority(display, ContextPriority::High)?;
        let capabilities = unsafe { GlesRenderer::supported_capabilities(&context)? };
        Ok(unsafe { GlesRenderer::with_capabilities(context, capabilities)? })
    }))?;

    state.udev = Some(UdevData {
        handle: event_loop.handle(),
        session,
        primary_gpu,
        gpus,
        backends: HashMap::new(),
        pointer_image: crate::cursor::Cursor::load(),
        pointer_images: Vec::new(),
        pointer_element: PointerElement::default(),
    });

    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.udev.as_ref().unwrap().session.clone().into(),
    );
    libinput_context
        .udev_assign_seat(&state.udev.as_ref().unwrap().session.seat())
        .map_err(|()| "Failed to assign libinput seat")?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop.handle().insert_source(libinput_backend, move |event, (), data| {
        data.process_input_event(event);
    })?;

    event_loop.handle().insert_source(notifier, move |event, (), _data| match event {
        SessionEvent::PauseSession => {
            libinput_context.suspend();
        }
        SessionEvent::ActivateSession => {
            let _ = libinput_context.resume();
        }
    })?;

    let udev_backend = UdevBackend::new(state.udev.as_ref().unwrap().session.seat())?;

    for (device_id, path) in udev_backend.device_list() {
        state.on_udev_event(UdevEvent::Added { device_id, path: path.to_path_buf() });
    }

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, (), data| data.on_udev_event(event))?;

    crate::set_wayland_display(&state.socket_name);
    crate::spawn_client();

    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}

impl Smallvil {
    pub(crate) fn request_redraw_all(&mut self) {
        let Some(udev) = self.udev.as_ref() else {
            return;
        };

        let mut targets = Vec::new();
        for (node, backend) in &udev.backends {
            for crtc in backend.surfaces.keys() {
                targets.push((*node, *crtc));
            }
        }

        for (node, crtc) in targets {
            self.render_surface(node, crtc);
        }
    }

    fn on_udev_event(&mut self, event: UdevEvent) {
        match event {
            UdevEvent::Added { device_id, path } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id)
                    && let Err(err) = self.device_added(node, &path)
                {
                    tracing::error!("Failed to add drm device {device_id}: {err}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.device_changed(node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.device_removed(node);
                }
            }
        }
    }

    fn device_added(
        &mut self,
        node: DrmNode,
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let udev = self.udev.as_mut().unwrap();

        let fd = udev
            .session
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
        let gbm = GbmDevice::new(fd)?;

        let registration_token =
            udev.handle.insert_source(notifier, move |event, metadata, data| match event {
                DrmEvent::VBlank(crtc) => data.frame_finish(node, crtc, metadata),
                DrmEvent::Error(err) => tracing::error!("drm event error: {err}"),
            })?;

        let render_node = {
            let display = unsafe { EGLDisplay::new(gbm.clone())? };
            let egl_device = EGLDevice::device_for_display(&display)?;
            egl_device.try_get_render_node().ok().flatten().unwrap_or(node)
        };

        udev.gpus.as_mut().add_node(render_node, gbm.clone())?;

        let allocator =
            GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        let framebuffer_exporter =
            smithay::backend::drm::exporter::gbm::GbmFramebufferExporter::new(
                gbm,
                Some(render_node),
            );

        let mut renderer = udev.gpus.single_renderer(&render_node)?;
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect::<FormatSet>();

        let drm_output_manager = DrmOutputManager::new(
            drm,
            allocator,
            framebuffer_exporter,
            None,
            [Fourcc::Abgr8888, Fourcc::Argb8888],
            render_formats,
        );

        udev.backends.insert(
            node,
            BackendData {
                drm_output_manager,
                drm_scanner: DrmScanner::new(),
                surfaces: HashMap::new(),
                registration_token,
                render_node: Some(render_node),
            },
        );

        self.device_changed(node);

        Ok(())
    }

    fn connector_connected(
        &mut self,
        node: DrmNode,
        connector: &connector::Info,
        crtc: crtc::Handle,
    ) {
        let Some(udev) = self.udev.as_mut() else {
            return;
        };

        let Some(device) = udev.backends.get_mut(&node) else {
            return;
        };

        let render_node = device.render_node.unwrap_or(udev.primary_gpu);
        let mut renderer = match udev.gpus.single_renderer(&render_node) {
            Ok(renderer) => renderer,
            Err(err) => {
                tracing::warn!("Failed to get renderer: {err}");
                return;
            }
        };

        let output_name =
            format!("{}-{}", connector.interface().as_str(), connector.interface_id());

        let make = String::from("Unknown");
        let model = String::from("Unknown");

        let mode_id = connector
            .modes()
            .iter()
            .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(0);
        let drm_mode = connector.modes()[mode_id];
        let wl_mode = WlMode::from(drm_mode);

        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name,
            PhysicalProperties {
                size: (u32_to_i32_saturating(phys_w), u32_to_i32_saturating(phys_h)).into(),
                subpixel: connector.subpixel().into(),
                make,
                model,
            },
        );
        let _global = output.create_global::<Self>(&self.display_handle);

        let x = self
            .space
            .outputs()
            .filter_map(|o| self.space.output_geometry(o).map(|geo| geo.size.w))
            .sum();
        let position = (x, 0).into();

        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some(position));
        self.space.map_output(&output, position);

        output.user_data().insert_if_missing(|| UdevOutputId { device_id: node, crtc });

        let drm_output = match device
            .drm_output_manager
            .initialize_output::<_, smithay::desktop::space::SpaceRenderElements<
                UdevRenderer<'_>,
                WaylandSurfaceRenderElement<UdevRenderer<'_>>,
            >>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                None,
                &mut renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(drm_output) => drm_output,
            Err(err) => {
                tracing::warn!("Failed to initialize output: {err}");
                return;
            }
        };

        device.surfaces.insert(crtc, SurfaceData { output, drm_output });

        self.render_surface(node, crtc);
    }

    fn connector_disconnected(
        &mut self,
        node: DrmNode,
        _connector: &connector::Info,
        crtc: crtc::Handle,
    ) {
        let Some(udev) = self.udev.as_mut() else {
            return;
        };

        let Some(device) = udev.backends.get_mut(&node) else {
            return;
        };

        if let Some(surface) = device.surfaces.remove(&crtc) {
            self.space.unmap_output(&surface.output);
            self.space.refresh();
        }
    }

    fn device_changed(&mut self, node: DrmNode) {
        let scan_result = {
            let Some(udev) = self.udev.as_mut() else {
                return;
            };
            let Some(device) = udev.backends.get_mut(&node) else {
                return;
            };

            match device.drm_scanner.scan_connectors(device.drm_output_manager.device()) {
                Ok(result) => result,
                Err(err) => {
                    tracing::warn!("Failed to scan connectors: {err}");
                    return;
                }
            }
        };

        for event in scan_result {
            match event {
                DrmScanEvent::Connected { connector, crtc: Some(crtc) } => {
                    self.connector_connected(node, &connector, crtc);
                }
                DrmScanEvent::Disconnected { connector, crtc: Some(crtc) } => {
                    self.connector_disconnected(node, &connector, crtc);
                }
                _ => {}
            }
        }
    }

    fn device_removed(&mut self, node: DrmNode) {
        let Some(udev) = self.udev.as_mut() else {
            return;
        };

        let Some(mut device) = udev.backends.remove(&node) else {
            return;
        };

        let crtcs: Vec<_> = device.surfaces.keys().copied().collect();
        for crtc in crtcs {
            if let Some(surface) = device.surfaces.remove(&crtc) {
                self.space.unmap_output(&surface.output);
            }
        }

        udev.handle.remove(device.registration_token);
    }

    fn frame_finish(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        _metadata: &mut Option<DrmEventMetadata>,
    ) {
        let Some(udev) = self.udev.as_mut() else {
            return;
        };

        let Some(device) = udev.backends.get_mut(&node) else {
            return;
        };

        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            return;
        };

        if let Err(err) = surface.drm_output.frame_submitted() {
            tracing::warn!("Failed to submit frame: {err}");
            return;
        }

        self.render_surface(node, crtc);
    }

    #[allow(clippy::too_many_lines)]
    fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let (output, render_result) = {
            let Some(udev) = self.udev.as_mut() else {
                return;
            };

            let Some(device) = udev.backends.get_mut(&node) else {
                return;
            };

            let Some(surface) = device.surfaces.get_mut(&crtc) else {
                return;
            };

            let Some(output_geometry) = self.space.output_geometry(&surface.output) else {
                return;
            };

            let primary_gpu = udev.primary_gpu;
            let render_node = device.render_node.unwrap_or(primary_gpu);

            let mut renderer = if primary_gpu == render_node {
                match udev.gpus.single_renderer(&render_node) {
                    Ok(renderer) => renderer,
                    Err(err) => {
                        tracing::warn!("Failed to get single renderer: {err}");
                        return;
                    }
                }
            } else {
                let format = surface.drm_output.format();
                match udev.gpus.renderer(&primary_gpu, &render_node, format) {
                    Ok(renderer) => renderer,
                    Err(err) => {
                        tracing::warn!("Failed to get multi renderer: {err}");
                        return;
                    }
                }
            };

            let space_elements = match smithay::desktop::space::space_render_elements(
                &mut renderer,
                [&self.space],
                &surface.output,
                1.0,
            ) {
                Ok(elements) => elements,
                Err(err) => {
                    tracing::warn!("Failed to collect render elements: {err}");
                    return;
                }
            };

            let mut elements: Vec<
                UdevOutputRenderElements<
                    UdevRenderer<'_>,
                    WaylandSurfaceRenderElement<UdevRenderer<'_>>,
                >,
            > = Vec::new();

            let frame = udev.pointer_image.get_image(1, self.start_time.elapsed());
            let pointer_image = udev
                .pointer_images
                .iter()
                .find_map(
                    |(image, texture)| {
                        if image == &frame { Some(texture.clone()) } else { None }
                    },
                )
                .unwrap_or_else(|| {
                    let buffer = MemoryRenderBuffer::from_slice(
                        &frame.pixels_rgba,
                        Fourcc::Argb8888,
                        (u32_to_i32_saturating(frame.width), u32_to_i32_saturating(frame.height)),
                        1,
                        Transform::Normal,
                        None,
                    );
                    udev.pointer_images.push((frame.clone(), buffer.clone()));
                    buffer
                });

            if let smithay::input::pointer::CursorImageStatus::Surface(ref cursor_surface) =
                self.cursor_status
                && !cursor_surface.alive()
            {
                self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
            }

            let hotspot =
                if let smithay::input::pointer::CursorImageStatus::Surface(ref cursor_surface) =
                    self.cursor_status
                {
                    compositor::with_states(cursor_surface, |states| {
                        states
                        .data_map
                        .get::<std::sync::Mutex<smithay::input::pointer::CursorImageAttributes>>()
                        .and_then(|attrs| attrs.lock().ok().map(|guard| guard.hotspot))
                        .unwrap_or_else(|| {
                            (
                                u32_to_i32_saturating(frame.xhot),
                                u32_to_i32_saturating(frame.yhot),
                            )
                                .into()
                        })
                    })
                } else {
                    (u32_to_i32_saturating(frame.xhot), u32_to_i32_saturating(frame.yhot)).into()
                };

            let Some(pointer) = self.seat.get_pointer() else {
                return;
            };

            let pointer_location = pointer.current_location();
            if output_geometry.to_f64().contains(pointer_location) {
                let cursor_pos = pointer_location - output_geometry.loc.to_f64();
                udev.pointer_element.set_buffer(pointer_image);
                udev.pointer_element.set_status(self.cursor_status.clone());
                elements.extend(
                    udev.pointer_element
                        .render_elements(
                            &mut renderer,
                            (cursor_pos - hotspot.to_f64())
                                .to_physical(Scale::from(
                                    surface.output.current_scale().fractional_scale(),
                                ))
                                .to_i32_round(),
                            Scale::from(surface.output.current_scale().fractional_scale()),
                            1.0,
                        )
                        .into_iter()
                        .map(UdevOutputRenderElements::Pointer),
                );
            }

            elements.extend(space_elements.into_iter().map(UdevOutputRenderElements::Space));

            let is_empty = match surface.drm_output.render_frame(
                &mut renderer,
                &elements,
                [0.1, 0.1, 0.1, 1.0],
                smithay::backend::drm::compositor::FrameFlags::DEFAULT,
            ) {
                Ok(result) => result.is_empty,
                Err(err) => {
                    tracing::warn!("Render failed: {err}");
                    return;
                }
            };

            (surface.output.clone(), is_empty)
        };

        if !render_result {
            let Some(udev) = self.udev.as_mut() else {
                return;
            };
            let Some(device) = udev.backends.get_mut(&node) else {
                return;
            };
            let Some(surface) = device.surfaces.get_mut(&crtc) else {
                return;
            };
            if let Err(err) = surface.drm_output.queue_frame(()) {
                tracing::warn!("Failed to queue frame: {err}");
            }
        }

        self.space.elements().for_each(|window| {
            window.send_frame(&output, self.start_time.elapsed(), Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        });

        self.space.refresh();
        self.popups.cleanup();
        let _ = self.display_handle.flush_clients();
    }
}
