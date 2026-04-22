use crate::keywords;

// ── Token types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Literals
    String,   // "...", '...', `...`
    Number,   // 42, 3.14, .5, 1e10
    Duration, // 5m, 1h30m, 500ms

    // Identifiers & keywords
    Ident, // label names, function names

    // Operators
    Pipe,      // |
    PipeExact, // |=
    PipeMatch, // |~
    NotEqual,  // !=
    NotMatch,  // !~
    Eq,        // =
    EqRegex,   // =~
    Plus,      // +
    Minus,     // -
    Star,      // *
    Slash,     // /
    Percent,   // %
    Caret,     // ^
    Lt,        // <
    Gt,        // >
    LtEq,      // <=
    GtEq,      // >=
    CmpEq,     // ==

    // Delimiters
    LBrace,   // {
    RBrace,   // }
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Dot,      // .

    // Other
    Whitespace,  // spaces, tabs, newlines
    LineComment, // # ...
    Error,       // unrecognized byte
    Eof,         // end of input
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize, // byte offset
    pub end: usize,   // byte offset (exclusive)
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// Extract the text slice for this token from the original input.
    pub fn text<'a>(&self, input: &'a str) -> &'a str {
        &input[self.span.start..self.span.end]
    }
}

// ── Keyword classification ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordKind {
    RangeFunction,       // rate, count_over_time, sum_over_time, etc.
    AggregationOp,       // sum, avg, min, max, count, topk, etc.
    AggregationModifier, // by, without
    BinaryKeyword,       // and, or, unless
    PipelineParser,      // json, logfmt, pattern, regexp, unpack
    PipelineFormatter,   // line_format, label_format
    PipelineLabelOp,     // drop, keep, decolorize
    FilterKeyword,       // unwrap
    NotKeyword,          // not a known keyword
}

/// Categorize an identifier string into a LogQL keyword kind.
pub fn classify_keyword(text: &str) -> KeywordKind {
    if keywords::RANGE_FUNCTIONS.contains(&text) {
        KeywordKind::RangeFunction
    } else if keywords::AGGREGATION_OPS.contains(&text) {
        KeywordKind::AggregationOp
    } else if keywords::AGGREGATION_MODIFIERS.contains(&text) {
        KeywordKind::AggregationModifier
    } else if keywords::BINARY_KEYWORDS.contains(&text) {
        KeywordKind::BinaryKeyword
    } else {
        match text {
            "json" | "logfmt" | "pattern" | "regexp" | "unpack" | "pack" => {
                KeywordKind::PipelineParser
            }
            "line_format" | "label_format" => KeywordKind::PipelineFormatter,
            "drop" | "keep" | "decolorize" => KeywordKind::PipelineLabelOp,
            "unwrap" => KeywordKind::FilterKeyword,
            _ => KeywordKind::NotKeyword,
        }
    }
}

// ── Duration suffix helpers ──────────────────────────────────────────────────

/// Check if `bytes[pos..]` starts with a duration suffix. Returns the number
/// of bytes consumed (0 if no suffix matched).
fn eat_duration_suffix(bytes: &[u8], mut pos: usize) -> usize {
    let start = pos;
    let len = bytes.len();

    // Consume one or more duration segments: e.g. 1h30m10s
    loop {
        // First, eat any digits (for composite durations like 1h30m)
        if pos > start {
            // After the first suffix, we need digits before the next suffix
            let digit_start = pos;
            while pos < len && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos == digit_start {
                // No more digits — done with duration segments
                break;
            }
        }

        if pos >= len {
            break;
        }

        match bytes[pos] {
            b'n' => {
                // ns
                if pos + 1 < len && bytes[pos + 1] == b's' {
                    pos += 2;
                } else {
                    break;
                }
            }
            b'u' => {
                // us
                if pos + 1 < len && bytes[pos + 1] == b's' {
                    pos += 2;
                } else {
                    break;
                }
            }
            b'm' => {
                // ms or m
                pos += 1;
                if pos < len && bytes[pos] == b's' {
                    pos += 1;
                }
            }
            b's' | b'h' | b'd' | b'w' | b'y' => {
                pos += 1;
            }
            _ => break,
        }
    }

    pos - start
}

