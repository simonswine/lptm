/// PromQL aggregation operators (from promql.terms.ts).
pub const AGGREGATION_OPS: &[&str] = &[
    "avg",
    "bottomk",
    "count",
    "count_values",
    "group",
    "limitk",
    "limit_ratio",
    "max",
    "min",
    "quantile",
    "stddev",
    "stdvar",
    "sum",
    "topk",
];

/// Aggregation modifiers.
pub const AGGREGATION_MODIFIERS: &[&str] = &["by", "without"];

/// All PromQL built-in functions (from promql.terms.ts).
pub const FUNCTIONS: &[&str] = &[
    "abs",
    "absent",
    "absent_over_time",
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atanh",
    "avg_over_time",
    "ceil",
    "changes",
    "clamp",
    "clamp_max",
    "clamp_min",
    "cos",
    "cosh",
    "count_over_time",
    "day_of_month",
    "day_of_week",
    "day_of_year",
    "days_in_month",
    "deg",
    "delta",
    "deriv",
    "double_exponential_smoothing",
    "exp",
    "floor",
    "histogram_avg",
    "histogram_count",
    "histogram_fraction",
    "histogram_quantile",
    "histogram_quantiles",
    "histogram_stddev",
    "histogram_stdvar",
    "histogram_sum",
    "holt_winters",
    "hour",
    "idelta",
    "increase",
    "info",
    "irate",
    "label_join",
    "label_replace",
    "last_over_time",
    "ln",
    "log2",
    "log10",
    "mad_over_time",
    "max_over_time",
    "min_over_time",
    "minute",
    "month",
    "pi",
    "predict_linear",
    "present_over_time",
    "quantile_over_time",
    "rad",
    "rate",
    "resets",
    "round",
    "scalar",
    "sgn",
    "sin",
    "sinh",
    "sort",
    "sort_by_label",
    "sort_by_label_desc",
    "sort_desc",
    "sqrt",
    "stddev_over_time",
    "stdvar_over_time",
    "sum_over_time",
    "tan",
    "tanh",
    "time",
    "timestamp",
    "ts_of_last_over_time",
    "ts_of_max_over_time",
    "ts_of_min_over_time",
    "vector",
    "year",
];

/// Binary operator keywords.
pub const BINARY_KEYWORDS: &[&str] = &["and", "atan2", "or", "unless"];

/// Binary expression modifier keywords.
pub const BINARY_MODIFIERS: &[&str] = &[
    "bool",
    "fill",
    "fill_left",
    "fill_right",
    "group_left",
    "group_right",
    "ignoring",
    "on",
];

/// Special value literals.
pub const LITERALS: &[&str] = &["inf", "nan"];

/// Returns all keywords that appear as identifiers in a query expression
/// (aggregation ops + functions + binary keywords + modifiers + literals).
pub fn all_completable_keywords() -> impl Iterator<Item = &'static str> {
    AGGREGATION_OPS
        .iter()
        .chain(AGGREGATION_MODIFIERS.iter())
        .chain(FUNCTIONS.iter())
        .chain(BINARY_KEYWORDS.iter())
        .chain(BINARY_MODIFIERS.iter())
        .chain(LITERALS.iter())
        .copied()
}
