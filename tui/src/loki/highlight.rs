use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use shared::loki::tokenizer::{self, classify_keyword, KeywordKind, TokenKind};

fn token_style(kind: TokenKind, keyword: Option<KeywordKind>, in_braces: bool) -> Style {
    match kind {
        TokenKind::Ident => {
            if in_braces {
                Style::default().fg(Color::LightCyan)
            } else {
                match keyword.unwrap_or(KeywordKind::NotKeyword) {
                    KeywordKind::AggregationOp => {
                        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
                    }
                    KeywordKind::RangeFunction => Style::default().fg(Color::Cyan),
                    KeywordKind::BinaryKeyword => Style::default().fg(Color::Yellow),
                    KeywordKind::AggregationModifier => Style::default().fg(Color::Blue),
                    KeywordKind::PipelineParser => {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    }
                    KeywordKind::PipelineFormatter => {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                    }
                    KeywordKind::PipelineLabelOp => Style::default().fg(Color::LightBlue),
                    KeywordKind::FilterKeyword => Style::default().fg(Color::Yellow),
                    KeywordKind::NotKeyword => Style::default().fg(Color::White),
                }
            }
        }

        TokenKind::String => {
            if in_braces {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Green)
            }
        }

        TokenKind::Number => Style::default().fg(Color::Yellow),
        TokenKind::Duration => Style::default().fg(Color::Yellow),

        // Pipe operators — distinctive in LogQL
        TokenKind::Pipe => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        TokenKind::PipeExact => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        TokenKind::PipeMatch => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),

        // Filter / comparison operators
        TokenKind::NotEqual | TokenKind::NotMatch => Style::default().fg(Color::Magenta),
        TokenKind::Eq | TokenKind::EqRegex | TokenKind::CmpEq => Style::default().fg(Color::Magenta),
        TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq => {
            Style::default().fg(Color::Magenta)
        }

        // Arithmetic operators
        TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash
        | TokenKind::Percent | TokenKind::Caret => Style::default().fg(Color::Magenta),

        // Delimiters
        TokenKind::LBrace | TokenKind::RBrace | TokenKind::LParen | TokenKind::RParen
        | TokenKind::LBracket | TokenKind::RBracket | TokenKind::Comma | TokenKind::Dot => {
            Style::default().fg(Color::DarkGray)
        }

        TokenKind::LineComment => Style::default().fg(Color::DarkGray),
        TokenKind::Error => Style::default().fg(Color::Red),
        TokenKind::Whitespace | TokenKind::Eof => Style::default(),
    }
}

