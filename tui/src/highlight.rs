use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use shared::prometheus::keywords;

// ── Token types ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum TokenKind {
    AggregationOp,   // bold magenta
    Function,        // cyan
    BinaryKeyword,   // yellow
    ModifierKeyword, // blue
    MetricName,      // white (plain identifier)
    LabelName,       // light cyan (inside {})
    LabelValue,      // green (string inside {})
    StringLit,       // green
    Number,          // yellow
    Duration,        // yellow
    Operator,        // magenta
    Punct,           // dark gray
    Whitespace,      // default
    Other,           // default
}

struct Token {
    start: usize, // byte offset in source
    end: usize,
    kind: TokenKind,
}

// ── Tokenizer ─────────────────────────────────────────────────────────────────

fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut tokens = Vec::new();
    let mut brace_depth: i32 = 0;

    while pos < len {
        let start = pos;

        match bytes[pos] {
            // Whitespace
            b' ' | b'\t' | b'\r' | b'\n' => {
                while pos < len && matches!(bytes[pos], b' ' | b'\t' | b'\r' | b'\n') {
                    pos += 1;
                }
                tokens.push(Token { start, end: pos, kind: TokenKind::Whitespace });
            }

            // String literals
            b'"' | b'\'' | b'`' => {
                let delim = bytes[pos];
                pos += 1;
                while pos < len {
                    if bytes[pos] == b'\\' && delim != b'`' {
                        pos = (pos + 2).min(len);
                        continue;
                    }
                    if bytes[pos] == delim {
                        pos += 1;
                        break;
                    }
                    pos += 1;
                }
                let kind = if brace_depth > 0 {
                    TokenKind::LabelValue
                } else {
                    TokenKind::StringLit
                };
                tokens.push(Token { start, end: pos, kind });
            }

            // Numbers (and duration suffixes: ms s m h d w y)
            b'0'..=b'9' => {
                while pos < len && (bytes[pos].is_ascii_digit() || bytes[pos] == b'.') {
                    pos += 1;
                }
                let kind = if pos < len {
                    match bytes[pos] {
                        b'm' => {
                            pos += 1;
                            if pos < len && bytes[pos] == b's' {
                                pos += 1;
                            }
                            TokenKind::Duration
                        }
                        b's' | b'h' | b'd' | b'w' | b'y' => {
                            pos += 1;
                            TokenKind::Duration
                        }
                        _ => TokenKind::Number,
                    }
                } else {
                    TokenKind::Number
                };
                tokens.push(Token { start, end: pos, kind });
            }

            // Identifiers and keywords
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                // PromQL metric names may include colons (recording rules)
                while pos < len
                    && (bytes[pos].is_ascii_alphanumeric()
                        || bytes[pos] == b'_'
                        || bytes[pos] == b':')
                {
                    pos += 1;
                }
                let text = &input[start..pos];
                let kind = if brace_depth > 0 {
                    TokenKind::LabelName
                } else {
                    classify_ident(text)
                };
                tokens.push(Token { start, end: pos, kind });
            }

            // Operators
            b'=' => {
                pos += 1;
                if pos < len && bytes[pos] == b'~' {
                    pos += 1;
                }
                tokens.push(Token { start, end: pos, kind: TokenKind::Operator });
            }
            b'!' => {
                pos += 1;
                if pos < len && (bytes[pos] == b'=' || bytes[pos] == b'~') {
                    pos += 1;
                }
                tokens.push(Token { start, end: pos, kind: TokenKind::Operator });
            }
            b'+' | b'-' | b'*' | b'/' | b'%' | b'^' => {
                pos += 1;
                tokens.push(Token { start, end: pos, kind: TokenKind::Operator });
            }
            b'<' | b'>' => {
                pos += 1;
                if pos < len && bytes[pos] == b'=' {
                    pos += 1;
                }
                tokens.push(Token { start, end: pos, kind: TokenKind::Operator });
            }
            b'@' => {
                pos += 1;
                tokens.push(Token { start, end: pos, kind: TokenKind::Operator });
            }

            // Punctuation – track brace depth for label context
            b'{' => {
                pos += 1;
                brace_depth += 1;
                tokens.push(Token { start, end: pos, kind: TokenKind::Punct });
            }
            b'}' => {
                pos += 1;
                brace_depth = (brace_depth - 1).max(0);
                tokens.push(Token { start, end: pos, kind: TokenKind::Punct });
            }
            b'(' | b')' | b'[' | b']' | b',' | b';' => {
                pos += 1;
                tokens.push(Token { start, end: pos, kind: TokenKind::Punct });
            }

            // Multi-byte UTF-8 or unrecognised ASCII
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
                tokens.push(Token { start, end: pos, kind: TokenKind::Other });
            }
        }
    }

    tokens
}

