use std::{ffi::OsString, io::Read, os::unix::net::UnixListener, path::PathBuf, sync::Arc};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::pointer::CursorImageStatus,
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

pub struct Smallvil {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub cursor_status: CursorImageStatus,

    pub seat: Seat<Self>,
    pub wallpaper: crate::config::WallpaperState,
    pub active_surface: Option<WlSurface>,
    pub active_border_color: [f32; 4],
    pub inactive_border_color: [f32; 4],
    pub border_width: i32,
    pub config_path: PathBuf,
    pub ipc_socket_path: PathBuf,
    pub udev: Option<crate::udev::UdevData>,
}

impl Smallvil {
    pub fn new(event_loop: &mut EventLoop<Self>, display: Display<Self>) -> Self {
        let start_time = std::time::Instant::now();
        let config_path = crate::config::config_path();
        let config = crate::config::load_or_create_config();

        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();

        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);

        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");

        let xkb_config = smithay::input::keyboard::XkbConfig {
            layout: &config.keyboard_layout,
            variant: &config.keyboard_variant,
            ..Default::default()
        };

        if let Err(err) = seat.add_keyboard(xkb_config, 200, 25) {
            tracing::error!("Failed to add keyboard to seat: {err}");
        }

        seat.add_pointer();

        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        let loop_signal = event_loop.get_signal();
        let wallpaper = crate::config::WallpaperState::from_config(&config);
        let ipc_socket_path = ipc_socket_path();

        let mut state = Self {
            start_time,
            display_handle: dh,

            space,
            loop_signal,
            socket_name,

            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            cursor_status: CursorImageStatus::default_named(),
            seat,
            wallpaper,
            active_surface: None,
            active_border_color: config.active_border_color,
            inactive_border_color: config.inactive_border_color,
            border_width: 2,
            config_path,
            ipc_socket_path,
            udev: None,
        };

        state.init_ipc_listener(event_loop);