/// Produce a syntax-highlighted `Line` with a block cursor at `cursor_byte`
/// (a byte offset into `query`).
pub fn highlight_loki_query(query: &str, cursor_byte: usize) -> Line<'static> {
    let cursor_style = Style::default().fg(Color::Black).bg(Color::White);

    let tokens = tokenizer::tokenize(query);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor_done = false;
    let mut brace_depth: i32 = 0;

    for tok in &tokens {
        if tok.kind == TokenKind::Eof {
            break;
        }

        // Track brace depth for label-name context
        match tok.kind {
            TokenKind::LBrace => brace_depth += 1,
            TokenKind::RBrace => brace_depth = (brace_depth - 1).max(0),
            _ => {}
        }

        let in_braces = brace_depth > 0
            && !matches!(tok.kind, TokenKind::LBrace | TokenKind::RBrace);

        let keyword = if tok.kind == TokenKind::Ident {
            Some(classify_keyword(tok.text(query)))
        } else {
            None
        };

        let style = token_style(tok.kind, keyword, in_braces);
        let start = tok.span.start;
        let end = tok.span.end;

        if cursor_done {
            spans.push(Span::styled(query[start..end].to_owned(), style));
            continue;
        }

        if cursor_byte >= start && cursor_byte < end {
            // Cursor inside this token
            if start < cursor_byte {
                spans.push(Span::styled(query[start..cursor_byte].to_owned(), style));
            }

            let ch_len = query[cursor_byte..]
                .chars()
                .next()
                .map_or(1, |c| c.len_utf8());
            spans.push(Span::styled(
                query[cursor_byte..cursor_byte + ch_len].to_owned(),
                cursor_style,
            ));
            cursor_done = true;

            if cursor_byte + ch_len < end {
                spans.push(Span::styled(
                    query[cursor_byte + ch_len..end].to_owned(),
                    style,
                ));
            }
        } else {
            spans.push(Span::styled(query[start..end].to_owned(), style));
        }
    }

    if !cursor_done {
        spans.push(Span::styled(" ", cursor_style));
    }

    Line::from(spans)
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn highlight_empty_query() {
        let line = highlight_loki_query("", 0);
        assert_eq!(plain_text(&line), " ");
    }

    #[test]
    fn highlight_cursor_at_end() {
        let line = highlight_loki_query("{job=\"app\"}", 11);
        assert_eq!(plain_text(&line), "{job=\"app\"} ");
    }

    #[test]
    fn highlight_cursor_in_middle() {
        let line = highlight_loki_query("rate", 2);
        assert_eq!(plain_text(&line), "rate");
    }

    #[test]
    fn highlight_cursor_at_start() {
        let line = highlight_loki_query("sum", 0);
        let cursor = &line.spans[0];
        assert_eq!(cursor.content.as_ref(), "s");
        assert_eq!(cursor.style.bg, Some(Color::White));
        assert_eq!(cursor.style.fg, Some(Color::Black));
    }

    #[test]
    fn highlight_range_function() {
        let line = highlight_loki_query("rate({job=\"app\"}[5m])", 21);
        let rate_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "rate")
            .expect("rate span missing");
        assert_eq!(rate_span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn highlight_aggregation_op() {
        let line = highlight_loki_query("sum by (job) (rate({app=\"x\"}[1m]))", 34);
        let sum_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "sum")
            .expect("sum span missing");
        assert_eq!(sum_span.style.fg, Some(Color::Magenta));
        assert!(sum_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn highlight_aggregation_modifier() {
        let line = highlight_loki_query("sum by (job) (x)", 16);
        let by_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "by")
            .expect("by span missing");
        assert_eq!(by_span.style.fg, Some(Color::Blue));
    }

    #[test]
    fn highlight_label_name_inside_braces() {
        let query = r#"{job="varlogs"}"#;
        let line = highlight_loki_query(query, query.len());
        let job_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "job")
            .expect("job span missing");
        assert_eq!(job_span.style.fg, Some(Color::LightCyan));
    }

    #[test]
    fn highlight_label_value_inside_braces() {
        let query = r#"{job="varlogs"}"#;
        let line = highlight_loki_query(query, query.len());
        let val_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == r#""varlogs""#)
            .expect("value span missing");
        assert_eq!(val_span.style.fg, Some(Color::Green));
    }

    #[test]
    fn highlight_pipe_operator() {
        let query = r#"{job="app"} |= "error""#;
        let line = highlight_loki_query(query, query.len());
        let pipe_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "|=")
            .expect("|= span missing");
        assert_eq!(pipe_span.style.fg, Some(Color::Magenta));
        assert!(pipe_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn highlight_pipeline_parser() {
        let query = r#"{job="app"} | json"#;
        let line = highlight_loki_query(query, query.len());
        let json_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "json")
            .expect("json span missing");
        assert_eq!(json_span.style.fg, Some(Color::Cyan));
        assert!(json_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn highlight_duration() {
        let query = r#"rate({job="app"}[5m])"#;
        let line = highlight_loki_query(query, query.len());
        let dur_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "5m")
            .expect("5m span missing");
        assert_eq!(dur_span.style.fg, Some(Color::Yellow));
    }

    #[test]
    fn highlight_string_outside_braces() {
        let query = r#"{job="app"} |= "error""#;
        let line = highlight_loki_query(query, query.len());
        let str_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == r#""error""#)
            .expect("error string span missing");
        assert_eq!(str_span.style.fg, Some(Color::Green));
    }

    #[test]
    fn highlight_preserves_string_width() {
        let query = r#"rate({job="app"}[5m])"#;
        for pos in 0..=query.len() {
            // Only test at valid char boundaries
            if !query.is_char_boundary(pos) {
                continue;
            }
            let line = highlight_loki_query(query, pos);
            let text = plain_text(&line);
            if pos < query.len() {
                assert_eq!(text, query, "width mismatch at cursor_byte={pos}");
            } else {
                assert_eq!(text, format!("{query} "), "end cursor at pos={pos}");
            }
        }
    }

    #[test]
    fn highlight_cursor_inside_each_token() {
        let query = r#"{job="app"} |= "err""#;
        // Test cursor at byte 0 ('{'), byte 1 ('j'), byte 5 ('"'), etc.
        for pos in 0..query.len() {
            if !query.is_char_boundary(pos) {
                continue;
            }
            let line = highlight_loki_query(query, pos);
            let text = plain_text(&line);
            assert_eq!(text, query, "mismatch at pos={pos}");
            // Exactly one span should have cursor style
            let cursor_spans: Vec<_> = line
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(Color::White) && s.style.fg == Some(Color::Black))
                .collect();
            assert_eq!(cursor_spans.len(), 1, "expected 1 cursor span at pos={pos}");
        }
    }

    #[test]
    fn highlight_line_comment() {
        let query = "rate # comment";
        let line = highlight_loki_query(query, query.len());
        let comment_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "# comment")
            .expect("comment span missing");
        assert_eq!(comment_span.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn highlight_error_token() {
        let query = r#""unclosed"#;
        let line = highlight_loki_query(query, query.len());
        let err_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == r#""unclosed"#)
            .expect("error span missing");
        assert_eq!(err_span.style.fg, Some(Color::Red));
    }
}
