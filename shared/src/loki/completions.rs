//! AST-driven structured completion engine for LogQL.
//!
//! Uses the error-recovering parser and tokenizer to understand query context
//! and provide intelligent, semantically ranked completions with documentation.

use std::collections::HashMap;

use super::ast::*;
use super::keywords;
use super::parser::parse;
use super::tokenizer::{tokenize, Token, TokenKind};

use crate::prometheus::context::char_to_byte;

// ── Completion item ─────────────────────────────────────────────────────────

/// A rich completion item with metadata for display and ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// The text to insert.
    pub label: String,
    /// Short description shown alongside the label.
    pub detail: Option<String>,
    /// Longer documentation shown in a tooltip/panel.
    pub documentation: Option<String>,
    /// The kind of completion (for icon/category).
    pub kind: CompletionKind,
    /// Lower values sort first (semantic ranking).
    pub sort_priority: u32,
}

/// Categories of completion items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    LabelName,
    LabelValue,
    PipelineKeyword,
    Function,
    AggregationOp,
    Duration,
    Keyword,
}

// ── Cursor context ──────────────────────────────────────────────────────────

/// The detected cursor context, determined by analyzing tokens and AST.
#[derive(Debug, Clone)]
pub enum CursorContext {
    /// Inside `{}` — completing a label name.
    LabelName { prefix: String, selector: String },
    /// Inside a string value in a label matcher — completing a label value.
    LabelValue {
        label: String,
        prefix: String,
        selector: String,
    },
    /// After `|` at pipeline level — completing a pipeline keyword.
    PipelineStage { prefix: String },
    /// At expression level — completing functions or starting a selector.
    Expression { prefix: String },
    /// Inside `[]` — completing a duration.
    Duration { prefix: String },
    /// No useful context.
    None,
}

// ── Context detection (token + AST driven) ──────────────────────────────────

/// Detect the cursor context from a LogQL query and cursor position.
///
/// Uses the tokenizer for positional analysis and the error-recovering parser
/// for structural understanding (e.g., extracting the stream selector).
pub fn detect_cursor_context(query: &str, cursor_pos: usize) -> CursorContext {
    let cursor_byte = char_to_byte(query, cursor_pos);
    let before = &query[..cursor_byte];

    if before.is_empty() {
        return CursorContext::Expression {
            prefix: String::new(),
        };
    }

    let tokens = tokenize(before);

    // Build structural state from tokens
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut has_closed_selector = false;
    let mut brace_positions: Vec<usize> = Vec::new();

    for token in &tokens {
        match token.kind {
            TokenKind::LBrace => {
                brace_depth += 1;
                brace_positions.push(token.span.start);
            }
            TokenKind::RBrace => {
                brace_depth = (brace_depth - 1).max(0);
                brace_positions.pop();
                if brace_depth == 0 {
                    has_closed_selector = true;
                }
            }
            TokenKind::LBracket => bracket_depth += 1,
            TokenKind::RBracket => bracket_depth = (bracket_depth - 1).max(0),
            TokenKind::Eof => break,
            _ => {}
        }
    }

    // Significant tokens (no whitespace, comments, or EOF)
    let sig: Vec<&Token> = tokens
        .iter()
        .filter(|t| {
            !matches!(
                t.kind,
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::Eof
            )
        })
        .collect();
    let last_sig = sig.last().copied();
    let second_last_sig = if sig.len() >= 2 {
        Some(sig[sig.len() - 2])
    } else {
        None
    };

    // 1. Check for unclosed string at the end
    if let Some(tok) = last_sig {
        if tok.kind == TokenKind::Error {
            let text = tok.text(before);
            if text.starts_with('"') || text.starts_with('\'') || text.starts_with('`') {
                if brace_depth > 0 {
                    // Label value context: unclosed string inside braces
                    let prefix = before[tok.span.start + 1..].to_string();
                    let label = find_label_from_sig_tokens(&sig, before);
                    let selector = build_selector_from_brace(&brace_positions, before);
                    return CursorContext::LabelValue {
                        label,
                        prefix,
                        selector,
                    };
                } else {
                    // Unclosed string outside braces (filter pattern, template, etc.)
                    return CursorContext::None;
                }
            }
        }
    }

    // 2. Inside braces: label name context
    if brace_depth > 0 {
        let prefix = word_prefix(before);
        // Suppress completions right after an operator
        if prefix.is_empty() {
            if let Some(tok) = last_sig {
                if matches!(
                    tok.kind,
                    TokenKind::Eq
                        | TokenKind::EqRegex
                        | TokenKind::NotEqual
                        | TokenKind::NotMatch
                ) {
                    return CursorContext::None;
                }
                // Bare `!` (partial operator)
                if tok.kind == TokenKind::Error && tok.text(before) == "!" {
                    return CursorContext::None;
                }
            }
        }
        let selector = build_selector_from_brace(&brace_positions, before);
        return CursorContext::LabelName { prefix, selector };
    }

    // 3. Inside brackets: duration context
    if bracket_depth > 0 {
        let prefix = word_prefix(before);
        return CursorContext::Duration { prefix };
    }

    // 4. Inside by()/without(): label name context
    if is_in_label_list_paren(&tokens, before) {
        let prefix = word_prefix(before);
        return CursorContext::LabelName {
            prefix,
            selector: String::new(),
        };
    }

    // 5. Pipeline: after `|` following a closed stream selector
    if has_closed_selector {
        if let Some(tok) = last_sig {
            if tok.kind == TokenKind::Pipe {
                return CursorContext::PipelineStage {
                    prefix: String::new(),
                };
            }
        }
        let prefix = word_prefix(before);
        if !prefix.is_empty() {
            if let Some(tok) = second_last_sig {
                if tok.kind == TokenKind::Pipe {
                    return CursorContext::PipelineStage { prefix };
                }
            }
        }
    }

    // 6. Default: expression context
    CursorContext::Expression {
        prefix: word_prefix(before),
    }
}

