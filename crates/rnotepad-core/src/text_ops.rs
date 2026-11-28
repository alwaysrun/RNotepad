//! Line-oriented text transformations behind the Text Tools menu.
//!
//! Every function changes only what its name says: line terminators (`\r\n`,
//! `\r` or `\n`) and the presence or absence of a final newline are preserved,
//! so the result can be written back without touching the EOL policy.

use unicode_width::UnicodeWidthChar;

/// Whitespace removed by the trim commands: indentation only.
fn is_indent_space(value: char) -> bool {
    value == ' ' || value == '\t'
}

/// Splits `text` into `(content, terminator)` pairs.
///
/// A terminator is one of `\r\n`, `\r` or `\n`; the last pair may carry an
/// empty terminator when the text does not end with a newline.
fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                let terminator_len = usize::from(bytes.get(index + 1) == Some(&b'\n')) + 1;
                lines.push((&text[start..index], &text[index..index + terminator_len]));
                index += terminator_len;
                start = index;
            }
            b'\n' => {
                lines.push((&text[start..index], &text[index..index + 1]));
                index += 1;
                start = index;
            }
            _ => index += 1,
        }
    }
    // A trailing newline already produced its line; only a remaining fragment
    // (or empty input) needs one more pair.
    if start < bytes.len() || lines.is_empty() {
        lines.push((&text[start..], ""));
    }
    lines
}

/// Rebuilds `text` after applying `transform` to every line content.
fn map_lines(text: &str, mut transform: impl FnMut(&str) -> String) -> String {
    let mut output = String::with_capacity(text.len());
    for (content, terminator) in split_lines(text) {
        output.push_str(&transform(content));
        output.push_str(terminator);
    }
    output
}

/// Drops every line that contains only spaces or tabs.
///
/// The terminator of each surviving line is kept, so a document that ends with
/// a newline still ends with one.
#[must_use]
pub fn remove_blank_lines(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for (content, terminator) in split_lines(text) {
        if content.trim_matches(is_indent_space).is_empty() {
            continue;
        }
        output.push_str(content);
        output.push_str(terminator);
    }
    output
}

/// Removes spaces and tabs before the first non-blank character of each line.
#[must_use]
pub fn trim_leading_whitespace(text: &str) -> String {
    map_lines(text, |line| {
        line.trim_start_matches(is_indent_space).to_owned()
    })
}

/// Removes spaces and tabs after the last non-blank character of each line.
#[must_use]
pub fn trim_trailing_whitespace(text: &str) -> String {
    map_lines(text, |line| {
        line.trim_end_matches(is_indent_space).to_owned()
    })
}

/// Replaces every tab with the spaces needed to reach the next tab stop.
///
/// Tab stops follow the displayed column, so wide characters count as two
/// columns and a line break resets the position.
#[must_use]
pub fn tabs_to_spaces(text: &str, tab_width: usize) -> String {
    if tab_width == 0 {
        return text.to_owned();
    }
    let mut output = String::with_capacity(text.len());
    let mut column = 0;
    for value in text.chars() {
        match value {
            '\t' => {
                let spaces = tab_width - column % tab_width;
                output.extend(std::iter::repeat(' ').take(spaces));
                column += spaces;
            }
            '\r' | '\n' => {
                output.push(value);
                column = 0;
            }
            _ => {
                output.push(value);
                column += UnicodeWidthChar::width(value).unwrap_or(0);
            }
        }
    }
    output
}

/// Replaces leading spaces with tabs, leaving the rest of each line untouched.
///
/// A partial group of spaces shorter than `tab_width` is kept, which makes the
/// command reversible through [`tabs_to_spaces`] for plain indentation.
#[must_use]
pub fn spaces_to_tabs(text: &str, tab_width: usize) -> String {
    if tab_width == 0 {
        return text.to_owned();
    }
    map_lines(text, |line| {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let tabs = indent / tab_width;
        let mut converted = String::with_capacity(line.len());
        converted.extend(std::iter::repeat('\t').take(tabs));
        converted.push_str(&line[tabs * tab_width..]);
        converted
    })
}

