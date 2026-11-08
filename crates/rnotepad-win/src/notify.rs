//! External-change detection backed by the `notify` crate.
//!
//! The watcher only signals that the file changed; the application decides
//! whether to reload (read-only documents and followed logs) or to ask the
//! user (dirty editable documents).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Watches one file and coalesces pending change notifications.
pub struct ChangeWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<PathBuf>,
    path: PathBuf,
}

impl ChangeWatcher {
    /// Starts watching `path`.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, notify::Error> {
        let path = path.as_ref().to_path_buf();
        let (sender, receiver) = channel();
        let reported = path.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    if is_change(&event.kind) {
                        // A send failure only means the UI stopped listening.
                        let _ = sender.send(reported.clone());
                    }
                }
            },
            notify::Config::default(),
        )?;
        watcher.watch(&path, RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            receiver,
            path,
        })
    }

    /// Path being watched.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drains pending notifications and reports whether any arrived.
    #[must_use]
    pub fn take_change(&self) -> bool {
        let mut changed = false;
        loop {
            match self.receiver.try_recv() {
                Ok(_) => changed = true,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed
    }
}

fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => true,
        _ => false,
    }
}
