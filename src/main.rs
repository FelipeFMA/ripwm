#![allow(irrefutable_let_patterns)]

mod handlers;

mod cursor;
mod drawing;
mod grabs;
mod input;
mod state;
mod udev;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::Smallvil;
use std::io::IsTerminal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    Winit,
    TtyUdev,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    if wants_help() {
        print_help();
        return Ok(());
    }

    let backend = select_backend()?;
    tracing::info!("Selected backend: {:?}", backend);

    match backend {
        Backend::TtyUdev => {
            crate::udev::run_udev()?;
            Ok(())
        }
        Backend::Winit => run_winit(),
    }
}

fn wants_help() -> bool {
    std::env::args().skip(1).any(|arg| arg == "-h" || arg == "--help")
}

fn print_help() {
    println!(
        "ripwm\n\nUsage:\n  ripwm [OPTIONS]\n\nOptions:\n  --tty-udev            Force DRM/udev backend\n  --winit               Force nested winit backend\n  -c, --command <CMD>   Spawn command inside compositor\n  -h, --help            Print help\n\nBackend selection:\n  If no backend flag is provided, ripwm auto-detects:\n  - Uses winit when running under Wayland/X11\n  - Uses tty-udev when started from a real Linux tty"
    );
}

fn select_backend() -> Result<Backend, Box<dyn std::error::Error>> {
    if let Some(cli_backend) = parse_backend_override()? {
        return Ok(cli_backend);
    }

    Ok(detect_backend())
}

fn parse_backend_override() -> Result<Option<Backend>, Box<dyn std::error::Error>> {
    let mut selected_backend = None;

    for arg in std::env::args().skip(1) {
        let backend = match arg.as_str() {
            "--tty-udev" => Some(Backend::TtyUdev),
            "--winit" => Some(Backend::Winit),
            _ => None,
        };

        if let Some(backend) = backend {
            if let Some(existing) = selected_backend
                && existing != backend
            {
                return Err(
                    "Conflicting backend flags: use only one of --tty-udev or --winit".into()
                );
            }
            selected_backend = Some(backend);
        }
    }

    Ok(selected_backend)
}

fn detect_backend() -> Backend {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
        return Backend::Winit;
    }

    if !std::io::stdin().is_terminal() {
        return Backend::Winit;
    }

    if let Ok(tty_path) = std::fs::read_link("/proc/self/fd/0") {
        let path = tty_path.to_string_lossy();
        if path == "/dev/console" || path.starts_with("/dev/tty") {
            return Backend::TtyUdev;
        }
    }

    Backend::Winit
}

fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new()?;

    let display: Display<Smallvil> = Display::new()?;

    let mut state = Smallvil::new(&mut event_loop, display);

    crate::winit::init_winit(&event_loop, &mut state)?;

    set_wayland_display(&state.socket_name);

    spawn_client();

    event_loop.run(None, &mut state, move |_| {})?;

    Ok(())
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}

pub(crate) fn spawn_client() {
    let mut args = std::env::args().skip(1).peekable();

    while matches!(args.peek().map(String::as_str), Some("--winit" | "--tty-udev")) {
        let _ = args.next();
    }

    let flag = args.next();
    let arg = args.next();

    match (flag.as_deref(), arg) {
        (Some("-c" | "--command"), Some(command)) => {
            if let Err(err) = std::process::Command::new(command).spawn() {
                tracing::error!("Failed to spawn command: {err}");
            }
        }
        _ => {
            if let Err(err) = std::process::Command::new("foot").spawn() {
                tracing::error!("Failed to spawn foot: {err}");
            }
        }
    }
}

pub(crate) fn set_wayland_display(socket_name: &std::ffi::OsStr) {
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
    }
}
