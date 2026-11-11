//! Persistent list of recently opened files and files pinned by the user.
//!
//! Both lists share one JSON document beside the executable, so a portable
//! copy carries them along.  Entries are absolute paths, newest first, and
//! duplicate paths are collapsed case-insensitively because Windows paths are
//! not case-sensitive.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many entries the File > Recent Files menu and the taskbar jump list
/// show per category.
pub const RECENT_LIMIT: usize = 10;

const STORAGE_FILE: &str = "recent.json";

/// On-disk shape of `recent.json`; strings keep non-UTF-8 paths lossy instead
/// of failing the whole document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct RecentDocument {
    recent: Vec<String>,
    pinned: Vec<String>,
}

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
    /// Loads the persisted lists, tolerating a missing or unreadable file.
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
        match serde_json::from_str::<RecentDocument>(&text) {
            Ok(document) => Self {
                paths: to_paths(document.recent),
                pinned: to_paths(document.pinned),
            },
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "recent files are not valid JSON; starting empty");
                Self::default()
            }
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
        self.save();
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
        self.save();
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
        self.save();
        true
    }

    /// Writes both lists, replacing the previous contents.
    fn save(&self) {
        let Some(path) = storage_path() else {
            return;
        };
        let document = RecentDocument {
            recent: self
                .paths
                .iter()
                .map(|entry| entry.to_string_lossy().into_owned())
                .collect(),
            pinned: self
                .pinned
                .iter()
                .map(|entry| entry.to_string_lossy().into_owned())
                .collect(),
        };
        let text = match serde_json::to_string_pretty(&document) {
            Ok(text) => text + "\n",
            Err(err) => {
                tracing::warn!(error = %err, "recent files could not be serialised");
                return;
            }
        };
        if let Err(err) = fs::write(&path, text) {
            tracing::warn!(path = %path.display(), error = %err, "recent files could not be written");
        }
    }
}

/// Converts stored strings back into paths, dropping blanks and
/// case-insensitive duplicates.
fn to_paths(entries: Vec<String>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::with_capacity(RECENT_LIMIT);
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(entry);
        if paths.iter().any(|existing| same_file(existing, &candidate)) {
            continue;
        }
        paths.push(candidate);
        if paths.len() >= RECENT_LIMIT {
            break;
        }
    }
    paths
}

/// Location of `recent.json` next to the executable, so a portable copy
/// carries its recent files along.
fn storage_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(executable.parent()?.join(STORAGE_FILE))
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
