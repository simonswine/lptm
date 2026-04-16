use std::collections::HashMap;

use super::keywords;

// ── Completion context ────────────────────────────────────────────────────────

/// Completion context derived from the query text up to the cursor.
#[derive(Debug, Clone)]
pub enum CompletionCtx {
    /// Outside `{}` – completing a metric name or keyword.
    MetricOrKeyword { prefix: String },
    /// Inside `{}` but not in a string – completing a label name.
    /// `selector` mirrors the same field in `LabelValue`: the complete matchers
    /// already present in the same `{…}` block, used to scope the label-names
    /// API call.  Empty for `by(…)`/`without(…)` contexts.
    LabelName { prefix: String, selector: String },
    /// Inside a string in a label matcher – completing a label value.
    /// `selector` is a Prometheus selector built from the other complete matchers
    /// in the same `{…}` block, e.g. `up{namespace="prod"}`.  Empty when there
    /// are no other complete matchers.
    LabelValue { label: String, prefix: String, selector: String },
    /// No useful context (e.g. right after an operator, or empty).
    None,
}

/// Keywords whose parenthesised argument list contains label names.
const LABEL_LIST_KEYWORDS: &[&str] =
    &["by", "without", "on", "ignoring", "group_left", "group_right"];

// ── Public API ────────────────────────────────────────────────────────────────

/// Detect the completion context from the part of the query before the cursor.
pub fn detect_context(query: &str, cursor_pos: usize) -> CompletionCtx {
    let cursor_byte = char_to_byte(query, cursor_pos);
    let before = &query[..cursor_byte];

    // 1. Check for unclosed string inside braces (label value context).
    if let Some((label, prefix)) = find_label_value_context(before) {
        let selector = extract_label_selector(before);
        return CompletionCtx::LabelValue { label, prefix, selector };
    }

    // 2. Find the current word prefix.
    let word_start = word_boundary_byte(before);
    let prefix = before[word_start..].to_string();

    // 3. Brace context: inside a label selector `{…}`.
    let brace_depth = count_brace_depth(before);
    if brace_depth > 0 {
        // Suppress label-name completions when cursor is right after an operator
        // (the user is about to open a string value).
        if prefix.is_empty() && word_start > 0 {
            let before_word = before[..word_start].trim_end();
            if before_word.ends_with(|c: char| c == '=' || c == '~' || c == '!') {
                return CompletionCtx::None;
            }
        }
        let selector = extract_label_selector(before);
        return CompletionCtx::LabelName { prefix, selector };
    }

    // 4. Bracket context: inside a range/subquery `[…]` — durations only, no completions.
    if in_bracket_context(before) {
        return CompletionCtx::None;
    }

    // 5. Label-list paren context: inside `by(…)`, `without(…)`, `on(…)`, etc.
    // No {…} block here, so no selector to extract.
    if in_label_list_paren(before) {
        return CompletionCtx::LabelName { prefix, selector: String::new() };
    }

    // 6. Default: metric name or keyword.
    CompletionCtx::MetricOrKeyword { prefix }
}

