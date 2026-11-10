//! Persistent list of recently opened files and files pinned by the user.
//!
//! Both lists are stored as plain text so the application needs no
//! serialisation dependency.  Entries are absolute paths, newest first, and
//! duplicate paths are collapsed case-insensitively because Windows paths are
//! not case-sensitive.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// How many entries the File > Recent Files menu and the taskbar jump list
/// show per category.
pub const RECENT_LIMIT: usize = 10;

const STORAGE_DIRECTORY: &str = "RNotepad";
const STORAGE_FILE: &str = "recent.txt";
const PINNED_FILE: &str = "pinned.txt";

/// Recently opened paths, ordered from newest to oldest, plus the paths the
/// user pinned to the taskbar jump list.
#[derive(Debug, Default)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
    pinned: Vec<PathBuf>,
}

/// One row of the File > Recent Files submenu.
///
/// `pinned` marks the row with a check mark and orders it before plain
/// recent entries, mirroring the jump list's pinned category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    pub path: PathBuf,
    pub pinned: bool,
}

impl RecentFiles {
    /// Loads the persisted lists, tolerating missing or unreadable files.
    #[must_use]
    pub fn load() -> Self {
        Self {
            paths: load_list(STORAGE_FILE),
            pinned: load_list(PINNED_FILE),
        }
    }

    /// Paths in display order.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Pinned paths in the order the user pinned them.
    #[must_use]
    pub fn pinned_paths(&self) -> &[PathBuf] {
        &self.pinned
    }

    /// Rows for File > Recent Files: pinned paths first, then the remaining
    /// recent paths, capped at [`RECENT_LIMIT`] so the command id range lasts.
    #[must_use]
    pub fn menu_entries(&self) -> Vec<MenuEntry> {
        let mut entries: Vec<MenuEntry> = self
            .pinned
            .iter()
            .take(RECENT_LIMIT)
            .map(|path| MenuEntry {
                path: path.clone(),
                pinned: true,
            })
            .collect();
        for path in &self.paths {
            if entries.len() >= RECENT_LIMIT {
                break;
            }
            if entries.iter().any(|entry| same_file(&entry.path, path)) {
                continue;
            }
            entries.push(MenuEntry {
                path: path.clone(),
                pinned: false,
            });
        }
        entries
    }

    /// `true` when `path` is pinned to the taskbar jump list.
    #[must_use]
    pub fn is_pinned(&self, path: &Path) -> bool {
        self.pinned.iter().any(|entry| same_file(entry, path))
    }

    /// Pins `path` at the front of the pinned list.
    ///
    /// Returns `true` when the list changed, so callers can refresh the menu
    /// and the jump list only when needed.
    pub fn pin(&mut self, path: &Path) -> bool {
        if self.is_pinned(path) {
            return false;
        }
        self.pinned.insert(0, absolute_path(path));
        self.pinned.truncate(RECENT_LIMIT);
        save_list(PINNED_FILE, &self.pinned);
        true
    }

    /// Removes `path` from the pinned list.
    ///
    /// Returns `true` when the path was pinned, so callers can refresh the
    /// menu and the jump list only when needed.
    pub fn unpin(&mut self, path: &Path) -> bool {
        let Some(index) = self.pinned.iter().position(|entry| same_file(entry, path)) else {
            return false;
        };
        self.pinned.remove(index);
        save_list(PINNED_FILE, &self.pinned);
        true
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
        save_list(STORAGE_FILE, &self.paths);
        true
    }
}

/// Reads one persisted list, dropping blanks and case-insensitive duplicates.
fn load_list(file: &str) -> Vec<PathBuf> {
    let Some(path) = storage_path(file) else {
        return Vec::new();
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "recent files could not be read");
            return Vec::new();
        }
    };
    let mut entries: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(line);
        if entries
            .iter()
            .any(|existing| same_file(existing, &candidate))
        {
            continue;
        }
        entries.push(candidate);
        if entries.len() >= RECENT_LIMIT {
            break;
        }
    }
    entries
}

/// Writes one list, replacing the previous contents.
fn save_list(file: &str, entries: &[PathBuf]) {
    let Some(path) = storage_path(file) else {
        return;
    };
    if let Some(directory) = path.parent() {
        if let Err(err) = fs::create_dir_all(directory) {
            tracing::warn!(path = %directory.display(), error = %err, "recent directory could not be created");
            return;
        }
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.to_string_lossy());
        text.push('\n');
    }
    if let Err(err) = fs::write(&path, text) {
        tracing::warn!(path = %path.display(), error = %err, "recent files could not be written");
    }
}

/// Per-user storage location; the lists must not be written next to the
/// executable because that directory may be read-only.
fn storage_path(file: &str) -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(base)
            .join(STORAGE_DIRECTORY)
            .join(file),
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
