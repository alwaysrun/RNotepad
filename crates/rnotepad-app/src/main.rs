#![windows_subsystem = "windows"]

//! RNotepad entry point.
//!
//! Logging is disabled by default and can be enabled from `Help > Enable
//! Logging`.  When enabled, events go to a daily rolling file
//! (`logs/rnotepad.<date>.log`) next to the executable through a non-blocking
//! worker thread.  While logging stays disabled no directory or file is
//! created.

mod commands;
mod main_window;
mod recent;
mod session;
mod tasks;

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::EnvFilter;

/// Directory name for the rolling log files.
const LOG_DIRECTORY: &str = "logs";
/// Base name of the log file; the date is inserted before the suffix.
const LOG_FILE_PREFIX: &str = "rnotepad";
/// Extension of the log file, kept as a real `.log`.
const LOG_FILE_SUFFIX: &str = "log";
/// Default filter when `RUST_LOG` is not set.
const DEFAULT_LOG_FILTER: &str = "info";

/// Process-wide sink shared by the tracing layer and the menu toggle.
static LOG_SINK: OnceLock<LogSink> = OnceLock::new();

fn main() {
    init_tracing();
    if let Err(err) = main_window::run() {
        tracing::error!(error = %err, "application startup failed");
        show_fatal(&err);
    }
}

/// Installs the tracing subscriber with logging switched off.
///
/// The file appender is created lazily by [`LogSink::prepare`] once the user
/// enables logging, so startup never touches the log directory.
fn init_tracing() {
    let Some(directory) = log_directory() else {
        return;
    };
    let sink = LOG_SINK.get_or_init(|| LogSink::new(directory));
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(sink)
        // Log files are plain text; ANSI colour escapes would embed ESC bytes.
        .with_ansi(false)
        .try_init();
}

/// `true` when the user enabled file logging.
#[must_use]
pub fn logging_enabled() -> bool {
    LOG_SINK
        .get()
        .is_some_and(|sink| sink.enabled.load(Ordering::Relaxed))
}

/// Enables logging after opening the file appender.
///
/// Returns an explicit error so the caller can report why the log is
/// unavailable instead of failing silently.
pub fn enable_logging() -> Result<(), String> {
    let Some(sink) = LOG_SINK.get() else {
        return Err("log directory is unavailable".to_owned());
    };
    sink.prepare()?;
    sink.enabled.store(true, Ordering::Relaxed);
    Ok(())
}

/// Disables logging and flushes buffered records.
pub fn disable_logging() {
    if let Some(sink) = LOG_SINK.get() {
        sink.enabled.store(false, Ordering::Relaxed);
        sink.flush();
    }
}

/// Stops logging at shutdown so buffered records reach the file.
pub fn shutdown_logging() {
    disable_logging();
}

fn log_directory() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let parent = executable.parent()?;
    Some(parent.join(LOG_DIRECTORY))
}

/// Runtime toggle plus lazily opened rolling file appender.
struct LogSink {
    enabled: AtomicBool,
    directory: PathBuf,
    target: Mutex<Option<LogTarget>>,
}

/// Open appender and its worker guard, dropped to flush on shutdown.
struct LogTarget {
    writer: tracing_appender::non_blocking::NonBlocking,
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

impl LogSink {
    fn new(directory: PathBuf) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            directory,
            target: Mutex::new(None),
        }
    }

    /// Opens the daily appender on demand; repeated calls are no-ops.
    fn prepare(&self) -> Result<(), String> {
        let mut target = self
            .target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if target.is_some() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory)
            .map_err(|err| format!("create log directory failed: {err}"))?;
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(LOG_FILE_PREFIX)
            .filename_suffix(LOG_FILE_SUFFIX)
            .build(&self.directory)
            .map_err(|err| format!("open log file failed: {err}"))?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        *target = Some(LogTarget {
            writer,
            _guard: guard,
        });
        Ok(())
    }

    /// Writes one record; drops it when logging is disabled or not prepared.
    fn write(&self, buffer: &[u8]) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut target = self
            .target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if target.is_none() {
            drop(target);
            if self.prepare().is_err() {
                return;
            }
            target = self
                .target
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if let Some(target) = target.as_mut() {
            let _ = target.writer.write_all(buffer);
        }
    }

    /// Drops the appender so the worker flushes before exiting.
    fn flush(&self) {
        let mut target = self
            .target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(target.take());
    }
}

/// `MakeWriter` so the tracing layer checks the toggle per record.
struct LogWriter<'a> {
    sink: &'a LogSink,
}

impl Write for LogWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.sink.write(buffer);
        Ok(buffer.len())
    }

    fn write_all(&mut self, buffer: &[u8]) -> std::io::Result<()> {
        self.sink.write(buffer);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for &'static LogSink {
    type Writer = LogWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter { sink: self }
    }
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
