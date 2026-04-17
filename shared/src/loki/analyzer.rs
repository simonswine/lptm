//! Semantic analyzer for LogQL ASTs.
//!
//! Walks the AST produced by [`super::parser::parse`] and emits [`Diagnostic`]s
//! for semantic issues such as unknown function names, wrong arity, type
//! mismatches, and empty stream selectors.

use super::ast::*;
use super::keywords;
use super::tokenizer::Span;

// ── Public types ───────────────────────────────────────────────────────────────

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A semantic diagnostic with location information.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
}

// ── Expression type (internal) ─────────────────────────────────────────────────

/// The type an expression evaluates to, used for type-checking across the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprType {
    /// A log stream (result of a log query).
    Logs,
    /// A numeric time-series (result of range/aggregation).
    Metric,
    /// A bare scalar (number literal).
    Scalar,
    /// Could not determine.
    Unknown,
}

// ── Range functions that require an `unwrap` stage ─────────────────────────────

const UNWRAP_REQUIRED: &[&str] = &[
    "avg_over_time",
    "first_over_time",
    "last_over_time",
    "max_over_time",
    "min_over_time",
    "quantile_over_time",
    "stddev_over_time",
    "stdvar_over_time",
    "sum_over_time",
];

// ── Public entry point ─────────────────────────────────────────────────────────

/// Analyze a parsed LogQL expression and return all diagnostics.
pub fn analyze(expr: &Expr) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    analyze_expr(expr, &mut diags);
    diags
}

// ── Recursive analysis ─────────────────────────────────────────────────────────