// ── Completion generation ───────────────────────────────────────────────────

/// Compute rich completion items for the given cursor context.
pub fn compute_completion_items(
    ctx: &CursorContext,
    label_names_cache: &HashMap<String, Vec<String>>,
    label_values_cache: &HashMap<(String, String), Vec<String>>,
    max: usize,
) -> Vec<CompletionItem> {
    match ctx {
        CursorContext::LabelName { prefix, selector } => {
            let pl = prefix.to_lowercase();
            let names = label_names_cache
                .get(selector.as_str())
                .or_else(|| label_names_cache.get(""))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut items: Vec<CompletionItem> = names
                .iter()
                .filter(|n| n.to_lowercase().starts_with(&pl))
                .map(|name| CompletionItem {
                    label: name.clone(),
                    detail: Some("Label".to_string()),
                    documentation: None,
                    kind: CompletionKind::LabelName,
                    sort_priority: 0,
                })
                .collect();
            items.sort_by(|a, b| a.label.cmp(&b.label));
            for (i, item) in items.iter_mut().enumerate() {
                item.sort_priority = i as u32;
            }
            items.truncate(max);
            items
        }

        CursorContext::LabelValue {
            label,
            prefix,
            selector,
        } => {
            let pl = prefix.to_lowercase();
            let key = (label.clone(), selector.clone());
            let values = label_values_cache
                .get(&key)
                .or_else(|| label_values_cache.get(&(label.clone(), String::new())))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut items: Vec<CompletionItem> = values
                .iter()
                .filter(|v| v.to_lowercase().starts_with(&pl))
                .map(|value| CompletionItem {
                    label: value.clone(),
                    detail: Some(format!("{label} value")),
                    documentation: None,
                    kind: CompletionKind::LabelValue,
                    sort_priority: 0,
                })
                .collect();
            items.sort_by(|a, b| a.label.cmp(&b.label));
            for (i, item) in items.iter_mut().enumerate() {
                item.sort_priority = i as u32;
            }
            items.truncate(max);
            items
        }

        CursorContext::PipelineStage { prefix } => complete_pipeline_keywords(prefix, max),

        CursorContext::Expression { prefix } if !prefix.is_empty() => {
            complete_expression(prefix, max)
        }

        CursorContext::Duration { prefix } => complete_durations(prefix, max),

        _ => vec![],
    }
}

/// Extract just the label strings from completion items (backward-compatible).
pub fn completion_labels(items: &[CompletionItem]) -> Vec<String> {
    items.iter().map(|i| i.label.clone()).collect()
}

// ── Private helpers: context detection ──────────────────────────────────────

/// Return the last "word" in `before` (alphanumeric + underscore).
fn word_prefix(before: &str) -> String {
    let bytes = before.as_bytes();
    let mut i = bytes.len();
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    before[i..].to_string()
}

