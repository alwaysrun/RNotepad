//! Search result model backing the owner-data list view.
//!
//! The model stores only [`MatchHit`] values: no paths and no preview strings.
//! At 100k hits the memory cost is a few megabytes and the list view asks for
//! each row's text only when it becomes visible.

use rnotepad_core::document::DocumentId;
use rnotepad_core::search::MatchHit;

/// One row shown in the results panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultRow {
    pub line: u32,
    pub column: u32,
}

/// Hits for the current search.
#[derive(Debug, Clone, Default)]
pub struct ResultsModel {
    doc_id: DocumentId,
    revision: u64,
    hits: Vec<MatchHit>,
    complete: bool,
    stale: bool,
}

impl ResultsModel {
    /// Starts a new result set bound to a document revision.
    #[must_use]
    pub fn start(doc_id: DocumentId, revision: u64) -> Self {
        Self {
            doc_id,
            revision,
            hits: Vec::new(),
            complete: false,
            stale: false,
        }
    }

    /// Appends one batch of hits.
    pub fn push(&mut self, hits: &[MatchHit]) {
        self.hits.extend_from_slice(hits);
    }

    /// Marks the set complete.
    pub fn finish(&mut self) {
        self.complete = true;
    }

    /// Clears all hits and marks the set complete.
    pub fn clear(&mut self) {
        self.hits.clear();
        self.complete = true;
    }

    /// Marks the set stale when the document revision changed.
    pub fn sync_revision(&mut self, revision: u64) {
        if revision != self.revision {
            self.stale = true;
        }
    }

    /// Bound document identifier.
    #[must_use]
    pub fn doc_id(&self) -> DocumentId {
        self.doc_id
    }

    /// Revision the hits were produced from.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// `true` when the results no longer match the document.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// `true` when the search finished.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Number of hits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// `true` when there are no hits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Returns one hit.
    #[must_use]
    pub fn hit(&self, index: usize) -> Option<&MatchHit> {
        self.hits.get(index)
    }

    /// Returns the display row for `index`.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<ResultRow> {
        self.hits.get(index).map(|hit| ResultRow {
            line: hit.line_no.saturating_add(1),
            column: hit.col.saturating_add(1),
        })
    }

    /// All hits.
    #[must_use]
    pub fn hits(&self) -> &[MatchHit] {
        &self.hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(offset: u64, line: u32, col: u32) -> MatchHit {
        MatchHit {
            doc_id: 7,
            raw_offset: offset,
            match_len: 3,
            line_no: line,
            col,
            doc_revision: 1,
        }
    }

    #[test]
    fn rows_are_one_based_for_display() {
        let mut model = ResultsModel::start(7, 1);
        model.push(&[hit(16, 0, 0)]);
        let row = model.row(0).unwrap();
        assert_eq!(row.line, 1);
        assert_eq!(row.column, 1);
    }

    #[test]
    fn revision_change_marks_results_stale() {
        let mut model = ResultsModel::start(7, 1);
        model.sync_revision(1);
        assert!(!model.is_stale());
        model.sync_revision(2);
        assert!(model.is_stale());
    }
}
