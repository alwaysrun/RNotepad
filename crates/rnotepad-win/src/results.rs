//! Multi-group search result model backing the owner-data list view.
//!
//! Every Find All appends one group below the existing ones, so earlier groups
//! keep their position and the newest search reads last.  Groups are
//! collapsible; the model keeps a flat row list (one header row per group plus
//! the hits of expanded groups) because a `LVS_OWNERDATA` list view addresses
//! rows by index only.
//!
//! The model stores `MatchHit` values, not preview strings: at 100k hits the
//! memory cost is a few megabytes and each row's text is built only when the
//! list view paints it.

use rnotepad_core::document::DocumentId;
use rnotepad_core::search::MatchHit;

/// One row of the flattened table shown in the results panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultRow {
    /// Header row of group `group`.
    Group { group: usize },
    /// Hit `hit` of group `group`.
    Hit { group: usize, hit: usize },
}

/// Hits produced by one Find All.
#[derive(Debug, Clone)]
pub struct ResultGroup {
    query: String,
    doc_id: DocumentId,
    revision: u64,
    hits: Vec<MatchHit>,
    expanded: bool,
    complete: bool,
}

impl ResultGroup {
    fn new(query: String, doc_id: DocumentId, revision: u64) -> Self {
        Self {
            query,
            doc_id,
            revision,
            hits: Vec::new(),
            // A fresh group shows its hits as they stream in.
            expanded: true,
            complete: false,
        }
    }

    /// Pattern the group was searched with.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Document the hits belong to.
    #[must_use]
    pub fn doc_id(&self) -> DocumentId {
        self.doc_id
    }

    /// Document revision the hits were produced from.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Hits in scan order.
    #[must_use]
    pub fn hits(&self) -> &[MatchHit] {
        &self.hits
    }

    /// Returns one hit.
    #[must_use]
    pub fn hit(&self, index: usize) -> Option<&MatchHit> {
        self.hits.get(index)
    }

    /// Number of hits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// `true` when the group holds no hits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// `true` when the hits are visible below the header.
    #[must_use]
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// `true` when the search finished.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }
}

/// All Find All groups of the session, oldest first.
#[derive(Debug, Clone, Default)]
pub struct ResultsModel {
    groups: Vec<ResultGroup>,
    rows: Vec<ResultRow>,
}

impl ResultsModel {
    /// Appends a new empty group below the existing ones.
    pub fn start_group(&mut self, query: impl Into<String>, doc_id: DocumentId, revision: u64) {
        self.groups
            .push(ResultGroup::new(query.into(), doc_id, revision));
        self.rebuild_rows();
    }

    /// Appends a batch of hits to the newest group.
    ///
    /// Hits always belong to the group the running search was started for,
    /// which is the newest one because a new search cancels the previous job.
    /// Batches are dropped when that group was removed or already finished,
    /// so a late event can never leak into an older result set.
    pub fn push_hits(&mut self, hits: &[MatchHit]) {
        let Some(group) = self.groups.last_mut() else {
            return;
        };
        if group.complete {
            return;
        }
        group.hits.extend_from_slice(hits);
        self.rebuild_rows();
    }

    /// Marks the newest group finished.
    pub fn finish_newest(&mut self) {
        if let Some(group) = self.groups.last_mut() {
            if !group.complete {
                group.complete = true;
            }
        }
    }

    /// Returns the newest group, which owns the running search.
    #[must_use]
    pub fn newest(&self) -> Option<&ResultGroup> {
        self.groups.last()
    }

    /// Index of the newest group, i.e. the one a running job feeds.
    #[must_use]
    pub fn newest_index(&self) -> Option<usize> {
        self.groups.len().checked_sub(1)
    }

    /// Hits of the newest group; these drive highlights and Find Next.
    #[must_use]
    pub fn active_hits(&self) -> &[MatchHit] {
        self.groups
            .last()
            .map(|group| group.hits.as_slice())
            .unwrap_or(&[])
    }

    /// `true` while a group has not signalled completion.
    #[must_use]
    pub fn has_running(&self) -> bool {
        self.groups.last().is_some_and(|group| !group.complete)
    }

    /// Drops an unfinished newest group, e.g. after its job was cancelled.
    ///
    /// Returns `true` when a group was removed.
    pub fn remove_incomplete(&mut self) -> bool {
        let Some(newest) = self.groups.last() else {
            return false;
        };
        if newest.complete {
            return false;
        }
        self.groups.pop();
        self.rebuild_rows();
        true
    }

    /// Expands or collapses one group.
    ///
    /// Returns `false` when `group` does not exist.
    pub fn toggle_group(&mut self, group: usize) -> bool {
        let Some(target) = self.groups.get_mut(group) else {
            return false;
        };
        target.expanded = !target.expanded;
        self.rebuild_rows();
        true
    }

    /// Expands or collapses every group.
    pub fn set_all_expanded(&mut self, expanded: bool) {
        for group in &mut self.groups {
            group.expanded = expanded;
        }
        self.rebuild_rows();
    }

