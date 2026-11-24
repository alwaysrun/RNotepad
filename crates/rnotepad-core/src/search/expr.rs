//! Line-scoped evaluation of a compiled search expression.
//!
//! Each logical line is decoded once, every leaf is matched inside that line
//! and the boolean tree is folded into a single result.  Nothing wider than
//! one line is retained, so a multi-gigabyte file never turns into an
//! in-memory hit table.

use crossbeam_channel::Sender;
use memchr::memmem;
use regex::Regex;

use crate::bytes::ByteSource;
use crate::coord::{CoordMap, RawByteOffset, Utf8Pos};
use crate::document::Document;
use crate::encoding::encode;

use super::query::{CompiledQuery, QueryNode};
use super::{
    build_text_regex, parse_hex_pattern, report_progress, CancelToken, SearchError, SearchMode,
    SearchQuery, TaskEvent, SEARCH_WINDOW,
};

/// How one leaf is matched against a single line.
enum LeafMatcher {
    /// Literal or hex bytes, matched directly in the raw line bytes.
    Bytes(Vec<u8>),
    /// Case-insensitive or regular-expression text, matched on decoded UTF-8.
    Text(Regex),
}

/// Boolean tree whose leaves are indices into the compiled matcher list.
enum PlanNode {
    Leaf(usize),
    And(Vec<PlanNode>),
    Or(Vec<PlanNode>),
    Not(Box<PlanNode>),
}

/// Result of evaluating one line.
enum LineResult {
    NoMatch,
    /// The line matched; the span covers the positive matches, or is absent
    /// when only negated terms decided the result.
    Match(Option<(u64, u64)>),
}

/// Runs a compiled expression, emitting one hit per matching line.
///
/// `emit` receives the raw start offset and the byte length of the span that
/// covers the line's positive matches.
pub(crate) fn run(
    query: &CompiledQuery,
    document: &Document,
    cancel: &CancelToken,
    sink: &Sender<TaskEvent>,
    emit: &mut dyn FnMut(u64, u64) -> Result<(), SearchError>,
) -> Result<(), SearchError> {
    let mut leaves = Vec::new();
    let plan = compile(&query.root, document, &mut leaves)?;
    let total = document.raw.len();
    let mut reported = 0u64;
    let mut line = 0u32;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let Some(range) = document.line_index.line_range(line) else {
            break;
        };
        let line_bytes = document.raw.slice(range.start..range.end).unwrap_or_default();
        let spans = collect_spans(&leaves, document, range.start, line_bytes);
        match evaluate(&plan, &spans) {
            LineResult::NoMatch => {}
            LineResult::Match(Some((start, end))) => {
                emit(start, end.saturating_sub(start))?;
            }
            // A purely negative result still points at the line it matched.
            LineResult::Match(None) => {
                emit(range.start, range.end.saturating_sub(range.start))?;
            }
        }
        if range.end.saturating_sub(reported) >= SEARCH_WINDOW {
            reported = range.end;
            report_progress(sink, reported, total);
        }
        if range.end >= total {
            break;
        }
        line = line.saturating_add(1);
    }
    report_progress(sink, total, total);
    Ok(())
}

fn compile(
    node: &QueryNode,
    document: &Document,
    leaves: &mut Vec<LeafMatcher>,
) -> Result<PlanNode, SearchError> {
    Ok(match node {
        QueryNode::Term(term) => {
            let index = leaves.len();
            leaves.push(compile_leaf(term, document)?);
            PlanNode::Leaf(index)
        }
        QueryNode::And(children) => PlanNode::And(
            children
                .iter()
                .map(|child| compile(child, document, leaves))
                .collect::<Result<_, _>>()?,
        ),
        QueryNode::Or(children) => PlanNode::Or(
            children
                .iter()
                .map(|child| compile(child, document, leaves))
                .collect::<Result<_, _>>()?,
        ),
        QueryNode::Not(inner) => PlanNode::Not(Box::new(compile(inner, document, leaves)?)),
    })
}

