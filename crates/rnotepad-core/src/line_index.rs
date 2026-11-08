//! Two-level line index.
//!
//! The index stores one anchor per 64 KB scanned chunk and keeps the line
//! starts of recently visited chunks in a small LRU cache.  Building the
//! index is incremental: [`LineIndex::scan_step`] can be called from a
//! background thread while the UI queries the already-scanned prefix.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use crate::bytes::{read_window, ByteSource, RawBytes};
use crate::coord::RawByteOffset;
use crate::encoding::Charset;

/// Size of one indexed chunk.
pub const CHUNK_SIZE: u64 = 64 * 1024;
/// Bytes scanned synchronously when a document is opened, so the first screen
/// can be rendered without waiting for the full index.
pub const PRIME_BYTES: u64 = 1 << 20;
/// Approximate memory budget of the chunk line cache.
pub const CHUNK_CACHE_BUDGET: usize = 32 * 1024 * 1024;

struct AnchorRec {
    raw: u64,
    line_no: u32,
    line_start: u64,
}

struct ChunkLines {
    /// Absolute offsets of lines starting after the anchor line, inside the
    /// scanned window.
    starts: Vec<u64>,
    end_raw: u64,
    end_line_no: u32,
    end_line_start: u64,
}

struct IndexState {
    anchors: Vec<AnchorRec>,
    complete: bool,
    total_lines: Option<u32>,
    cache: ChunkCache,
}

struct ChunkCache {
    map: HashMap<u64, Arc<ChunkLines>>,
    order: VecDeque<u64>,
    bytes: usize,
}

impl ChunkCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }

    fn get(&mut self, key: u64) -> Option<Arc<ChunkLines>> {
        if let Some(value) = self.map.get(&key).cloned() {
            self.touch(key);
            Some(value)
        } else {
            None
        }
    }

    fn insert(&mut self, key: u64, value: Arc<ChunkLines>) {
        let size = std::mem::size_of::<ChunkLines>() + value.starts.len() * 8;
        if self.map.insert(key, value).is_none() {
            self.order.push_back(key);
            self.bytes += size;
        }
        while self.bytes > CHUNK_CACHE_BUDGET && self.order.len() > 1 {
            if let Some(old) = self.order.pop_front() {
                if let Some(removed) = self.map.remove(&old) {
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<ChunkLines>() + removed.starts.len() * 8);
                }
            }
        }
    }

    fn touch(&mut self, key: u64) {
        if let Some(pos) = self.order.iter().position(|entry| *entry == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
    }
}

/// Line index shared by all views of one document.
pub struct LineIndex {
    bytes: RawBytes,
    charset: Charset,
    bom_len: u64,
    state: Mutex<IndexState>,
}

impl LineIndex {
    /// Creates an empty index over `bytes`.  Call [`Self::scan_step`] or
    /// [`Self::build_all`] to populate it.
    #[must_use]
    pub fn new(bytes: RawBytes, charset: Charset, bom_len: u64) -> Self {
        let end = bytes.len();
        let mut anchors = vec![AnchorRec {
            raw: bom_len,
            line_no: 0,
            line_start: bom_len,
        }];
        let complete = bom_len >= end;
        if complete {
            anchors[0].raw = end;
            anchors[0].line_start = end;
        }
        Self {
            bytes,
            charset,
            bom_len,
            state: Mutex::new(IndexState {
                anchors,
                complete,
                total_lines: if complete { Some(1) } else { None },
                cache: ChunkCache::new(),
            }),
        }
    }

    /// Charset the index was built for.
    #[must_use]
    pub const fn charset(&self) -> Charset {
        self.charset
    }

