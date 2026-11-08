//! Format providers: validation, pretty-printing and minification.
//!
//! P0 ships the JSON provider.  Diagnostics carry the UTF-8 span, the display
//! line/column and, when available, a hint that explains how to fix the
//! problem.  Formatting never writes to the document; the caller previews
//! [`FormatOutput`] and applies it as a single edit.

use serde::Serialize;
use serde_json::ser::{PrettyFormatter, Serializer};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::coord::{Span, Utf8Pos};

/// Supported formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Json,
}

/// Formatting options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    /// Number of spaces per indent level.
    pub indent: usize,
    /// Ignore comments, single-quoted strings and trailing commas.
    pub lenient: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent: 2,
            lenient: false,
        }
    }
}

/// A problem found while validating a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span<Utf8Pos>,
    /// One-based line and display column.
    pub line_col: (u32, u32),
    pub message: String,
    pub hint: Option<String>,
}

/// Maps positions in formatted output back to the original text.
//[`SourceMap`] uses the fact that formatting only inserts or removes
/// whitespace: the token stream is identical, so token *n* in the output
/// corresponds to token *n* in the source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    /// `(formatted_offset, original_offset)` pairs, sorted by the first field.
    pub anchors: Vec<(usize, usize)>,
}

impl SourceMap {
    /// Builds the map by pairing tokens in sequence.
    #[must_use]
    pub fn build(original: &str, formatted: &str) -> Self {
        let source_tokens = token_starts(original);
        let formatted_tokens = token_starts(formatted);
        let anchors = formatted_tokens
            .iter()
            .copied()
            .zip(source_tokens.iter().copied())
            .collect();
        Self { anchors }
    }

    /// Maps an offset in the formatted text to the original text.
    #[must_use]
    pub fn to_original(&self, formatted_pos: usize) -> Option<usize> {
        if self.anchors.is_empty() {
            return Some(formatted_pos);
        }
        let idx = self
            .anchors
            .partition_point(|(formatted, _)| *formatted <= formatted_pos)
            .saturating_sub(1);
        let (formatted_start, original_start) = self.anchors[idx];
        Some(original_start + formatted_pos.saturating_sub(formatted_start))
    }
}

/// Formatted text plus its source map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOutput {
    pub text: String,
    pub source_map: SourceMap,
}

/// A format-specific implementation.
pub trait FormatProvider: Send + Sync {
    fn kind(&self) -> FormatKind;
    fn validate(&self, text: &str) -> Result<(), Vec<Diagnostic>>;
    fn format(&self, text: &str, options: &FormatOptions)
        -> Result<FormatOutput, Vec<Diagnostic>>;
    fn minify(&self, text: &str) -> Result<FormatOutput, Vec<Diagnostic>>;
}

/// JSON provider built on `serde_json` with order preservation and arbitrary
/// precision numbers.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonFormatProvider;

impl FormatProvider for JsonFormatProvider {
    fn kind(&self) -> FormatKind {
        FormatKind::Json
    }

    fn validate(&self, text: &str) -> Result<(), Vec<Diagnostic>> {
        validate_strict(text)
    }

    fn format(
        &self,
        text: &str,
        options: &FormatOptions,
    ) -> Result<FormatOutput, Vec<Diagnostic>> {
        let (value, _) = parse_for_output(text, options)?;
        let indent = vec![b' '; options.indent.min(MAX_INDENT)];
        let formatter = PrettyFormatter::with_indent(&indent);
        let mut out = Vec::new();
        let mut serializer = Serializer::with_formatter(&mut out, formatter);
        value
            .serialize(&mut serializer)
            .map_err(|err| vec![diagnostic_from_error(text, &err)])?;
        let formatted = String::from_utf8(out)
            .map_err(|_| vec![internal_diagnostic(text, "serializer produced invalid UTF-8")])?;
        let source_map = SourceMap::build(text, &formatted);
        Ok(FormatOutput {
            text: formatted,
            source_map,
        })
    }

    fn minify(&self, text: &str) -> Result<FormatOutput, Vec<Diagnostic>> {
        let (value, _) = parse_for_output(text, &FormatOptions::default())?;
        let formatted = value.to_string();
        let source_map = SourceMap::build(text, &formatted);
        Ok(FormatOutput {
            text: formatted,
            source_map,
        })
    }
}

const MAX_INDENT: usize = 8;

fn parse_for_output(
    text: &str,
    options: &FormatOptions,
) -> Result<(Value, String), Vec<Diagnostic>> {
    let prepared = if options.lenient {
        strip_lenient(text)
    } else {
        text.to_owned()
    };
    let value: Value = serde_json::from_str(&prepared)
        .map_err(|err| vec![diagnostic_from_error(text, &err)])?;
    Ok((value, prepared))
}

fn validate_strict(text: &str) -> Result<(), Vec<Diagnostic>> {
    serde_json::from_str::<Value>(text)
        .map(|_| ())
        .map_err(|err| vec![diagnostic_from_error(text, &err)])
}

fn diagnostic_from_error(text: &str, err: &serde_json::Error) -> Diagnostic {
    let line = u32::try_from(err.line()).unwrap_or(1);
    let column = u32::try_from(err.column()).unwrap_or(1);
    let offset = position_to_offset(text, line, column);
    let display_col = display_column(text, offset);
    Diagnostic {
        span: Span::new(Utf8Pos(offset), Utf8Pos((offset + 1).min(text.len()))),
        line_col: (line, display_col),
        message: err.to_string(),
        hint: Some(format!(
            "check JSON syntax near line {line}, column {display_col}"
        )),
    }
}