/// Pure completion filter.
pub fn compute_completions(
    ctx: &CompletionCtx,
    metric_names: &[String],
    label_names_cache: &HashMap<String, Vec<String>>,
    label_values_cache: &HashMap<(String, String), Vec<String>>,
    max: usize,
) -> Vec<String> {
    match ctx {
        CompletionCtx::MetricOrKeyword { prefix } if !prefix.is_empty() => {
            let pl = prefix.to_lowercase();
            let mut results = Vec::new();

            // Static keywords first (they tend to be shorter / more recognisable).
            for kw in keywords::all_completable_keywords() {
                if kw.starts_with(pl.as_str()) {
                    results.push(kw.to_string());
                }
            }

            // Metric names from Prometheus.
            for name in metric_names {
                if name.to_lowercase().starts_with(&pl) {
                    results.push(name.clone());
                }
            }

            results.sort();
            results.dedup();
            results.truncate(max);
            results
        }

        CompletionCtx::LabelName { prefix, selector } => {
            let pl = prefix.to_lowercase();
            let names = label_names_cache
                .get(selector.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut results: Vec<String> = names
                .iter()
                .filter(|n| n.to_lowercase().starts_with(&pl))
                .cloned()
                .collect();
            // __name__ is a valid label selector; only offered without a filter selector.
            if selector.is_empty()
                && "__name__".starts_with(&pl)
                && !results.contains(&"__name__".to_string())
            {
                results.push("__name__".to_string());
            }
            results.sort();
            results.truncate(max);
            results
        }

        CompletionCtx::LabelValue { label, prefix, selector } => {
            let pl = prefix.to_lowercase();
            let key = (label.clone(), selector.clone());
            let values = label_values_cache
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut results: Vec<String> = values
                .iter()
                .filter(|v| v.to_lowercase().starts_with(&pl))
                .cloned()
                .collect();
            results.sort();
            results.truncate(max);
            results
        }

        _ => vec![],
    }
}

/// Return the byte offset of the start of the last "word" in `s`.
/// Word characters: ASCII alphanumeric, `_`, `:` (PromQL recording-rule names).
pub fn word_boundary_byte(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 0
        && (bytes[i - 1].is_ascii_alphanumeric()
            || bytes[i - 1] == b'_'
            || bytes[i - 1] == b':')
    {
        i -= 1;
    }
    i
}

/// Convert a char-indexed cursor position to a byte offset in `s`.
pub fn char_to_byte(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// True if the cursor sits inside an unmatched `[…]` (range vector or subquery).
pub(crate) fn in_bracket_context(before: &str) -> bool {
    let bytes = before.as_bytes();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut string_delim = b'"';
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes[i] == string_delim {
                in_string = false;
            }
        } else {
            match bytes[i] {
                b'"' | b'\'' | b'`' => {
                    in_string = true;
                    string_delim = bytes[i];
                }
                b'[' => depth += 1,
                b']' => depth = (depth - 1).max(0),
                _ => {}
            }
        }
        i += 1;
    }
    depth > 0
}

/// True if the cursor sits inside a paren group opened by a label-list keyword
/// (`by`, `without`, `on`, `ignoring`, `group_left`, `group_right`).
///
/// Works by scanning forward and maintaining a stack of paren contexts, where
/// each entry records whether that `(` was opened by a label-list keyword.
fn in_label_list_paren(before: &str) -> bool {
    let bytes = before.as_bytes();
    let mut in_string = false;
    let mut string_delim = b'"';
    // Stack entry: true = label-list paren, false = ordinary paren.
    let mut stack: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes[i] == string_delim {
                in_string = false;
            }
        } else {
            match bytes[i] {
                b'"' | b'\'' | b'`' => {
                    in_string = true;
                    string_delim = bytes[i];
                }
                b'(' => {
                    // Identify the keyword immediately before this `(`.
                    let before_paren = before[..i].trim_end();
                    let wb = word_boundary_byte(before_paren);
                    let kw = &before_paren[wb..];
                    stack.push(LABEL_LIST_KEYWORDS.contains(&kw));
                }
                b')' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        i += 1;
    }
    stack.last().copied().unwrap_or(false)
}

/// Find label value completion context: returns `(label_name, value_prefix)` if
/// the cursor is inside an unclosed quoted string that is part of a label matcher.
pub(crate) fn find_label_value_context(before: &str) -> Option<(String, String)> {
    let bytes = before.as_bytes();
    let len = bytes.len();
    let mut in_string = false;
    let mut string_delim = b'"';
    let mut string_start: Option<usize> = None;
    let mut brace_depth: i32 = 0;
    let mut i = 0;

    while i < len {
        if in_string {
            if bytes[i] == b'\\' {
                i = (i + 2).min(len);
                continue;
            }
            if bytes[i] == string_delim {
                in_string = false;
                string_start = None;
            }
        } else {
            match bytes[i] {
                b'{' => brace_depth += 1,
                b'}' => brace_depth = (brace_depth - 1).max(0),
                b'"' | b'\'' | b'`' if brace_depth > 0 => {
                    in_string = true;
                    string_delim = bytes[i];
                    string_start = Some(i);
                }
                _ => {}
            }
        }
        i += 1;
    }

    let start = string_start?;
    if !in_string {
        return None;
    }

    let value_prefix = before[start + 1..].to_string();

    // Find the label name by scanning backwards from the opening quote.
    let before_quote = &before[..start];
    let before_op = before_quote
        .trim_end_matches(|c: char| c == '=' || c == '~' || c == '!')
        .trim_end();
    let wb = word_boundary_byte(before_op);
    let label = before_op[wb..].to_string();

    if label.is_empty() {
        return None;
    }

    Some((label, value_prefix))
}