    /// `true` once the whole byte stream has been scanned.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .complete
    }

    /// Number of line starts known to the index so far.  Useful while a full
    /// scan is still running: callers may display lines below this count.
    #[must_use]
    pub fn scanned_line_hint(&self) -> u32 {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .anchors
            .last()
            .map(|anchor| anchor.line_no.saturating_add(1))
            .unwrap_or(1)
    }

    /// Number of logical lines, available only after a complete scan.
    #[must_use]
    pub fn total_lines(&self) -> Option<u32> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .total_lines
    }

    /// Scans at most `max_bytes`, returning `true` when the scan finished.
    pub fn scan_step(&self, max_bytes: u64) -> bool {
        let mut budget = max_bytes;
        loop {
            let (complete, next) = {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.complete {
                    return true;
                }
                let last = state
                    .anchors
                    .last()
                    .expect("anchor list is never empty");
                (state.complete, (last.raw, last.line_no, last.line_start))
            };
            if complete || budget == 0 {
                return complete;
            }
            let window = CHUNK_SIZE.min(budget.max(MIN_WINDOW_BYTES));
            let window_end = (next.0 + window).min(self.bytes.len());
            let scanned = self.scan_chunk(next.0, next.1, next.2, window);
            let end = scanned.end_raw;
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.anchors.push(AnchorRec {
                raw: end,
                line_no: scanned.end_line_no,
                line_start: scanned.end_line_start,
            });
            if window_end >= self.bytes.len() {
                // The window reached the end of the file.  A trailing partial
                // UTF-16 unit is ignored, matching the decoder.
                state.complete = true;
                state.total_lines = Some(scanned.end_line_no.saturating_add(1));
            }
            drop(state);
            budget = budget.saturating_sub(end.saturating_sub(next.0));
            if end == next.0 {
                // Defensive: the scanner could not advance; stop instead of
                // spinning forever.
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.complete = true;
                return true;
            }
            if self.is_complete() {
                return true;
            }
        }
    }

    /// Scans the whole byte stream synchronously.
    pub fn build_all(&self) {
        while !self.scan_step(CHUNK_SIZE * 16) {}
    }

    /// Returns the first raw offset of `line`.
    #[must_use]
    pub fn line_start(&self, line: u32) -> Option<RawByteOffset> {
        let (idx, anchor) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                let last = state.anchors.last().expect("anchor list is never empty");
                if state.complete || last.line_no >= line {
                    break;
                }
                drop(state);
                self.scan_step(CHUNK_SIZE);
                state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let idx = state
                .anchors
                .partition_point(|anchor| anchor.line_no <= line)
                .saturating_sub(1);
            let anchor = &state.anchors[idx];
            (
                idx,
                (anchor.raw, anchor.line_no, anchor.line_start),
            )
        };
        if line == anchor.1 {
            return Some(RawByteOffset(anchor.2));
        }
        let lines = self.chunk_lines(anchor.0, anchor.1, anchor.2);
        let offset = line.saturating_sub(anchor.1).saturating_sub(1) as usize;
        if let Some(start) = lines.starts.get(offset) {
            return Some(RawByteOffset(*start));
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(next) = state.anchors.get(idx + 1) {
            if next.line_no == line {
                return Some(RawByteOffset(next.line_start));
            }
        }
        None
    }

    /// Returns the logical line containing `off`.
    #[must_use]
    pub fn line_of(&self, off: RawByteOffset) -> Option<u32> {
        if off.0 < self.bom_len || off.0 > self.bytes.len() {
            return None;
        }
        let (raw, line_no, line_start) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                let last = state.anchors.last().expect("anchor list is never empty");
                if state.complete || last.raw > off.0 {
                    break;
                }
                drop(state);
                self.scan_step(CHUNK_SIZE);
                state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let idx = state
                .anchors
                .partition_point(|anchor| anchor.raw <= off.0)
                .saturating_sub(1);
            let anchor = &state.anchors[idx];
            (anchor.raw, anchor.line_no, anchor.line_start)
        };
        if raw == off.0 {
            return Some(line_no);
        }
        let lines = self.chunk_lines(raw, line_no, line_start);
        let started = lines.starts.partition_point(|start| *start <= off.0);
        Some(line_no.saturating_add(started as u32))
    }

    /// Raw byte range covered by `line`, including its newline sequence.
    #[must_use]
    pub fn line_range(&self, line: u32) -> Option<Range<u64>> {
        let start = self.line_start(line)?.0;
        let end = match self.line_start(line.saturating_add(1)) {
            Some(next) => next.0,
            None => self.bytes.len(),
        };
        if start > end {
            return None;
        }
        Some(start..end)
    }

    /// Drops index state at or after `off`, then rescans from there.
    ///
    /// Used when a log file grows: appending only requires scanning the new
    /// tail instead of rebuilding the whole index.
    pub fn append_from(&self, off: RawByteOffset) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let idx = state
            .anchors
            .partition_point(|anchor| anchor.raw <= off.0)
            .saturating_sub(1);
        state.anchors.truncate(idx + 1);
        state.complete = false;
        state.total_lines = None;
        state.cache = ChunkCache::new();
    }

    fn chunk_lines(&self, raw: u64, line_no: u32, line_start: u64) -> Arc<ChunkLines> {
        if let Some(cached) = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cache
            .get(raw)
        {
            return cached;
        }
        let scanned = Arc::new(self.scan_chunk(raw, line_no, line_start, CHUNK_SIZE));
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cache
            .insert(raw, scanned.clone());
        scanned
    }

    fn scan_chunk(
        &self,
        start: u64,
        first_line_no: u32,
        first_line_start: u64,
        max_bytes: u64,
    ) -> ChunkLines {
        let len = self.bytes.len();
        let end = (start + max_bytes).min(len);
        let mut window = Vec::new();
        if end > start {
            // `read_window` never fails for a valid in-memory source; an
            // unreadable tail is treated as an empty window.
            let _ = read_window(&self.bytes, start..end, &mut window);
        }
        let mut line_no = first_line_no;
        let mut line_start = first_line_start;
        let mut starts = Vec::new();
        let mut i = 0usize;
        let limit = window.len();
        let has_more = (start + limit as u64) < len;

        if self.charset.is_utf16() {
            let little = self.charset == Charset::Utf16Le;
            while i + 1 < limit {
                let unit = if little {
                    u16::from_le_bytes([window[i], window[i + 1]])
                } else {
                    u16::from_be_bytes([window[i], window[i + 1]])
                };
                if unit == 0x000D {
                    if i + 3 < limit && read_u16(&window, i + 2, little) == 0x000A {
                        advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 4);
                        i += 4;
                        continue;
                    }
                    if i + 2 >= limit && has_more {
                        break; // CR at window edge; scan it next round.
                    }
                    advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 2);
                    i += 2;
                    continue;
                }
                if unit == 0x000A {
                    advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 2);
                    i += 2;
                    continue;
                }
                i += 2;
            }
        } else {
            while i < limit {
                match window[i] {
                    0x0D => {
                        if i + 1 < limit && window[i + 1] == 0x0A {
                            advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 2);
                            i += 2;
                            continue;
                        }
                        if i + 1 >= limit && has_more {
                            break; // CR at window edge; scan it next round.
                        }
                        advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 1);
                        i += 1;
                        continue;
                    }
                    0x0A => {
                        advance_line(&mut line_no, &mut line_start, &mut starts, start + i as u64 + 1);
                        i += 1;
                        continue;
                    }
                    _ => i += 1,
                }
            }
        }

        ChunkLines {
            starts,
            end_raw: start + i as u64,
            end_line_no: line_no,
            end_line_start: line_start,
        }
    }
}

