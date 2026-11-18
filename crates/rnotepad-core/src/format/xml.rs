//! XML configuration provider.
//!
//! Formatting forwards raw byte slices instead of re-serialising events, so
//! attribute order, quote style, entity spelling and element form (`<a/>` vs
//! `<a></a>`) survive byte for byte.  A pre-scan decides which elements must
//! stay on one line and which subtrees are verbatim; the output pass only
//! inserts indentation between elements.
//!
//! The provider is stateless and built on demand, so an unused XML feature
//! costs nothing at runtime.

use std::collections::{HashMap, HashSet};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{
    display_column, Diagnostic, FormatKind, FormatOptions, FormatOutput, FormatProvider, SourceMap,
    MAX_INDENT,
};
use crate::coord::{Span, Utf8Pos};

/// Attribute that controls whitespace handling inside a subtree.
const PRESERVE_SPACE_ATTRIBUTE: &[u8] = b"xml:space";
/// Attribute value that keeps a subtree verbatim.
const PRESERVE_SPACE_VALUE: &str = "preserve";

/// XML provider for configuration documents (pom.xml, web.config, ...).
///
/// The policy is fixed to "safe for configuration files": no entity expansion,
/// no DTD handling and no attribute rewriting.  [`FormatOptions::lenient`] is
/// ignored because XML has no comment/trailing-comma relaxation.
#[derive(Debug, Clone, Copy, Default)]
pub struct XmlConfigProvider;

impl FormatProvider for XmlConfigProvider {
    fn kind(&self) -> FormatKind {
        FormatKind::Xml
    }

    fn validate(&self, text: &str) -> Result<(), Vec<Diagnostic>> {
        scan(text).map(|_layout| ())
    }

    fn format(&self, text: &str, options: &FormatOptions) -> Result<FormatOutput, Vec<Diagnostic>> {
        let layout = scan(text)?;
        let mut out = String::with_capacity(text.len() + text.len() / 8);
        render(
            text,
            &layout,
            options.indent.min(MAX_INDENT),
            true,
            &mut out,
        )?;
        Ok(FormatOutput {
            text: out,
            source_map: SourceMap::default(),
        })
    }

    fn minify(&self, text: &str) -> Result<FormatOutput, Vec<Diagnostic>> {
        let layout = scan(text)?;
        let mut out = String::with_capacity(text.len());
        render(text, &layout, 0, false, &mut out)?;
        Ok(FormatOutput {
            text: out,
            source_map: SourceMap::default(),
        })
    }
}

/// Per-element facts gathered by the pre-scan.
#[derive(Debug, Default)]
struct ElementInfo {
    /// Offset of the start tag.
    start: usize,
    /// The element has at least one non-blank text child.
    has_text: bool,
    /// The element has at least one CDATA child.
    has_cdata: bool,
    /// The element has at least one element, comment or processing child.
    has_structural_child: bool,
    /// Number of whitespace-only text children.
    whitespace_texts: usize,
    /// `xml:space="preserve"` is set on this element.
    preserve_space: bool,
}

/// Pre-scan result consulted by the output pass.
#[derive(Debug, Default)]
struct Layout {
    /// Start-tag offsets whose children must stay on the current line.
    inline: HashSet<usize>,
    /// Start-tag offset -> offset of the matching end tag, whose content must
    /// be forwarded verbatim.
    verbatim: HashMap<usize, usize>,
}

/// One open element during the output pass.
#[derive(Debug)]
struct RenderFrame {
    /// The element keeps all children on one line.
    inline: bool,
    /// Content up to this offset was forwarded verbatim and events must skip.
    verbatim_until: Option<usize>,
    /// Number of child events, used to keep `<a></a>` on one line.
    children: usize,
}

