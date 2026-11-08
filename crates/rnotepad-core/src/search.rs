//! Searching: literal, regular expression and hex byte patterns.
//!
//! One [`SearchService`] serves the text view, the hex view and the result
//! panel.  Searches run on a worker thread, report progress, can be cancelled
//! and deliver hits in small batches so the UI stays responsive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use memchr::memmem;
use regex::RegexBuilder;
use thiserror::Error;

use crate::bytes::ByteSource;
use crate::coord::{CoordMap, RawByteOffset, Utf8Pos};
use crate::document::{Document, DocumentId};
use crate::encoding::{encode, Charset, UnmappableChar};

/// Maximum number of hits delivered in one batch.
pub const HIT_BATCH_SIZE: usize = 200;
/// Maximum time spent accumulating a batch before flushing it.
pub const HIT_BATCH_INTERVAL: Duration = Duration::from_millis(16);
/// Raw bytes read per search step.
pub const SEARCH_WINDOW: u64 = 1 << 20;
/// Overlap between windows, so matches crossing a boundary are not lost.
pub const SEARCH_OVERLAP: u64 = 64 * 1024;

/// Search algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Literal,
    Regex,
    HexBytes,
}

/// A search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl SearchQuery {
    #[must_use]
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            mode: SearchMode::Literal,
            case_sensitive: false,
            whole_word: false,
        }
    }
}

/// One match, expressed in the raw byte space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchHit {
    pub doc_id: DocumentId,
    pub raw_offset: u64,
    pub match_len: u32,
    pub line_no: u32,
    pub col: u32,
    pub doc_revision: u64,
}

/// Events pushed to the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Progress(u64, u64),
    Hits(Vec<MatchHit>),
    Done,
    Failed(String),
}

/// Summary returned when a search finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    pub total: u64,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// Cooperative cancellation flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Search failure.
#[derive(Debug, Error)]
pub enum SearchError {
    #[error("the search pattern is empty")]
    EmptyPattern,
    #[error("invalid regular expression: {0}")]
    InvalidRegex(#[from] regex::Error),
    #[error("the pattern cannot be encoded with the document charset")]
    UnmappablePattern(Vec<UnmappableChar>),
    #[error("invalid hex byte pattern: {0}")]
    InvalidHex(String),
    #[error("the reader is unavailable")]
    ReaderUnavailable,
}

/// Interface implemented by the core search engine.
pub trait SearchService {
    fn run(
        &self,
        query: &SearchQuery,
        document: &Document,
        cancel: CancelToken,
        sink: Sender<TaskEvent>,
    ) -> Result<SearchSummary, SearchError>;
}

/// Built-in search engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreSearchService;

impl SearchService for CoreSearchService {
    fn run(
        &self,
        query: &SearchQuery,
        document: &Document,
        cancel: CancelToken,
        sink: Sender<TaskEvent>,
    ) -> Result<SearchSummary, SearchError> {
        run_search(query, document, &cancel, &sink)
    }
}

/// Runs a search and returns every hit.  Useful for tests and for small
/// documents where batching is not needed.
pub fn search_all(
    query: &SearchQuery,
    document: &Document,
    cancel: &CancelToken,
) -> Result<Vec<MatchHit>, SearchError> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let summary = run_search(query, document, cancel, &tx)?;
    drop(tx);
    let mut hits = Vec::new();
    while let Ok(event) = rx.recv() {
        if let TaskEvent::Hits(mut batch) = event {
            hits.append(&mut batch);
        }
    }
    let _ = summary;
    Ok(hits)
}

fn run_search(
    query: &SearchQuery,
    document: &Document,
    cancel: &CancelToken,
    sink: &Sender<TaskEvent>,
) -> Result<SearchSummary, SearchError> {
    let started = Instant::now();
    let mut total = 0u64;
    let mut batcher = HitBatcher::new(document.id, document.revision, sink.clone());

    if query.pattern.is_empty() {
        return Err(SearchError::EmptyPattern);
    }

    let result = if query.mode == SearchMode::HexBytes {
        search_hex(query, document, cancel, sink, &mut batcher, &mut total)
    } else if query.mode == SearchMode::Literal && query.case_sensitive && !query.whole_word {
        search_literal_raw(query, document, cancel, sink, &mut batcher, &mut total)
    } else {
        search_text(query, document, cancel, sink, &mut batcher, &mut total)
    };

    batcher.flush()?;
    result?;
    let cancelled = cancel.is_cancelled();
    let _ = sink.send(TaskEvent::Done);
    Ok(SearchSummary {
        total,
        cancelled,
        elapsed: started.elapsed(),
    })
}