/// Find the label name preceding the unclosed string value.
///
/// Expects the significant token pattern: `..., Ident(label), Operator, Error(string)`.
fn find_label_from_sig_tokens(sig_tokens: &[&Token], before: &str) -> String {
    let len = sig_tokens.len();
    if len >= 3 {
        let op = sig_tokens[len - 2];
        let ident = sig_tokens[len - 3];
        if matches!(
            op.kind,
            TokenKind::Eq | TokenKind::EqRegex | TokenKind::NotEqual | TokenKind::NotMatch
        ) && ident.kind == TokenKind::Ident
        {
            return ident.text(before).to_string();
        }
    }
    String::new()
}

/// Build a selector string from the AST by parsing from the innermost open brace.
///
/// This is the key AST-driven enhancement: the error-recovering parser extracts
/// complete label matchers even from partial queries, giving us a proper selector
/// for scoped label name/value API calls.
fn build_selector_from_brace(brace_positions: &[usize], before: &str) -> String {
    let brace_start = match brace_positions.last() {
        Some(&pos) => pos,
        None => return String::new(),
    };

    // Parse from the innermost open brace — the error-recovering parser will
    // extract any complete matchers even though the selector is incomplete.
    let brace_text = &before[brace_start..];
    let result = parse(brace_text);

    if let Some(ref expr) = result.expr {
        if let Some(selector) = find_deepest_selector(expr) {
            return format_selector(selector);
        }
    }

    String::new()
}

/// Walk the AST to find the deepest stream selector.
fn find_deepest_selector(expr: &Expr) -> Option<&StreamSelector> {
    match &expr.kind {
        ExprKind::LogQuery { selector, .. } => Some(selector),
        ExprKind::RangeAggregation { log_query, .. } => find_deepest_selector(log_query),
        ExprKind::Aggregation { expr, .. } => find_deepest_selector(expr),
        ExprKind::BinOp { lhs, rhs, .. } => {
            find_deepest_selector(rhs).or_else(|| find_deepest_selector(lhs))
        }
        ExprKind::Paren(inner) => find_deepest_selector(inner),
        _ => None,
    }
}

