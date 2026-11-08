//! Pane tree and layout for the docked views.
//!
//! The first release exposes two presets (main view with a right-hand panel,
//! and main view beside the hex view), but the tree already supports nesting
//! so future presets do not require a rewrite.

use windows::Win32::Foundation::RECT;

/// Identifies a leaf in the pane tree.
pub type LeafId = u32;

/// Which view a leaf hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafKind {
    Text,
    Hex,
    Results,
    Problems,
}

/// Split direction.  `Horizontal` places the first child on the left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Width of the gap between two children, in physical pixels.
pub const SPLITTER_WIDTH: i32 = 4;
const MIN_RATIO: f32 = 0.15;
const MAX_RATIO: f32 = 0.85;
const DEFAULT_RATIO: f32 = 0.62;

/// One laid-out leaf.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub id: LeafId,
    pub kind: LeafKind,
    pub rect: RECT,
}

/// A node in the pane tree.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneNode {
    Leaf { id: LeafId, kind: LeafKind },
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

/// The pane tree of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneTree {
    root: PaneNode,
    next_id: LeafId,
}

impl PaneTree {
    /// Creates a tree with a single leaf.
    #[must_use]
    pub fn single(kind: LeafKind) -> Self {
        Self {
            root: PaneNode::Leaf { id: 0, kind },
            next_id: 1,
        }
    }

    /// Creates a split of two new leaves.
    #[must_use]
    pub fn split(axis: Axis, first: LeafKind, second: LeafKind) -> Self {
        Self {
            root: PaneNode::Split {
                axis,
                ratio: DEFAULT_RATIO,
                first: Box::new(PaneNode::Leaf { id: 0, kind: first }),
                second: Box::new(PaneNode::Leaf { id: 1, kind: second }),
            },
            next_id: 2,
        }
    }

    /// Replaces the tree with a two-leaf preset.
    pub fn set_preset(&mut self, axis: Axis, first: LeafKind, second: LeafKind) {
        *self = Self::split(axis, first, second);
    }

    /// Builds the preset used by the "main view | hex" command.
    pub fn text_hex_preset() -> Self {
        Self::split(Axis::Horizontal, LeafKind::Text, LeafKind::Hex)
    }

    /// Lays the tree out into `bounds`.
    #[must_use]
    pub fn layout(&self, bounds: RECT) -> Vec<Placement> {
        let mut placements = Vec::new();
        layout_node(&self.root, bounds, &mut placements);
        placements
    }

    /// Adjusts the ratio of the first split, clamping it to the valid range.
    pub fn set_root_ratio(&mut self, ratio: f32) {
        if let PaneNode::Split { ratio: current, .. } = &mut self.root {
            *current = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }

    /// Number of leaves in the tree.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        fn count(node: &PaneNode) -> usize {
            match node {
                PaneNode::Leaf { .. } => 1,
                PaneNode::Split { first, second, .. } => count(first) + count(second),
            }
        }
        count(&self.root)
    }
}

fn layout_node(node: &PaneNode, bounds: RECT, out: &mut Vec<Placement>) {
    match node {
        PaneNode::Leaf { id, kind } => out.push(Placement {
            id: *id,
            kind: *kind,
            rect: bounds,
        }),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => match axis {
            Axis::Horizontal => {
                let usable = (bounds.right - bounds.left - SPLITTER_WIDTH).max(0);
                let split = ((usable as f32) * ratio.clamp(MIN_RATIO, MAX_RATIO)) as i32;
                let first_rect = RECT {
                    left: bounds.left,
                    top: bounds.top,
                    right: bounds.left + split,
                    bottom: bounds.bottom,
                };
                let second_rect = RECT {
                    left: bounds.left + split + SPLITTER_WIDTH,
                    top: bounds.top,
                    right: bounds.right,
                    bottom: bounds.bottom,
                };
                layout_node(first, first_rect, out);
                layout_node(second, second_rect, out);
            }
            Axis::Vertical => {
                let usable = (bounds.bottom - bounds.top - SPLITTER_WIDTH).max(0);
                let split = ((usable as f32) * ratio.clamp(MIN_RATIO, MAX_RATIO)) as i32;
                let first_rect = RECT {
                    left: bounds.left,
                    top: bounds.top,
                    right: bounds.right,
                    bottom: bounds.top + split,
                };
                let second_rect = RECT {
                    left: bounds.left,
                    top: bounds.top + split + SPLITTER_WIDTH,
                    right: bounds.right,
                    bottom: bounds.bottom,
                };
                layout_node(first, first_rect, out);
                layout_node(second, second_rect, out);
            }
        },
    }
}

/// Full-area bounds helper.
#[must_use]
pub fn full_bounds(right: i32, bottom: i32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: right.max(0),
        bottom: bottom.max(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_leaf_covers_bounds() {
        let tree = PaneTree::single(LeafKind::Text);
        let placements = tree.layout(full_bounds(800, 600));
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].rect.right, 800);
        assert_eq!(placements[0].rect.bottom, 600);
    }

    #[test]
    fn horizontal_split_reserves_a_gap() {
        let tree = PaneTree::text_hex_preset();
        let placements = tree.layout(full_bounds(800, 600));
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].kind, LeafKind::Text);
        assert_eq!(placements[1].kind, LeafKind::Hex);
        assert_eq!(placements[1].rect.left, placements[0].rect.right + SPLITTER_WIDTH);
    }
}
