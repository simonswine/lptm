//! Recursive-descent parser for LogQL with error recovery.
//!
//! Entry point: [`parse`] — tokenizes the input and produces an AST with
//! [`Span`] on every node. The parser records [`ParseError`] diagnostics and
//! skips to sync points so it can produce partial ASTs for incomplete queries.

use super::ast::*;
use super::tokenizer::{classify_keyword, tokenize, KeywordKind, Span, Token, TokenKind};

/// Parse a LogQL query string into an AST with error recovery.
pub fn parse(input: &str) -> ParseResult {
    let all_tokens = tokenize(input);
    let tokens: Vec<Token> = all_tokens
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::LineComment))
        .collect();
    let mut parser = Parser {
        input,
        tokens,
        pos: 0,
        errors: Vec::new(),
    };
    let expr = parser.parse_expr();
    if !parser.at_eof() {
        let tok = parser.peek().clone();
        parser.errors.push(ParseError {
            message: format!("unexpected trailing token {:?}", tok.kind),
            span: tok.span,
        });
    }
    ParseResult {
        expr,
        errors: parser.errors,
    }
}

// ── Parser state ────────────────────────────────────────────────────────────

struct Parser<'a> {
    input: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    // ── Token access ────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> TokenKind {
        self.peek().kind
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn text(&self, token: &Token) -> &'a str {
        token.text(self.input)
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.advance())
        } else {
            let tok = self.peek().clone();
            self.errors.push(ParseError {
                message: format!("expected {:?}, found {:?}", kind, tok.kind),
                span: tok.span,
            });
            None
        }
    }

    /// Strip delimiters from a String token to get the inner value.
    fn extract_string(&self, token: &Token) -> String {
        let text = self.text(token);
        if text.len() < 2 {
            return text.to_string();
        }
        text[1..text.len() - 1].to_string()
    }

    /// Skip tokens until a sync point for error recovery.
    fn skip_to_sync(&mut self) {
        while !self.at_eof() {
            match self.peek_kind() {
                TokenKind::Pipe
                | TokenKind::PipeExact
                | TokenKind::PipeMatch
                | TokenKind::NotEqual
                | TokenKind::NotMatch
                | TokenKind::RBrace
                | TokenKind::RParen
                | TokenKind::RBracket => break,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ── Expression parsing ──────────────────────────────────────────────

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_binary_expr(1)
    }

    /// Precedence-climbing binary expression parser.
    ///
    /// Precedence levels:
    /// 1 = or, 2 = and/unless, 3 = ==/!=/</>/<=/>=,
    /// 4 = +/-, 5 = */%,  6 = ^ (right-associative)
    fn parse_binary_expr(&mut self, min_prec: u8) -> Option<Expr> {
        let mut lhs = self.parse_unary_expr()?;

        loop {
            let (op, prec) = match self.peek_kind() {
                TokenKind::Ident => {
                    let text = self.text(self.peek());
                    match text {
                        "or" => (BinOpKind::Or, 1),
                        "and" => (BinOpKind::And, 2),
                        "unless" => (BinOpKind::Unless, 2),
                        _ => break,
                    }
                }
                TokenKind::CmpEq => (BinOpKind::CmpEq, 3),
                TokenKind::NotEqual => (BinOpKind::Neq, 3),
                TokenKind::Lt => (BinOpKind::Lt, 3),
                TokenKind::Gt => (BinOpKind::Gt, 3),
                TokenKind::LtEq => (BinOpKind::LtEq, 3),
                TokenKind::GtEq => (BinOpKind::GtEq, 3),
                TokenKind::Plus => (BinOpKind::Add, 4),
                TokenKind::Minus => (BinOpKind::Sub, 4),
                TokenKind::Star => (BinOpKind::Mul, 5),
                TokenKind::Slash => (BinOpKind::Div, 5),
                TokenKind::Percent => (BinOpKind::Mod, 5),
                TokenKind::Caret => (BinOpKind::Pow, 6),
                _ => break,
            };

            if prec < min_prec {
                break;
            }

            self.advance();

            let next_prec = if op == BinOpKind::Pow {
                prec // right-associative
            } else {
                prec + 1
            };

            let rhs = match self.parse_binary_expr(next_prec) {
                Some(r) => r,
                None => break,
            };

            let span = Span {
                start: lhs.span.start,
                end: rhs.span.end,
            };
            lhs = Expr {
                kind: ExprKind::BinOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }

        Some(lhs)
    }

    fn parse_unary_expr(&mut self) -> Option<Expr> {
        match self.peek_kind() {
            TokenKind::LBrace => self.parse_log_query(),
            TokenKind::LParen => self.parse_paren_expr(),
            TokenKind::Number => self.parse_number_lit(),
            TokenKind::String => self.parse_string_lit(),
            TokenKind::Ident => {
                let kw = classify_keyword(self.text(self.peek()));
                match kw {
                    KeywordKind::RangeFunction => self.parse_range_aggregation(),
                    KeywordKind::AggregationOp => self.parse_aggregation(),
                    _ => {
                        let tok = self.peek().clone();
                        self.errors.push(ParseError {
                            message: format!(
                                "unexpected identifier '{}'",
                                self.text(&tok)
                            ),
                            span: tok.span,
                        });
                        self.skip_to_sync();
                        None
                    }
                }
            }
            TokenKind::Eof => None,
            _ => {
                let tok = self.peek().clone();
                self.errors.push(ParseError {
                    message: format!("unexpected token {:?}", tok.kind),
                    span: tok.span,
                });
                self.skip_to_sync();
                None
            }
        }
    }

    fn parse_paren_expr(&mut self) -> Option<Expr> {
        let open = self.advance();
        let inner = self.parse_expr()?;
        let close = self.expect(TokenKind::RParen);
        let end = close.map_or(inner.span.end, |t| t.span.end);
        Some(Expr {
            kind: ExprKind::Paren(Box::new(inner)),
            span: Span {
                start: open.span.start,
                end,
            },
        })
    }

    fn parse_number_lit(&mut self) -> Option<Expr> {
        let tok = self.advance();
        let value = self.text(&tok).parse::<f64>().unwrap_or(0.0);
        Some(Expr {
            kind: ExprKind::NumberLit(value),
            span: tok.span,
        })
    }

    fn parse_string_lit(&mut self) -> Option<Expr> {
        let tok = self.advance();
        let value = self.extract_string(&tok);
        Some(Expr {
            kind: ExprKind::StringLit(value),
            span: tok.span,
        })
    }

    // ── Log query ───────────────────────────────────────────────────────

    fn parse_log_query(&mut self) -> Option<Expr> {
        let selector = self.parse_stream_selector()?;
        let start = selector.span.start;
        let mut end = selector.span.end;
        let mut pipeline = Vec::new();

        loop {
            match self.peek_kind() {
                TokenKind::PipeExact
                | TokenKind::PipeMatch
                | TokenKind::NotEqual
                | TokenKind::NotMatch => {
                    if let Some(stage) = self.parse_filter_stage() {
                        end = stage.span().end;
                        pipeline.push(stage);
                    }
                }
                TokenKind::Pipe => {
                    if let Some(stage) = self.parse_pipe_stage() {
                        end = stage.span().end;
                        pipeline.push(stage);
                    }
                }
                _ => break,
            }
        }

        Some(Expr {
            kind: ExprKind::LogQuery { selector, pipeline },
            span: Span { start, end },
        })
    }

    fn parse_stream_selector(&mut self) -> Option<StreamSelector> {
        let open = self.expect(TokenKind::LBrace)?;
        let start = open.span.start;
        let mut matchers = Vec::new();

        while !self.at(TokenKind::RBrace) && !self.at_eof() {
            match self.parse_label_matcher() {
                Some(m) => matchers.push(m),
                None => {
                    self.skip_to_sync();
                    if self.at(TokenKind::RBrace) {
                        break;
                    }
                    // If we landed on a pipe operator, stop the selector.
                    if matches!(
                        self.peek_kind(),
                        TokenKind::Pipe
                            | TokenKind::PipeExact
                            | TokenKind::PipeMatch
                            | TokenKind::NotEqual
                            | TokenKind::NotMatch
                    ) {
                        break;
                    }
                    break;
                }
            }
            if self.at(TokenKind::Comma) {
                self.advance();
            }
        }

        let close = self.expect(TokenKind::RBrace);
        let end = close.map_or_else(
            || matchers.last().map_or(open.span.end, |m| m.span.end),
            |t| t.span.end,
        );

        Some(StreamSelector {
            matchers,
            span: Span { start, end },
        })
    }

    fn parse_label_matcher(&mut self) -> Option<LabelMatcher> {
        let name_tok = self.expect(TokenKind::Ident)?;
        let name = self.text(&name_tok).to_string();
        let start = name_tok.span.start;

        let op = match self.peek_kind() {
            TokenKind::Eq => {
                self.advance();
                MatchOp::Eq
            }
            TokenKind::NotEqual => {
                self.advance();
                MatchOp::Neq
            }
            TokenKind::EqRegex => {
                self.advance();
                MatchOp::Re
            }
            TokenKind::NotMatch => {
                self.advance();
                MatchOp::Nre
            }
            _ => {
                let tok = self.peek().clone();
                self.errors.push(ParseError {
                    message: format!("expected matcher operator, found {:?}", tok.kind),
                    span: tok.span,
                });
                return None;
            }
        };

        let value_tok = self.expect(TokenKind::String)?;
        let value = self.extract_string(&value_tok);

        Some(LabelMatcher {
            name,
            op,
            value,
            span: Span {
                start,
                end: value_tok.span.end,
            },
        })
    }

    // ── Pipeline stages ─────────────────────────────────────────────────

    fn parse_filter_stage(&mut self) -> Option<PipelineStage> {
        let tok = self.advance();
        let op = match tok.kind {
            TokenKind::PipeExact => FilterOp::PipeExact,
            TokenKind::PipeMatch => FilterOp::PipeMatch,
            TokenKind::NotEqual => FilterOp::NotEqual,
            TokenKind::NotMatch => FilterOp::NotMatch,
            _ => unreachable!(),
        };

        let pattern_tok = self.expect(TokenKind::String)?;
        let pattern = self.extract_string(&pattern_tok);

        Some(PipelineStage::Filter {
            op,
            pattern,
            span: Span {
                start: tok.span.start,
                end: pattern_tok.span.end,
            },
        })
    }

    fn parse_pipe_stage(&mut self) -> Option<PipelineStage> {
        let pipe_tok = self.advance(); // consume |
        let start = pipe_tok.span.start;

        if !self.at(TokenKind::Ident) {
            let tok = self.peek().clone();
            self.errors.push(ParseError {
                message: format!("expected stage keyword after '|', found {:?}", tok.kind),
                span: tok.span,
            });
            self.skip_to_sync();
            return None;
        }

        let ident_text = self.text(self.peek()).to_string();
        let kw = classify_keyword(&ident_text);

        match kw {
            KeywordKind::PipelineParser => self.parse_parser_stage(start),
            KeywordKind::PipelineFormatter => self.parse_formatter_stage(start),
            KeywordKind::PipelineLabelOp => self.parse_label_op_stage(start),
            KeywordKind::FilterKeyword => self.parse_unwrap_stage(start),
            _ => self.parse_label_filter_or_error(start),
        }
    }

    fn parse_parser_stage(&mut self, start: usize) -> Option<PipelineStage> {
        let ident_tok = self.advance();
        let kind = match self.text(&ident_tok) {
            "json" => ParserKind::Json,
            "logfmt" => ParserKind::Logfmt,
            "pattern" => ParserKind::Pattern,
            "regexp" => ParserKind::Regexp,
            "unpack" => ParserKind::Unpack,
            "pack" => ParserKind::Pack,
            _ => unreachable!(),
        };

        let mut params = Vec::new();
        let mut end = ident_tok.span.end;

        if self.at(TokenKind::String) {
            let param_tok = self.advance();
            params.push(self.extract_string(&param_tok));
            end = param_tok.span.end;
        }

        Some(PipelineStage::Parser {
            kind,
            params,
            span: Span { start, end },
        })
    }

    fn parse_formatter_stage(&mut self, start: usize) -> Option<PipelineStage> {
        let ident_tok = self.advance();
        let ident_text = self.text(&ident_tok).to_string();

        match ident_text.as_str() {
            "line_format" => {
                let tmpl_tok = self.expect(TokenKind::String)?;
                let template = self.extract_string(&tmpl_tok);
                Some(PipelineStage::LineFormat {
                    template,
                    span: Span {
                        start,
                        end: tmpl_tok.span.end,
                    },
                })
            }
            "label_format" => {
                let mut mappings = Vec::new();
                let mut end = ident_tok.span.end;

                while self.at(TokenKind::Ident) {
                    let dst_tok = self.advance();
                    let dst = self.text(&dst_tok).to_string();

                    if self.at(TokenKind::Eq) {
                        self.advance();
                        if self.at(TokenKind::String) {
                            let val_tok = self.advance();
                            end = val_tok.span.end;
                            mappings.push((dst, self.extract_string(&val_tok)));
                        } else if self.at(TokenKind::Ident) {
                            let val_tok = self.advance();
                            end = val_tok.span.end;
                            mappings.push((dst, self.text(&val_tok).to_string()));
                        } else {
                            break;
                        }
                    } else {
                        end = dst_tok.span.end;
                        mappings.push((dst, String::new()));
                    }

                    if self.at(TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }

                Some(PipelineStage::LabelFormat {
                    mappings,
                    span: Span { start, end },
                })
            }
            _ => unreachable!(),
        }
    }

    fn parse_label_op_stage(&mut self, start: usize) -> Option<PipelineStage> {
        let ident_tok = self.advance();
        let ident_text = self.text(&ident_tok);

        if ident_text == "decolorize" {
            return Some(PipelineStage::Decolorize {
                span: Span {
                    start,
                    end: ident_tok.span.end,
                },
            });
        }

        let kind = match ident_text {
            "drop" => LabelOpKind::Drop,
            "keep" => LabelOpKind::Keep,
            _ => unreachable!(),
        };

        let mut labels = Vec::new();
        let mut end = ident_tok.span.end;

        while self.at(TokenKind::Ident) {
            let label_tok = self.advance();
            labels.push(self.text(&label_tok).to_string());
            end = label_tok.span.end;
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        Some(PipelineStage::LabelOp {
            kind,
            labels,
            span: Span { start, end },
        })
    }

    fn parse_unwrap_stage(&mut self, start: usize) -> Option<PipelineStage> {
        self.advance(); // consume "unwrap"

        if !self.at(TokenKind::Ident) {
            let tok = self.peek().clone();
            self.errors.push(ParseError {
                message: "expected label name after 'unwrap'".to_string(),
                span: tok.span,
            });
            return None;
        }

        let ident_tok = self.advance();
        let ident = self.text(&ident_tok).to_string();

        if self.at(TokenKind::LParen)
            && matches!(
                ident.as_str(),
                "duration" | "duration_seconds" | "bytes"
            )
        {
            self.advance(); // consume (
            let label_tok = self.expect(TokenKind::Ident)?;
            let label = self.text(&label_tok).to_string();
            let close = self.expect(TokenKind::RParen);
            let end = close.map_or(label_tok.span.end, |t| t.span.end);
            Some(PipelineStage::Unwrap {
                label,
                conversion: Some(ident),
                span: Span { start, end },
            })
        } else {
            Some(PipelineStage::Unwrap {
                label: ident,
                conversion: None,
                span: Span {
                    start,
                    end: ident_tok.span.end,
                },
            })
        }
    }

    fn parse_label_filter_or_error(&mut self, start: usize) -> Option<PipelineStage> {
        let ident_tok = self.advance();
        let name = self.text(&ident_tok).to_string();

        let op = match self.peek_kind() {
            TokenKind::Eq => {
                self.advance();
                LabelFilterOp::Eq
            }
            TokenKind::NotEqual => {
                self.advance();
                LabelFilterOp::Neq
            }
            TokenKind::EqRegex => {
                self.advance();
                LabelFilterOp::Re
            }
            TokenKind::NotMatch => {
                self.advance();
                LabelFilterOp::Nre
            }
            TokenKind::CmpEq => {
                self.advance();
                LabelFilterOp::CmpEq
            }
            TokenKind::Gt => {
                self.advance();
                LabelFilterOp::Gt
            }
            TokenKind::GtEq => {
                self.advance();
                LabelFilterOp::GtEq
            }
            TokenKind::Lt => {
                self.advance();
                LabelFilterOp::Lt
            }
            TokenKind::LtEq => {
                self.advance();
                LabelFilterOp::LtEq
            }
            _ => {
                self.errors.push(ParseError {
                    message: format!("unexpected '{}' in pipeline", name),
                    span: Span {
                        start,
                        end: ident_tok.span.end,
                    },
                });
                self.skip_to_sync();
                return None;
            }
        };

        let (value, end) = if self.at(TokenKind::String) {
            let tok = self.advance();
            (self.extract_string(&tok), tok.span.end)
        } else if self.at(TokenKind::Number) {
            let tok = self.advance();
            (self.text(&tok).to_string(), tok.span.end)
        } else {
            let tok = self.peek().clone();
            self.errors.push(ParseError {
                message: "expected value after comparison operator".to_string(),
                span: tok.span,
            });
            return None;
        };

        Some(PipelineStage::LabelFilter {
            name,
            op,
            value,
            span: Span { start, end },
        })
    }

    // ── Range aggregation ───────────────────────────────────────────────

    fn parse_range_aggregation(&mut self) -> Option<Expr> {
        let func_tok = self.advance();
        let func = self.text(&func_tok).to_string();
        let start = func_tok.span.start;

        self.expect(TokenKind::LParen)?;

        let param = if func == "quantile_over_time" && self.at(TokenKind::Number) {
            let num = self.parse_number_lit();
            if self.at(TokenKind::Comma) {
                self.advance();
            }
            num.map(Box::new)
        } else {
            None
        };

        let log_query = self.parse_log_query()?;

        self.expect(TokenKind::LBracket)?;
        let dur_tok = self.expect(TokenKind::Duration)?;
        let duration = self.text(&dur_tok).to_string();
        self.expect(TokenKind::RBracket);

        let close = self.expect(TokenKind::RParen);
        let end = close.map_or(dur_tok.span.end, |t| t.span.end);

        Some(Expr {
            kind: ExprKind::RangeAggregation {
                func,
                log_query: Box::new(log_query),
                duration,
                param,
            },
            span: Span { start, end },
        })
    }

    // ── Aggregation ─────────────────────────────────────────────────────

    fn parse_aggregation(&mut self) -> Option<Expr> {
        let op_tok = self.advance();
        let op = self.text(&op_tok).to_string();
        let start = op_tok.span.start;
        let has_param = matches!(op.as_str(), "topk" | "bottomk");

        let mut grouping = None;
        if self.at(TokenKind::Ident) {
            let text = self.text(self.peek()).to_string();
            if text == "by" || text == "without" {
                grouping = self.parse_grouping();
            }
        }

        self.expect(TokenKind::LParen)?;

        let mut param = None;
        if has_param && self.at(TokenKind::Number) {
            param = self.parse_number_lit().map(Box::new);
            if self.at(TokenKind::Comma) {
                self.advance();
            }
        }

        let expr = self.parse_expr()?;

        let close = self.expect(TokenKind::RParen);
        let mut end = close.map_or(expr.span.end, |t| t.span.end);

        if grouping.is_none() && self.at(TokenKind::Ident) {
            let text = self.text(self.peek()).to_string();
            if text == "by" || text == "without" {
                grouping = self.parse_grouping();
                if let Some(ref g) = grouping {
                    end = g.span.end;
                }
            }
        }

        Some(Expr {
            kind: ExprKind::Aggregation {
                op,
                expr: Box::new(expr),
                grouping,
                param,
            },
            span: Span { start, end },
        })
    }

    fn parse_grouping(&mut self) -> Option<Grouping> {
        let mod_tok = self.advance();
        let without = self.text(&mod_tok) == "without";
        let start = mod_tok.span.start;

        self.expect(TokenKind::LParen)?;

        let mut labels = Vec::new();
        while self.at(TokenKind::Ident) {
            let label_tok = self.advance();
            labels.push(self.text(&label_tok).to_string());
            if self.at(TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        let close = self.expect(TokenKind::RParen);
        let end = close.map_or(mod_tok.span.end, |t| t.span.end);

        Some(Grouping {
            without,
            labels,
            span: Span { start, end },
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(input: &str) -> Expr {
        let result = parse(input);
        assert!(
            result.errors.is_empty(),
            "unexpected errors for {input:?}: {:?}",
            result.errors
        );
        result.expr.expect("expected expression")
    }

    fn span_text<'a>(input: &'a str, span: Span) -> &'a str {
        &input[span.start..span.end]
    }

    // Helper to unwrap a LogQuery expression
    fn as_log_query(expr: &Expr) -> (&StreamSelector, &[PipelineStage]) {
        match &expr.kind {
            ExprKind::LogQuery { selector, pipeline } => (selector, pipeline),
            other => panic!("expected LogQuery, got {other:?}"),
        }
    }

    // ── Stream selectors ────────────────────────────────────────────────

    #[test]
    fn empty_selector() {
        let expr = parse_ok("{}");
        let (sel, pipeline) = as_log_query(&expr);
        assert!(sel.matchers.is_empty());
        assert!(pipeline.is_empty());
    }

    #[test]
    fn single_matcher() {
        let input = r#"{job="varlogs"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(sel.matchers.len(), 1);
        assert_eq!(sel.matchers[0].name, "job");
        assert_eq!(sel.matchers[0].op, MatchOp::Eq);
        assert_eq!(sel.matchers[0].value, "varlogs");
    }

    #[test]
    fn multiple_matchers() {
        let input = r#"{job="varlogs", env="prod"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(sel.matchers.len(), 2);
        assert_eq!(sel.matchers[0].name, "job");
        assert_eq!(sel.matchers[1].name, "env");
        assert_eq!(sel.matchers[1].value, "prod");
    }

    #[test]
    fn regex_matcher() {
        let input = r#"{job=~"var.*"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(sel.matchers[0].op, MatchOp::Re);
        assert_eq!(sel.matchers[0].value, "var.*");
    }

    #[test]
    fn not_equal_matcher() {
        let input = r#"{job!="system"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(sel.matchers[0].op, MatchOp::Neq);
    }

    #[test]
    fn not_regex_matcher() {
        let input = r#"{job!~"sys.*"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(sel.matchers[0].op, MatchOp::Nre);
    }

    // ── Filter pipelines ────────────────────────────────────────────────

    #[test]
    fn pipe_exact_filter() {
        let input = r#"{j="t"} |= "error""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(pipeline.len(), 1);
        match &pipeline[0] {
            PipelineStage::Filter { op, pattern, .. } => {
                assert_eq!(*op, FilterOp::PipeExact);
                assert_eq!(pattern, "error");
            }
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    #[test]
    fn pipe_match_filter() {
        let input = r#"{j="t"} |~ "5\\d+""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Filter { op, .. } => assert_eq!(*op, FilterOp::PipeMatch),
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    #[test]
    fn multiple_filters() {
        let input = r#"{j="t"} |= "error" != "timeout""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(pipeline.len(), 2);
        match &pipeline[0] {
            PipelineStage::Filter { op, .. } => assert_eq!(*op, FilterOp::PipeExact),
            other => panic!("expected Filter, got {other:?}"),
        }
        match &pipeline[1] {
            PipelineStage::Filter { op, pattern, .. } => {
                assert_eq!(*op, FilterOp::NotEqual);
                assert_eq!(pattern, "timeout");
            }
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    #[test]
    fn not_match_filter() {
        let input = r#"{j="t"} !~ "debug""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Filter { op, .. } => assert_eq!(*op, FilterOp::NotMatch),
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    // ── Parser stages ───────────────────────────────────────────────────

    #[test]
    fn json_parser() {
        let input = r#"{j="t"} | json"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Parser { kind, params, .. } => {
                assert_eq!(*kind, ParserKind::Json);
                assert!(params.is_empty());
            }
            other => panic!("expected Parser, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_parser() {
        let input = r#"{j="t"} | logfmt"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Parser { kind, .. } => assert_eq!(*kind, ParserKind::Logfmt),
            other => panic!("expected Parser, got {other:?}"),
        }
    }

    #[test]
    fn regexp_parser_with_param() {
        let input = r#"{j="t"} | regexp "(?P<ip>\\S+)""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Parser { kind, params, .. } => {
                assert_eq!(*kind, ParserKind::Regexp);
                assert_eq!(params.len(), 1);
            }
            other => panic!("expected Parser, got {other:?}"),
        }
    }

    #[test]
    fn pattern_parser() {
        let input = r#"{j="t"} | pattern "<ip> - <_>""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Parser { kind, .. } => assert_eq!(*kind, ParserKind::Pattern),
            other => panic!("expected Parser, got {other:?}"),
        }
    }

    #[test]
    fn unpack_parser() {
        let input = r#"{j="t"} | unpack"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Parser { kind, .. } => assert_eq!(*kind, ParserKind::Unpack),
            other => panic!("expected Parser, got {other:?}"),
        }
    }

    // ── Formatter stages ────────────────────────────────────────────────

    #[test]
    fn line_format_stage() {
        let input = r#"{j="t"} | line_format "{{.msg}}""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::LineFormat { template, .. } => {
                assert_eq!(template, "{{.msg}}");
            }
            other => panic!("expected LineFormat, got {other:?}"),
        }
    }

    #[test]
    fn label_format_stage() {
        let input = r#"{j="t"} | label_format dst=src"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::LabelFormat { mappings, .. } => {
                assert_eq!(mappings.len(), 1);
                assert_eq!(mappings[0], ("dst".to_string(), "src".to_string()));
            }
            other => panic!("expected LabelFormat, got {other:?}"),
        }
    }

    // ── Label operations ────────────────────────────────────────────────

    #[test]
    fn drop_labels() {
        let input = r#"{j="t"} | drop level"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::LabelOp { kind, labels, .. } => {
                assert_eq!(*kind, LabelOpKind::Drop);
                assert_eq!(labels, &["level"]);
            }
            other => panic!("expected LabelOp, got {other:?}"),
        }
    }

    #[test]
    fn keep_multiple_labels() {
        let input = r#"{j="t"} | keep level, msg"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::LabelOp { kind, labels, .. } => {
                assert_eq!(*kind, LabelOpKind::Keep);
                assert_eq!(labels, &["level", "msg"]);
            }
            other => panic!("expected LabelOp, got {other:?}"),
        }
    }

    #[test]
    fn decolorize_stage() {
        let input = r#"{j="t"} | decolorize"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert!(matches!(pipeline[0], PipelineStage::Decolorize { .. }));
    }

    // ── Unwrap ──────────────────────────────────────────────────────────

    #[test]
    fn simple_unwrap() {
        let input = r#"{j="t"} | unwrap duration"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Unwrap {
                label, conversion, ..
            } => {
                assert_eq!(label, "duration");
                assert!(conversion.is_none());
            }
            other => panic!("expected Unwrap, got {other:?}"),
        }
    }

    #[test]
    fn unwrap_with_conversion() {
        let input = r#"{j="t"} | unwrap duration_seconds(response_time)"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[0] {
            PipelineStage::Unwrap {
                label, conversion, ..
            } => {
                assert_eq!(label, "response_time");
                assert_eq!(conversion.as_deref(), Some("duration_seconds"));
            }
            other => panic!("expected Unwrap, got {other:?}"),
        }
    }

    // ── Label filters ───────────────────────────────────────────────────

    #[test]
    fn label_filter_eq() {
        let input = r#"{j="t"} | json | level="error""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(pipeline.len(), 2);
        match &pipeline[1] {
            PipelineStage::LabelFilter {
                name, op, value, ..
            } => {
                assert_eq!(name, "level");
                assert_eq!(*op, LabelFilterOp::Eq);
                assert_eq!(value, "error");
            }
            other => panic!("expected LabelFilter, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_numeric() {
        let input = r#"{j="t"} | json | status>=400"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        match &pipeline[1] {
            PipelineStage::LabelFilter {
                name, op, value, ..
            } => {
                assert_eq!(name, "status");
                assert_eq!(*op, LabelFilterOp::GtEq);
                assert_eq!(value, "400");
            }
            other => panic!("expected LabelFilter, got {other:?}"),
        }
    }

    // ── Range aggregations ──────────────────────────────────────────────

    #[test]
    fn rate_query() {
        let input = r#"rate({job="test"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation {
                func,
                duration,
                param,
                ..
            } => {
                assert_eq!(func, "rate");
                assert_eq!(duration, "5m");
                assert!(param.is_none());
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn count_over_time() {
        let input = r#"count_over_time({job="test"}[1h])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation { func, duration, .. } => {
                assert_eq!(func, "count_over_time");
                assert_eq!(duration, "1h");
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn sum_over_time_with_unwrap() {
        let input = r#"sum_over_time({j="t"} | unwrap dur [5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation {
                func, log_query, ..
            } => {
                assert_eq!(func, "sum_over_time");
                let (_, pipeline) = as_log_query(log_query);
                assert_eq!(pipeline.len(), 1);
                assert!(matches!(pipeline[0], PipelineStage::Unwrap { .. }));
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn quantile_over_time() {
        let input = r#"quantile_over_time(0.99, {job="test"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation {
                func,
                param,
                duration,
                ..
            } => {
                assert_eq!(func, "quantile_over_time");
                assert_eq!(duration, "5m");
                let p = param.as_ref().expect("expected param");
                match &p.kind {
                    ExprKind::NumberLit(v) => assert!((v - 0.99).abs() < f64::EPSILON),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn bytes_rate() {
        let input = r#"bytes_rate({job="test"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation { func, .. } => assert_eq!(func, "bytes_rate"),
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    // ── Aggregations ────────────────────────────────────────────────────

    #[test]
    fn sum_by() {
        let input = r#"sum by (job) (rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation {
                op,
                grouping,
                param,
                ..
            } => {
                assert_eq!(op, "sum");
                let g = grouping.as_ref().expect("expected grouping");
                assert!(!g.without);
                assert_eq!(g.labels, vec!["job"]);
                assert!(param.is_none());
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn avg_by() {
        let input = r#"avg by (host) (rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { op, grouping, .. } => {
                assert_eq!(op, "avg");
                assert_eq!(grouping.as_ref().unwrap().labels, vec!["host"]);
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn sum_without() {
        let input = r#"sum without (job) (rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { grouping, .. } => {
                let g = grouping.as_ref().unwrap();
                assert!(g.without);
                assert_eq!(g.labels, vec!["job"]);
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn aggregation_post_modifier() {
        let input = r#"sum(rate({j="t"}[5m])) by (job)"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { op, grouping, .. } => {
                assert_eq!(op, "sum");
                let g = grouping.as_ref().expect("expected grouping");
                assert_eq!(g.labels, vec!["job"]);
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn topk_aggregation() {
        let input = r#"topk(10, rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { op, param, .. } => {
                assert_eq!(op, "topk");
                let p = param.as_ref().expect("expected param");
                match &p.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 10.0),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn bottomk_aggregation() {
        let input = r#"bottomk(5, rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { op, param, .. } => {
                assert_eq!(op, "bottomk");
                let p = param.as_ref().expect("expected param");
                match &p.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 5.0),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn count_aggregation() {
        let input = r#"count(rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { op, grouping, .. } => {
                assert_eq!(op, "count");
                assert!(grouping.is_none());
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    // ── Binary operations ───────────────────────────────────────────────

    #[test]
    fn binary_addition() {
        let input = r#"rate({a="1"}[5m]) + rate({b="2"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Add),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_comparison_gt() {
        let input = r#"rate({j="t"}[5m]) > 100"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, rhs, .. } => {
                assert_eq!(*op, BinOpKind::Gt);
                match &rhs.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 100.0),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_and() {
        let input = r#"rate({a="1"}[5m]) > 0 and rate({b="2"}[5m]) > 0"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::And),
            other => panic!("expected BinOp(And), got {other:?}"),
        }
    }

    #[test]
    fn binary_or() {
        let input = r#"rate({a="1"}[5m]) or rate({b="2"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Or),
            other => panic!("expected BinOp(Or), got {other:?}"),
        }
    }

    #[test]
    fn binary_unless() {
        let input = r#"rate({a="1"}[5m]) unless rate({b="2"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Unless),
            other => panic!("expected BinOp(Unless), got {other:?}"),
        }
    }

    // ── Operator precedence ─────────────────────────────────────────────

    #[test]
    fn mul_before_add() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let expr = parse_ok("1 + 2 * 3");
        match &expr.kind {
            ExprKind::BinOp { op, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::Add);
                match &lhs.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 1.0),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
                match &rhs.kind {
                    ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Mul),
                    other => panic!("expected BinOp(Mul), got {other:?}"),
                }
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn pow_right_associative() {
        // 2 ^ 3 ^ 4 should parse as 2 ^ (3 ^ 4)
        let expr = parse_ok("2 ^ 3 ^ 4");
        match &expr.kind {
            ExprKind::BinOp { op, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::Pow);
                match &lhs.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 2.0),
                    other => panic!("expected NumberLit(2), got {other:?}"),
                }
                match &rhs.kind {
                    ExprKind::BinOp { op, lhs, rhs } => {
                        assert_eq!(*op, BinOpKind::Pow);
                        match &lhs.kind {
                            ExprKind::NumberLit(v) => assert_eq!(*v, 3.0),
                            other => panic!("expected NumberLit(3), got {other:?}"),
                        }
                        match &rhs.kind {
                            ExprKind::NumberLit(v) => assert_eq!(*v, 4.0),
                            other => panic!("expected NumberLit(4), got {other:?}"),
                        }
                    }
                    other => panic!("expected BinOp(Pow), got {other:?}"),
                }
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn comparison_before_and() {
        // a > 0 and b > 0 parses as (a > 0) and (b > 0)
        let input = r#"rate({a="1"}[5m]) > 0 and rate({b="2"}[5m]) > 0"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::And);
                match &lhs.kind {
                    ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Gt),
                    other => panic!("expected BinOp(Gt), got {other:?}"),
                }
                match &rhs.kind {
                    ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Gt),
                    other => panic!("expected BinOp(Gt), got {other:?}"),
                }
            }
            other => panic!("expected BinOp(And), got {other:?}"),
        }
    }

    // ── Nested expressions ──────────────────────────────────────────────

    #[test]
    fn paren_expr() {
        let input = r#"(rate({j="t"}[5m]) + 1) * 2"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, lhs, rhs } => {
                assert_eq!(*op, BinOpKind::Mul);
                assert!(matches!(lhs.kind, ExprKind::Paren(_)));
                match &rhs.kind {
                    ExprKind::NumberLit(v) => assert_eq!(*v, 2.0),
                    other => panic!("expected NumberLit, got {other:?}"),
                }
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn complex_nested() {
        let input = r#"sum by (job) (rate({j="t"} |= "error" | json [5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation {
                op,
                expr: inner,
                grouping,
                ..
            } => {
                assert_eq!(op, "sum");
                assert_eq!(
                    grouping.as_ref().unwrap().labels,
                    vec!["job"]
                );
                match &inner.kind {
                    ExprKind::RangeAggregation {
                        func, log_query, ..
                    } => {
                        assert_eq!(func, "rate");
                        let (_, pipeline) = as_log_query(log_query);
                        assert_eq!(pipeline.len(), 2);
                    }
                    other => panic!("expected RangeAggregation, got {other:?}"),
                }
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn mul_precedence_in_binary() {
        // a + b * c  should be a + (b * c)
        let input =
            r#"rate({a="1"}[5m]) + rate({b="2"}[5m]) * rate({c="3"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, rhs, .. } => {
                assert_eq!(*op, BinOpKind::Add);
                match &rhs.kind {
                    ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Mul),
                    other => panic!("expected BinOp(Mul), got {other:?}"),
                }
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    // ── Error recovery ──────────────────────────────────────────────────

    #[test]
    fn error_recovery_bad_syntax() {
        let input = r#"{j="t"} | json | bad$syntax | logfmt"#;
        let result = parse(input);
        assert!(!result.errors.is_empty(), "expected errors");
        let expr = result.expr.expect("expected partial AST");
        let (_, pipeline) = as_log_query(&expr);
        // json and logfmt should be present, bad$syntax skipped
        let parser_kinds: Vec<_> = pipeline
            .iter()
            .filter_map(|s| match s {
                PipelineStage::Parser { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert!(
            parser_kinds.contains(&ParserKind::Json),
            "json should be parsed"
        );
        assert!(
            parser_kinds.contains(&ParserKind::Logfmt),
            "logfmt should be parsed after error recovery"
        );
    }

    #[test]
    fn error_recovery_trailing_pipe() {
        let input = r#"{j="t"} | json |"#;
        let result = parse(input);
        assert!(!result.errors.is_empty());
        // Should still produce partial AST with json parsed
        let expr = result.expr.expect("expected partial AST");
        let (_, pipeline) = as_log_query(&expr);
        assert!(
            pipeline.iter().any(|s| matches!(s, PipelineStage::Parser { kind: ParserKind::Json, .. })),
            "json should be parsed"
        );
    }

    #[test]
    fn empty_input() {
        let result = parse("");
        assert!(result.expr.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn number_literal() {
        let expr = parse_ok("42");
        match &expr.kind {
            ExprKind::NumberLit(v) => assert_eq!(*v, 42.0),
            other => panic!("expected NumberLit, got {other:?}"),
        }
    }

    #[test]
    fn string_literal() {
        let expr = parse_ok(r#""hello""#);
        match &expr.kind {
            ExprKind::StringLit(v) => assert_eq!(v, "hello"),
            other => panic!("expected StringLit, got {other:?}"),
        }
    }

    // ── Span accuracy ───────────────────────────────────────────────────

    #[test]
    fn span_selector() {
        let input = r#"{job="varlogs"}"#;
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), input);
        let (sel, _) = as_log_query(&expr);
        assert_eq!(span_text(input, sel.span), input);
    }

    #[test]
    fn span_matcher() {
        let input = r#"{job="varlogs"}"#;
        let expr = parse_ok(input);
        let (sel, _) = as_log_query(&expr);
        let m = &sel.matchers[0];
        assert_eq!(span_text(input, m.span), r#"job="varlogs""#);
    }

    #[test]
    fn span_filter_stage() {
        let input = r#"{j="t"} |= "error""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(span_text(input, pipeline[0].span()), r#"|= "error""#);
    }

    #[test]
    fn span_range_aggregation() {
        let input = r#"rate({job="test"}[5m])"#;
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), input);
    }

    #[test]
    fn span_aggregation_with_by() {
        let input = r#"sum by (job) (rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), input);
    }

    #[test]
    fn span_binary_op() {
        let input = "1 + 2";
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), "1 + 2");
        match &expr.kind {
            ExprKind::BinOp { lhs, rhs, .. } => {
                assert_eq!(span_text(input, lhs.span), "1");
                assert_eq!(span_text(input, rhs.span), "2");
            }
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn span_paren() {
        let input = "(42)";
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), "(42)");
    }

    #[test]
    fn span_log_query_with_pipeline() {
        let input = r#"{j="t"} |= "error" | json"#;
        let expr = parse_ok(input);
        assert_eq!(span_text(input, expr.span), input);
    }

    #[test]
    fn span_pipe_stage() {
        let input = r#"{j="t"} | json"#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(span_text(input, pipeline[0].span()), "| json");
    }

    // ── Duration parsing ────────────────────────────────────────────────

    #[test]
    fn duration_hours() {
        let input = r#"rate({j="t"}[1h])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation { duration, .. } => assert_eq!(duration, "1h"),
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn duration_composite() {
        let input = r#"rate({j="t"}[1h30m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation { duration, .. } => {
                assert_eq!(duration, "1h30m");
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    // ── Realistic queries ───────────────────────────────────────────────

    #[test]
    fn realistic_rate_with_pipeline() {
        let input = r#"rate({job="varlogs"} |= "error" | json [5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::RangeAggregation {
                func,
                log_query,
                duration,
                ..
            } => {
                assert_eq!(func, "rate");
                assert_eq!(duration, "5m");
                let (sel, pipeline) = as_log_query(log_query);
                assert_eq!(sel.matchers[0].value, "varlogs");
                assert_eq!(pipeline.len(), 2);
            }
            other => panic!("expected RangeAggregation, got {other:?}"),
        }
    }

    #[test]
    fn realistic_sum_count_over_time() {
        let input = r#"sum by (job) (count_over_time({app="api"}[1h]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation {
                op,
                expr: inner,
                grouping,
                ..
            } => {
                assert_eq!(op, "sum");
                assert_eq!(grouping.as_ref().unwrap().labels, vec!["job"]);
                match &inner.kind {
                    ExprKind::RangeAggregation { func, duration, .. } => {
                        assert_eq!(func, "count_over_time");
                        assert_eq!(duration, "1h");
                    }
                    other => panic!("expected RangeAggregation, got {other:?}"),
                }
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn realistic_logfmt_pipeline() {
        let input = r#"{job="syslog"} | logfmt | level="error" | line_format "{{.msg}}""#;
        let expr = parse_ok(input);
        let (sel, pipeline) = as_log_query(&expr);
        assert_eq!(sel.matchers[0].value, "syslog");
        assert_eq!(pipeline.len(), 3);
        assert!(matches!(pipeline[0], PipelineStage::Parser { kind: ParserKind::Logfmt, .. }));
        assert!(matches!(pipeline[1], PipelineStage::LabelFilter { .. }));
        assert!(matches!(pipeline[2], PipelineStage::LineFormat { .. }));
    }

    #[test]
    fn complex_multi_filter_pipeline() {
        let input =
            r#"{job="app"} |= "error" != "timeout" |~ "5\\d{2}" | json | line_format "{{.msg}}""#;
        let expr = parse_ok(input);
        let (_, pipeline) = as_log_query(&expr);
        assert_eq!(pipeline.len(), 5);
    }

    #[test]
    fn aggregation_multiple_labels() {
        let input = r#"sum by (job, namespace, pod) (rate({j="t"}[5m]))"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::Aggregation { grouping, .. } => {
                let g = grouping.as_ref().unwrap();
                assert_eq!(g.labels, vec!["job", "namespace", "pod"]);
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn binary_subtraction() {
        let input = r#"rate({a="1"}[5m]) - rate({b="2"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Sub),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_division() {
        let input = r#"rate({a="1"}[5m]) / rate({b="2"}[5m])"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Div),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_modulo() {
        let input = "10 % 3";
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Mod),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_cmp_eq() {
        let input = r#"rate({j="t"}[5m]) == 0"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::CmpEq),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }

    #[test]
    fn binary_neq() {
        let input = r#"rate({j="t"}[5m]) != 0"#;
        let expr = parse_ok(input);
        match &expr.kind {
            ExprKind::BinOp { op, .. } => assert_eq!(*op, BinOpKind::Neq),
            other => panic!("expected BinOp, got {other:?}"),
        }
    }
}
