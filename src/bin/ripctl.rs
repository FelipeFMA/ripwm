use std::{io::Write, os::unix::net::UnixStream, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("reload") => send_reload(),
        Some("-h" | "--help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!("Unknown command: {other}").into()),
    }
}

fn print_help() {
    println!(
        "ripctl\n\nUsage:\n  ripctl reload\n\nCommands:\n  reload    Ask a running ripwm instance to reload configuration"
    );
}

fn send_reload() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = ipc_socket_path();

    let mut stream = UnixStream::connect(&socket_path).map_err(|err| {
        format!("Failed to connect to ripwm IPC socket at {}: {err}", socket_path.display())
    })?;

    stream.write_all(b"reload\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    println!("Sent reload request to ripwm");
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
