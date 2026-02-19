use smithay::{
    backend::renderer::{
        ImportAll, ImportMem, Renderer, Texture,
        element::{
            AsRenderElements, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::WaylandSurfaceRenderElement,
        },
    },
    desktop::{Space, Window},
    input::pointer::CursorImageStatus,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};

pub struct PointerElement {
    buffer: Option<MemoryRenderBuffer>,
    status: CursorImageStatus,
}

impl Default for PointerElement {
    fn default() -> Self {
        Self { buffer: Option::default(), status: CursorImageStatus::default_named() }
    }
}

impl PointerElement {
    pub fn set_status(&mut self, status: CursorImageStatus) {
        self.status = status;
    }

    pub fn set_buffer(&mut self, buffer: MemoryRenderBuffer) {
        self.buffer = Some(buffer);
    }
}

render_elements! {
    pub PointerRenderElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
}

impl<R: Renderer> std::fmt::Debug for PointerRenderElement<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(arg0) => f.debug_tuple("Surface").field(arg0).finish(),
            Self::Memory(arg0) => f.debug_tuple("Memory").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<T: Texture + Clone + Send + 'static, R> AsRenderElements<R> for PointerElement
where
    R: Renderer<TextureId = T> + ImportAll + ImportMem,
{
    type RenderElement = PointerRenderElement<R>;

    fn render_elements<E>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<E>
    where
        E: From<PointerRenderElement<R>>,
    {
        match &self.status {
            CursorImageStatus::Hidden => vec![],
            CursorImageStatus::Named(_) => self.buffer.as_ref().map_or_else(Vec::new, |buffer| {
                vec![
                    PointerRenderElement::<R>::from(
                        MemoryRenderBufferRenderElement::from_buffer(
                            renderer,
                            location.to_f64(),
                            buffer,
                            None,
                            None,
                            None,
                            Kind::Cursor,
                        )
                        .expect("Lost system pointer buffer"),
                    )
                    .into(),
                ]
            }),
            CursorImageStatus::Surface(surface) => {
                let elements: Vec<PointerRenderElement<R>> =
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                        renderer,
                        surface,
                        location,
                        scale,
                        alpha,
                        Kind::Cursor,
                    );
                elements.into_iter().map(E::from).collect()
            }
        }
    }
}

pub fn tiled_border_elements(
    output_geo: Rectangle<i32, Logical>,
    space: &Space<Window>,
    focused_surface: Option<&WlSurface>,
    active_color: [f32; 4],
    inactive_color: [f32; 4],
    border_width: i32,
) -> Vec<SolidColorRenderElement> {
    let mut elements = Vec::new();
    let border = border_width.max(1);

    for window in space.elements() {
        let Some(location) = space.element_location(window) else {
            continue;
        };

        let geometry = window.geometry();
        if geometry.size.w <= 0 || geometry.size.h <= 0 {
            continue;
        }

        let window_rect = Rectangle::new(location, geometry.size);
        if !window_rect.overlaps(output_geo) {
            continue;
        }

        let relative_loc = window_rect.loc - output_geo.loc;
        let width = window_rect.size.w;
        let height = window_rect.size.h;
        let border_thickness = border.min(width).min(height);
        if border_thickness <= 0 {
            continue;
        }

        let color = if let Some(toplevel) = window.toplevel() {
            if focused_surface.is_some_and(|focused| focused == toplevel.wl_surface()) {
                active_color
            } else {
                inactive_color
            }
        } else {
            inactive_color
        };

        let segments = [
            Rectangle::new(relative_loc, (width, border_thickness).into()),
            Rectangle::new(
                (relative_loc.x, relative_loc.y + height - border_thickness).into(),
                (width, border_thickness).into(),
            ),
            Rectangle::new(
                (relative_loc.x, relative_loc.y + border_thickness).into(),
                (border_thickness, height - (border_thickness * 2)).into(),
            ),
            Rectangle::new(
                (relative_loc.x + width - border_thickness, relative_loc.y + border_thickness)
                    .into(),
                (border_thickness, height - (border_thickness * 2)).into(),
            ),
        ];

        for segment in segments {
            if segment.size.w <= 0 || segment.size.h <= 0 {
                continue;
            }

            let buffer = SolidColorBuffer::new(segment.size, color);
            elements.push(SolidColorRenderElement::from_buffer(
                &buffer,
                (segment.loc.x, segment.loc.y),
                Scale::from(1.0),
                1.0,
                Kind::Unspecified,
            ));
        }
    }

    elements
}