fn classify_ident(text: &str) -> TokenKind {
    if keywords::AGGREGATION_OPS.contains(&text) {
        TokenKind::AggregationOp
    } else if keywords::FUNCTIONS.contains(&text) {
        TokenKind::Function
    } else if keywords::BINARY_KEYWORDS.contains(&text) {
        TokenKind::BinaryKeyword
    } else if keywords::BINARY_MODIFIERS.contains(&text)
        || keywords::AGGREGATION_MODIFIERS.contains(&text)
    {
        TokenKind::ModifierKeyword
    } else {
        TokenKind::MetricName
    }
}

fn token_style(kind: TokenKind) -> Style {
    match kind {
        TokenKind::AggregationOp => {
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
        }
        TokenKind::Function => Style::default().fg(Color::Cyan),
        TokenKind::BinaryKeyword => Style::default().fg(Color::Yellow),
        TokenKind::ModifierKeyword => Style::default().fg(Color::Blue),
        TokenKind::MetricName => Style::default().fg(Color::White),
        TokenKind::LabelName => Style::default().fg(Color::LightCyan),
        TokenKind::LabelValue | TokenKind::StringLit => Style::default().fg(Color::Green),
        TokenKind::Number | TokenKind::Duration => Style::default().fg(Color::Yellow),
        TokenKind::Operator => Style::default().fg(Color::Magenta),
        TokenKind::Punct => Style::default().fg(Color::DarkGray),
        TokenKind::Whitespace | TokenKind::Other => Style::default(),
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Produce a syntax-highlighted `Line` with a block cursor injected at
/// `cursor_pos` (a char index, not a byte offset).
pub fn highlight_query(query: &str, cursor_pos: usize) -> Line<'static> {
    let cursor_byte = query
        .char_indices()
        .nth(cursor_pos)
        .map(|(i, _)| i)
        .unwrap_or(query.len());

    let cursor_style = Style::default().fg(Color::Black).bg(Color::White);

    let tokens = tokenize(query);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor_done = false;

    for tok in &tokens {
        if cursor_done {
            spans.push(Span::styled(
                query[tok.start..tok.end].to_owned(),
                token_style(tok.kind),
            ));
            continue;
        }

        if cursor_byte >= tok.start && cursor_byte < tok.end {
            // Cursor is inside this token: highlight the character under the cursor
            // without inserting any extra space, so the string width stays constant.
            let style = token_style(tok.kind);

            if tok.start < cursor_byte {
                spans.push(Span::styled(query[tok.start..cursor_byte].to_owned(), style));
            }

            // One UTF-8 character at cursor_byte.
            let ch_len = query[cursor_byte..]
                .chars()
                .next()
                .map_or(1, |c| c.len_utf8());
            spans.push(Span::styled(
                query[cursor_byte..cursor_byte + ch_len].to_owned(),
                cursor_style,
            ));
            cursor_done = true;

            if cursor_byte + ch_len < tok.end {
                spans.push(Span::styled(
                    query[cursor_byte + ch_len..tok.end].to_owned(),
                    style,
                ));
            }
        } else {
            spans.push(Span::styled(
                query[tok.start..tok.end].to_owned(),
                token_style(tok.kind),
            ));
        }
    }

    if !cursor_done {
        spans.push(Span::styled(" ", cursor_style));
    }

    Line::from(spans)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn highlight_empty_query() {
        let line = highlight_query("", 0);
        assert_eq!(plain_text(&line), " ");
    }

    #[test]
    fn highlight_cursor_at_end() {
        let line = highlight_query("up", 2);
        assert_eq!(plain_text(&line), "up ");
    }

    #[test]
    fn highlight_cursor_in_middle() {
        // Cursor highlights the char under it; no extra space is inserted.
        let line = highlight_query("up", 1);
        assert_eq!(plain_text(&line), "up");
    }

    #[test]
    fn highlight_function_keyword() {
        let line = highlight_query("rate(up[5m])", 12);
        let text = plain_text(&line);
        assert!(text.contains("rate"));
        assert!(text.contains("up"));
        assert!(text.contains("5m"));

        let rate_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "rate")
            .expect("rate span missing");
        assert_eq!(rate_span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn highlight_aggregation_op() {
        let line = highlight_query("sum(up)", 7);
        let sum_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "sum")
            .expect("sum span missing");
        assert_eq!(sum_span.style.fg, Some(Color::Magenta));
        assert!(sum_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn highlight_label_context() {
        let query = r#"up{job="node"}"#;
        let line = highlight_query(query, query.chars().count());
        let job_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "job")
            .expect("job span missing");
        assert_eq!(job_span.style.fg, Some(Color::LightCyan));

        let val_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == r#""node""#)
            .expect("value span missing");
        assert_eq!(val_span.style.fg, Some(Color::Green));
    }

    #[test]
    fn highlight_cursor_style() {
        // The cursor highlights the character under it, not an injected space.
        let line = highlight_query("x", 0);
        let cursor = &line.spans[0];
        assert_eq!(cursor.content.as_ref(), "x");
        assert_eq!(cursor.style.bg, Some(Color::White));
        assert_eq!(cursor.style.fg, Some(Color::Black));
    }
}
