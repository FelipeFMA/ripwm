use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            ImportAll, ImportMem, damage::OutputDamageTracker,
            element::memory::MemoryRenderBufferRenderElement,
            element::solid::SolidColorRenderElement, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::Smallvil;

smithay::backend::renderer::element::render_elements! {
    pub WinitOutputRenderElements<R, E> where R: ImportAll + ImportMem;
    Space=smithay::desktop::space::SpaceRenderElements<R, E>,
    Wallpaper=MemoryRenderBufferRenderElement<R>,
    Border=SolidColorRenderElement,
}

pub fn init_winit(
    event_loop: &EventLoop<Smallvil>,
    state: &mut Smallvil,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit) = winit::init()?;

    let mode = Mode { size: backend.window_size(), refresh: 60_000 };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
    );
    let _global = output.create_global::<Smallvil>(&state.display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop.handle().insert_source(winit, move |event, (), state| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
            state.arrange_windows_tiled();
        }
        WinitEvent::Input(event) => state.process_input_event(event),
        WinitEvent::Redraw => {
            let size = backend.window_size();
            let damage = Rectangle::from_size(size);

            {
                let (renderer, mut framebuffer) = match backend.bind() {
                    Ok(bind) => bind,
                    Err(err) => {
                        tracing::error!("Failed to bind winit backend framebuffer: {err}");
                        return;
                    }
                };

                let mut elements: Vec<
                    WinitOutputRenderElements<
                        GlesRenderer,
                        WaylandSurfaceRenderElement<GlesRenderer>,
                    >,
                > = Vec::new();

                let space_elements = match smithay::desktop::space::space_render_elements(
                    renderer,
                    [&state.space],
                    &output,
                    1.0,
                ) {
                    Ok(elements) => elements,
                    Err(err) => {
                        tracing::error!("Failed to collect render elements: {err}");
                        return;
                    }
                };

                if let Some(output_geo) = state.space.output_geometry(&output) {
                    let border_elements = crate::drawing::tiled_border_elements(
                        output_geo,
                        &state.space,
                        state.active_surface.as_ref(),
                        state.active_border_color,
                        state.inactive_border_color,
                        state.border_width,
                    );
                    elements
                        .extend(border_elements.into_iter().map(WinitOutputRenderElements::Border));
                }

                elements.extend(space_elements.into_iter().map(WinitOutputRenderElements::Space));

                if let Some(mode) = output.current_mode()
                    && let Some(wallpaper_element) =
                        state.wallpaper.render_element(renderer, mode.size)
                {
                    elements.push(WinitOutputRenderElements::Wallpaper(wallpaper_element));
                }

                if let Err(err) = damage_tracker.render_output(
                    renderer,
                    &mut framebuffer,
                    0,
                    &elements,
                    [0.0, 0.0, 0.0, 1.0],
                ) {
                    tracing::error!("Failed to render output: {err}");
                    return;
                }
            }

            if let Err(err) = backend.submit(Some(&[damage])) {
                tracing::error!("Failed to submit frame to winit backend: {err}");
                return;
            }

            state.space.elements().for_each(|window| {
                window.send_frame(
                    &output,
                    state.start_time.elapsed(),
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            });

            state.space.refresh();
            state.popups.cleanup();
            let _ = state.display_handle.flush_clients();

            backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => {
            state.loop_signal.stop();
        }
        WinitEvent::Focus(_) => {}
    })?;

    Ok(())
}