fn compile_leaf(term: &SearchQuery, document: &Document) -> Result<LeafMatcher, SearchError> {
    match term.mode {
        SearchMode::HexBytes => Ok(LeafMatcher::Bytes(parse_hex_pattern(&term.pattern)?)),
        SearchMode::Literal if term.case_sensitive && !term.whole_word => {
            let bytes = encode(&term.pattern, document.encoding.charset)
                .map_err(SearchError::UnmappablePattern)?;
            Ok(LeafMatcher::Bytes(bytes))
        }
        _ => Ok(LeafMatcher::Text(build_text_regex(term)?)),
    }
}

fn collect_spans(
    leaves: &[LeafMatcher],
    document: &Document,
    line_start: u64,
    line_bytes: &[u8],
) -> Vec<Vec<(u64, u64)>> {
    let mut spans = Vec::with_capacity(leaves.len());
    let mut decoded: Option<String> = None;
    for leaf in leaves {
        match leaf {
            LeafMatcher::Bytes(pattern) => {
                let end = line_start + pattern.len() as u64;
                spans.push(
                    memmem::find_iter(line_bytes, pattern)
                        .map(|offset| (line_start + offset as u64, end + offset as u64))
                        .collect(),
                );
            }
            LeafMatcher::Text(regex) => {
                let text = decoded.get_or_insert_with(|| {
                    crate::encoding::decode(line_bytes, document.encoding.charset, 0).text
                });
                // Expressions are line scoped, so `^` and `$` must anchor on
                // the logical line rather than on its terminating newline.
                let line_text = text.trim_end_matches(['\r', '\n']);
                let found = match document.coord.raw_to_utf8(RawByteOffset(line_start)) {
                    Some(base) => match_spans(regex, document, base, line_text),
                    None => Vec::new(),
                };
                spans.push(found);
            }
        }
    }
    spans
}

fn match_spans(
    regex: &Regex,
    document: &Document,
    base: Utf8Pos,
    text: &str,
) -> Vec<(u64, u64)> {
    let mut spans = Vec::new();
    for found in regex.find_iter(text) {
        let Some(start) = document.coord.utf8_to_raw(Utf8Pos(base.0 + found.start())) else {
            continue;
        };
        let Some(end) = document.coord.utf8_to_raw(Utf8Pos(base.0 + found.end())) else {
            continue;
        };
        spans.push((start.0, end.0));
    }
    spans
}

fn evaluate(node: &PlanNode, spans: &[Vec<(u64, u64)>]) -> LineResult {
    match node {
        PlanNode::Leaf(index) => match spans.get(*index) {
            Some(found) if !found.is_empty() => {
                let start = found.iter().map(|span| span.0).min().unwrap_or(0);
                let end = found.iter().map(|span| span.1).max().unwrap_or(start);
                LineResult::Match(Some((start, end)))
            }
            _ => LineResult::NoMatch,
        },
        PlanNode::And(children) => {
            let mut union = None;
            for child in children {
                match evaluate(child, spans) {
                    LineResult::NoMatch => return LineResult::NoMatch,
                    LineResult::Match(Some(span)) => merge(&mut union, span),
                    LineResult::Match(None) => {}
                }
            }
            LineResult::Match(union)
        }
        PlanNode::Or(children) => {
            let mut union = None;
            let mut matched = false;
            for child in children {
                if let LineResult::Match(span) = evaluate(child, spans) {
                    matched = true;
                    if let Some(span) = span {
                        merge(&mut union, span);
                    }
                }
            }
            if matched {
                LineResult::Match(union)
            } else {
                LineResult::NoMatch
            }
        }
        PlanNode::Not(inner) => match evaluate(inner, spans) {
            LineResult::NoMatch => LineResult::Match(None),
            LineResult::Match(_) => LineResult::NoMatch,
        },
    }
}

fn merge(union: &mut Option<(u64, u64)>, span: (u64, u64)) {
    *union = Some(match *union {
        Some((start, end)) => (start.min(span.0), end.max(span.1)),
        None => span,
    });
}
