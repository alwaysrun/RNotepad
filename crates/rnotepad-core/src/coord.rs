//! Coordinate systems and the raw <-> UTF-8 <-> view mapping.
//!
//! Three newtypes keep the three coordinate spaces apart: [`RawByteOffset`]
//! addresses the bytes on disk, [`Utf8Pos`] addresses the decoded UTF-8 text
//! (Scintilla positions), and [`ViewPos`] is a logical line plus display
//! column.  Conversion always goes through a [`CoordMap`].

use std::sync::{Arc, Mutex};

use unicode_width::UnicodeWidthStr;

use crate::bytes::{ByteSource, RawBytes};
use crate::encoding::{decode, unit_len, Charset};
use crate::line_index::LineIndex;

/// Distance between two coordinate anchors, in raw bytes.
pub const ANCHOR_STRIDE: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RawByteOffset(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utf8Pos(pub usize);

/// Logical line and display column.  Both are zero-based; the UI adds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ViewPos {
    pub line: u32,
    pub col: u32,
}

/// A half-open range in an arbitrary coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span<T> {
    pub start: T,
    pub end: T,
}

impl<T> Span<T> {
    #[must_use]
    pub const fn new(start: T, end: T) -> Self {
        Self { start, end }
    }
}

/// Bidirectional mapping between the three coordinate spaces.
pub trait CoordMap: Send + Sync {
    /// Raw byte offset -> UTF-8 byte offset.  Returns the start of the
    /// character when `raw` points inside a multi-byte unit.
    fn raw_to_utf8(&self, raw: RawByteOffset) -> Option<Utf8Pos>;

    /// UTF-8 byte offset -> raw byte offset.  Returns `None` when `pos` is
    /// not on a character boundary.
    fn utf8_to_raw(&self, pos: Utf8Pos) -> Option<RawByteOffset>;

    /// Raw byte offset -> logical line and display column.
    fn raw_to_view(&self, raw: RawByteOffset) -> Option<ViewPos>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchor {
    raw: u64,
    utf8: u64,
}

struct CoordState {
    anchors: Vec<Anchor>,
    complete: bool,
}

/// Sparse-anchor implementation used by [`crate::document::Document`].
///
/// Anchors are 4 KB apart and always sit on a character boundary.  A query
/// walks at most one stride to reach the requested position, so memory stays
/// proportional to the file size divided by the stride.
pub struct AnchoredCoordMap {
    bytes: RawBytes,
    charset: Charset,
    bom_len: u64,
    line_index: Arc<LineIndex>,
    state: Mutex<CoordState>,
}

impl AnchoredCoordMap {
    /// Creates a map over `bytes`.
    ///
    /// `line_index` must describe the same bytes and charset.
    #[must_use]
    pub fn new(
        bytes: RawBytes,
        charset: Charset,
        bom_len: u64,
        line_index: Arc<LineIndex>,
    ) -> Self {
        let complete = bytes.len() <= bom_len;
        Self {
            bytes,
            charset,
            bom_len,
            line_index,
            state: Mutex::new(CoordState {
                anchors: vec![Anchor {
                    raw: bom_len,
                    utf8: 0,
                }],
                complete,
            }),
        }
    }

    /// The charset this map was built for.
    #[must_use]
    pub const fn charset(&self) -> Charset {
        self.charset
    }

    /// Builds every anchor.  Intended for background tasks.
    pub fn build_all(&self) {
        self.ensure_raw(self.bytes.len());
    }

    fn ensure_raw(&self, target: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !state.complete {
            let last = *state.anchors.last().expect("anchor list is never empty");
            if last.raw >= target {
                break;
            }
            let len = self.bytes.len();
            let mut pos = last.raw;
            let end = (last.raw + ANCHOR_STRIDE).min(len);
            while pos < end {
                let unit = unit_len_bytes(&self.bytes, pos, self.charset);
                if pos + unit > end {
                    break;
                }
                pos += unit;
            }
            if pos <= last.raw {
                // Defensive: cannot advance, stop instead of looping forever.
                state.complete = true;
                break;
            }
            let utf8_len = self.utf8_len_of(last.raw, pos);
            state.anchors.push(Anchor {
                raw: pos,
                utf8: last.utf8 + utf8_len,
            });
            if pos >= len {
                state.complete = true;
            }
        }
    }

