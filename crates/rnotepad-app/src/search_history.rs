//! Persistent search and replacement history.
//!
//! Both lists share one JSON document beside the executable so a portable copy
//! carries its history along.  Duplicates are compared exactly because the
//! find bar can run case-sensitive searches.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How many recent entries each list keeps.
pub const SEARCH_HISTORY_LIMIT: usize = 20;

const STORAGE_FILE: &str = "history.json";

/// On-disk shape of `history.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct HistoryDocument {
    search: Vec<String>,
    replace: Vec<String>,
}

/// Recently used find and replacement text, newest first.
#[derive(Debug, Default)]
pub struct SearchHistory {
    document: HistoryDocument,
}

impl SearchHistory {
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
                tracing::warn!(path = %path.display(), error = %err, "search history could not be read");
                return Self::default();
            }
        };
        match serde_json::from_str::<HistoryDocument>(&text) {
            Ok(document) => Self {
                document: HistoryDocument {
                    search: sanitise(document.search),
                    replace: sanitise(document.replace),
                },
            },
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "search history is not valid JSON; starting empty");
                Self::default()
            }
        }
    }

    /// Find patterns in display order, newest first.
    #[must_use]
    pub fn search_patterns(&self) -> &[String] {
        &self.document.search
    }

    /// Replacement texts in display order, newest first.
    #[must_use]
    pub fn replace_patterns(&self) -> &[String] {
        &self.document.replace
    }

    /// Moves `pattern` to the front of the find list and persists the change.
    ///
    /// Returns `true` when the visible list changed, so callers can skip
    /// redundant work.
    pub fn record_search(&mut self, pattern: &str) -> bool {
        if promote(&mut self.document.search, pattern) {
            self.save();
            true
        } else {
            false
        }
    }

    /// Moves `replacement` to the front of the replace list and persists it.
    ///
    /// Returns `true` when the visible list changed, so callers can skip
    /// redundant work.
    pub fn record_replace(&mut self, replacement: &str) -> bool {
        if promote(&mut self.document.replace, replacement) {
            self.save();
            true
        } else {
            false
        }
    }

    fn save(&self) {
        let Some(path) = storage_path() else {
            return;
        };
        let text = match serde_json::to_string_pretty(&self.document) {
            Ok(text) => text + "\n",
            Err(err) => {
                tracing::warn!(error = %err, "search history could not be serialised");
                return;
            }
        };
        if let Err(err) = fs::write(&path, text) {
            tracing::warn!(path = %path.display(), error = %err, "search history could not be written");
        }
    }
}

/// Promotes `pattern` to the front of one list, keeping it bounded.
fn promote(patterns: &mut Vec<String>, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if patterns.first().is_some_and(|entry| entry == pattern) {
        return false;
    }
    patterns.retain(|entry| entry != pattern);
    patterns.insert(0, pattern.to_owned());
    patterns.truncate(SEARCH_HISTORY_LIMIT);
    true
}

/// Drops blanks and duplicates from a list read from disk.
fn sanitise(patterns: Vec<String>) -> Vec<String> {
    let mut accepted: Vec<String> = Vec::with_capacity(SEARCH_HISTORY_LIMIT);
    for pattern in patterns {
        if pattern.is_empty() || accepted.contains(&pattern) {
            continue;
        }
        accepted.push(pattern);
        if accepted.len() >= SEARCH_HISTORY_LIMIT {
            break;
        }
    }
    accepted
}

/// Location of `history.json` next to the executable, so a portable copy
/// carries its history with it.
fn storage_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(executable.parent()?.join(STORAGE_FILE))
}