/// Format an AST StreamSelector into a selector string like `{job="varlogs"}`.
fn format_selector(selector: &StreamSelector) -> String {
    if selector.matchers.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = selector
        .matchers
        .iter()
        .map(|m| {
            let op_str = match m.op {
                MatchOp::Eq => "=",
                MatchOp::Neq => "!=",
                MatchOp::Re => "=~",
                MatchOp::Nre => "!~",
            };
            format!("{}{}\"{}\"", m.name, op_str, m.value)
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Check if the cursor is inside a `by(...)` or `without(...)` paren group.
fn is_in_label_list_paren(tokens: &[Token], before: &str) -> bool {
    let mut paren_stack: Vec<bool> = Vec::new();
    let mut prev_ident: Option<&str> = None;

    for token in tokens {
        match token.kind {
            TokenKind::Ident => {
                prev_ident = Some(token.text(before));
            }
            TokenKind::LParen => {
                let is_label_list = prev_ident
                    .map(|kw| kw == "by" || kw == "without")
                    .unwrap_or(false);
                paren_stack.push(is_label_list);
                prev_ident = None;
            }
            TokenKind::RParen => {
                paren_stack.pop();
                prev_ident = None;
            }
            TokenKind::Whitespace | TokenKind::LineComment => {
                // Don't reset prev_ident for trivia
            }
            TokenKind::Eof => break,
            _ => {
                prev_ident = None;
            }
        }
    }

    paren_stack.last().copied().unwrap_or(false)
}

// ── Private helpers: completion generation ──────────────────────────────────

/// Documentation for LogQL functions.
fn function_docs(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        // Range functions
        "rate" => (
            "Range function",
            "Calculates per-second rate of log entries over the given range",
        ),
        "count_over_time" => (
            "Range function",
            "Counts the log entries over the given range",
        ),
        "sum_over_time" => (
            "Range function",
            "Sums extracted values over time (requires unwrap)",
        ),
        "avg_over_time" => (
            "Range function",
            "Averages extracted values over time (requires unwrap)",
        ),
        "max_over_time" => (
            "Range function",
            "Maximum of extracted values over time (requires unwrap)",
        ),
        "min_over_time" => (
            "Range function",
            "Minimum of extracted values over time (requires unwrap)",
        ),
        "first_over_time" => (
            "Range function",
            "First value within the range (requires unwrap)",
        ),
        "last_over_time" => (
            "Range function",
            "Last value within the range (requires unwrap)",
        ),
        "stddev_over_time" => (
            "Range function",
            "Standard deviation over time (requires unwrap)",
        ),
        "stdvar_over_time" => (
            "Range function",
            "Standard variance over time (requires unwrap)",
        ),
        "quantile_over_time" => (
            "Range function",
            "Calculates the \u{03c6}-quantile over time: quantile_over_time(\u{03c6}, expr[d])",
        ),
        "bytes_over_time" => (
            "Range function",
            "Counts the bytes of log entries over the given range",
        ),
        "bytes_rate" => (
            "Range function",
            "Per-second byte rate of log entries",
        ),
        "absent_over_time" => (
            "Range function",
            "Returns 1 if the range vector has no entries, empty otherwise",
        ),
        // Aggregation operators
        "sum" => ("Aggregation", "Calculate the sum over dimensions"),
        "avg" => ("Aggregation", "Calculate the average over dimensions"),
        "min" => ("Aggregation", "Select minimum over dimensions"),
        "max" => ("Aggregation", "Select maximum over dimensions"),
        "count" => (
            "Aggregation",
            "Count number of elements in the vector",
        ),
        "topk" => ("Aggregation", "Largest k elements by value"),
        "bottomk" => ("Aggregation", "Smallest k elements by value"),
        "stddev" => (
            "Aggregation",
            "Calculate population standard deviation over dimensions",
        ),
        "stdvar" => (
            "Aggregation",
            "Calculate population standard variance over dimensions",
        ),
        "sort" => ("Aggregation", "Sort elements by value, ascending"),
        "sort_desc" => ("Aggregation", "Sort elements by value, descending"),
        _ => return None,
    })
}

/// Documentation for pipeline keywords.
fn pipeline_docs(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "json" => (
            "Parser",
            "Parse log lines as JSON, extracting fields as labels",
        ),
        "logfmt" => (
            "Parser",
            "Parse log lines as logfmt key=value pairs",
        ),
        "pattern" => (
            "Parser",
            "Extract fields using a pattern expression: pattern \"<ip> - <_>\"",
        ),
        "regexp" => (
            "Parser",
            "Extract fields using a regular expression with named capture groups",
        ),
        "unpack" => (
            "Parser",
            "Unpack packed log entries (used with pack stage)",
        ),
        "pack" => (
            "Parser",
            "Pack log entry and labels into a JSON object",
        ),
        "line_format" => (
            "Formatter",
            "Rewrite the log line using a Go template: line_format \"{{.field}}\"",
        ),
        "label_format" => (
            "Formatter",
            "Rename, modify, or add labels: label_format dst=src",
        ),
        "drop" => (
            "Label operation",
            "Remove specified labels from the stream",
        ),
        "keep" => (
            "Label operation",
            "Keep only the specified labels, removing all others",
        ),
        "decolorize" => (
            "Label operation",
            "Strip ANSI color escape codes from log lines",
        ),
        "unwrap" => (
            "Unwrap",
            "Extract a numeric value from a label for metric aggregation",
        ),
        _ => return None,
    })
}

/// Common duration suggestions with descriptions.
const DURATION_SUGGESTIONS: &[(&str, &str)] = &[
    ("1m", "1 minute"),
    ("5m", "5 minutes"),
    ("10m", "10 minutes"),
    ("15m", "15 minutes"),
    ("30m", "30 minutes"),
    ("1h", "1 hour"),
    ("2h", "2 hours"),
    ("6h", "6 hours"),
    ("12h", "12 hours"),
    ("24h", "24 hours"),
    ("1d", "1 day"),
    ("7d", "7 days"),
    ("30d", "30 days"),
];

/// Pipeline keyword completions with semantic ranking.
///
/// Parsers are ranked first (most commonly used), then formatters,
/// label operations, and unwrap.
fn complete_pipeline_keywords(prefix: &str, max: usize) -> Vec<CompletionItem> {
    let pl = prefix.to_lowercase();

    // Semantic ranking: parsers first, then formatters, then label ops
    let ranked: &[(&str, u32)] = &[
        ("json", 0),
        ("logfmt", 1),
        ("regexp", 2),
        ("pattern", 3),
        ("unpack", 4),
        ("pack", 5),
        ("line_format", 10),
        ("label_format", 11),
        ("drop", 20),
        ("keep", 21),
        ("decolorize", 22),
        ("unwrap", 30),
    ];

    let mut items: Vec<CompletionItem> = ranked
        .iter()
        .filter(|(kw, _)| pl.is_empty() || kw.starts_with(pl.as_str()))
        .map(|(kw, priority)| {
            let (detail, doc) = pipeline_docs(kw).unwrap_or(("Pipeline", ""));
            CompletionItem {
                label: kw.to_string(),
                detail: Some(detail.to_string()),
                documentation: if doc.is_empty() {
                    None
                } else {
                    Some(doc.to_string())
                },
                kind: CompletionKind::PipelineKeyword,
                sort_priority: *priority,
            }
        })
        .collect();

    items.sort_by_key(|i| i.sort_priority);
    items.truncate(max);
    items
}