struct HitBatcher {
    doc_id: DocumentId,
    revision: u64,
    sink: Sender<TaskEvent>,
    buffer: Vec<MatchHit>,
    last_flush: Instant,
}

impl HitBatcher {
    fn new(doc_id: DocumentId, revision: u64, sink: Sender<TaskEvent>) -> Self {
        Self {
            doc_id,
            revision,
            sink,
            buffer: Vec::with_capacity(HIT_BATCH_SIZE),
            last_flush: Instant::now(),
        }
    }

    fn push(&mut self, hit: MatchHit) -> Result<(), SearchError> {
        self.buffer.push(hit);
        if self.buffer.len() >= HIT_BATCH_SIZE
            || self.last_flush.elapsed() >= HIT_BATCH_INTERVAL
        {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), SearchError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.buffer);
        self.buffer = Vec::with_capacity(HIT_BATCH_SIZE);
        self.last_flush = Instant::now();
        self.sink
            .send(TaskEvent::Hits(batch))
            .map_err(|_| SearchError::ReaderUnavailable)?;
        Ok(())
    }

    fn make_hit(&self, document: &Document, raw: u64, len: u64) -> MatchHit {
        let line_no = document
            .line_index
            .line_of(RawByteOffset(raw))
            .unwrap_or(0);
        let col = document
            .coord
            .raw_to_view(RawByteOffset(raw))
            .map_or(0, |view| view.col);
        MatchHit {
            doc_id: self.doc_id,
            raw_offset: raw,
            match_len: u32::try_from(len).unwrap_or(u32::MAX),
            line_no,
            col,
            doc_revision: self.revision,
        }
    }
}

fn report_progress(sink: &Sender<TaskEvent>, done: u64, total: u64) {
    let _ = sink.send(TaskEvent::Progress(done, total));
}

fn search_literal_raw(
    query: &SearchQuery,
    document: &Document,
    cancel: &CancelToken,
    sink: &Sender<TaskEvent>,
    batcher: &mut HitBatcher,
    total: &mut u64,
) -> Result<(), SearchError> {
    let pattern = encode(&query.pattern, document.encoding.charset)
        .map_err(SearchError::UnmappablePattern)?;
    if pattern.is_empty() {
        return Err(SearchError::EmptyPattern);
    }
    let len = document.raw.len();
    let mut start = document.encoding.bom.len();
    let mut reported = 0u64;
    while start < len {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let core_end = (start + SEARCH_WINDOW).min(len);
        let end = (core_end + SEARCH_OVERLAP + pattern.len() as u64).min(len);
        let window = document.raw.slice(start..end).unwrap_or_default();
        let core_len = usize::try_from(core_end - start).unwrap_or(window.len());
        for found in memmem::find_iter(window, &pattern) {
            if found >= core_len {
                break;
            }
            let raw = start + found as u64;
            let hit = batcher.make_hit(document, raw, pattern.len() as u64);
            batcher.push(hit)?;
            *total += 1;
        }
        if core_end > reported {
            reported = core_end;
            report_progress(sink, reported, len);
        }
        if core_end >= len {
            break;
        }
        start = core_end;
    }
    Ok(())
}

fn search_hex(
    query: &SearchQuery,
    document: &Document,
    cancel: &CancelToken,
    sink: &Sender<TaskEvent>,
    batcher: &mut HitBatcher,
    total: &mut u64,
) -> Result<(), SearchError> {
    let pattern = parse_hex_pattern(&query.pattern)?;
    if pattern.is_empty() {
        return Err(SearchError::EmptyPattern);
    }
    let len = document.raw.len();
    let mut start = document.encoding.bom.len();
    let mut reported = 0u64;
    while start < len {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let core_end = (start + SEARCH_WINDOW).min(len);
        let end = (core_end + SEARCH_OVERLAP + pattern.len() as u64).min(len);
        let window = document.raw.slice(start..end).unwrap_or_default();
        let core_len = usize::try_from(core_end - start).unwrap_or(window.len());
        for found in memmem::find_iter(window, &pattern) {
            if found >= core_len {
                break;
            }
            let hit = batcher.make_hit(document, start + found as u64, pattern.len() as u64);
            batcher.push(hit)?;
            *total += 1;
        }
        if core_end > reported {
            reported = core_end;
            report_progress(sink, reported, len);
        }
        if core_end >= len {
            break;
        }
        start = core_end;
    }
    Ok(())
}

