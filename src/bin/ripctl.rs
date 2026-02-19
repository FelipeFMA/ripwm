use std::{io::Write, os::unix::net::UnixStream, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("reload") => send_reload(),
        Some("keyboard") => send_keyboard(args),
        Some("-h" | "--help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!("Unknown command: {other}").into()),
    }
}

fn print_help() {
    println!(
        "ripctl\n\nUsage:\n  ripctl reload\n  ripctl keyboard <layout> [variant]\n\nCommands:\n  reload                       Ask a running ripwm instance to reload configuration\n  keyboard <layout> [variant]  Set keyboard layout/variant on a running ripwm instance"
    );
}

fn send_reload() -> Result<(), Box<dyn std::error::Error>> {
    send_ipc_command("reload\n")?;
    println!("Sent reload request to ripwm");
    Ok(())
}

fn send_keyboard(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let Some(layout) = args.next() else {
        return Err("Missing <layout>. Usage: ripctl keyboard <layout> [variant]".into());
    };

    let variant = args.next().unwrap_or_default();
    if args.next().is_some() {
        return Err("Too many arguments. Usage: ripctl keyboard <layout> [variant]".into());
    }

    let command = if variant.is_empty() {
        format!("keyboard {layout}\n")
    } else {
        format!("keyboard {layout} {variant}\n")
    };

    send_ipc_command(&command)?;
    println!("Sent keyboard update to ripwm: layout={layout}, variant={variant}");
    Ok(())
}

fn send_ipc_command(command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = ipc_socket_path();

    let mut stream = UnixStream::connect(&socket_path).map_err(|err| {
        format!("Failed to connect to ripwm IPC socket at {}: {err}", socket_path.display())
    })?;

    stream.write_all(command.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(())
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