/// Pre-scan that records inline elements and verbatim subtrees.
fn scan(text: &str) -> Result<Layout, Vec<Diagnostic>> {
    let mut reader = new_xml_reader(text);
    let mut stack: Vec<ElementInfo> = Vec::new();
    let mut layout = Layout::default();
    loop {
        let start = reader_position(&reader);
        let event = reader
            .read_event()
            .map_err(|err| vec![diagnostic_from_error(text, &reader, &err)])?;
        let end = reader_position(&reader);
        match &event {
            Event::Start(tag) => {
                mark_structural_child(&mut stack);
                let preserve_space = has_preserve_space(tag)
                    .map_err(|err| vec![diagnostic_from_error(text, &reader, &err)])?;
                stack.push(ElementInfo {
                    start,
                    preserve_space,
                    ..ElementInfo::default()
                });
            }
            Event::Empty(_) => mark_structural_child(&mut stack),
            Event::End(_) => {
                let Some(element) = stack.pop() else {
                    return Err(vec![unexpected_end_diagnostic(text, start)]);
                };
                if element.has_text || element.has_cdata {
                    layout.inline.insert(element.start);
                }
                let sole_whitespace = element.whitespace_texts == 1
                    && !element.has_text
                    && !element.has_cdata
                    && !element.has_structural_child;
                if element.preserve_space || sole_whitespace {
                    layout.verbatim.insert(element.start, start);
                }
            }
            Event::Text(_) => {
                if let Some(element) = stack.last_mut() {
                    if raw_slice(text, start, end).trim().is_empty() {
                        element.whitespace_texts += 1;
                    } else {
                        element.has_text = true;
                    }
                }
            }
            Event::CData(_) | Event::GeneralRef(_) => {
                if let Some(element) = stack.last_mut() {
                    element.has_cdata = true;
                }
            }
            Event::Comment(_) | Event::Decl(_) | Event::DocType(_) | Event::PI(_) => {
                mark_structural_child(&mut stack);
            }
            Event::Eof => break,
        }
    }
    if let Some(element) = stack.last() {
        return Err(vec![unclosed_element_diagnostic(text, element.start)]);
    }
    Ok(layout)
}

/// Output pass that inserts indentation but never rebuilds markup.
fn render(
    text: &str,
    layout: &Layout,
    indent_width: usize,
    pretty: bool,
    out: &mut String,
) -> Result<(), Vec<Diagnostic>> {
    let mut reader = new_xml_reader(text);
    let indent = " ".repeat(indent_width);
    let mut frames: Vec<RenderFrame> = Vec::new();
    let mut skip_until: Option<usize> = None;
    loop {
        let start = reader_position(&reader);
        let event = reader
            .read_event()
            .map_err(|err| vec![diagnostic_from_error(text, &reader, &err)])?;
        let end = reader_position(&reader);
        if let Some(limit) = skip_until {
            if start < limit {
                continue;
            }
            skip_until = None;
        }
        let raw = raw_slice(text, start, end);
        let verbatim = frames
            .iter()
            .any(|frame| frame.inline || frame.verbatim_until.is_some());
        match &event {
            Event::Start(_) => {
                count_child(&mut frames);
                if verbatim {
                    out.push_str(raw);
                } else {
                    write_line_break(out, &indent, frames.len(), pretty);
                    out.push_str(raw);
                }
                let verbatim_until = layout.verbatim.get(&start).copied();
                if let Some(limit) = verbatim_until {
                    out.push_str(raw_slice(text, end, limit));
                    skip_until = Some(limit);
                }
                frames.push(RenderFrame {
                    inline: layout.inline.contains(&start),
                    verbatim_until,
                    children: 0,
                });
            }
            Event::Empty(_) => {
                count_child(&mut frames);
                if !verbatim {
                    write_line_break(out, &indent, frames.len(), pretty);
                }
                out.push_str(raw);
            }
            Event::End(_) => {
                let Some(frame) = frames.pop() else {
                    return Err(vec![unexpected_end_diagnostic(text, start)]);
                };
                let parent_verbatim = frames
                    .iter()
                    .any(|parent| parent.inline || parent.verbatim_until.is_some());
                let on_own_line = !frame.inline
                    && frame.verbatim_until.is_none()
                    && !parent_verbatim
                    && frame.children > 0;
                if on_own_line {
                    write_line_break(out, &indent, frames.len(), pretty);
                }
                out.push_str(raw);
            }
            Event::Text(_) => {
                count_child(&mut frames);
                // Whitespace-only nodes are indentation: they are dropped and
                // the output pass inserts its own line breaks.  Elements where
                // such a node is content were captured as verbatim ranges.
                if !raw.trim().is_empty() {
                    out.push_str(raw);
                }
            }
            Event::CData(_) | Event::GeneralRef(_) => {
                count_child(&mut frames);
                out.push_str(raw);
            }
            Event::Comment(_) | Event::Decl(_) | Event::DocType(_) | Event::PI(_) => {
                count_child(&mut frames);
                if verbatim {
                    out.push_str(raw);
                } else {
                    write_line_break(out, &indent, frames.len(), pretty);
                    out.push_str(raw);
                }
            }
            Event::Eof => break,
        }
    }
    Ok(())
}