/// Count the net open-brace depth of `s`, respecting string literals.
pub(crate) fn count_brace_depth(s: &str) -> i32 {
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut string_delim = b'"';
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes[i] == string_delim {
                in_string = false;
            }
        } else {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => depth = (depth - 1).max(0),
                b'"' | b'\'' | b'`' => {
                    in_string = true;
                    string_delim = bytes[i];
                }
                _ => {}
            }
        }
        i += 1;
    }
    depth
}

/// Parse fully-formed `label op "value"` matchers from the text inside a `{…}`
/// block.  Stops as soon as it encounters an incomplete matcher (open string,
/// missing operator, etc.).  Returns each matcher as the raw original text, e.g.
/// `namespace="prod"` or `job!~"prom.*"`.
fn parse_complete_matchers(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut matchers = Vec::new();

    loop {
        // Skip whitespace and commas.
        while pos < len && matches!(bytes[pos], b' ' | b'\t' | b',') {
            pos += 1;
        }
        if pos >= len {
            break;
        }

        let matcher_start = pos;

        // Label name: [a-zA-Z_][a-zA-Z0-9_]*
        while pos < len && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
            pos += 1;
        }
        if pos == matcher_start {
            break; // no identifier
        }

        // Skip whitespace before operator.
        while pos < len && bytes[pos] == b' ' {
            pos += 1;
        }

        // Operator: = | != | =~ | !~
        if pos >= len {
            break;
        }
        match bytes[pos] {
            b'=' => {
                pos += 1;
                if pos < len && bytes[pos] == b'~' {
                    pos += 1;
                }
            }
            b'!' => {
                pos += 1;
                if pos < len && (bytes[pos] == b'=' || bytes[pos] == b'~') {
                    pos += 1;
                } else {
                    break; // invalid
                }
            }
            _ => break, // no valid operator
        }

        // Skip whitespace before value.
        while pos < len && bytes[pos] == b' ' {
            pos += 1;
        }

        // Quoted value (only double-quoted strings appear in Prometheus matchers).
        if pos >= len || bytes[pos] != b'"' {
            break; // incomplete – no opening quote
        }
        pos += 1; // skip "

        let mut escaped = false;
        let mut closed = false;
        while pos < len {
            if escaped {
                escaped = false;
                pos += 1;
                continue;
            }
            if bytes[pos] == b'\\' {
                escaped = true;
                pos += 1;
                continue;
            }
            if bytes[pos] == b'"' {
                pos += 1; // skip closing "
                closed = true;
                break;
            }
            pos += 1;
        }
        if !closed {
            break; // unclosed string – this is the one being edited
        }

        matchers.push(s[matcher_start..pos].to_string());
    }

    matchers
}