        state
    }

    fn init_ipc_listener(&mut self, event_loop: &EventLoop<Self>) {
        if let Some(parent) = self.ipc_socket_path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("Failed to create IPC directory {}: {err}", parent.display());
            return;
        }

        if self.ipc_socket_path.exists()
            && let Err(err) = std::fs::remove_file(&self.ipc_socket_path)
        {
            tracing::warn!(
                "Failed to remove old IPC socket {}: {err}",
                self.ipc_socket_path.display()
            );
            return;
        }

        let listener = match UnixListener::bind(&self.ipc_socket_path) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!(
                    "Failed to bind IPC socket {}: {err}",
                    self.ipc_socket_path.display()
                );
                return;
            }
        };

        if let Err(err) = listener.set_nonblocking(true) {
            tracing::warn!("Failed to set IPC socket non-blocking: {err}");
            return;
        }

        let result = event_loop.handle().insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, state| {
                loop {
                    let stream = match unsafe { listener.get_mut() }.accept() {
                        Ok((stream, _)) => stream,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(err) => {
                            tracing::warn!("Failed to accept IPC connection: {err}");
                            break;
                        }
                    };

                    state.handle_ipc_client(stream);
                }

                Ok(PostAction::Continue)
            },
        );

        match result {
            Ok(_) => {
                tracing::info!("IPC socket listening at {}", self.ipc_socket_path.display());
            }
            Err(err) => {
                tracing::warn!("Failed to initialize IPC event source: {err}");
            }
        }
    }

    fn handle_ipc_client(&mut self, mut stream: std::os::unix::net::UnixStream) {
        if let Err(err) = stream.set_nonblocking(false) {
            tracing::warn!("Failed to configure IPC stream: {err}");
            return;
        }

        let mut command = String::new();
        if let Err(err) = stream.read_to_string(&mut command) {
            tracing::warn!("Failed to read IPC command: {err}");
            return;
        }

        let command = command.trim();

        if command == "reload" {
            self.reload_config();
            return;
        }

        if let Some(layout_args) = command.strip_prefix("keyboard ") {
            let mut parts = layout_args.splitn(2, ' ');
            let Some(layout) = parts.next().map(str::trim).filter(|part| !part.is_empty()) else {
                tracing::warn!("Invalid keyboard IPC command, missing layout");
                return;
            };
            let variant = parts.next().map(str::trim).unwrap_or("");

            let xkb_config =
                smithay::input::keyboard::XkbConfig { layout, variant, ..Default::default() };

            match self.seat.add_keyboard(xkb_config, 200, 25) {
                Ok(_) => {
                    tracing::info!(
                        "Updated keyboard layout via IPC: layout={layout}, variant={variant}"
                    );
                }
                Err(err) => {
                    tracing::error!("Failed to update keyboard layout via IPC: {err}");
                }
            }

            return;
        }

        tracing::warn!("Unknown IPC command: {command}");
    }

    pub fn reload_config(&mut self) {
        let config = crate::config::load_or_create_config();
        self.wallpaper = crate::config::WallpaperState::from_config(&config);
        self.active_border_color = config.active_border_color;
        self.inactive_border_color = config.inactive_border_color;

        let xkb_config = smithay::input::keyboard::XkbConfig {
            layout: &config.keyboard_layout,
            variant: &config.keyboard_variant,
            ..Default::default()
        };

        if let Err(err) = self.seat.add_keyboard(xkb_config, 200, 25) {
            tracing::error!("Failed to update keyboard layout: {err}");
        }

        self.arrange_windows_tiled();

        self.request_redraw_all();
        tracing::info!("Reloaded configuration from {}", self.config_path.display());
    }

    pub fn arrange_windows_tiled(&mut self) {
        self.space.refresh();

        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };

        let windows: Vec<Window> = self.space.elements().cloned().collect();
        if windows.is_empty() {
            return;
        }

        let mut remaining = output_geo;
        let count = windows.len();

        for (index, window) in windows.into_iter().enumerate() {
            let tile = if index + 1 == count {
                remaining
            } else if remaining.size.w >= remaining.size.h && remaining.size.w > 1 {
                let left_width = (remaining.size.w / 2).max(1);
                let right_width = remaining.size.w - left_width;
                let left = Rectangle::new(remaining.loc, (left_width, remaining.size.h).into());
                remaining = Rectangle::new(
                    (remaining.loc.x + left_width, remaining.loc.y).into(),
                    (right_width, remaining.size.h).into(),
                );
                left
            } else if remaining.size.h > 1 {
                let top_height = (remaining.size.h / 2).max(1);
                let bottom_height = remaining.size.h - top_height;
                let top = Rectangle::new(remaining.loc, (remaining.size.w, top_height).into());
                remaining = Rectangle::new(
                    (remaining.loc.x, remaining.loc.y + top_height).into(),
                    (remaining.size.w, bottom_height).into(),
                );
                top
            } else {
                remaining
            };

            if let Some(toplevel) = window.toplevel() {
                let is_active = self
                    .active_surface
                    .as_ref()
                    .is_some_and(|focused| focused == toplevel.wl_surface());
                window.set_activated(is_active);

                toplevel.with_pending_state(|state| {
                    state.states.unset(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                    );
                    state.states.unset(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                    );
                    state.size = Some(tile.size);
                });
                toplevel.send_pending_configure();
            }

            self.space.map_element(window, tile.loc, false);
        }

        self.space.refresh();
    }

    fn init_wayland_listener(display: Display<Self>, event_loop: &EventLoop<Self>) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, (), state| {
                if let Err(err) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                {
                    tracing::warn!("Failed to insert wayland client: {err}");
                }
            })
            .expect("Failed to init the wayland event source.");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    unsafe {
                        if let Err(err) = display.get_mut().dispatch_clients(state) {
                            tracing::warn!("Failed to dispatch wayland clients: {err}");
                        }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }
}

impl Drop for Smallvil {
    fn drop(&mut self) {
        if self.ipc_socket_path.exists() {
            let _ = std::fs::remove_file(&self.ipc_socket_path);
        }
    }
}

fn ipc_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("RIPWM_IPC_SOCKET") {
        return PathBuf::from(path);
    }

    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("ripwm.sock");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/ripwm/ripwm.sock");
    }

    PathBuf::from("/tmp/ripwm.sock")
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