/// Creates a reader with the well-formedness checks this provider relies on.
fn new_xml_reader(text: &str) -> Reader<&[u8]> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().enable_all_checks(true);
    reader.config_mut().trim_text(false);
    reader
}

/// Byte offset of the parser inside the input.
fn reader_position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

/// Source slice for one event, empty when the offsets are out of range.
fn raw_slice(text: &str, start: usize, end: usize) -> &str {
    text.get(start..end).unwrap_or_default()
}

/// Marks the innermost open element as having a structural child.
fn mark_structural_child(stack: &mut [ElementInfo]) {
    if let Some(element) = stack.last_mut() {
        element.has_structural_child = true;
    }
}

/// Increments the child counter of the innermost open element.
fn count_child(frames: &mut [RenderFrame]) {
    if let Some(frame) = frames.last_mut() {
        frame.children += 1;
    }
}

/// Starts a new output line at `depth`, or nothing when minifying.
fn write_line_break(out: &mut String, indent: &str, depth: usize, pretty: bool) {
    if !pretty {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    for _ in 0..depth {
        out.push_str(indent);
    }
}

/// `true` when the start tag carries `xml:space="preserve"`.
///
/// The value is compared as raw ASCII: `xml:space` is a plain token, so no
/// entity decoding is required and a malformed escape cannot hide the flag.
fn has_preserve_space(tag: &BytesStart<'_>) -> Result<bool, quick_xml::Error> {
    for attribute in tag.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == PRESERVE_SPACE_ATTRIBUTE {
            return Ok(
                trim_ascii(&attribute.value).eq_ignore_ascii_case(PRESERVE_SPACE_VALUE.as_bytes())
            );
        }
    }
    Ok(false)
}

/// Trims ASCII whitespace from both ends of a byte slice.
fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = value {
        if !first.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    while let [rest @ .., last] = value {
        if !last.is_ascii_whitespace() {
            break;
        }
        value = rest;
    }
    value
}

/// Builds a diagnostic from a parser error, using its byte position.
fn diagnostic_from_error(text: &str, reader: &Reader<&[u8]>, err: &quick_xml::Error) -> Diagnostic {
    let offset = char_boundary_at(text, usize::try_from(reader.error_position()).unwrap_or(0));
    diagnostic_at(text, offset, err.to_string())
}

/// Reports an unexpected close tag.
fn unexpected_end_diagnostic(text: &str, start: usize) -> Diagnostic {
    diagnostic_at(
        text,
        char_boundary_at(text, start),
        "unexpected close tag".to_owned(),
    )
}

/// Reports an element that is still open at end of input.
fn unclosed_element_diagnostic(text: &str, start: usize) -> Diagnostic {
    diagnostic_at(
        text,
        char_boundary_at(text, start),
        "element is not closed before the end of the document".to_owned(),
    )
}

/// Builds a diagnostic whose span covers one character at `offset`.
fn diagnostic_at(text: &str, offset: usize, message: String) -> Diagnostic {
    let line = u32::try_from(text[..offset].matches('\n').count() + 1).unwrap_or(u32::MAX);
    let column = display_column(text, offset);
    let end = text[offset..]
        .chars()
        .next()
        .map_or(offset, |ch| offset + ch.len_utf8());
    Diagnostic {
        span: Span::new(Utf8Pos(offset), Utf8Pos(end)),
        line_col: (line, column),
        message,
        hint: Some(format!(
            "check XML syntax near line {line}, column {column}"
        )),
    }
}