fn advance_line(
    line_no: &mut u32,
    line_start: &mut u64,
    starts: &mut Vec<u64>,
    next_start: u64,
) {
    *line_no = line_no.saturating_add(1);
    *line_start = next_start;
    starts.push(next_start);
}

fn read_u16(window: &[u8], pos: usize, little_endian: bool) -> u16 {
    let raw = [window[pos], window[pos + 1]];
    if little_endian {
        u16::from_le_bytes(raw)
    } else {
        u16::from_be_bytes(raw)
    }
}

const MIN_WINDOW_BYTES: u64 = 8;

/// Reads a line-scoped prefix of `raw`, trimming incomplete trailing units.
#[must_use]
pub fn trim_to_unit(raw: &[u8], charset: Charset) -> &[u8] {
    if charset.is_utf16() && raw.len() % 2 != 0 {
        &raw[..raw.len() - 1]
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::encode;

    fn index_for(bytes: Vec<u8>, charset: Charset) -> LineIndex {
        LineIndex::new(RawBytes::from_vec(bytes), charset, 0)
    }

    #[test]
    fn indexes_utf8_lines() {
        let index = index_for(b"one\ntwo\r\nthree\rfour".to_vec(), Charset::Utf8);
        index.build_all();
        assert_eq!(index.total_lines(), Some(4));
        assert_eq!(index.line_start(0).map(|o| o.0), Some(0));
        assert_eq!(index.line_start(1).map(|o| o.0), Some(4));
        assert_eq!(index.line_start(2).map(|o| o.0), Some(9));
        assert_eq!(index.line_start(3).map(|o| o.0), Some(15));
        assert_eq!(index.line_start(4), None);
    }

    #[test]
    fn line_of_finds_containing_line() {
        let index = index_for(b"abc\ndef".to_vec(), Charset::Utf8);
        index.build_all();
        assert_eq!(index.line_of(RawByteOffset(0)), Some(0));
        assert_eq!(index.line_of(RawByteOffset(3)), Some(0));
        assert_eq!(index.line_of(RawByteOffset(4)), Some(1));
        assert_eq!(index.line_of(RawByteOffset(7)), Some(1));
    }

    #[test]
    fn crlf_split_across_chunks_is_not_double_counted() {
        let mut bytes = vec![b'a'; CHUNK_SIZE as usize - 1];
        bytes.push(0x0D);
        bytes.push(0x0A);
        bytes.push(b'b');
        let index = index_for(bytes, Charset::Utf8);
        index.build_all();
        assert_eq!(index.total_lines(), Some(2));
        assert_eq!(
            index.line_start(1).map(|o| o.0),
            Some(CHUNK_SIZE + 1)
        );
    }

    #[test]
    fn utf16_lines_use_code_units() {
        let text = "a\r\nb\nc";
        let bytes = encode(text, Charset::Utf16Le).unwrap();
        let index = index_for(bytes, Charset::Utf16Le);
        index.build_all();
        assert_eq!(index.total_lines(), Some(3));
        assert_eq!(index.line_start(1).map(|o| o.0), Some(6));
        assert_eq!(index.line_start(2).map(|o| o.0), Some(10));
    }

    #[test]
    fn append_from_resumes_scanning() {
        let index = index_for(b"a\nb".to_vec(), Charset::Utf8);
        index.build_all();
        assert_eq!(index.total_lines(), Some(2));
        index.append_from(RawByteOffset(3));
        index.build_all();
        assert_eq!(index.total_lines(), Some(2));
    }
}