    fn ensure_utf8(&self, target: u64) {
        loop {
            let (last, complete) = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (
                    *state.anchors.last().expect("anchor list is never empty"),
                    state.complete,
                )
            };
            if complete || last.utf8 >= target {
                break;
            }
            self.ensure_raw(last.raw + ANCHOR_STRIDE);
            let new_last = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *state.anchors.last().expect("anchor list is never empty")
            };
            if new_last.raw == last.raw {
                break; // Nothing progressed; avoid an infinite loop.
            }
        }
    }

    fn utf8_len_of(&self, start: u64, end: u64) -> u64 {
        if end <= start {
            return 0;
        }
        let Some(slice) = self.bytes.slice(start..end) else {
            return 0;
        };
        u64::try_from(decode(slice, self.charset, 0).text.len()).unwrap_or(0)
    }

    /// Clamps `raw` down to the start of the character that contains it.
    #[must_use]
    pub fn char_boundary(&self, raw: RawByteOffset) -> RawByteOffset {
        if raw.0 <= self.bom_len {
            return RawByteOffset(self.bom_len);
        }
        if self.charset.is_utf8() {
            return raw;
        }
        let anchor = self.floor_anchor_raw(raw.0);
        let mut pos = anchor.raw;
        while pos < raw.0 {
            let unit = unit_len_bytes(&self.bytes, pos, self.charset);
            if pos + unit > raw.0 {
                return RawByteOffset(pos);
            }
            pos += unit;
        }
        RawByteOffset(raw.0)
    }

    fn floor_anchor_raw(&self, raw: u64) -> Anchor {
        self.ensure_raw(raw);
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let idx = state
            .anchors
            .partition_point(|anchor| anchor.raw <= raw)
            .saturating_sub(1);
        state.anchors[idx]
    }

    fn floor_anchor_utf8(&self, utf8: u64) -> Anchor {
        self.ensure_utf8(utf8);
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let idx = state
            .anchors
            .partition_point(|anchor| anchor.utf8 <= utf8)
            .saturating_sub(1);
        state.anchors[idx]
    }

    fn unit_text_len(&self, raw: u64) -> u64 {
        let unit = unit_len_bytes(&self.bytes, raw, self.charset);
        let Some(slice) = self.bytes.slice(raw..(raw + unit).min(self.bytes.len())) else {
            return 0;
        };
        u64::try_from(decode(slice, self.charset, 0).text.len()).unwrap_or(0)
    }
}

impl CoordMap for AnchoredCoordMap {
    fn raw_to_utf8(&self, raw: RawByteOffset) -> Option<Utf8Pos> {
        if raw.0 > self.bytes.len() || raw.0 < self.bom_len {
            return None;
        }
        if self.charset.is_utf8() {
            return Some(Utf8Pos(usize::try_from(raw.0 - self.bom_len).ok()?));
        }
        let boundary = self.char_boundary(raw);
        let anchor = self.floor_anchor_raw(boundary.0);
        let utf8 = anchor.utf8 + self.utf8_len_of(anchor.raw, boundary.0);
        Some(Utf8Pos(usize::try_from(utf8).ok()?))
    }

    fn utf8_to_raw(&self, pos: Utf8Pos) -> Option<RawByteOffset> {
        if self.charset.is_utf8() {
            return Some(RawByteOffset(self.bom_len + pos.0 as u64));
        }
        let target = pos.0 as u64;
        let anchor = self.floor_anchor_utf8(target);
        let mut raw = anchor.raw;
        let mut utf8 = anchor.utf8;
        while raw < self.bytes.len() && utf8 < target {
            let unit_text = self.unit_text_len(raw);
            if utf8 + unit_text > target {
                return None; // Not on a character boundary.
            }
            utf8 += unit_text;
            raw += unit_len_bytes(&self.bytes, raw, self.charset);
        }
        if utf8 == target {
            Some(RawByteOffset(raw))
        } else {
            None
        }
    }

    fn raw_to_view(&self, raw: RawByteOffset) -> Option<ViewPos> {
        let boundary = self.char_boundary(raw).0;
        let line = self.line_index.line_of(RawByteOffset(boundary))?;
        let line_start = self.line_index.line_start(line)?;
        let text = if let Some(slice) = self.bytes.slice(line_start.0..boundary) {
            decode(slice, self.charset, 0).text
        } else {
            String::new()
        };
        let col = text.width() as u32;
        Some(ViewPos { line, col })
    }
}

