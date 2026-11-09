//! Persistent list of recent search patterns.
//!
//! Patterns come from a single-line edit control, so storing one per line is
//! lossless.  Entries are kept newest first next to the executable, so a
//! portable copy carries its history along.  Duplicates are compared exactly
//! because the find bar can run case-sensitive searches.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

/// How many recent patterns the find bar keeps.
pub const SEARCH_HISTORY_LIMIT: usize = 20;

const STORAGE_FILE: &str = "search-history.txt";

/// Recently used patterns, ordered from newest to oldest.
#[derive(Debug, Default)]
pub struct SearchHistory {
    patterns: Vec<String>,
}

impl SearchHistory {
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
                tracing::warn!(path = %path.display(), error = %err, "search history could not be read");
                return Self::default();
            }
        };
        let mut history = Self::default();
        for line in text.split('\n') {
            // Strip the CR of CRLF files but keep the pattern byte-for-byte
            // otherwise: leading and trailing spaces are searchable.
            let pattern = line.strip_suffix('\r').unwrap_or(line);
            if pattern.is_empty() || history.patterns.iter().any(|entry| entry == pattern) {
                continue;
            }
            history.patterns.push(pattern.to_owned());
            if history.patterns.len() >= SEARCH_HISTORY_LIMIT {
                break;
            }
        }
        history
    }

    /// Patterns in display order, newest first.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Moves `pattern` to the front of the list and persists the change.
    ///
    /// Returns `true` when the visible list changed, so callers can skip
    /// redundant work.
    pub fn record(&mut self, pattern: &str) -> bool {
        if pattern.is_empty() {
            return false;
        }
        let existing = self.patterns.iter().position(|entry| entry == pattern);
        if existing == Some(0) {
            return false;
        }
        if let Some(index) = existing {
            self.patterns.remove(index);
        }
        self.patterns.insert(0, pattern.to_owned());
        self.patterns.truncate(SEARCH_HISTORY_LIMIT);
        self.save();
        true
    }

    fn save(&self) {
        let Some(path) = storage_path() else {
            return;
        };
        if let Some(directory) = path.parent() {
            if let Err(err) = fs::create_dir_all(directory) {
                tracing::warn!(path = %directory.display(), error = %err, "search history directory could not be created");
                return;
            }
        }
        let mut text = String::new();
        for pattern in &self.patterns {
            text.push_str(pattern);
            text.push('\n');
        }
        if let Err(err) = fs::write(&path, text) {
            tracing::warn!(path = %path.display(), error = %err, "search history could not be written");
        }
    }
}

/// Per-user storage location; the list is kept next to the executable so a
/// portable copy carries its history with it.
fn storage_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(STORAGE_FILE))
}
