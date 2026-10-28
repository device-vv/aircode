#![windows_subsystem = "windows"]

use std::io::Write;
use std::path::PathBuf;

/// Directory for the crash log. The Windows build has no console, so without
/// this file a panic would just make the window disappear.
fn crash_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("AIRcode");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("crash.log")
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crash_log_path())
        {
            let _ = writeln!(
                file,
                "--- AIRcode {} panic at unix time {seconds} ---\n{info}\n{backtrace}\n",
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
