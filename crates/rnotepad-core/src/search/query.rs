//! Search expression syntax: AST, parser and validation.
//!
//! Expressions are line scoped: every term is matched inside one logical line,
//! so `AND`, `OR` and `NOT` combine matches of the same line instead of
//! combining document-wide hit lists.  Keeping the whole expression in one
//! line lets the evaluator stream over the file with bounded memory, which is
//! what a multi-gigabyte log needs.
//!
//! Grammar:
//!
//! ```text
//! query   := or
//! or      := and ("OR" | "||" and)*
//! and     := unary (("AND" | "&&")? unary)*
//! unary   := ("NOT" | "!") unary | primary
//! primary := "(" query ")" | term
//! term    := ("re:" | "hex:") (quoted | bare)
//! ```

use thiserror::Error;

use super::{build_text_regex, parse_hex_pattern, SearchMode, SearchQuery};

/// One node of a parsed search expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryNode {
    /// Leaf pattern with the same semantics as a plain search.
    Term(SearchQuery),
    /// Every child must match the same logical line.
    And(Vec<QueryNode>),
    /// At least one child must match the logical line.
    Or(Vec<QueryNode>),
    /// The child must not match the logical line.
    Not(Box<QueryNode>),
}

/// A validated expression plus the options shared by all of its terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledQuery {
    pub root: QueryNode,
    /// Case sensitivity applied to every text term.
    pub case_sensitive: bool,
    /// Expression text as typed, reused for logging and group headers.
    pub source: String,
}

impl CompiledQuery {
    /// Parses `text` into an expression.
    ///
    /// Terms default to literal matching; `re:` selects a regular expression
    /// and `hex:` a byte pattern.  The query must contain at least one term
    /// outside `NOT`, because a purely negative expression has no span to
    /// report as a hit.
    pub fn parse(text: &str, case_sensitive: bool) -> Result<Self, QueryError> {
        Self::parse_with_options(text, case_sensitive, false)
    }

    /// Parses `text` with the same semantics as [`Self::parse`], applying
    /// `whole_word` to every text term.
    pub fn parse_with_options(
        text: &str,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Result<Self, QueryError> {
        let mut parser = Parser::new(text, case_sensitive, whole_word);
        let root = parser.parse_query()?;
        if !has_positive_term(&root, false) {
            return Err(QueryError::NoPositiveTerm);
        }
        Ok(Self {
            root,
            case_sensitive,
            source: text.to_owned(),
        })
    }

    /// Wraps a plain pattern so callers can use one query type everywhere.
    #[must_use]
    pub fn term(query: SearchQuery) -> Self {
        Self {
            source: query.pattern.clone(),
            case_sensitive: query.case_sensitive,
            root: QueryNode::Term(query),
        }
    }

    /// Returns the wrapped pattern when the query is a single term.
    ///
    /// The search service uses this to keep the original streaming scanners
    /// for plain searches, so expression support costs them nothing.
    #[must_use]
    pub fn single_term(&self) -> Option<&SearchQuery> {
        match &self.root {
            QueryNode::Term(term) => Some(term),
            _ => None,
        }
    }
}

/// Expression syntax or validation failure.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QueryError {
    #[error("the search expression is empty")]
    Empty,
    #[error("expected a term at column {column}")]
    ExpectedTerm { column: usize },
    #[error("expected ')' at column {column}")]
    ExpectedCloseParen { column: usize },
    #[error("unexpected character '{ch}' at column {column}")]
    UnexpectedChar { ch: char, column: usize },
    #[error("unterminated quoted term starting at column {column}")]
    UnterminatedQuote { column: usize },
    #[error("a term cannot span lines (column {column})")]
    MultiLineTerm { column: usize },
    #[error("the expression must contain at least one term outside NOT")]
    NoPositiveTerm,
    #[error("invalid regular expression \"{pattern}\": {message}")]
    InvalidRegex { pattern: String, message: String },
    #[error("invalid hex byte pattern \"{pattern}\": {message}")]
    InvalidHex { pattern: String, message: String },
}