/// Expression-level completions with semantic ranking.
///
/// Range functions are ranked first (most commonly used at expression level),
/// then aggregation operators, then other keywords.
fn complete_expression(prefix: &str, max: usize) -> Vec<CompletionItem> {
    let pl = prefix.to_lowercase();
    let mut items = Vec::new();

    // Range functions (most commonly used at expression level)
    for (i, func) in keywords::RANGE_FUNCTIONS.iter().enumerate() {
        if func.starts_with(pl.as_str()) {
            let (detail, doc) = function_docs(func).unwrap_or(("Function", ""));
            items.push(CompletionItem {
                label: func.to_string(),
                detail: Some(detail.to_string()),
                documentation: if doc.is_empty() {
                    None
                } else {
                    Some(doc.to_string())
                },
                kind: CompletionKind::Function,
                sort_priority: i as u32,
            });
        }
    }

    // Aggregation operators
    for (i, op) in keywords::AGGREGATION_OPS.iter().enumerate() {
        if op.starts_with(pl.as_str()) {
            let (detail, doc) = function_docs(op).unwrap_or(("Aggregation", ""));
            items.push(CompletionItem {
                label: op.to_string(),
                detail: Some(detail.to_string()),
                documentation: if doc.is_empty() {
                    None
                } else {
                    Some(doc.to_string())
                },
                kind: CompletionKind::AggregationOp,
                sort_priority: 100 + i as u32,
            });
        }
    }

    // Other keywords (by, without, and, or, unless)
    for kw in keywords::AGGREGATION_MODIFIERS
        .iter()
        .chain(keywords::BINARY_KEYWORDS.iter())
    {
        if kw.starts_with(pl.as_str()) {
            items.push(CompletionItem {
                label: kw.to_string(),
                detail: Some("Keyword".to_string()),
                documentation: None,
                kind: CompletionKind::Keyword,
                sort_priority: 200,
            });
        }
    }

    items.sort_by(|a, b| a.sort_priority.cmp(&b.sort_priority).then(a.label.cmp(&b.label)));
    items.dedup_by(|a, b| a.label == b.label);
    items.truncate(max);
    items
}

