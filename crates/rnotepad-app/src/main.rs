#![windows_subsystem = "windows"]

//! RNotepad entry point.
//!
//! Logging goes to a daily file next to the executable; the application itself
//! is a pure Win32 process without a console.

mod commands;
mod main_window;
mod session;
mod tasks;

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

/// Directory name for the rolling log files.
const LOG_DIRECTORY: &str = "logs";
/// Base name of the log file.
const LOG_FILE: &str = "rnotepad.log";
/// Default filter when `RUST_LOG` is not set.
const DEFAULT_LOG_FILTER: &str = "info";

fn main() {
    let _guard = init_tracing();
    if let Err(err) = main_window::run() {
        tracing::error!(error = %err, "application startup failed");
        show_fatal(&err);
    }
}

fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let directory = log_directory()?;
    std::fs::create_dir_all(&directory).ok()?;
    let writer = tracing_appender::rolling::daily(&directory, LOG_FILE);
    let (writer, guard) = tracing_appender::non_blocking(writer);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .try_init();
    Some(guard)
}

fn log_directory() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let parent = executable.parent()?;
    Some(parent.join(LOG_DIRECTORY))
}

fn show_fatal(message: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let text = wide("RNotepad could not start");
    let detail = wide(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(detail.as_ptr()),
            PCWSTR(text.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide(text: &str) -> Vec<u16> {
    let mut buffer: Vec<u16> = text.encode_utf16().collect();
    buffer.push(0);
    buffer
}