fn search_text(
    query: &SearchQuery,
    document: &Document,
    cancel: &CancelToken,
    sink: &Sender<TaskEvent>,
    batcher: &mut HitBatcher,
    total: &mut u64,
) -> Result<(), SearchError> {
    let regex = build_text_regex(query)?;
    let len = document.raw.len();
    let bom = document.encoding.bom.len();
    let mut start = bom;
    let mut reported = 0u64;

    while start < len {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let boundary = document.coord.char_boundary(RawByteOffset(start)).0;
        start = boundary;
        let core_end = (start + SEARCH_WINDOW).min(len);
        let end = (core_end + SEARCH_OVERLAP).min(len);
        let end = document.coord.char_boundary(RawByteOffset(end)).0;
        let core_raw_boundary = document.coord.char_boundary(RawByteOffset(core_end)).0;

        let Some(base_pos) = document.coord.raw_to_utf8(RawByteOffset(start)) else {
            return Err(SearchError::ReaderUnavailable);
        };
        let base_utf8 = base_pos.0 as u64;
        let Some(core_utf8_pos) = document.coord.raw_to_utf8(RawByteOffset(core_raw_boundary)) else {
            return Err(SearchError::ReaderUnavailable);
        };
        let core_utf8_len = (core_utf8_pos.0 as u64).saturating_sub(base_utf8) as usize;

        let window = document.raw.slice(start..end).unwrap_or_default();
        let text = decode_window_text(document, window, start == bom);
        for found in regex.find_iter(&text) {
            if found.start() >= core_utf8_len {
                break;
            }
            let abs_utf8 = base_utf8 + found.start() as u64;
            let Some(raw_start) = document.coord.utf8_to_raw(Utf8Pos(abs_utf8 as usize)) else {
                continue; // Not on a character boundary.
            };
            let Some(raw_end) =
                document
                    .coord
                    .utf8_to_raw(Utf8Pos((abs_utf8 + found.len() as u64) as usize))
            else {
                continue;
            };
            let match_len = raw_end.0.saturating_sub(raw_start.0);
            let hit = batcher.make_hit(document, raw_start.0, match_len);
            batcher.push(hit)?;
            *total += 1;
        }

        if core_end > reported {
            reported = core_end;
            report_progress(sink, reported, len);
        }
        if core_end >= len {
            break;
        }
        start = core_raw_boundary;
    }
    Ok(())
}

fn decode_window_text(document: &Document, window: &[u8], skip_bom: bool) -> String {
    let charset = document.encoding.charset;
    let data = if skip_bom && charset.is_utf8() {
        window
    } else {
        window
    };
    crate::encoding::decode(data, charset, 0).text
}

fn build_text_regex(query: &SearchQuery) -> Result<regex::Regex, SearchError> {
    let pattern = if query.mode == SearchMode::Literal {
        regex::escape(&query.pattern)
    } else {
        query.pattern.clone()
    };
    let pattern = if query.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    let mut builder = RegexBuilder::new(&pattern);
    builder.case_insensitive(!query.case_sensitive);
    builder.size_limit(4 * 1024 * 1024);
    builder.dfa_size_limit(4 * 1024 * 1024);
    Ok(builder.build()?)
}