/// Duration completions filtered by prefix.
fn complete_durations(prefix: &str, max: usize) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = DURATION_SUGGESTIONS
        .iter()
        .enumerate()
        .filter(|(_, (dur, _))| prefix.is_empty() || dur.starts_with(prefix))
        .map(|(i, (dur, desc))| CompletionItem {
            label: dur.to_string(),
            detail: Some(desc.to_string()),
            documentation: None,
            kind: CompletionKind::Duration,
            sort_priority: i as u32,
        })
        .collect();

    items.truncate(max);
    items
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(query: &str, pos: usize) -> CursorContext {
        detect_cursor_context(query, pos)
    }

    fn items(
        ctx: &CursorContext,
        names: &HashMap<String, Vec<String>>,
        values: &HashMap<(String, String), Vec<String>>,
        max: usize,
    ) -> Vec<CompletionItem> {
        compute_completion_items(ctx, names, values, max)
    }

    fn labels(completions: &[CompletionItem]) -> Vec<String> {
        completion_labels(completions)
    }

    // ── Context detection: empty / expression ──────────────────────────

    #[test]
    fn context_empty_query() {
        match ctx("", 0) {
            CursorContext::Expression { prefix } => assert!(prefix.is_empty()),
            other => panic!("expected Expression, got {other:?}"),
        }
    }

    #[test]
    fn context_expression_prefix() {
        match ctx("ra", 2) {
            CursorContext::Expression { prefix } => assert_eq!(prefix, "ra"),
            other => panic!("expected Expression, got {other:?}"),
        }
    }

    #[test]
    fn context_expression_full_function_name() {
        match ctx("rate", 4) {
            CursorContext::Expression { prefix } => assert_eq!(prefix, "rate"),
            other => panic!("expected Expression, got {other:?}"),
        }
    }

    // ── Context detection: label name ──────────────────────────────────

    #[test]
    fn context_opening_brace() {
        match ctx("{", 1) {
            CursorContext::LabelName { prefix, selector } => {
                assert!(prefix.is_empty());
                assert!(selector.is_empty());
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn context_partial_label_name() {
        match ctx("{jo", 3) {
            CursorContext::LabelName { prefix, .. } => assert_eq!(prefix, "jo"),
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn context_after_comma_in_braces() {
        match ctx(r#"{job="varlogs","#, 15) {
            CursorContext::LabelName { prefix, selector } => {
                assert!(prefix.is_empty());
                assert_eq!(selector, r#"{job="varlogs"}"#);
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn context_nested_in_rate() {
        match ctx("rate({", 6) {
            CursorContext::LabelName { prefix, selector } => {
                assert!(prefix.is_empty());
                assert!(selector.is_empty());
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn context_label_name_in_rate_with_matchers() {
        match ctx(r#"rate({job="varlogs","#, 20) {
            CursorContext::LabelName { prefix, selector } => {
                assert!(prefix.is_empty());
                assert_eq!(selector, r#"{job="varlogs"}"#);
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    // ── Context detection: operator suppression ────────────────────────

    #[test]
    fn context_after_eq_suppressed() {
        assert!(matches!(ctx("{job=", 5), CursorContext::None));
    }

    #[test]
    fn context_after_neq_suppressed() {
        assert!(matches!(ctx("{job!=", 6), CursorContext::None));
    }

    #[test]
    fn context_after_regex_suppressed() {
        assert!(matches!(ctx("{job=~", 6), CursorContext::None));
    }

    #[test]
    fn context_after_notmatch_suppressed() {
        assert!(matches!(ctx("{job!~", 6), CursorContext::None));
    }

    // ── Context detection: label value ─────────────────────────────────

    #[test]
    fn context_label_value_basic() {
        match ctx(r#"{job="no"#, 8) {
            CursorContext::LabelValue {
                label,
                prefix,
                selector,
            } => {
                assert_eq!(label, "job");
                assert_eq!(prefix, "no");
                assert!(selector.is_empty());
            }
            other => panic!("expected LabelValue, got {other:?}"),
        }
    }

    #[test]
    fn context_label_value_empty_prefix() {
        match ctx(r#"{job=""#, 6) {
            CursorContext::LabelValue {
                label, prefix, ..
            } => {
                assert_eq!(label, "job");
                assert!(prefix.is_empty());
            }
            other => panic!("expected LabelValue, got {other:?}"),
        }
    }

    #[test]
    fn context_label_value_regex_operator() {
        match ctx(r#"{job=~"no"#, 9) {
            CursorContext::LabelValue {
                label, prefix, ..
            } => {
                assert_eq!(label, "job");
                assert_eq!(prefix, "no");
            }
            other => panic!("expected LabelValue, got {other:?}"),
        }
    }

    #[test]
    fn context_label_value_with_prior_matchers() {
        match ctx(r#"{job="varlogs", env=""#, 21) {
            CursorContext::LabelValue {
                label, selector, ..
            } => {
                assert_eq!(label, "env");
                assert_eq!(selector, r#"{job="varlogs"}"#);
            }
            other => panic!("expected LabelValue, got {other:?}"),
        }
    }

    #[test]
    fn context_unclosed_string_outside_braces_is_none() {
        match ctx(r#"{job="varlogs"} |= "err"#, 23) {
            CursorContext::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    // ── Context detection: duration ────────────────────────────────────

    #[test]
    fn context_duration_empty() {
        match ctx(r#"rate({job="varlogs"}["#, 21) {
            CursorContext::Duration { prefix } => assert!(prefix.is_empty()),
            other => panic!("expected Duration, got {other:?}"),
        }
    }

    #[test]
    fn context_duration_with_prefix() {
        match ctx(r#"rate({job="varlogs"}[5"#, 22) {
            CursorContext::Duration { prefix } => assert_eq!(prefix, "5"),
            other => panic!("expected Duration, got {other:?}"),
        }
    }

    // ── Context detection: pipeline ────────────────────────────────────

    #[test]
    fn context_pipeline_after_pipe() {
        match ctx(r#"{job="varlogs"} | "#, 18) {
            CursorContext::PipelineStage { prefix } => assert!(prefix.is_empty()),
            other => panic!("expected PipelineStage, got {other:?}"),
        }
    }

    #[test]
    fn context_pipeline_with_prefix() {
        match ctx(r#"{job="varlogs"} | js"#, 20) {
            CursorContext::PipelineStage { prefix } => assert_eq!(prefix, "js"),
            other => panic!("expected PipelineStage, got {other:?}"),
        }
    }

    #[test]
    fn context_pipeline_after_filter_stage() {
        match ctx(r#"{job="varlogs"} |= "error" | "#, 29) {
            CursorContext::PipelineStage { prefix } => assert!(prefix.is_empty()),
            other => panic!("expected PipelineStage, got {other:?}"),
        }
    }

    #[test]
    fn context_pipeline_no_space_after_pipe() {
        match ctx(r#"{job="varlogs"} |js"#, 19) {
            CursorContext::PipelineStage { prefix } => assert_eq!(prefix, "js"),
            other => panic!("expected PipelineStage, got {other:?}"),
        }
    }

    // ── Context detection: by/without ──────────────────────────────────

    #[test]
    fn context_label_list_by() {
        match ctx("sum by (jo", 10) {
            CursorContext::LabelName { prefix, selector } => {
                assert_eq!(prefix, "jo");
                assert!(selector.is_empty());
            }
            other => panic!("expected LabelName in by(), got {other:?}"),
        }
    }

    #[test]
    fn context_label_list_without() {
        match ctx(r#"sum(rate({j="t"}[5m])) without (ins"#, 35) {
            CursorContext::LabelName { prefix, selector } => {
                assert_eq!(prefix, "ins");
                assert!(selector.is_empty());
            }
            other => panic!("expected LabelName in without(), got {other:?}"),
        }
    }

    #[test]
    fn context_by_after_comma() {
        match ctx("sum by (job, ", 13) {
            CursorContext::LabelName { prefix, .. } => assert!(prefix.is_empty()),
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    // ── Completion items: label names ──────────────────────────────────

    #[test]
    fn complete_label_names_basic() {
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        cache.insert(
            String::new(),
            vec!["job".into(), "instance".into(), "env".into()],
        );
        let c = CursorContext::LabelName {
            prefix: "j".into(),
            selector: String::new(),
        };
        let result = items(&c, &cache, &HashMap::new(), 10);
        assert_eq!(labels(&result), vec!["job"]);
        assert_eq!(result[0].kind, CompletionKind::LabelName);
        assert_eq!(result[0].detail.as_deref(), Some("Label"));
    }

    #[test]
    fn complete_label_names_with_selector() {
        let selector = r#"{job="varlogs"}"#.to_string();
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        cache.insert(String::new(), vec!["job".into(), "instance".into()]);
        cache.insert(selector.clone(), vec!["pod".into(), "container".into()]);

        let c = CursorContext::LabelName {
            prefix: String::new(),
            selector: selector.clone(),
        };
        let result = items(&c, &cache, &HashMap::new(), 10);
        assert_eq!(labels(&result), vec!["container", "pod"]);
    }

    // ── Completion items: label values ─────────────────────────────────

    #[test]
    fn complete_label_values_filtered() {
        let mut cache: HashMap<(String, String), Vec<String>> = HashMap::new();
        cache.insert(
            ("job".into(), String::new()),
            vec![
                "node".into(),
                "prometheus".into(),
                "alertmanager".into(),
            ],
        );
        let c = CursorContext::LabelValue {
            label: "job".into(),
            prefix: "pro".into(),
            selector: String::new(),
        };
        let result = items(&c, &HashMap::new(), &cache, 10);
        assert_eq!(labels(&result), vec!["prometheus"]);
        assert_eq!(result[0].kind, CompletionKind::LabelValue);
    }

    // ── Completion items: pipeline ─────────────────────────────────────

    #[test]
    fn complete_pipeline_all_shown() {
        let c = CursorContext::PipelineStage {
            prefix: String::new(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 20);
        let l = labels(&result);
        assert!(l.contains(&"json".to_string()));
        assert!(l.contains(&"logfmt".to_string()));
        assert!(l.contains(&"unwrap".to_string()));
        assert!(l.contains(&"line_format".to_string()));
        assert!(l.contains(&"drop".to_string()));
    }

    #[test]
    fn complete_pipeline_filtered() {
        let c = CursorContext::PipelineStage {
            prefix: "js".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert_eq!(labels(&result), vec!["json"]);
    }

    #[test]
    fn complete_pipeline_has_documentation() {
        let c = CursorContext::PipelineStage {
            prefix: "json".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].detail.as_deref(), Some("Parser"));
        assert!(result[0].documentation.is_some());
        assert!(result[0]
            .documentation
            .as_ref()
            .unwrap()
            .contains("JSON"));
    }

    #[test]
    fn complete_pipeline_semantic_ranking() {
        let c = CursorContext::PipelineStage {
            prefix: String::new(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 20);
        let l = labels(&result);
        // Parsers should come before formatters, which come before label ops
        let json_pos = l.iter().position(|s| s == "json").unwrap();
        let line_format_pos = l.iter().position(|s| s == "line_format").unwrap();
        let drop_pos = l.iter().position(|s| s == "drop").unwrap();
        let unwrap_pos = l.iter().position(|s| s == "unwrap").unwrap();
        assert!(json_pos < line_format_pos);
        assert!(line_format_pos < drop_pos);
        assert!(drop_pos < unwrap_pos);
    }

    // ── Completion items: expression ───────────────────────────────────

    #[test]
    fn complete_expression_filtered() {
        let c = CursorContext::Expression {
            prefix: "ra".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        let l = labels(&result);
        assert!(l.contains(&"rate".to_string()));
        assert!(l.iter().all(|s| s.starts_with("ra")));
    }

    #[test]
    fn complete_expression_has_documentation() {
        let c = CursorContext::Expression {
            prefix: "rate".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert!(!result.is_empty());
        let rate_item = result.iter().find(|i| i.label == "rate").unwrap();
        assert_eq!(rate_item.kind, CompletionKind::Function);
        assert_eq!(rate_item.detail.as_deref(), Some("Range function"));
        assert!(rate_item.documentation.is_some());
    }

    #[test]
    fn complete_expression_includes_aggregations() {
        let c = CursorContext::Expression {
            prefix: "su".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        let l = labels(&result);
        assert!(l.contains(&"sum".to_string()));
        assert!(l.contains(&"sum_over_time".to_string()));
    }

    #[test]
    fn complete_expression_semantic_ranking() {
        let c = CursorContext::Expression {
            prefix: "s".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 30);
        // Range functions should come before aggregation ops
        let sum_over_time_idx = result
            .iter()
            .position(|i| i.label == "sum_over_time")
            .unwrap();
        let sum_idx = result.iter().position(|i| i.label == "sum").unwrap();
        assert!(
            sum_over_time_idx < sum_idx,
            "range functions should rank before aggregation ops"
        );
    }

    #[test]
    fn complete_empty_expression_returns_empty() {
        let c = CursorContext::Expression {
            prefix: String::new(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert!(result.is_empty());
    }

    // ── Completion items: duration ─────────────────────────────────────

    #[test]
    fn complete_durations_all() {
        let c = CursorContext::Duration {
            prefix: String::new(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 20);
        let l = labels(&result);
        assert!(l.contains(&"5m".to_string()));
        assert!(l.contains(&"1h".to_string()));
        assert!(l.contains(&"1d".to_string()));
        assert!(result[0].kind == CompletionKind::Duration);
    }

    #[test]
    fn complete_durations_filtered() {
        let c = CursorContext::Duration {
            prefix: "1".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 20);
        let l = labels(&result);
        assert!(l.contains(&"1m".to_string()));
        assert!(l.contains(&"1h".to_string()));
        assert!(l.contains(&"1d".to_string()));
        // "5m" should NOT be included
        assert!(!l.contains(&"5m".to_string()));
    }

    #[test]
    fn complete_durations_have_descriptions() {
        let c = CursorContext::Duration {
            prefix: "5m".into(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].label, "5m");
        assert_eq!(result[0].detail.as_deref(), Some("5 minutes"));
    }

    // ── Completion items: limits and edge cases ────────────────────────

    #[test]
    fn complete_respects_max_limit() {
        let c = CursorContext::PipelineStage {
            prefix: String::new(),
        };
        let result = items(&c, &HashMap::new(), &HashMap::new(), 3);
        assert!(result.len() <= 3);
    }

    #[test]
    fn complete_none_returns_empty() {
        let c = CursorContext::None;
        let result = items(&c, &HashMap::new(), &HashMap::new(), 10);
        assert!(result.is_empty());
    }

    #[test]
    fn convenience_labels_function() {
        let completions = vec![
            CompletionItem {
                label: "json".into(),
                detail: None,
                documentation: None,
                kind: CompletionKind::PipelineKeyword,
                sort_priority: 0,
            },
            CompletionItem {
                label: "logfmt".into(),
                detail: None,
                documentation: None,
                kind: CompletionKind::PipelineKeyword,
                sort_priority: 1,
            },
        ];
        assert_eq!(completion_labels(&completions), vec!["json", "logfmt"]);
    }
}
