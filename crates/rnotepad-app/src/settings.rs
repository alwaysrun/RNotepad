//! Persistent window placement and editor preferences.
//!
//! Stored as a JSON document beside the recent-file lists so the file stays
//! human-readable and forward-compatible.  Missing or malformed entries fall
//! back to defaults instead of failing startup.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use rnotepad_win::dw::{DEFAULT_FONT_POINTS, MAX_FONT_POINTS, MIN_FONT_POINTS};

const STORAGE_FILE: &str = "settings.json";

/// Word wrap is opt-in so a freshly installed editor keeps horizontal scrolling
/// instead of reflowing long log lines.
pub const DEFAULT_WORD_WRAP: bool = false;

/// Restored window geometry in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl WindowPlacement {
    /// Converts the stored origin and size into a Win32 rectangle.
    #[must_use]
    pub fn rect(&self) -> windows::Win32::Foundation::RECT {
        windows::Win32::Foundation::RECT {
            left: self.x,
            top: self.y,
            right: self.x.saturating_add(self.width),
            bottom: self.y.saturating_add(self.height),
        }
    }

    /// Rejects placements whose extent cannot describe a real window.
    fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// User preferences that must survive a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(rename = "window")]
    pub placement: Option<WindowPlacement>,
    pub font_points: i32,
    pub word_wrap: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            placement: None,
            font_points: DEFAULT_FONT_POINTS,
            word_wrap: DEFAULT_WORD_WRAP,
        }
    }
}

impl Settings {
    /// Loads the persisted preferences, tolerating missing or unreadable files.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = storage_path() else {
            return Self::default();
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "settings could not be read");
                return Self::default();
            }
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(settings) => settings.sanitised(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "settings are not valid JSON; using defaults");
                Self::default()
            }
        }
    }

    /// Writes the preferences, replacing the previous contents.
    ///
    /// Failures are logged because startup still has to succeed with defaults.
    pub fn save(&self) {
        let Some(path) = storage_path() else {
            tracing::warn!("settings directory is unavailable");
            return;
        };
        if let Some(directory) = path.parent() {
            if let Err(err) = fs::create_dir_all(directory) {
                tracing::warn!(path = %directory.display(), error = %err, "settings directory could not be created");
                return;
            }
        }
        let text = match serde_json::to_string_pretty(self) {
            Ok(text) => text + "\n",
            Err(err) => {
                tracing::warn!(error = %err, "settings could not be serialised");
                return;
            }
        };
        if let Err(err) = fs::write(&path, text) {
            tracing::warn!(path = %path.display(), error = %err, "settings could not be written");
        }
    }

    /// Clamps values that would leave the application in an unusable state.
    fn sanitised(mut self) -> Self {
        self.font_points = self.font_points.clamp(MIN_FONT_POINTS, MAX_FONT_POINTS);
        if self.placement.is_some_and(|placement| !placement.is_valid()) {
            self.placement = None;
        }
        self
    }
}

/// Location of `settings.json` next to the executable, so a portable copy
/// carries its preferences with it.
fn storage_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(executable.parent()?.join(STORAGE_FILE))
}