/// Parses `DE AD BE EF`, `\xDE\xAD` or `DEADBEEF`.
pub fn parse_hex_pattern(pattern: &str) -> Result<Vec<u8>, SearchError> {
    let mut cleaned = String::with_capacity(pattern.len());
    for ch in pattern.chars() {
        if ch.is_ascii_hexdigit() {
            cleaned.push(ch);
        } else if ch.is_whitespace() || ch == ',' || ch == '_' {
            continue;
        } else if ch == '\\' || ch == 'x' || ch == 'X' || ch == '0' {
            continue;
        } else {
            return Err(SearchError::InvalidHex(format!("unexpected character '{ch}'")));
        }
    }
    if cleaned.is_empty() {
        return Err(SearchError::EmptyPattern);
    }
    if cleaned.len() % 2 != 0 {
        return Err(SearchError::InvalidHex(
            "hex patterns must contain an even number of digits".to_owned(),
        ));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_value(chunk[0]);
        let lo = hex_value(chunk[1]);
        match (hi, lo) {
            (Some(hi), Some(lo)) => out.push((hi << 4) | lo),
            _ => return Err(SearchError::InvalidHex("invalid hex digit".to_owned())),
        }
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Encodes a literal query for the charset of `document`, used by the hex
/// view's "search text literal" action.
pub fn encode_pattern_for(document: &Document, pattern: &str) -> Result<Vec<u8>, SearchError> {
    if document.encoding.charset == Charset::Utf8 {
        Ok(pattern.as_bytes().to_vec())
    } else {
        encode(pattern, document.encoding.charset).map_err(SearchError::UnmappablePattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bytes::RawBytes, document::Document};

    fn doc(text: &[u8]) -> Document {
        Document::from_bytes(7, text.to_vec())
    }

    #[test]
    fn literal_search_finds_all_occurrences() {
        let doc = doc(b"alpha beta alpha\nALPHA");
        let query = SearchQuery {
            pattern: "alpha".to_owned(),
            mode: SearchMode::Literal,
            case_sensitive: true,
            whole_word: false,
        };
        let hits = search_all(&query, &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].raw_offset, 0);
        assert_eq!(hits[1].raw_offset, 11);
    }

    #[test]
    fn case_insensitive_search_matches_both_cases() {
        let doc = doc(b"alpha ALPHA");
        let query = SearchQuery::literal("alpha");
        let hits = search_all(&query, &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn whole_word_search_rejects_substrings() {
        let doc = doc(b"alphabet alpha");
        let query = SearchQuery {
            pattern: "alpha".to_owned(),
            mode: SearchMode::Literal,
            case_sensitive: true,
            whole_word: true,
        };
        let hits = search_all(&query, &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].raw_offset, 9);
    }

    #[test]
    fn hex_search_finds_bytes() {
        let doc = doc(&[0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x00]);
        let query = SearchQuery {
            pattern: "DE AD BE EF".to_owned(),
            mode: SearchMode::HexBytes,
            case_sensitive: true,
            whole_word: false,
        };
        let hits = search_all(&query, &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].raw_offset, 1);
        assert_eq!(hits[0].match_len, 4);
    }

    #[test]
    fn hex_pattern_forms_are_equivalent() {
        assert_eq!(parse_hex_pattern("DE AD").unwrap(), vec![0xDE, 0xAD]);
        assert_eq!(parse_hex_pattern(r"\xDE\xAD").unwrap(), vec![0xDE, 0xAD]);
        assert_eq!(parse_hex_pattern("DEAD").unwrap(), vec![0xDE, 0xAD]);
    }

    #[test]
    fn regex_search_reports_line_and_column() {
        let doc = doc(b"one\ntwo 42\nthree");
        let query = SearchQuery {
            pattern: r"\d+".to_owned(),
            mode: SearchMode::Regex,
            case_sensitive: true,
            whole_word: false,
        };
        let hits = search_all(&query, &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 1);
        assert_eq!(hits[0].col, 4);
        assert_eq!(hits[0].raw_offset, 8);
    }

    #[test]
    fn gbk_literal_search_uses_encoded_bytes() {
        let bytes = crate::encoding::encode("中文日志 中文", Charset::Gbk).unwrap();
        let mut doc = Document::from_bytes(8, bytes);
        doc.encoding.charset = Charset::Gbk;
        let hits = search_all(&SearchQuery::literal("中文"), &doc, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn cancellation_stops_the_scan() {
        let doc = doc(b"needle needle needle");
        let token = CancelToken::new();
        token.cancel();
        let hits = search_all(&SearchQuery::literal("needle"), &doc, &token).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn empty_pattern_is_rejected() {
        let doc = doc(b"abc");
        assert!(matches!(
            search_all(&SearchQuery::literal(""), &doc, &CancelToken::new()),
            Err(SearchError::EmptyPattern)
        ));
    }

    #[test]
    fn raw_bytes_clone_shares_storage() {
        let raw = RawBytes::from_vec(vec![1, 2, 3]);
        let clone = raw.clone();
        assert_eq!(clone.len(), 3);
    }
}