/// One line-oriented transformation offered by the Text Tools menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    RemoveBlankLines,
    TrimLeadingWhitespace,
    TrimTrailingWhitespace,
    TabsToSpaces,
    SpacesToTabs,
}

impl TextTransform {
    /// Every transform in menu order.
    pub const ALL: [Self; 5] = [
        Self::RemoveBlankLines,
        Self::TrimLeadingWhitespace,
        Self::TrimTrailingWhitespace,
        Self::TabsToSpaces,
        Self::SpacesToTabs,
    ];

    /// Menu label, also used in status messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::RemoveBlankLines => "Remove Blank Lines",
            Self::TrimLeadingWhitespace => "Trim Leading Whitespace",
            Self::TrimTrailingWhitespace => "Trim Trailing Whitespace",
            Self::TabsToSpaces => "Tabs to Spaces",
            Self::SpacesToTabs => "Spaces to Tabs",
        }
    }

    /// Returns the transformed text; `tab_width` is the editor tab size.
    #[must_use]
    pub fn apply(self, text: &str, tab_width: usize) -> String {
        match self {
            Self::RemoveBlankLines => remove_blank_lines(text),
            Self::TrimLeadingWhitespace => trim_leading_whitespace(text),
            Self::TrimTrailingWhitespace => trim_trailing_whitespace(text),
            Self::TabsToSpaces => tabs_to_spaces(text, tab_width),
            Self::SpacesToTabs => spaces_to_tabs(text, tab_width),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_blank_lines_and_keeps_crlf() {
        assert_eq!(
            remove_blank_lines("a\r\n\r\n  \t\r\nb\r\n"),
            "a\r\nb\r\n"
        );
    }

    #[test]
    fn removes_blank_lines_without_final_newline() {
        assert_eq!(remove_blank_lines("a\n\nb"), "a\nb");
        assert_eq!(remove_blank_lines("\n\n"), "");
    }

    #[test]
    fn handles_cr_only_terminators() {
        assert_eq!(remove_blank_lines("a\r\rb\r"), "a\rb\r");
        assert_eq!(trim_trailing_whitespace("a  \rb\t\r"), "a\rb\r");
    }

    #[test]
    fn trims_only_the_requested_side() {
        assert_eq!(trim_leading_whitespace("  a  \n\tb\t"), "a  \nb\t");
        assert_eq!(trim_trailing_whitespace("  a  \n\tb\t"), "  a\n\tb");
    }

    #[test]
    fn converts_tabs_on_column_stops() {
        assert_eq!(tabs_to_spaces("a\tb", 4), "a   b");
        assert_eq!(tabs_to_spaces("\tX", 4), "    X");
        // A wide character occupies two columns, so the tab only adds two.
        assert_eq!(tabs_to_spaces("中\tX", 4), "中  X");
        assert_eq!(tabs_to_spaces("a\t\r\nb\t", 4), "a   \r\nb   ");
    }

    #[test]
    fn converts_leading_spaces_only() {
        assert_eq!(spaces_to_tabs("    a  b", 4), "\ta  b");
        assert_eq!(spaces_to_tabs("      a", 4), "\t  a");
        assert_eq!(spaces_to_tabs("\t  a", 4), "\t  a");
        assert_eq!(spaces_to_tabs("a    b\r\n    c", 4), "a    b\r\n\tc");
    }

    #[test]
    fn transforms_share_one_dispatch() {
        let text = "  a  \n\n";
        assert_eq!(
            TextTransform::TrimTrailingWhitespace.apply(text, 4),
            "  a\n\n"
        );
        assert_eq!(TextTransform::RemoveBlankLines.apply(text, 4), "  a  \n");
    }
}
