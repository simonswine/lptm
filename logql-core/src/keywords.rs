/// LogQL pipeline stage parsers (suggested after `|`).
pub const PARSERS: &[&str] = &[
    "decolorize",
    "drop",
    "json",
    "keep",
    "label_format",
    "line_format",
    "logfmt",
    "pack",
    "pattern",
    "regexp",
    "unpack",
    "unwrap",
];

/// LogQL range aggregation functions (wrap a log query in `func({...}[dur])`).
pub const RANGE_FUNCTIONS: &[&str] = &[
    "absent_over_time",
    "avg_over_time",
    "bytes_over_time",
    "bytes_rate",
    "count_over_time",
    "first_over_time",
    "last_over_time",
    "max_over_time",
    "min_over_time",
    "quantile_over_time",
    "rate",
    "stddev_over_time",
    "stdvar_over_time",
    "sum_over_time",
];

/// LogQL aggregation operators.
pub const AGGREGATION_OPS: &[&str] = &[
    "avg",
    "bottomk",
    "count",
    "max",
    "min",
    "sort",
    "sort_desc",
    "stddev",
    "stdvar",
    "sum",
    "topk",
];

/// Aggregation modifiers.
pub const AGGREGATION_MODIFIERS: &[&str] = &["by", "without"];

/// Binary operator keywords.
pub const BINARY_KEYWORDS: &[&str] = &["and", "or", "unless"];

/// Returns all keywords that can appear at expression level.
pub fn all_expression_keywords() -> impl Iterator<Item = &'static str> {
    RANGE_FUNCTIONS
        .iter()
        .chain(AGGREGATION_OPS.iter())
        .chain(AGGREGATION_MODIFIERS.iter())
        .chain(BINARY_KEYWORDS.iter())
        .copied()
}

/// Returns all keywords that can appear after `|` in a pipeline.
pub fn all_pipeline_keywords() -> impl Iterator<Item = &'static str> {
    PARSERS.iter().copied()
}