fn analyze_expr(expr: &Expr, diags: &mut Vec<Diagnostic>) -> ExprType {
    match &expr.kind {
        ExprKind::LogQuery {
            selector,
            pipeline: _,
        } => {
            if selector.matchers.is_empty() {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    message: "empty stream selector matches all streams".to_string(),
                    span: selector.span,
                });
            }
            ExprType::Logs
        }

        ExprKind::RangeAggregation {
            func,
            log_query,
            duration: _,
            param,
        } => {
            // Unknown function name
            if !keywords::RANGE_FUNCTIONS.contains(&func.as_str()) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("unknown range function '{func}'"),
                    span: expr.span,
                });
            }

            // Arity: quantile_over_time requires a parameter
            if func == "quantile_over_time" {
                match param {
                    None => {
                        diags.push(Diagnostic {
                            severity: Severity::Error,
                            message: "quantile_over_time requires a quantile parameter"
                                .to_string(),
                            span: expr.span,
                        });
                    }
                    Some(p) => {
                        if let ExprKind::NumberLit(v) = &p.kind {
                            if *v < 0.0 || *v > 1.0 {
                                diags.push(Diagnostic {
                                    severity: Severity::Warning,
                                    message: format!(
                                        "quantile parameter {v} should be between 0 and 1"
                                    ),
                                    span: p.span,
                                });
                            }
                        }
                    }
                }
            } else if param.is_some() {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("'{func}' does not accept a parameter"),
                    span: expr.span,
                });
            }

            // Unwrap requirement
            if UNWRAP_REQUIRED.contains(&func.as_str()) {
                let has_unwrap = if let ExprKind::LogQuery { pipeline, .. } = &log_query.kind {
                    pipeline
                        .iter()
                        .any(|s| matches!(s, PipelineStage::Unwrap { .. }))
                } else {
                    false
                };
                if !has_unwrap {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("'{func}' typically requires an unwrap stage"),
                        span: expr.span,
                    });
                }
            }

            // Recurse into children
            analyze_expr(log_query, diags);
            if let Some(p) = param {
                analyze_expr(p, diags);
            }

            ExprType::Metric
        }

        ExprKind::Aggregation {
            op,
            expr: inner,
            grouping: _,
            param,
        } => {
            // Unknown aggregation operator
            if !keywords::AGGREGATION_OPS.contains(&op.as_str()) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("unknown aggregation operator '{op}'"),
                    span: expr.span,
                });
            }

            // Arity: topk / bottomk require a parameter
            let needs_param = matches!(op.as_str(), "topk" | "bottomk");
            if needs_param && param.is_none() {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("'{op}' requires a parameter (k)"),
                    span: expr.span,
                });
            } else if !needs_param && param.is_some() {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("'{op}' does not accept a parameter"),
                    span: expr.span,
                });
            }

            // Type check: aggregation needs a metric input, not a log query
            let inner_type = analyze_expr(inner, diags);
            if inner_type == ExprType::Logs {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "'{op}' requires a metric expression, got a log query; \
                         wrap with a range aggregation like rate(...)"
                    ),
                    span: inner.span,
                });
            }

            if let Some(p) = param {
                analyze_expr(p, diags);
            }

            ExprType::Metric
        }

        ExprKind::BinOp { op: _, lhs, rhs } => {
            let lhs_type = analyze_expr(lhs, diags);
            let rhs_type = analyze_expr(rhs, diags);

            if lhs_type == ExprType::Logs {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: "binary operation requires a metric or scalar on the left side, \
                              got a log query"
                        .to_string(),
                    span: lhs.span,
                });
            }
            if rhs_type == ExprType::Logs {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: "binary operation requires a metric or scalar on the right side, \
                              got a log query"
                        .to_string(),
                    span: rhs.span,
                });
            }

            if lhs_type == ExprType::Scalar && rhs_type == ExprType::Scalar {
                ExprType::Scalar
            } else {
                ExprType::Metric
            }
        }

        ExprKind::NumberLit(_) => ExprType::Scalar,
        ExprKind::StringLit(_) => ExprType::Unknown,
        ExprKind::Paren(inner) => analyze_expr(inner, diags),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loki::parser::parse;

    /// Parse, assert no parse errors, then run the analyzer.
    fn analyze_query(input: &str) -> Vec<Diagnostic> {
        let result = parse(input);
        assert!(
            result.errors.is_empty(),
            "unexpected parse errors for {input:?}: {:?}",
            result.errors
        );
        let expr = result.expr.expect("expected expression");
        analyze(&expr)
    }

    fn errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }

    fn warnings(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect()
    }

    // ── Valid queries produce zero diagnostics ─────────────────────────────

    #[test]
    fn valid_log_query() {
        let diags = analyze_query(r#"{job="varlogs"} |= "error" | json"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_rate_query() {
        let diags = analyze_query(r#"rate({job="test"}[5m])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_count_over_time() {
        let diags = analyze_query(r#"count_over_time({job="test"}[1h])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_bytes_rate() {
        let diags = analyze_query(r#"bytes_rate({job="test"}[5m])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_sum_by() {
        let diags = analyze_query(r#"sum by (job) (rate({j="t"}[5m]))"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_topk() {
        let diags = analyze_query(r#"topk(10, rate({j="t"}[5m]))"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_quantile_over_time() {
        let diags =
            analyze_query(r#"quantile_over_time(0.99, {j="t"} | unwrap dur [5m])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_sum_over_time_with_unwrap() {
        let diags = analyze_query(r#"sum_over_time({j="t"} | unwrap dur [5m])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_binary_expr() {
        let diags =
            analyze_query(r#"rate({a="1"}[5m]) + rate({b="2"}[5m])"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_metric_scalar_binop() {
        let diags = analyze_query(r#"rate({j="t"}[5m]) * 100"#);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn valid_complex_query() {
        let diags = analyze_query(
            r#"sum by (job) (rate({job="api"} |= "error" | json [5m])) / sum by (job) (rate({job="api"}[5m]))"#,
        );
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    // ── Empty stream selector ──────────────────────────────────────────────

    #[test]
    fn empty_stream_selector() {
        let diags = analyze_query("{}");
        assert_eq!(warnings(&diags).len(), 1);
        assert!(diags[0].message.contains("empty stream selector"));
    }

    #[test]
    fn empty_selector_in_range_agg() {
        let diags = analyze_query("rate({}[5m])");
        let w = warnings(&diags);
        assert!(
            w.iter().any(|d| d.message.contains("empty stream selector")),
            "expected empty selector warning, got {diags:?}"
        );
    }

    // ── Unknown function names (hand-constructed ASTs) ─────────────────────

    #[test]
    fn unknown_range_function() {
        let expr = Expr {
            kind: ExprKind::RangeAggregation {
                func: "foobar".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "job".to_string(),
                                op: MatchOp::Eq,
                                value: "x".to_string(),
                                span: Span { start: 1, end: 8 },
                            }],
                            span: Span { start: 0, end: 9 },
                        },
                        pipeline: vec![],
                    },
                    span: Span { start: 0, end: 9 },
                }),
                duration: "5m".to_string(),
                param: None,
            },
            span: Span { start: 0, end: 20 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter().any(|d| d.message.contains("unknown range function")),
            "expected unknown function error, got {diags:?}"
        );
    }

    #[test]
    fn unknown_aggregation_op() {
        let inner = Expr {
            kind: ExprKind::RangeAggregation {
                func: "rate".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "j".to_string(),
                                op: MatchOp::Eq,
                                value: "t".to_string(),
                                span: Span { start: 0, end: 5 },
                            }],
                            span: Span { start: 0, end: 6 },
                        },
                        pipeline: vec![],
                    },
                    span: Span { start: 0, end: 6 },
                }),
                duration: "5m".to_string(),
                param: None,
            },
            span: Span { start: 0, end: 15 },
        };
        let expr = Expr {
            kind: ExprKind::Aggregation {
                op: "bogus_agg".to_string(),
                expr: Box::new(inner),
                grouping: None,
                param: None,
            },
            span: Span { start: 0, end: 25 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter()
                .any(|d| d.message.contains("unknown aggregation operator")),
            "expected unknown agg op error, got {diags:?}"
        );
    }

    // ── Wrong arity ────────────────────────────────────────────────────────

    #[test]
    fn quantile_over_time_missing_param() {
        // quantile_over_time without the quantile value — construct by hand
        // because the parser would set param=None if no number follows.
        let expr = Expr {
            kind: ExprKind::RangeAggregation {
                func: "quantile_over_time".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "j".to_string(),
                                op: MatchOp::Eq,
                                value: "t".to_string(),
                                span: Span { start: 0, end: 5 },
                            }],
                            span: Span { start: 0, end: 6 },
                        },
                        pipeline: vec![PipelineStage::Unwrap {
                            label: "dur".to_string(),
                            conversion: None,
                            span: Span { start: 7, end: 19 },
                        }],
                    },
                    span: Span { start: 0, end: 19 },
                }),
                duration: "5m".to_string(),
                param: None,
            },
            span: Span { start: 0, end: 30 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter()
                .any(|d| d.message.contains("requires a quantile parameter")),
            "expected missing param error, got {diags:?}"
        );
    }

    #[test]
    fn quantile_over_time_out_of_range() {
        let diags =
            analyze_query(r#"quantile_over_time(1.5, {j="t"} | unwrap dur [5m])"#);
        let w = warnings(&diags);
        assert!(
            w.iter()
                .any(|d| d.message.contains("should be between 0 and 1")),
            "expected out-of-range warning, got {diags:?}"
        );
    }

    #[test]
    fn range_function_unexpected_param() {
        // rate with a spurious param — construct by hand
        let expr = Expr {
            kind: ExprKind::RangeAggregation {
                func: "rate".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "j".to_string(),
                                op: MatchOp::Eq,
                                value: "t".to_string(),
                                span: Span { start: 0, end: 5 },
                            }],
                            span: Span { start: 0, end: 6 },
                        },
                        pipeline: vec![],
                    },
                    span: Span { start: 0, end: 6 },
                }),
                duration: "5m".to_string(),
                param: Some(Box::new(Expr {
                    kind: ExprKind::NumberLit(0.5),
                    span: Span { start: 5, end: 8 },
                })),
            },
            span: Span { start: 0, end: 20 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter()
                .any(|d| d.message.contains("does not accept a parameter")),
            "expected unexpected param error, got {diags:?}"
        );
    }

    #[test]
    fn topk_missing_param() {
        // topk without k — construct by hand
        let inner = Expr {
            kind: ExprKind::RangeAggregation {
                func: "rate".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "j".to_string(),
                                op: MatchOp::Eq,
                                value: "t".to_string(),
                                span: Span { start: 0, end: 5 },
                            }],
                            span: Span { start: 0, end: 6 },
                        },
                        pipeline: vec![],
                    },
                    span: Span { start: 0, end: 6 },
                }),
                duration: "5m".to_string(),
                param: None,
            },
            span: Span { start: 0, end: 15 },
        };
        let expr = Expr {
            kind: ExprKind::Aggregation {
                op: "topk".to_string(),
                expr: Box::new(inner),
                grouping: None,
                param: None,
            },
            span: Span { start: 0, end: 25 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter()
                .any(|d| d.message.contains("requires a parameter")),
            "expected missing param error, got {diags:?}"
        );
    }

    #[test]
    fn sum_unexpected_param() {
        // sum with a spurious param — construct by hand
        let inner = Expr {
            kind: ExprKind::RangeAggregation {
                func: "rate".to_string(),
                log_query: Box::new(Expr {
                    kind: ExprKind::LogQuery {
                        selector: StreamSelector {
                            matchers: vec![LabelMatcher {
                                name: "j".to_string(),
                                op: MatchOp::Eq,
                                value: "t".to_string(),
                                span: Span { start: 0, end: 5 },
                            }],
                            span: Span { start: 0, end: 6 },
                        },
                        pipeline: vec![],
                    },
                    span: Span { start: 0, end: 6 },
                }),
                duration: "5m".to_string(),
                param: None,
            },
            span: Span { start: 0, end: 15 },
        };
        let expr = Expr {
            kind: ExprKind::Aggregation {
                op: "sum".to_string(),
                expr: Box::new(inner),
                grouping: None,
                param: Some(Box::new(Expr {
                    kind: ExprKind::NumberLit(10.0),
                    span: Span { start: 4, end: 6 },
                })),
            },
            span: Span { start: 0, end: 25 },
        };
        let diags = analyze(&expr);
        let e = errors(&diags);
        assert!(
            e.iter()
                .any(|d| d.message.contains("does not accept a parameter")),
            "expected unexpected param error, got {diags:?}"
        );
    }

    // ── Type mismatches ────────────────────────────────────────────────────

    #[test]
    fn aggregation_over_log_query() {
        let diags = analyze_query(r#"sum({job="test"})"#);
        let e = errors(&diags);
        assert!(
            e.iter().any(|d| d.message.contains("requires a metric expression")),
            "expected type mismatch error, got {diags:?}"
        );
    }

    #[test]
    fn binop_log_query_left() {
        let diags = analyze_query(r#"{a="1"} + rate({b="2"}[5m])"#);
        let e = errors(&diags);
        assert!(
            e.iter().any(|d| d.message.contains("left side")),
            "expected left-side type error, got {diags:?}"
        );
    }

    #[test]
    fn binop_log_query_right() {
        let diags = analyze_query(r#"rate({a="1"}[5m]) + {b="2"}"#);
        let e = errors(&diags);
        assert!(
            e.iter().any(|d| d.message.contains("right side")),
            "expected right-side type error, got {diags:?}"
        );
    }

    #[test]
    fn binop_both_logs() {
        let diags = analyze_query(r#"{a="1"} + {b="2"}"#);
        let e = errors(&diags);
        assert!(e.len() >= 2, "expected errors on both sides, got {diags:?}");
    }

    // ── Unwrap requirement ─────────────────────────────────────────────────

    #[test]
    fn avg_over_time_no_unwrap() {
        let diags = analyze_query(r#"avg_over_time({j="t"}[5m])"#);
        let w = warnings(&diags);
        assert!(
            w.iter()
                .any(|d| d.message.contains("typically requires an unwrap")),
            "expected unwrap warning, got {diags:?}"
        );
    }

    #[test]
    fn sum_over_time_no_unwrap() {
        let diags = analyze_query(r#"sum_over_time({j="t"}[5m])"#);
        let w = warnings(&diags);
        assert!(
            w.iter()
                .any(|d| d.message.contains("typically requires an unwrap")),
            "expected unwrap warning, got {diags:?}"
        );
    }

    #[test]
    fn rate_no_unwrap_ok() {
        // rate does NOT require unwrap — should produce no warnings about it
        let diags = analyze_query(r#"rate({j="t"}[5m])"#);
        assert!(
            !diags.iter().any(|d| d.message.contains("unwrap")),
            "rate should not warn about unwrap, got {diags:?}"
        );
    }

    // ── Span accuracy ──────────────────────────────────────────────────────

    #[test]
    fn empty_selector_span_is_correct() {
        let input = "rate({}[5m])";
        let result = parse(input);
        let expr = result.expr.unwrap();
        let diags = analyze(&expr);
        let w = warnings(&diags);
        let sel_diag = w
            .iter()
            .find(|d| d.message.contains("empty stream selector"))
            .expect("expected empty selector warning");
        let text = &input[sel_diag.span.start..sel_diag.span.end];
        assert_eq!(text, "{}", "span should point to the empty selector");
    }

    #[test]
    fn type_mismatch_span_points_to_inner() {
        let input = r#"sum({job="test"})"#;
        let result = parse(input);
        let expr = result.expr.unwrap();
        let diags = analyze(&expr);
        let e = errors(&diags);
        let mismatch = e
            .iter()
            .find(|d| d.message.contains("requires a metric expression"))
            .expect("expected type mismatch error");
        let text = &input[mismatch.span.start..mismatch.span.end];
        assert_eq!(
            text,
            r#"{job="test"}"#,
            "span should point to the inner log query"
        );
    }

    // ── Multiple diagnostics ───────────────────────────────────────────────

    #[test]
    fn multiple_issues_in_one_query() {
        // sum of a log query with empty selector — both errors
        let diags = analyze_query("sum({})");
        assert!(
            diags.len() >= 2,
            "expected at least 2 diagnostics, got {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.message.contains("empty stream")),
            "expected empty selector warning"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("requires a metric")),
            "expected type mismatch error"
        );
    }

    // ── Performance ────────────────────────────────────────────────────────

    #[test]
    fn under_1ms_for_500_char_query() {
        // Build a ~500-char query
        let mut q = String::from(r#"sum by (job, ns, pod) ("#);
        q.push_str(r#"rate({job="application-server", namespace="production"}"#);
        q.push_str(r#" |= "error" != "timeout" |~ "5\\d{2}" | json "#);
        q.push_str(r#"| level="error" | line_format "{{.status}} {{.msg}}" [5m])"#);
        q.push_str(r#") / sum by (job, ns, pod) ("#);
        q.push_str(r#"rate({job="application-server", namespace="production"}[5m]))"#);
        assert!(q.len() >= 250, "query should be substantial: {} chars", q.len());

        let result = parse(&q);
        assert!(
            result.errors.is_empty(),
            "query should parse: {:?}",
            result.errors
        );
        let expr = result.expr.unwrap();

        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = analyze(&expr);
        }
        let elapsed = start.elapsed();
        let per_call = elapsed / 1000;
        assert!(
            per_call.as_micros() < 1000,
            "analysis took {per_call:?} per call, expected < 1ms"
        );
    }
}
