//! Background search job.
//!
//! The core search engine runs on a worker thread and streams batches over a
//! channel; the UI drains the channel on its timer, so a multi-gigabyte scan
//! never blocks input.

use std::sync::RwLock;

use crossbeam_channel::{Receiver, TryRecvError};
use rnotepad_core::search::{CancelToken, CoreSearchService, SearchQuery, SearchService, TaskEvent};
use tracing::{error, info};

use crate::session::SharedDoc;

/// Handle to a running search.
pub struct SearchJob {
    cancel: CancelToken,
    receiver: Receiver<TaskEvent>,
    finished: bool,
}

impl SearchJob {
    /// Starts a search on a worker thread.
    #[must_use]
    pub fn spawn(document: SharedDoc, query: SearchQuery) -> Self {
        let cancel = CancelToken::new();
        let (sender, receiver) = crossbeam_channel::unbounded();
        let worker_cancel = cancel.clone();
        info!(pattern = %query.pattern, mode = ?query.mode, "starting search");
        std::thread::spawn(move || {
            let guard = document
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(err) = CoreSearchService.run(&query, &guard, worker_cancel, sender.clone()) {
                error!(error = %err, "search failed");
                let _ = sender.send(TaskEvent::Failed(format!("{err}")));
            }
        });
        Self {
            cancel,
            receiver,
            finished: false,
        }
    }

    /// Cancels the worker.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Drains pending events without blocking.
    pub fn drain(&mut self) -> Vec<TaskEvent> {
        let mut events = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    if matches!(event, TaskEvent::Done | TaskEvent::Failed(_)) {
                        self.finished = true;
                    }
                    events.push(event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        events
    }

    /// `true` when the worker signalled completion.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// Ensures the shared document type stays `Send` for the worker thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<RwLock<rnotepad_core::document::Document>>();
};