/// Returns true if the byte is a valid start of a duration suffix.
fn is_duration_suffix_start(b: u8) -> bool {
    matches!(b, b'n' | b'u' | b'm' | b's' | b'h' | b'd' | b'w' | b'y')
}

// ── Tokenizer ────────────────────────────────────────────────────────────────

/// Tokenize a LogQL input string into a vector of tokens.
///
/// The returned tokens cover every byte of the input (their spans are
/// contiguous and non-overlapping). An `Eof` token is always appended at
/// the end.
pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut tokens = Vec::new();

    while pos < len {
        let start = pos;

        match bytes[pos] {
            // ── Whitespace ───────────────────────────────────────────────
            b' ' | b'\t' | b'\r' | b'\n' => {
                while pos < len && matches!(bytes[pos], b' ' | b'\t' | b'\r' | b'\n') {
                    pos += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Whitespace,
                    span: Span { start, end: pos },
                });
            }

            // ── Line comments ────────────────────────────────────────────
            b'#' => {
                while pos < len && bytes[pos] != b'\n' {
                    pos += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::LineComment,
                    span: Span { start, end: pos },
                });
            }

            // ── String literals ──────────────────────────────────────────
            b'"' | b'\'' | b'`' => {
                let delim = bytes[pos];
                pos += 1;
                let mut closed = false;
                while pos < len {
                    if bytes[pos] == b'\\' && delim != b'`' {
                        // Skip escape sequence
                        pos = (pos + 2).min(len);
                        continue;
                    }
                    if bytes[pos] == delim {
                        pos += 1;
                        closed = true;
                        break;
                    }
                    pos += 1;
                }
                let kind = if closed {
                    TokenKind::String
                } else {
                    TokenKind::Error
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Numbers, durations ───────────────────────────────────────
            b'0'..=b'9' => {
                // Integer part
                while pos < len && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                let mut is_float = false;

                // Decimal part
                if pos < len && bytes[pos] == b'.' {
                    // Only consume the dot if followed by a digit (avoid 42.method)
                    if pos + 1 < len && bytes[pos + 1].is_ascii_digit() {
                        is_float = true;
                        pos += 1; // consume '.'
                        while pos < len && bytes[pos].is_ascii_digit() {
                            pos += 1;
                        }
                    }
                }

                // Exponent part (1e10, 3.14e-2)
                if pos < len && matches!(bytes[pos], b'e' | b'E') {
                    is_float = true;
                    pos += 1;
                    if pos < len && matches!(bytes[pos], b'+' | b'-') {
                        pos += 1;
                    }
                    while pos < len && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                }

                // Duration suffix check
                let kind = if !is_float
                    && pos < len
                    && is_duration_suffix_start(bytes[pos])
                {
                    let consumed = eat_duration_suffix(bytes, pos);
                    if consumed > 0 {
                        pos += consumed;
                        TokenKind::Duration
                    } else {
                        TokenKind::Number
                    }
                } else {
                    TokenKind::Number
                };

                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Dot-leading decimal (.5, .123) ───────────────────────────
            b'.' if pos + 1 < len && bytes[pos + 1].is_ascii_digit() => {
                pos += 1; // consume '.'
                while pos < len && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                // Exponent
                if pos < len && matches!(bytes[pos], b'e' | b'E') {
                    pos += 1;
                    if pos < len && matches!(bytes[pos], b'+' | b'-') {
                        pos += 1;
                    }
                    while pos < len && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Number,
                    span: Span { start, end: pos },
                });
            }

            // ── Identifiers and keywords ─────────────────────────────────
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while pos < len
                    && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_')
                {
                    pos += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Ident,
                    span: Span { start, end: pos },
                });
            }

            // ── Pipe operators ───────────────────────────────────────────
            b'|' => {
                pos += 1;
                let kind = if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                    TokenKind::PipeExact
                } else if pos < len && bytes[pos] == b'~' {
                    pos += 1;
                    TokenKind::PipeMatch
                } else {
                    TokenKind::Pipe
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Bang operators ───────────────────────────────────────────
            b'!' => {
                pos += 1;
                let kind = if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                    TokenKind::NotEqual
                } else if pos < len && bytes[pos] == b'~' {
                    pos += 1;
                    TokenKind::NotMatch
                } else {
                    TokenKind::Error
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Equal operators ──────────────────────────────────────────
            b'=' => {
                pos += 1;
                let kind = if pos < len && bytes[pos] == b'~' {
                    pos += 1;
                    TokenKind::EqRegex
                } else if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                    TokenKind::CmpEq
                } else {
                    TokenKind::Eq
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Comparison operators ─────────────────────────────────────
            b'<' => {
                pos += 1;
                let kind = if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }
            b'>' => {
                pos += 1;
                let kind = if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                };
                tokens.push(Token {
                    kind,
                    span: Span { start, end: pos },
                });
            }

            // ── Arithmetic operators ─────────────────────────────────────
            b'+' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Plus,
                    span: Span { start, end: pos },
                });
            }
            b'-' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Minus,
                    span: Span { start, end: pos },
                });
            }
            b'*' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Star,
                    span: Span { start, end: pos },
                });
            }
            b'/' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Slash,
                    span: Span { start, end: pos },
                });
            }
            b'%' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Percent,
                    span: Span { start, end: pos },
                });
            }
            b'^' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Caret,
                    span: Span { start, end: pos },
                });
            }

            // ── Delimiters ───────────────────────────────────────────────
            b'{' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::LBrace,
                    span: Span { start, end: pos },
                });
            }
            b'}' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::RBrace,
                    span: Span { start, end: pos },
                });
            }
            b'(' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    span: Span { start, end: pos },
                });
            }
            b')' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    span: Span { start, end: pos },
                });
            }
            b'[' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::LBracket,
                    span: Span { start, end: pos },
                });
            }
            b']' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::RBracket,
                    span: Span { start, end: pos },
                });
            }
            b',' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    span: Span { start, end: pos },
                });
            }
            b'.' => {
                pos += 1;
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    span: Span { start, end: pos },
                });
            }

            // ── Multi-byte UTF-8 or unrecognized ASCII ───────────────────
            b => {
                let ch_len = if b < 0x80 {
                    1
                } else if b < 0xe0 {
                    2
                } else if b < 0xf0 {
                    3
                } else {
                    4
                };
                pos = (pos + ch_len).min(len);
                tokens.push(Token {
                    kind: TokenKind::Error,
                    span: Span { start, end: pos },
                });
            }
        }
    }

    // Always append Eof
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            start: len,
            end: len,
        },
    });

    tokens
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: collect (kind, text) pairs, excluding Eof.
    fn tok(input: &str) -> Vec<(TokenKind, &str)> {
        tokenize(input)
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| (t.kind, t.text(input)))
            .collect()
    }

    /// Helper: collect just the kinds, excluding Eof.
    fn kinds(input: &str) -> Vec<TokenKind> {
        tokenize(input)
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| t.kind)
            .collect()
    }

    // ── Round-trip property ──────────────────────────────────────────────

    fn assert_round_trip(input: &str) {
        let tokens = tokenize(input);
        let reconstructed: String = tokens
            .iter()
            .map(|t| &input[t.span.start..t.span.end])
            .collect();
        assert_eq!(
            reconstructed, input,
            "round-trip failed for {:?}",
            input
        );
    }

    #[test]
    fn empty_input() {
        let tokens = tokenize("");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
        assert_eq!(tokens[0].span, Span { start: 0, end: 0 });
    }

    #[test]
    fn round_trip_simple() {
        assert_round_trip(r#"rate({job="varlogs"} |= "error" [5m])"#);
    }

    #[test]
    fn round_trip_complex() {
        assert_round_trip(
            r#"sum by (host) (rate({job="varlogs"} |= "error" != "timeout" |~ "5\\d{2}" [5m]))"#,
        );
    }

    #[test]
    fn round_trip_whitespace_and_comments() {
        assert_round_trip("  rate( { job = \"foo\" } [1h] ) # a comment\n");
    }

    #[test]
    fn round_trip_unicode() {
        assert_round_trip(r#"{app="日本語"} |= "émojis 🎉""#);
    }

    // ── String literals ──────────────────────────────────────────────────

    #[test]
    fn double_quoted_string() {
        let tokens = tok(r#""hello world""#);
        assert_eq!(tokens, vec![(TokenKind::String, r#""hello world""#)]);
    }

    #[test]
    fn single_quoted_string() {
        let tokens = tok("'hello'");
        assert_eq!(tokens, vec![(TokenKind::String, "'hello'")]);
    }

    #[test]
    fn backtick_string() {
        let tokens = tok("`raw string`");
        assert_eq!(tokens, vec![(TokenKind::String, "`raw string`")]);
    }

    #[test]
    fn string_with_escapes() {
        let tokens = tok(r#""hello \"world\" \\""#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, TokenKind::String);
    }

    #[test]
    fn backtick_no_escape_processing() {
        // Backtick strings do not process escapes
        let tokens = tok(r#"`hello \n world`"#);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, TokenKind::String);
    }

    #[test]
    fn unclosed_double_string() {
        let tokens = tok(r#""unclosed"#);
        assert_eq!(tokens, vec![(TokenKind::Error, r#""unclosed"#)]);
    }

    #[test]
    fn unclosed_single_string() {
        let tokens = tok("'unclosed");
        assert_eq!(tokens, vec![(TokenKind::Error, "'unclosed")]);
    }

    #[test]
    fn unclosed_backtick_string() {
        let tokens = tok("`unclosed");
        assert_eq!(tokens, vec![(TokenKind::Error, "`unclosed")]);
    }

    // ── Numbers ──────────────────────────────────────────────────────────

    #[test]
    fn integer() {
        let tokens = tok("42");
        assert_eq!(tokens, vec![(TokenKind::Number, "42")]);
    }

    #[test]
    fn float() {
        let tokens = tok("3.14");
        assert_eq!(tokens, vec![(TokenKind::Number, "3.14")]);
    }

    #[test]
    fn leading_dot_float() {
        let tokens = tok(".5");
        assert_eq!(tokens, vec![(TokenKind::Number, ".5")]);
    }

    #[test]
    fn scientific_notation() {
        let tokens = tok("1e10");
        assert_eq!(tokens, vec![(TokenKind::Number, "1e10")]);
    }

    #[test]
    fn scientific_notation_negative() {
        let tokens = tok("3.14e-2");
        assert_eq!(tokens, vec![(TokenKind::Number, "3.14e-2")]);
    }

    // ── Durations ────────────────────────────────────────────────────────

    #[test]
    fn simple_duration() {
        let tokens = tok("5m");
        assert_eq!(tokens, vec![(TokenKind::Duration, "5m")]);
    }

    #[test]
    fn duration_ms() {
        let tokens = tok("500ms");
        assert_eq!(tokens, vec![(TokenKind::Duration, "500ms")]);
    }

    #[test]
    fn duration_ns() {
        let tokens = tok("100ns");
        assert_eq!(tokens, vec![(TokenKind::Duration, "100ns")]);
    }

    #[test]
    fn duration_us() {
        let tokens = tok("200us");
        assert_eq!(tokens, vec![(TokenKind::Duration, "200us")]);
    }

    #[test]
    fn composite_duration() {
        let tokens = tok("1h30m");
        assert_eq!(tokens, vec![(TokenKind::Duration, "1h30m")]);
    }

    #[test]
    fn composite_duration_three_parts() {
        let tokens = tok("1h30m10s");
        assert_eq!(tokens, vec![(TokenKind::Duration, "1h30m10s")]);
    }

    #[test]
    fn all_duration_suffixes() {
        for (input, expected) in [
            ("1ns", "1ns"),
            ("2us", "2us"),
            ("3ms", "3ms"),
            ("4s", "4s"),
            ("5m", "5m"),
            ("6h", "6h"),
            ("7d", "7d"),
            ("8w", "8w"),
            ("9y", "9y"),
        ] {
            let tokens = tok(input);
            assert_eq!(tokens, vec![(TokenKind::Duration, expected)], "input: {input}");
        }
    }

    // ── Operators ────────────────────────────────────────────────────────

    #[test]
    fn pipe_operators() {
        assert_eq!(kinds("|"), vec![TokenKind::Pipe]);
        assert_eq!(kinds("|="), vec![TokenKind::PipeExact]);
        assert_eq!(kinds("|~"), vec![TokenKind::PipeMatch]);
    }

    #[test]
    fn bang_operators() {
        assert_eq!(kinds("!="), vec![TokenKind::NotEqual]);
        assert_eq!(kinds("!~"), vec![TokenKind::NotMatch]);
    }

    #[test]
    fn eq_operators() {
        assert_eq!(kinds("="), vec![TokenKind::Eq]);
        assert_eq!(kinds("=~"), vec![TokenKind::EqRegex]);
        assert_eq!(kinds("=="), vec![TokenKind::CmpEq]);
    }

    #[test]
    fn comparison_operators() {
        assert_eq!(kinds("<"), vec![TokenKind::Lt]);
        assert_eq!(kinds(">"), vec![TokenKind::Gt]);
        assert_eq!(kinds("<="), vec![TokenKind::LtEq]);
        assert_eq!(kinds(">="), vec![TokenKind::GtEq]);
    }

    #[test]
    fn arithmetic_operators() {
        assert_eq!(kinds("+"), vec![TokenKind::Plus]);
        assert_eq!(kinds("-"), vec![TokenKind::Minus]);
        assert_eq!(kinds("*"), vec![TokenKind::Star]);
        assert_eq!(kinds("/"), vec![TokenKind::Slash]);
        assert_eq!(kinds("%"), vec![TokenKind::Percent]);
        assert_eq!(kinds("^"), vec![TokenKind::Caret]);
    }

    // ── Delimiters ───────────────────────────────────────────────────────

    #[test]
    fn delimiters() {
        assert_eq!(kinds("{"), vec![TokenKind::LBrace]);
        assert_eq!(kinds("}"), vec![TokenKind::RBrace]);
        assert_eq!(kinds("("), vec![TokenKind::LParen]);
        assert_eq!(kinds(")"), vec![TokenKind::RParen]);
        assert_eq!(kinds("["), vec![TokenKind::LBracket]);
        assert_eq!(kinds("]"), vec![TokenKind::RBracket]);
        assert_eq!(kinds(","), vec![TokenKind::Comma]);
        assert_eq!(kinds("."), vec![TokenKind::Dot]);
    }

    // ── Identifiers ─────────────────────────────────────────────────────

    #[test]
    fn simple_ident() {
        let tokens = tok("job");
        assert_eq!(tokens, vec![(TokenKind::Ident, "job")]);
    }

    #[test]
    fn ident_with_underscore() {
        let tokens = tok("my_label_123");
        assert_eq!(tokens, vec![(TokenKind::Ident, "my_label_123")]);
    }

    #[test]
    fn ident_starting_with_underscore() {
        let tokens = tok("_private");
        assert_eq!(tokens, vec![(TokenKind::Ident, "_private")]);
    }

    // ── Comments ─────────────────────────────────────────────────────────

    #[test]
    fn line_comment() {
        let tokens = tok("# this is a comment");
        assert_eq!(tokens, vec![(TokenKind::LineComment, "# this is a comment")]);
    }

    #[test]
    fn comment_before_newline() {
        let tokens = tok("rate # comment\njob");
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Ident, "rate"),
                (TokenKind::Whitespace, " "),
                (TokenKind::LineComment, "# comment"),
                (TokenKind::Whitespace, "\n"),
                (TokenKind::Ident, "job"),
            ]
        );
    }

    // ── Whitespace ───────────────────────────────────────────────────────

    #[test]
    fn whitespace_run() {
        let tokens = tok("  \t\n  ");
        assert_eq!(tokens, vec![(TokenKind::Whitespace, "  \t\n  ")]);
    }

    // ── Error tokens ─────────────────────────────────────────────────────

    #[test]
    fn bare_bang_is_error() {
        // `!` alone (not followed by = or ~) is an error
        assert_eq!(kinds("!"), vec![TokenKind::Error]);
    }

    // ── Mixed / realistic expressions ────────────────────────────────────

    #[test]
    fn label_selector() {
        let input = r#"{job="varlogs"}"#;
        let tokens = tok(input);
        assert_eq!(
            tokens,
            vec![
                (TokenKind::LBrace, "{"),
                (TokenKind::Ident, "job"),
                (TokenKind::Eq, "="),
                (TokenKind::String, r#""varlogs""#),
                (TokenKind::RBrace, "}"),
            ]
        );
        assert_round_trip(input);
    }

    #[test]
    fn rate_query() {
        let input = r#"rate({job="varlogs"} |= "error" [5m])"#;
        let tokens = tok(input);
        let expected_kinds = vec![
            TokenKind::Ident,    // rate
            TokenKind::LParen,   // (
            TokenKind::LBrace,   // {
            TokenKind::Ident,    // job
            TokenKind::Eq,       // =
            TokenKind::String,   // "varlogs"
            TokenKind::RBrace,   // }
            TokenKind::Whitespace,
            TokenKind::PipeExact, // |=
            TokenKind::Whitespace,
            TokenKind::String,   // "error"
            TokenKind::Whitespace,
            TokenKind::LBracket, // [
            TokenKind::Duration, // 5m
            TokenKind::RBracket, // ]
            TokenKind::RParen,   // )
        ];
        let actual_kinds: Vec<_> = tokens.iter().map(|t| t.0).collect();
        assert_eq!(actual_kinds, expected_kinds);
    }

    #[test]
    fn aggregation_with_by() {
        let input = r#"sum by (host) (rate({job="varlogs"} [5m]))"#;
        assert_round_trip(input);

        let tokens = tok(input);
        assert_eq!(tokens[0], (TokenKind::Ident, "sum"));
        assert_eq!(tokens[2], (TokenKind::Ident, "by"));
    }

    #[test]
    fn pipeline_with_multiple_stages() {
        let input = r#"{job="app"} |= "error" != "timeout" |~ "5\\d{2}" | json | line_format "{{.msg}}""#;
        assert_round_trip(input);
    }

    #[test]
    fn mixed_operators_no_spaces() {
        let input = "!=|=|~=~==<=>=";
        let expected = vec![
            TokenKind::NotEqual,  // !=
            TokenKind::PipeExact, // |=
            TokenKind::PipeMatch, // |~
            TokenKind::EqRegex,   // =~
            TokenKind::CmpEq,    // ==
            TokenKind::LtEq,     // <=
            TokenKind::GtEq,     // >=
        ];
        assert_eq!(kinds(input), expected);
        assert_round_trip(input);
    }

    // ── classify_keyword ─────────────────────────────────────────────────

    #[test]
    fn classify_range_functions() {
        assert_eq!(classify_keyword("rate"), KeywordKind::RangeFunction);
        assert_eq!(classify_keyword("count_over_time"), KeywordKind::RangeFunction);
        assert_eq!(classify_keyword("sum_over_time"), KeywordKind::RangeFunction);
        assert_eq!(classify_keyword("bytes_rate"), KeywordKind::RangeFunction);
    }

    #[test]
    fn classify_aggregation_ops() {
        assert_eq!(classify_keyword("sum"), KeywordKind::AggregationOp);
        assert_eq!(classify_keyword("avg"), KeywordKind::AggregationOp);
        assert_eq!(classify_keyword("topk"), KeywordKind::AggregationOp);
        assert_eq!(classify_keyword("count"), KeywordKind::AggregationOp);
    }

    #[test]
    fn classify_aggregation_modifiers() {
        assert_eq!(classify_keyword("by"), KeywordKind::AggregationModifier);
        assert_eq!(classify_keyword("without"), KeywordKind::AggregationModifier);
    }

    #[test]
    fn classify_binary_keywords() {
        assert_eq!(classify_keyword("and"), KeywordKind::BinaryKeyword);
        assert_eq!(classify_keyword("or"), KeywordKind::BinaryKeyword);
        assert_eq!(classify_keyword("unless"), KeywordKind::BinaryKeyword);
    }

    #[test]
    fn classify_pipeline_parsers() {
        assert_eq!(classify_keyword("json"), KeywordKind::PipelineParser);
        assert_eq!(classify_keyword("logfmt"), KeywordKind::PipelineParser);
        assert_eq!(classify_keyword("regexp"), KeywordKind::PipelineParser);
        assert_eq!(classify_keyword("pattern"), KeywordKind::PipelineParser);
        assert_eq!(classify_keyword("unpack"), KeywordKind::PipelineParser);
    }

    #[test]
    fn classify_pipeline_formatters() {
        assert_eq!(classify_keyword("line_format"), KeywordKind::PipelineFormatter);
        assert_eq!(classify_keyword("label_format"), KeywordKind::PipelineFormatter);
    }

    #[test]
    fn classify_pipeline_label_ops() {
        assert_eq!(classify_keyword("drop"), KeywordKind::PipelineLabelOp);
        assert_eq!(classify_keyword("keep"), KeywordKind::PipelineLabelOp);
        assert_eq!(classify_keyword("decolorize"), KeywordKind::PipelineLabelOp);
    }

    #[test]
    fn classify_filter_keyword() {
        assert_eq!(classify_keyword("unwrap"), KeywordKind::FilterKeyword);
    }

    #[test]
    fn classify_not_keyword() {
        assert_eq!(classify_keyword("myapp"), KeywordKind::NotKeyword);
        assert_eq!(classify_keyword("job"), KeywordKind::NotKeyword);
        assert_eq!(classify_keyword("namespace"), KeywordKind::NotKeyword);
    }

    // ── Span correctness ─────────────────────────────────────────────────

    #[test]
    fn spans_are_contiguous() {
        let input = r#"sum(rate({job="test"}[5m]))"#;
        let tokens = tokenize(input);
        // Verify spans cover the entire input with no gaps
        let mut expected_start = 0;
        for token in &tokens {
            if token.kind == TokenKind::Eof {
                assert_eq!(token.span.start, input.len());
                assert_eq!(token.span.end, input.len());
                break;
            }
            assert_eq!(
                token.span.start, expected_start,
                "gap before token {:?} at byte {}",
                token.kind, token.span.start
            );
            assert!(token.span.end > token.span.start, "empty span");
            expected_start = token.span.end;
        }
        assert_eq!(expected_start, input.len());
    }

    #[test]
    fn eof_at_end_of_empty() {
        let tokens = tokenize("");
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn eof_at_end_of_nonempty() {
        let tokens = tokenize("x");
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
        assert_eq!(tokens.last().unwrap().span, Span { start: 1, end: 1 });
    }
}
