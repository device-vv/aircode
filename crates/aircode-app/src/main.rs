#![windows_subsystem = "windows"]

use std::{io::Write, path::PathBuf};

fn crash_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let directory = base.join("AIRcode");
    let _ = std::fs::create_dir_all(&directory);
    directory.join("crash.log")
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crash_log_path())
        {
            // Deliberately omit the payload: panic messages can contain user text.
            let location = info
                .location()
                .map(|location| location.to_string())
                .unwrap_or_else(|| "unknown location".into());
            let _ = writeln!(
                file,
                "--- AIRcode {} panic at unix time {seconds} ---\n{location}\n{backtrace}\n",
                env!("CARGO_PKG_VERSION")
            );
        }
        default_hook(info);
    }));
}

fn main() {
    install_panic_hook();
    aircode_ui::run();
}