    /// Removes one group and its hits.
    ///
    /// Returns `false` when `group` does not exist.
    pub fn remove_group(&mut self, group: usize) -> bool {
        if group >= self.groups.len() {
            return false;
        }
        self.groups.remove(group);
        self.rebuild_rows();
        true
    }

    /// Removes every group and hit.
    pub fn clear(&mut self) {
        self.groups.clear();
        self.rows.clear();
    }

    /// `true` when no group exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Number of flattened rows, i.e. the list-view item count.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Total hits across all groups.
    #[must_use]
    pub fn hit_count(&self) -> usize {
        self.groups.iter().map(ResultGroup::len).sum()
    }

    /// One flattened row.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<ResultRow> {
        self.rows.get(index).copied()
    }

    /// Row index of the newest group's header, used to scroll it into view.
    #[must_use]
    pub fn newest_header_row(&self) -> Option<usize> {
        self.rows
            .iter()
            .rposition(|row| matches!(row, ResultRow::Group { .. }))
    }

    /// One group by index.
    #[must_use]
    pub fn group(&self, index: usize) -> Option<&ResultGroup> {
        self.groups.get(index)
    }

    /// One hit by group and index.
    #[must_use]
    pub fn hit(&self, group: usize, index: usize) -> Option<&MatchHit> {
        self.groups.get(group)?.hits.get(index)
    }

    /// Row index of one hit, or `None` when its group is collapsed.
    #[must_use]
    pub fn row_of_hit(&self, group: usize, hit: usize) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(
                row,
                ResultRow::Hit {
                    group: row_group,
                    hit: row_hit,
                } if *row_group == group && *row_hit == hit
            )
        })
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        self.rows.reserve(self.hit_count() + self.groups.len());
        for (group_index, group) in self.groups.iter().enumerate() {
            self.rows.push(ResultRow::Group { group: group_index });
            if !group.expanded {
                continue;
            }
            self.rows.extend(
                (0..group.hits.len()).map(|hit| ResultRow::Hit {
                    group: group_index,
                    hit,
                }),
            );
        }
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
    fn newest_group_is_appended_and_owns_the_rows() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0), hit(10, 1, 0)]);
        model.finish_newest();
        model.start_group("beta", 7, 1);
        model.push_hits(&[hit(20, 2, 0)]);

        assert_eq!(model.row_count(), 5);
        assert_eq!(model.row(0), Some(ResultRow::Group { group: 0 }));
        assert_eq!(model.row(1), Some(ResultRow::Hit { group: 0, hit: 0 }));
        assert_eq!(model.row(2), Some(ResultRow::Hit { group: 0, hit: 1 }));
        assert_eq!(model.row(3), Some(ResultRow::Group { group: 1 }));
        assert_eq!(model.row(4), Some(ResultRow::Hit { group: 1, hit: 0 }));
        assert_eq!(model.newest().map(ResultGroup::query), Some("beta"));
        assert_eq!(model.newest_index(), Some(1));
        assert_eq!(model.newest_header_row(), Some(3));
        assert_eq!(model.hit_count(), 3);
    }

    #[test]
    fn collapsed_group_hides_its_hits() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0)]);
        model.finish_newest();

        assert!(model.toggle_group(0));
        assert_eq!(model.row_count(), 1);
        assert!(!model.toggle_group(5));
    }

    #[test]
    fn removing_a_group_keeps_the_rest() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0)]);
        model.finish_newest();
        model.start_group("beta", 7, 1);
        model.push_hits(&[hit(10, 1, 0)]);
        model.finish_newest();

        assert!(model.remove_group(1));
        assert_eq!(model.hit_count(), 1);
        assert_eq!(model.newest().map(ResultGroup::query), Some("alpha"));
        assert_eq!(model.row_count(), 2);
    }

    #[test]
    fn incomplete_group_is_dropped_before_a_new_search() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0)]);
        assert!(model.has_running());
        assert!(model.remove_incomplete());
        assert!(model.is_empty());

        model.start_group("beta", 7, 1);
        model.finish_newest();
        assert!(!model.remove_incomplete());
        assert_eq!(model.row_count(), 1);
    }

    #[test]
    fn late_batches_never_reach_a_finished_group() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0)]);
        model.finish_newest();
        model.push_hits(&[hit(10, 1, 0)]);

        assert_eq!(model.hit_count(), 1);
        assert_eq!(model.row_count(), 2);
    }

    #[test]
    fn expand_all_restores_hidden_hits() {
        let mut model = ResultsModel::default();
        model.start_group("alpha", 7, 1);
        model.push_hits(&[hit(0, 0, 0), hit(4, 0, 4)]);
        model.finish_newest();

        model.set_all_expanded(false);
        assert_eq!(model.row_count(), 1);
        model.set_all_expanded(true);
        assert_eq!(model.row_count(), 3);
    }
}
