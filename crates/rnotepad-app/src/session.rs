//! Document session state for the application.

use std::path::Path;
use std::sync::{Arc, RwLock};

use rnotepad_core::document::{Document, DocumentError, DocumentId, OpenOptions};
use rnotepad_win::results::ResultsModel;

/// Document shared with background searches and view windows.
pub type SharedDoc = Arc<RwLock<Document>>;

/// Which panes are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewLayout {
    Text,
    Hex,
    TextAndHex,
    TextAndResults,
}

/// One open document and the UI state attached to it.
pub struct Session {
    next_id: DocumentId,
    pub doc: Option<SharedDoc>,
    pub layout: ViewLayout,
    pub results: ResultsModel,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            next_id: 1,
            doc: None,
            layout: ViewLayout::Text,
            results: ResultsModel::default(),
        }
    }
}

impl Session {
    /// Opens a file and resets the session state.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<SharedDoc, DocumentError> {
        let document = Document::open(path, OpenOptions::default())?;
        let shared = self.register(document);
        self.layout = ViewLayout::Text;
        self.results = ResultsModel::default();
        Ok(shared)
    }

    /// Creates an empty in-memory document.
    #[must_use]
    pub fn new_blank(&mut self) -> SharedDoc {
        let document = Document::from_bytes(self.allocate_id(), Vec::new());
        let shared = Arc::new(RwLock::new(document));
        self.doc = Some(shared.clone());
        self.layout = ViewLayout::Text;
        self.results = ResultsModel::default();
        shared
    }

    fn register(&mut self, mut document: Document) -> SharedDoc {
        document.id = self.allocate_id();
        let shared = Arc::new(RwLock::new(document));
        self.doc = Some(shared.clone());
        shared
    }

    fn allocate_id(&mut self) -> DocumentId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Closes the document.
    pub fn close(&mut self) {
        self.doc = None;
        self.layout = ViewLayout::Text;
        self.results = ResultsModel::default();
    }

    /// Reads the document under a shared lock.
    pub fn read<R>(&self, visit: impl FnOnce(&Document) -> R) -> Option<R> {
        let shared = self.doc.as_ref()?;
        let guard = shared
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(visit(&guard))
    }

    /// Mutates the document under an exclusive lock.
    pub fn write<R>(&self, visit: impl FnOnce(&mut Document) -> R) -> Option<R> {
        let shared = self.doc.as_ref()?;
        let mut guard = shared
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(visit(&mut guard))
    }

    /// `true` when a document is open.
    #[must_use]
    pub fn has_document(&self) -> bool {
        self.doc.is_some()
    }
}