/// `true` when some term is not negated, directly or through an ancestor.
fn has_positive_term(node: &QueryNode, negated: bool) -> bool {
    match node {
        QueryNode::Term(_) => !negated,
        QueryNode::And(children) | QueryNode::Or(children) => children
            .iter()
            .any(|child| has_positive_term(child, negated)),
        QueryNode::Not(inner) => has_positive_term(inner, !negated),
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    case_sensitive: bool,
    whole_word: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, case_sensitive: bool, whole_word: bool) -> Self {
        Self {
            input,
            pos: 0,
            case_sensitive,
            whole_word,
        }
    }

    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    /// One-based column of the current position, counted in characters.
    fn column(&self) -> usize {
        self.input[..self.pos].chars().count() + 1
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.pos += ch.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if !ch.is_whitespace() {
                break;
            }
            self.pos += ch.len_utf8();
        }
    }

    /// Consumes `token` (an operator) when it is a standalone word.
    fn eat_keyword(&mut self, keyword: &str) -> bool {
        if !self.peek_keyword(keyword) {
            return false;
        }
        self.pos += keyword.len();
        true
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        let rest = self.rest();
        let Some(head) = rest.get(..keyword.len()) else {
            return false;
        };
        if !head.eq_ignore_ascii_case(keyword) {
            return false;
        }
        !matches!(
            rest[keyword.len()..].chars().next(),
            Some(ch) if ch.is_alphanumeric() || ch == '_'
        )
    }

    fn eat_symbol(&mut self, symbol: &str) -> bool {
        if self.rest().starts_with(symbol) {
            self.pos += symbol.len();
            true
        } else {
            false
        }
    }

    fn parse_query(&mut self) -> Result<QueryNode, QueryError> {
        self.skip_whitespace();
        if self.peek().is_none() {
            return Err(QueryError::Empty);
        }
        let node = self.parse_or()?;
        self.skip_whitespace();
        if let Some(ch) = self.peek() {
            return Err(QueryError::UnexpectedChar {
                ch,
                column: self.column(),
            });
        }
        Ok(node)
    }

    fn parse_or(&mut self) -> Result<QueryNode, QueryError> {
        let mut items = vec![self.parse_and()?];
        loop {
            self.skip_whitespace();
            if self.eat_keyword("OR") || self.eat_symbol("||") {
                items.push(self.parse_and()?);
            } else {
                break;
            }
        }
        Ok(if items.len() == 1 {
            items.pop().expect("one item is present")
        } else {
            QueryNode::Or(items)
        })
    }

    fn parse_and(&mut self) -> Result<QueryNode, QueryError> {
        let mut items = vec![self.parse_unary()?];
        loop {
            self.skip_whitespace();
            if self.peek().is_none() || self.peek() == Some(')') || self.peek_keyword("OR") {
                break;
            }
            if !(self.eat_keyword("AND") || self.eat_symbol("&&")) {
                // Adjacent terms are an implicit AND.
                if self.peek() == Some('|') {
                    break;
                }
            }
            items.push(self.parse_unary()?);
        }
        Ok(if items.len() == 1 {
            items.pop().expect("one item is present")
        } else {
            QueryNode::And(items)
        })
    }

    fn parse_unary(&mut self) -> Result<QueryNode, QueryError> {
        self.skip_whitespace();
        if self.eat('!') || self.eat_keyword("NOT") {
            let inner = self.parse_unary()?;
            return Ok(QueryNode::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<QueryNode, QueryError> {
        self.skip_whitespace();
        match self.peek() {
            Some('(') => {
                self.bump();
                let node = self.parse_or()?;
                self.skip_whitespace();
                if !self.eat(')') {
                    return Err(QueryError::ExpectedCloseParen {
                        column: self.column(),
                    });
                }
                Ok(node)
            }
            Some(_) => self.parse_term(),
            None => Err(QueryError::ExpectedTerm {
                column: self.column(),
            }),
        }
    }

    fn parse_term(&mut self) -> Result<QueryNode, QueryError> {
        let column = self.column();
        let mode = if self.eat_prefix("re:") {
            SearchMode::Regex
        } else if self.eat_prefix("hex:") {
            SearchMode::HexBytes
        } else {
            SearchMode::Literal
        };
        self.skip_whitespace();
        let pattern = match self.peek() {
            Some('"') => self.parse_quoted()?,
            Some(')') | None => return Err(QueryError::ExpectedTerm { column }),
            Some(_) => self.parse_bare(),
        };
        if pattern.is_empty() {
            return Err(QueryError::ExpectedTerm { column });
        }
        if pattern.contains(['\n', '\r']) {
            return Err(QueryError::MultiLineTerm { column });
        }
        self.validate(&pattern, mode)?;
        Ok(QueryNode::Term(SearchQuery {
            pattern,
            mode,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }))
    }

    /// Consumes an ASCII prefix such as `re:` at the current position.
    fn eat_prefix(&mut self, prefix: &str) -> bool {
        match self.rest().get(..prefix.len()) {
            Some(head) if head.eq_ignore_ascii_case(prefix) => {
                self.pos += prefix.len();
                true
            }
            _ => false,
        }
    }

    fn parse_bare(&mut self) -> String {
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || matches!(ch, '(' | ')' | '"') {
                break;
            }
            out.push(ch);
            self.pos += ch.len_utf8();
        }
        out
    }

    fn parse_quoted(&mut self) -> Result<String, QueryError> {
        let open_column = self.column();
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(QueryError::UnterminatedQuote { column: open_column }),
                Some('"') => return Ok(out),
                // Only the quote and the backslash are escapes; everything
                // else keeps the backslash so `"\d+"` survives as a regex.
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => return Err(QueryError::UnterminatedQuote { column: open_column }),
                },
                Some(ch) => out.push(ch),
            }
        }
    }

    fn validate(&self, pattern: &str, mode: SearchMode) -> Result<(), QueryError> {
        let query = SearchQuery {
            pattern: pattern.to_owned(),
            mode,
            case_sensitive: self.case_sensitive,
            whole_word: false,
        };
        match mode {
            SearchMode::Regex => build_text_regex(&query)
                .map(|_| ())
                .map_err(|err| QueryError::InvalidRegex {
                    pattern: pattern.to_owned(),
                    message: err.to_string(),
                }),
            SearchMode::HexBytes => parse_hex_pattern(pattern)
                .map(|_| ())
                .map_err(|err| QueryError::InvalidHex {
                    pattern: pattern.to_owned(),
                    message: err.to_string(),
                }),
            SearchMode::Literal => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(query: &CompiledQuery) -> Option<&SearchQuery> {
        query.single_term()
    }

    #[test]
    fn parses_operators_with_precedence() {
        let query = CompiledQuery::parse("alpha OR beta AND gamma", false).unwrap();
        match query.root {
            QueryNode::Or(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(&items[0], QueryNode::Term(term) if term.pattern == "alpha"));
                assert!(matches!(&items[1], QueryNode::And(parts) if parts.len() == 2));
            }
            other => panic!("unexpected root: {other:?}"),
        }
    }

    #[test]
    fn adjacent_terms_are_an_implicit_and() {
        let query = CompiledQuery::parse("\"foo\" bar", false).unwrap();
        assert!(matches!(query.root, QueryNode::And(ref parts) if parts.len() == 2));
    }

    #[test]
    fn not_nests_and_flips_the_rule() {
        let query = CompiledQuery::parse("alpha AND NOT beta", false).unwrap();
        assert!(matches!(query.root, QueryNode::And(ref parts) if parts.len() == 2));
        // A purely negative expression has no span to report.
        assert_eq!(
            CompiledQuery::parse("NOT (alpha OR beta)", false).unwrap_err(),
            QueryError::NoPositiveTerm
        );
    }

    #[test]
    fn quoted_terms_keep_unknown_escapes() {
        let query = CompiledQuery::parse(r#"re:"\d+ \"quoted\"""#, false).unwrap();
        let term = term(&query).expect("single term");
        assert_eq!(term.pattern, r#"\d+ "quoted""#);
        assert_eq!(term.mode, SearchMode::Regex);
    }

    #[test]
    fn hex_prefix_selects_byte_matching() {
        let query = CompiledQuery::parse("hex:\"DE AD\"", true).unwrap();
        let term = term(&query).expect("single term");
        assert_eq!(term.mode, SearchMode::HexBytes);
    }

    #[test]
    fn whole_word_option_applies_to_every_term() {
        let query = CompiledQuery::parse_with_options("alpha AND beta", false, true).unwrap();
        match query.root {
            QueryNode::And(items) => {
                assert!(
                    items
                        .iter()
                        .all(|item| matches!(item, QueryNode::Term(term) if term.whole_word))
                );
            }
            other => panic!("unexpected root: {other:?}"),
        }
    }

    #[test]
    fn invalid_input_is_rejected() {
        assert_eq!(CompiledQuery::parse("", false).unwrap_err(), QueryError::Empty);
        assert!(matches!(
            CompiledQuery::parse("(alpha", false),
            Err(QueryError::ExpectedCloseParen { .. })
        ));
        assert!(matches!(
            CompiledQuery::parse("\"alpha", false),
            Err(QueryError::UnterminatedQuote { .. })
        ));
        assert!(matches!(
            CompiledQuery::parse("re:*", false),
            Err(QueryError::InvalidRegex { .. })
        ));
    }

    #[test]
    fn multiline_terms_are_rejected() {
        assert!(matches!(
            CompiledQuery::parse("\"a\nb\"", false),
            Err(QueryError::MultiLineTerm { .. })
        ));
    }
}