/// Reads the structural length of the unit at `pos` from a [`RawBytes`].
fn unit_len_bytes(bytes: &RawBytes, pos: u64, charset: Charset) -> u64 {
    let Some(slice) = bytes.slice(pos..(pos + 4).min(bytes.len())) else {
        return 1;
    };
    u64::try_from(unit_len(slice, 0, charset)).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentKind;
    use crate::encoding::BomState;

    fn map_for(bytes: Vec<u8>, charset: Charset, bom_len: u64) -> AnchoredCoordMap {
        let raw = RawBytes::from_vec(bytes);
        let line_index = Arc::new(LineIndex::new(raw.clone(), charset, bom_len));
        AnchoredCoordMap::new(raw, charset, bom_len, line_index)
    }

    #[test]
    fn utf8_mapping_is_identity_without_bom() {
        let map = map_for(b"hello".to_vec(), Charset::Utf8, 0);
        assert_eq!(map.raw_to_utf8(RawByteOffset(3)), Some(Utf8Pos(3)));
        assert_eq!(map.utf8_to_raw(Utf8Pos(3)), Some(RawByteOffset(3)));
    }

    #[test]
    fn utf8_mapping_subtracts_bom() {
        let map = map_for(b"\xEF\xBB\xBFhi".to_vec(), Charset::Utf8, 3);
        assert_eq!(map.raw_to_utf8(RawByteOffset(4)), Some(Utf8Pos(1)));
        assert_eq!(map.utf8_to_raw(Utf8Pos(1)), Some(RawByteOffset(4)));
    }

    #[test]
    fn gbk_mapping_round_trips() {
        let text = "中a文b";
        let bytes = crate::encoding::encode(text, Charset::Gbk).unwrap();
        let map = map_for(bytes, Charset::Gbk, 0);
        let mut utf8 = 0usize;
        let mut raw = 0u64;
        for ch in text.chars() {
            assert_eq!(map.raw_to_utf8(RawByteOffset(raw)), Some(Utf8Pos(utf8)));
            assert_eq!(map.utf8_to_raw(Utf8Pos(utf8)), Some(RawByteOffset(raw)));
            utf8 += ch.len_utf8();
            raw += if ch.is_ascii() { 1 } else { 2 };
        }
    }

    #[test]
    fn utf16_mapping_handles_surrogates() {
        let bytes = crate::encoding::encode("A😀B", Charset::Utf16Le).unwrap();
        let map = map_for(bytes, Charset::Utf16Le, 0);
        assert_eq!(map.raw_to_utf8(RawByteOffset(0)), Some(Utf8Pos(0)));
        assert_eq!(map.utf8_to_raw(Utf8Pos(1)), Some(RawByteOffset(2)));
        assert_eq!(map.utf8_to_raw(Utf8Pos(5)), Some(RawByteOffset(6)));
        assert_eq!(map.utf8_to_raw(Utf8Pos(6)), Some(RawByteOffset(8)));
    }

    #[test]
    fn anchors_are_built_lazily_for_large_input() {
        let bytes = vec![b'a'; 20 * 1024];
        let map = map_for(bytes, Charset::Latin1, 0);
        assert_eq!(map.raw_to_utf8(RawByteOffset(19_000)), Some(Utf8Pos(19_000)));
        let state = map.state.lock().unwrap();
        assert!(state.anchors.len() >= 4);
    }

    #[test]
    fn raw_to_view_reports_line_and_column() {
        let map = map_for(b"ab\r\ncd".to_vec(), Charset::Utf8, 0);
        assert_eq!(
            map.raw_to_view(RawByteOffset(4)),
            Some(ViewPos { line: 1, col: 0 })
        );
        assert_eq!(
            map.raw_to_view(RawByteOffset(5)),
            Some(ViewPos { line: 1, col: 1 })
        );
    }

    #[test]
    fn document_kind_is_available_for_callers() {
        assert_eq!(DocumentKind::Editable as u8, 0);
        assert_eq!(BomState::None.len(), 0);
    }
}
