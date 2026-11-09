//! Persistent list of recently opened files.
//!
//! The list is stored as plain text so the application needs no serialisation
//! dependency.  Entries are absolute paths, newest first, and duplicate paths
//! are collapsed case-insensitively because Windows paths are not
//! case-sensitive.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// How many entries the File > Recent Files menu shows.
pub const RECENT_LIMIT: usize = 10;

const STORAGE_DIRECTORY: &str = "RNotepad";
const STORAGE_FILE: &str = "recent.txt";

/// Recently opened paths, ordered from newest to oldest.
#[derive(Debug, Default)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// Loads the persisted list, tolerating a missing or unreadable file.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = storage_path() else {
            return Self::default();
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "recent files could not be read");
                return Self::default();
            }
        };
        let mut recent = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(line);
            if recent
                .paths
                .iter()
                .any(|existing| same_file(existing, &candidate))
            {
                continue;
            }
            recent.paths.push(candidate);
            if recent.paths.len() >= RECENT_LIMIT {
                break;
            }
        }
        recent
    }

    /// Paths in display order.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Moves `path` to the front of the list and persists the change.
    ///
    /// Returns `true` when the visible list changed, so callers can refresh
    /// their menus only when needed.
    pub fn record(&mut self, path: &Path) -> bool {
        let entry = absolute_path(path);
        let existing = self
            .paths
            .iter()
            .position(|candidate| same_file(candidate, &entry));
        if existing == Some(0) {
            return false;
        }
        if let Some(index) = existing {
            self.paths.remove(index);
        }
        self.paths.insert(0, entry);
        self.paths.truncate(RECENT_LIMIT);
        self.save();
        true
    }

    fn save(&self) {
        let Some(path) = storage_path() else {
            return;
        };
        if let Some(directory) = path.parent() {
            if let Err(err) = fs::create_dir_all(directory) {
                tracing::warn!(path = %directory.display(), error = %err, "recent directory could not be created");
                return;
            }
        }
        let mut text = String::new();
        for entry in &self.paths {
            text.push_str(&entry.to_string_lossy());
            text.push('\n');
        }
        if let Err(err) = fs::write(&path, text) {
            tracing::warn!(path = %path.display(), error = %err, "recent files could not be written");
        }
    }
}

/// Per-user storage location; the list must not be written next to the
/// executable because that directory may be read-only.
fn storage_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(base)
            .join(STORAGE_DIRECTORY)
            .join(STORAGE_FILE),
    )
}

/// Makes a relative path absolute so the menu stays stable across launches.
fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Windows comparisons are case-insensitive.
fn same_file(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}