/// Build a Prometheus series selector from the complete label matchers that
/// appear before the cursor inside the same `{…}` block.
///
/// For `up{namespace="prod",pod="` this returns `up{namespace="prod"}`.
/// Returns an empty string when there are no complete matchers.
pub fn extract_label_selector(before: &str) -> String {
    let bytes = before.as_bytes();
    let len = bytes.len();

    // Forward-scan to find the innermost open `{`.
    let mut brace_stack: Vec<usize> = Vec::new();
    let mut in_string = false;
    let mut string_delim = b'"';
    let mut i = 0;

    while i < len {
        if in_string {
            if bytes[i] == b'\\' && string_delim != b'`' {
                i = (i + 2).min(len);
                continue;
            }
            if bytes[i] == string_delim {
                in_string = false;
            }
        } else {
            match bytes[i] {
                b'"' | b'\'' | b'`' => {
                    in_string = true;
                    string_delim = bytes[i];
                }
                b'{' => brace_stack.push(i),
                b'}' => {
                    brace_stack.pop();
                }
                _ => {}
            }
        }
        i += 1;
    }

    let brace_start = match brace_stack.last() {
        Some(&s) => s,
        None => return String::new(),
    };

    // Optional metric name immediately before `{`.
    let before_brace = before[..brace_start].trim_end_matches(' ');
    let name_start = before_brace
        .bytes()
        .enumerate()
        .rev()
        .find(|(_, b)| !b.is_ascii_alphanumeric() && *b != b'_' && *b != b':')
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    let metric_name = &before_brace[name_start..];

    let matchers = parse_complete_matchers(&before[brace_start + 1..]);

    if metric_name.is_empty() && matchers.is_empty() {
        return String::new();
    }

    format!("{}{{{}}}", metric_name, matchers.join(","))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn detect_context_metric_or_keyword() {
        let ctx = detect_context("ra", 2);
        assert!(matches!(ctx, CompletionCtx::MetricOrKeyword { prefix } if prefix == "ra"));
    }

    #[test]
    fn detect_context_empty_prefix_no_completions() {
        let ctx = detect_context("", 0);
        assert!(matches!(ctx, CompletionCtx::MetricOrKeyword { ref prefix } if prefix.is_empty()));
        let results = compute_completions(&ctx, &[], &HashMap::<String,Vec<String>>::new(), &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn detect_context_label_name() {
        let ctx = detect_context("up{jo", 5);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "jo"),
            "expected LabelName context, got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_label_name_selector_populated() {
        // When namespace is already set, the selector for completing the next
        // label name should include it.
        let ctx = detect_context(r#"up{namespace="prod","#, 20);
        match ctx {
            CompletionCtx::LabelName { ref prefix, ref selector } => {
                assert!(prefix.is_empty());
                assert_eq!(selector, r#"up{namespace="prod"}"#);
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_label_name_in_by_no_selector() {
        // by(…) has no {…} context, so selector must be empty.
        let ctx = detect_context("sum by (job", 11);
        match ctx {
            CompletionCtx::LabelName { ref selector, .. } => {
                assert!(selector.is_empty(), "expected empty selector in by(), got {selector:?}");
            }
            other => panic!("expected LabelName, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_label_value() {
        let ctx = detect_context(r#"up{job="no"#, 10);
        assert!(
            matches!(ctx, CompletionCtx::LabelValue { ref label, ref prefix, .. }
                if label == "job" && prefix == "no"),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_label_value_selector_populated() {
        // When namespace is already set, the selector for pod completions should
        // include namespace="prod".
        let query = r#"up{namespace="prod",pod=""#;
        let ctx = detect_context(query, query.chars().count());
        match ctx {
            CompletionCtx::LabelValue { ref label, ref selector, .. } => {
                assert_eq!(label, "pod");
                assert_eq!(selector, r#"up{namespace="prod"}"#);
            }
            other => panic!("expected LabelValue, got {other:?}"),
        }
    }

    #[test]
    fn extract_label_selector_with_metric() {
        let before = r#"up{namespace="prod",pod=""#;
        assert_eq!(extract_label_selector(before), r#"up{namespace="prod"}"#);
    }

    #[test]
    fn extract_label_selector_no_prior_matchers() {
        // Only one matcher and it's incomplete — nothing to build a selector from.
        let before = r#"{job=""#;
        assert_eq!(extract_label_selector(before), String::new());
    }

    #[test]
    fn extract_label_selector_multiple_matchers() {
        let before = r#"metric{a="1",b!~"x.*",c=""#;
        assert_eq!(extract_label_selector(before), r#"metric{a="1",b!~"x.*"}"#);
    }

    #[test]
    fn detect_context_after_operator_no_completion() {
        let ctx = detect_context("up{job=", 7);
        assert!(matches!(ctx, CompletionCtx::None), "got {:?}", ctx);
    }

    #[test]
    fn detect_context_by_clause() {
        let ctx = detect_context("sum by (jo", 10);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "jo"),
            "expected LabelName in by(), got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_without_clause() {
        let ctx = detect_context("sum(up) without (ins", 20);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "ins"),
            "expected LabelName in without(), got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_on_clause() {
        let ctx = detect_context("metric1 + on (jo", 16);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "jo"),
            "expected LabelName in on(), got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_group_left_clause() {
        let ctx = detect_context("a * group_left (ins", 19);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "ins"),
            "expected LabelName in group_left(), got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_in_bracket_suppresses_completion() {
        let ctx = detect_context("rate(http_requests_total[5", 25);
        assert!(matches!(ctx, CompletionCtx::None), "expected None inside [], got {:?}", ctx);
    }

    #[test]
    fn detect_context_after_closed_bracket_restores_completion() {
        let ctx = detect_context("rate(http_requests_total[5m])", 29);
        assert!(
            matches!(ctx, CompletionCtx::MetricOrKeyword { .. }),
            "expected MetricOrKeyword after closed brackets, got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_by_with_multiple_labels() {
        let ctx = detect_context("sum by (job, ", 13);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix.is_empty()),
            "expected LabelName with empty prefix after comma, got {:?}",
            ctx
        );
    }

    #[test]
    fn completions_filter_keywords() {
        let ctx = CompletionCtx::MetricOrKeyword { prefix: "ra".into() };
        let results = compute_completions(&ctx, &[], &HashMap::<String,Vec<String>>::new(), &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert!(results.contains(&"rate".to_string()), "{results:?}");
        assert!(!results.iter().any(|r| !r.starts_with("ra")));
    }

    #[test]
    fn completions_include_metric_names() {
        let metrics = vec![
            "rate_total".to_string(),
            "requests_total".to_string(),
            "up".to_string(),
        ];
        let ctx = CompletionCtx::MetricOrKeyword { prefix: "re".into() };
        let results = compute_completions(&ctx, &metrics, &HashMap::<String,Vec<String>>::new(), &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert!(results.contains(&"requests_total".to_string()), "{results:?}");
        assert!(!results.contains(&"up".to_string()));
    }

    #[test]
    fn completions_label_name() {
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        cache.insert(
            String::new(),
            vec!["job".to_string(), "instance".to_string(), "env".to_string()],
        );
        let ctx = CompletionCtx::LabelName { prefix: "j".into(), selector: String::new() };
        let results = compute_completions(&ctx, &[], &cache, &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert_eq!(results, vec!["job"]);
    }

    #[test]
    fn completions_label_name_with_selector() {
        // Only the labels cached under the specific selector should be returned.
        let selector = r#"up{namespace="prod"}"#.to_string();
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        cache.insert(String::new(), vec!["job".into(), "instance".into(), "namespace".into()]);
        cache.insert(selector.clone(), vec!["pod".into(), "container".into()]);

        let ctx = CompletionCtx::LabelName { prefix: String::new(), selector: selector.clone() };
        let results = compute_completions(&ctx, &[], &cache, &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert_eq!(results, vec!["container", "pod"]);

        // Unfiltered context still returns the full list.
        let ctx_all = CompletionCtx::LabelName { prefix: String::new(), selector: String::new() };
        let results_all = compute_completions(&ctx_all, &[], &cache, &HashMap::<(String,String),Vec<String>>::new(), 10);
        assert!(results_all.contains(&"job".to_string()));
    }

    #[test]
    fn completions_label_value() {
        let mut cache: HashMap<(String, String), Vec<String>> = HashMap::new();
        cache.insert(("job".into(), String::new()), vec![
            "node".to_string(),
            "prometheus".to_string(),
            "alertmanager".to_string(),
        ]);
        let ctx = CompletionCtx::LabelValue {
            label: "job".into(),
            prefix: "pro".into(),
            selector: String::new(),
        };
        let results = compute_completions(&ctx, &[], &HashMap::<String,Vec<String>>::new(), &cache, 10);
        assert_eq!(results, vec!["prometheus"]);
    }

    #[test]
    fn completions_label_value_with_selector() {
        // Values cached under a specific selector should be served when that
        // selector is active, and not served for a different selector.
        let selector = r#"up{namespace="prod"}"#.to_string();
        let mut cache: HashMap<(String, String), Vec<String>> = HashMap::new();
        cache.insert(("pod".into(), selector.clone()), vec![
            "app-1".to_string(),
            "app-2".to_string(),
        ]);

        let ctx_match = CompletionCtx::LabelValue {
            label: "pod".into(),
            prefix: String::new(),
            selector: selector.clone(),
        };
        assert_eq!(
            compute_completions(&ctx_match, &[], &HashMap::<String,Vec<String>>::new(), &cache, 10),
            vec!["app-1", "app-2"]
        );

        // Different selector → no cached values yet.
        let ctx_other = CompletionCtx::LabelValue {
            label: "pod".into(),
            prefix: String::new(),
            selector: String::new(),
        };
        assert!(compute_completions(&ctx_other, &[], &HashMap::<String,Vec<String>>::new(), &cache, 10).is_empty());
    }
}