/// Clamps an offset to the nearest character boundary.
fn char_boundary_at(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(text: &str) -> String {
        XmlConfigProvider
            .format(text, &FormatOptions::default())
            .unwrap()
            .text
    }

    fn minify(text: &str) -> String {
        XmlConfigProvider.minify(text).unwrap().text
    }

    #[test]
    fn indents_nested_elements() {
        let text = "<root>\n<a>\n<b/>\n</a>\n</root>";
        assert_eq!(format(text), "<root>\n  <a>\n    <b/>\n  </a>\n</root>");
    }

    #[test]
    fn keeps_text_only_elements_inline() {
        let text = "<configuration>\n<pattern>%d %msg%n</pattern>\n</configuration>";
        assert_eq!(
            format(text),
            "<configuration>\n  <pattern>%d %msg%n</pattern>\n</configuration>"
        );
    }

    #[test]
    fn keeps_cdata_inline_byte_for_byte() {
        let text = "<script>\n<![CDATA[ if (a < b) { x(); } ]]>\n</script>";
        assert_eq!(
            format(text),
            "<script><![CDATA[ if (a < b) { x(); } ]]></script>"
        );
    }

    #[test]
    fn preserves_declaration_comment_and_doctype() {
        let text = "<?xml version='1.0' encoding='GBK'?>\n<!DOCTYPE note SYSTEM \"note.dtd\">\n<!--keep-->\n<note><to>T</to></note>";
        let out = format(text);
        assert!(out.contains("<?xml version='1.0' encoding='GBK'?>"));
        assert!(out.contains("<!DOCTYPE note SYSTEM \"note.dtd\">"));
        assert!(out.contains("\n<!--keep-->"));
    }

    #[test]
    fn preserves_attributes_byte_for_byte() {
        let text = "<root a='1' b=\"x&amp;y\"\n  c = \"3\"><child/></root>";
        let out = format(text);
        assert!(out.contains("<root a='1' b=\"x&amp;y\"\n  c = \"3\">"));
    }

    #[test]
    fn keeps_empty_element_form() {
        assert_eq!(format("<a></a>"), "<a></a>");
        assert_eq!(format("<a/>"), "<a/>");
    }

    #[test]
    fn keeps_single_whitespace_text_child() {
        assert_eq!(format("<param> </param>"), "<param> </param>");
        assert_eq!(
            format("<root>\n<param>\n</param>\n</root>"),
            "<root>\n  <param>\n</param>\n</root>"
        );
    }

    #[test]
    fn preserves_xml_space_subtree() {
        let text = "<root>\n<p xml:space=\"preserve\">\n  <b> x </b>\n</p>\n</root>";
        let out = format(text);
        assert!(out.contains("<p xml:space=\"preserve\">\n  <b> x </b>\n</p>"));
    }

    #[test]
    fn reports_syntax_errors_with_position() {
        let errors = XmlConfigProvider
            .validate("<root>\n  <a>\n</root>")
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].line_col.0 >= 3);
        assert!(errors[0].hint.is_some());
    }

    #[test]
    fn rejects_unclosed_root() {
        assert!(XmlConfigProvider.validate("<root><a/>").is_err());
    }

    #[test]
    fn formatting_failure_never_returns_output() {
        assert!(XmlConfigProvider
            .format("<root>", &FormatOptions::default())
            .is_err());
    }

    #[test]
    fn minify_removes_only_inter_element_whitespace() {
        let text = "<root>\n  <a> 1 </a>\n  <b/>\n</root>";
        assert_eq!(minify(text), "<root><a> 1 </a><b/></root>");
    }

    #[test]
    fn formatting_is_idempotent() {
        let text = "<?xml version=\"1.0\"?>\n<root>\n  <a>1</a>\n  <b><![CDATA[x]]></b>\n</root>";
        let once = format(text);
        assert_eq!(format(&once), once);
    }

    #[test]
    fn event_sequence_is_stable() {
        let text = "<root a='1'>\n  <a>text</a>\n  <!--c-->\n  <b><![CDATA[x]]></b>\n</root>";
        assert_eq!(event_signature(text), event_signature(&format(text)));
    }

    /// Event stream without whitespace-only text nodes, used to prove that
    /// formatting only moves whitespace around.
    fn event_signature(text: &str) -> Vec<String> {
        let mut reader = new_xml_reader(text);
        let mut signature = Vec::new();
        loop {
            let start = reader_position(&reader);
            let event = reader.read_event().unwrap();
            let end = reader_position(&reader);
            match &event {
                Event::Eof => break,
                Event::Text(_) if raw_slice(text, start, end).trim().is_empty() => continue,
                _ => signature.push(format!("{event:?}")),
            }
        }
        signature
    }
}