fn internal_diagnostic(text: &str, message: &str) -> Diagnostic {
    Diagnostic {
        span: Span::new(Utf8Pos(0), Utf8Pos(text.len())),
        line_col: (1, 1),
        message: message.to_owned(),
        hint: None,
    }
}

fn position_to_offset(text: &str, line: u32, column: u32) -> usize {
    if line <= 1 {
        return (column.saturating_sub(1) as usize).min(text.len());
    }
    let mut seen = 1u32;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                let line_start = idx + 1;
                return (line_start + column.saturating_sub(1) as usize).min(text.len());
            }
        }
    }
    text.len()
}

fn display_column(text: &str, offset: usize) -> u32 {
    let line_start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..offset.min(text.len())];
    u32::try_from(prefix.width()).unwrap_or(u32::MAX) + 1
}

/// Replaces comments and trailing commas with spaces, and single quotes with
/// double quotes.  All replacements keep the byte length, so error offsets
/// remain valid for the original text.
fn strip_lenient(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(quote) = in_string {
            if escaped {
                out.push(byte);
                escaped = false;
            } else if byte == b'\\' {
                out.push(byte);
                escaped = true;
            } else if byte == quote {
                out.push(if quote == b'\'' { b'"' } else { byte });
                in_string = None;
            } else {
                out.push(byte);
            }
            i += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            in_string = Some(byte);
            out.push(if byte == b'\'' { b'"' } else { byte });
            i += 1;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            out.push(b' ');
            out.push(b' ');
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                out.push(if bytes[i] == b'\n' { b'\n' } else { b' ' });
                i += 1;
            }
            if i + 1 < bytes.len() {
                out.push(b' ');
                out.push(b' ');
                i += 2;
            }
            continue;
        }
        out.push(byte);
        i += 1;
    }
    let mut result = String::from_utf8_lossy(&out).into_owned();
    strip_trailing_commas(&mut result);
    result
}

fn strip_trailing_commas(text: &mut String) {
    let bytes = text.as_bytes();
    let mut replacements = Vec::new();
    let mut i = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if byte == b',' {
            let mut look = i + 1;
            while look < bytes.len() && bytes[look].is_ascii_whitespace() {
                look += 1;
            }
            if look < bytes.len() && (bytes[look] == b'}' || bytes[look] == b']') {
                replacements.push(i);
            }
        }
        i += 1;
    }
    if replacements.is_empty() {
        return;
    }
    let mut bytes = text.as_bytes().to_vec();
    for position in replacements {
        if let Some(slot) = bytes.get_mut(position) {
            *slot = b' ';
        }
    }
    *text = String::from_utf8(bytes).unwrap_or_default();
}

/// Offsets of JSON tokens (strings, numbers, literals and punctuation).
fn token_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        starts.push(i);
        if byte == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if is_punctuation(byte) {
            i += 1;
            continue;
        }
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !is_punctuation(bytes[i])
            && bytes[i] != b'"'
        {
            i += 1;
        }
    }
    starts
}

fn is_punctuation(byte: u8) -> bool {
    matches!(byte, b'{' | b'}' | b'[' | b']' | b':' | b',')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_valid_json() {
        let provider = JsonFormatProvider;
        assert!(provider.validate(r#"{"a":1}"#).is_ok());
    }

    #[test]
    fn reports_precise_error_position() {
        let provider = JsonFormatProvider;
        let text = "{\n  \"a\": 1,\n  \"b\": ]\n}";
        let errors = provider.validate(text).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line_col.0, 3);
        assert!(errors[0].line_col.1 >= 8);
    }

    #[test]
    fn pretty_print_uses_two_spaces() {
        let provider = JsonFormatProvider;
        let out = provider
            .format(r#"{"a":[1,2]}"#, &FormatOptions::default())
            .unwrap();
        assert!(out.text.contains("\n  \"a\""));
        assert!(out.text.contains("\n    1"));
    }

    #[test]
    fn minify_removes_whitespace() {
        let provider = JsonFormatProvider;
        let out = provider.minify("{\n  \"a\": 1\n}").unwrap();
        assert_eq!(out.text, r#"{"a":1}"#);
    }

    #[test]
    fn lenient_mode_accepts_comments_and_trailing_commas() {
        let provider = JsonFormatProvider;
        let text = "{\n // comment\n \"a\": 1,\n}";
        let out = provider
            .format(
                text,
                &FormatOptions {
                    indent: 2,
                    lenient: true,
                },
            )
            .unwrap();
        assert!(out.text.contains("\"a\": 1"));
    }

    #[test]
    fn source_map_pairs_tokens() {
        let map = SourceMap::build(r#"{"a":1}"#, "{\n  \"a\": 1\n}");
        assert!(!map.anchors.is_empty());
        assert_eq!(map.to_original(0), Some(0));
    }

    #[test]
    fn invalid_json_never_returns_output() {
        let provider = JsonFormatProvider;
        assert!(provider.format("{", &FormatOptions::default()).is_err());
    }
}
