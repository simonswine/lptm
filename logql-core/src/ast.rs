//! LogQL AST node types.
//!
//! Every node carries a [`Span`] that maps back to the original query string.

use crate::tokenizer::Span;

// ── Top-level result ────────────────────────────────────────────────────────

/// The result of parsing a LogQL query.
#[derive(Debug)]
pub struct ParseResult {
    pub expr: Option<Expr>,
    pub errors: Vec<ParseError>,
}

/// A parse error with location information.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

// ── Expression ──────────────────────────────────────────────────────────────

/// A parsed LogQL expression with span information.
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// The kind of expression node.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// Log query: stream selector with optional pipeline stages.
    LogQuery {
        selector: StreamSelector,
        pipeline: Vec<PipelineStage>,
    },
    /// Range aggregation: `rate({...}[5m])`, `quantile_over_time(0.99, {...}[5m])`
    RangeAggregation {
        func: String,
        log_query: Box<Expr>,
        duration: String,
        param: Option<Box<Expr>>,
    },
    /// Aggregation: `sum by (label) (expr)`, `topk(10, expr)`
    Aggregation {
        op: String,
        expr: Box<Expr>,
        grouping: Option<Grouping>,
        param: Option<Box<Expr>>,
    },
    /// Binary operation.
    BinOp {
        op: BinOpKind,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Number literal.
    NumberLit(f64),
    /// String literal.
    StringLit(String),
    /// Parenthesized expression.
    Paren(Box<Expr>),
}

// ── Stream selector ─────────────────────────────────────────────────────────

/// Stream selector: `{label="value", ...}`
#[derive(Debug, Clone)]
pub struct StreamSelector {
    pub matchers: Vec<LabelMatcher>,
    pub span: Span,
}

/// A single label matcher within a stream selector.
#[derive(Debug, Clone)]
pub struct LabelMatcher {
    pub name: String,
    pub op: MatchOp,
    pub value: String,
    pub span: Span,
}

/// Label matcher operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOp {
    Eq,  // =
    Neq, // !=
    Re,  // =~
    Nre, // !~
}

// ── Pipeline stages ─────────────────────────────────────────────────────────

/// Pipeline stage in a log query.
#[derive(Debug, Clone)]
pub enum PipelineStage {
    /// Line filter: `|= "error"`, `!= "debug"`, `|~ "pattern"`, `!~ "exclude"`
    Filter {
        op: FilterOp,
        pattern: String,
        span: Span,
    },
    /// Parser: `| json`, `| logfmt`, `| regexp "pattern"`
    Parser {
        kind: ParserKind,
        params: Vec<String>,
        span: Span,
    },
    /// Label format: `| label_format dst="src", ...`
    LabelFormat {
        mappings: Vec<(String, String)>,
        span: Span,
    },
    /// Line format: `| line_format "{{.msg}}"`
    LineFormat {
        template: String,
        span: Span,
    },
    /// Label operation: `| drop label1, label2` or `| keep label1`
    LabelOp {
        kind: LabelOpKind,
        labels: Vec<String>,
        span: Span,
    },
    /// Unwrap: `| unwrap label` or `| unwrap duration_seconds(label)`
    Unwrap {
        label: String,
        conversion: Option<String>,
        span: Span,
    },
    /// Decolorize: `| decolorize`
    Decolorize { span: Span },
    /// Label filter: `| level="error"`, `| status>=400`
    LabelFilter {
        name: String,
        op: LabelFilterOp,
        value: String,
        span: Span,
    },
}

impl PipelineStage {
    /// Returns the span of this pipeline stage.
    pub fn span(&self) -> Span {
        match self {
            Self::Filter { span, .. }
            | Self::Parser { span, .. }
            | Self::LabelFormat { span, .. }
            | Self::LineFormat { span, .. }
            | Self::LabelOp { span, .. }
            | Self::Unwrap { span, .. }
            | Self::Decolorize { span }
            | Self::LabelFilter { span, .. } => *span,
        }
    }
}

/// Filter operators for line filter stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    PipeExact, // |=
    PipeMatch, // |~
    NotEqual,  // !=
    NotMatch,  // !~
}

/// Parser kinds for pipeline parser stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserKind {
    Json,
    Logfmt,
    Pattern,
    Regexp,
    Unpack,
    Pack,
}

/// Label operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelOpKind {
    Drop,
    Keep,
}

/// Label filter operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelFilterOp {
    Eq,    // =
    Neq,   // !=
    Re,    // =~
    Nre,   // !~
    Gt,    // >
    GtEq,  // >=
    Lt,    // <
    LtEq,  // <=
    CmpEq, // ==
}

// ── Binary operations ───────────────────────────────────────────────────────

/// Binary operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOpKind {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    CmpEq,
    Neq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    Unless,
}

// ── Grouping ────────────────────────────────────────────────────────────────

/// Grouping clause: `by (label1, label2)` or `without (label1)`.
#[derive(Debug, Clone)]
pub struct Grouping {
    pub without: bool,
    pub labels: Vec<String>,
    pub span: Span,
}
