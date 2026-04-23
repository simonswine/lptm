use std::collections::HashMap;

use crate::prometheus::context::{
    char_to_byte, count_brace_depth, extract_label_selector, find_label_value_context,
    in_bracket_context, word_boundary_byte,
};

use super::keywords;

// ── Completion context ────────────────────────────────────────────────────────

/// Completion context derived from the LogQL query text up to the cursor.
#[derive(Debug, Clone)]
pub enum CompletionCtx {
    /// Inside `{}` but not in a string -- completing a label name.
    LabelName { prefix: String, selector: String },
    /// Inside a string in a label matcher -- completing a label value.
    LabelValue {
        label: String,
        prefix: String,
        selector: String,
    },
    /// After `|` at pipeline stage level -- completing a pipeline operator.
    PipelineStage { prefix: String },
    /// At expression level -- completing functions or starting a selector.
    Expression { prefix: String },
    /// No useful context.
    None,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Detect the completion context from the part of the LogQL query before the cursor.
pub fn detect_context(query: &str, cursor_pos: usize) -> CompletionCtx {
    let cursor_byte = char_to_byte(query, cursor_pos);
    let before = &query[..cursor_byte];

    // 1. Check for unclosed string inside braces (label value context).
    if let Some((label, prefix)) = find_label_value_context(before) {
        let selector = extract_label_selector(before);
        return CompletionCtx::LabelValue {
            label,
            prefix,
            selector,
        };
    }

    // 2. Find the current word prefix.
    let word_start = word_boundary_byte(before);
    let prefix = before[word_start..].to_string();

    // 3. Brace context: inside a label selector `{...}`.
    let brace_depth = count_brace_depth(before);
    if brace_depth > 0 {
        // Suppress label-name completions when cursor is right after an operator.
        if prefix.is_empty() && word_start > 0 {
            let before_word = before[..word_start].trim_end();
            if before_word.ends_with(['=', '~', '!']) {
                return CompletionCtx::None;
            }
        }
        let selector = extract_label_selector(before);
        return CompletionCtx::LabelName { prefix, selector };
    }

    // 4. Bracket context: inside `[...]` -- durations only, no completions.
    if in_bracket_context(before) {
        return CompletionCtx::None;
    }

    // 5. Pipeline context: after `|` (but not `|=`, `!=`, `|~`, `!~`).
    if is_pipeline_context(before, word_start) {
        return CompletionCtx::PipelineStage { prefix };
    }

    // 6. Default: expression (functions, stream selector start).
    CompletionCtx::Expression { prefix }
}

/// Pure completion filter for LogQL.
pub fn compute_completions(
    ctx: &CompletionCtx,
    label_names_cache: &HashMap<String, Vec<String>>,
    label_values_cache: &HashMap<(String, String), Vec<String>>,
    max: usize,
) -> Vec<String> {
    match ctx {
        CompletionCtx::LabelName { prefix, selector } => {
            let pl = prefix.to_lowercase();
            let names = label_names_cache
                .get(selector.as_str())
                .or_else(|| label_names_cache.get(""))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut results: Vec<String> = names
                .iter()
                .filter(|n| n.to_lowercase().starts_with(&pl))
                .cloned()
                .collect();
            results.sort();
            results.truncate(max);
            results
        }

        CompletionCtx::LabelValue {
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
            let mut results: Vec<String> = values
                .iter()
                .filter(|v| v.to_lowercase().starts_with(&pl))
                .cloned()
                .collect();
            results.sort();
            results.truncate(max);
            results
        }

        CompletionCtx::PipelineStage { prefix } if !prefix.is_empty() => {
            let pl = prefix.to_lowercase();
            let mut results: Vec<String> = keywords::all_pipeline_keywords()
                .filter(|kw| kw.starts_with(pl.as_str()))
                .map(|kw| kw.to_string())
                .collect();
            results.sort();
            results.dedup();
            results.truncate(max);
            results
        }

        CompletionCtx::PipelineStage { .. } => {
            // Show all pipeline keywords when prefix is empty.
            let mut results: Vec<String> = keywords::all_pipeline_keywords()
                .map(|kw| kw.to_string())
                .collect();
            results.sort();
            results.truncate(max);
            results
        }

        CompletionCtx::Expression { prefix } if !prefix.is_empty() => {
            let pl = prefix.to_lowercase();
            let mut results: Vec<String> = keywords::all_expression_keywords()
                .filter(|kw| kw.starts_with(pl.as_str()))
                .map(|kw| kw.to_string())
                .collect();
            results.sort();
            results.dedup();
            results.truncate(max);
            results
        }

        _ => vec![],
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Check if the cursor is in a pipeline context (after `|` but not a filter op like `|=`).
fn is_pipeline_context(before: &str, word_start: usize) -> bool {
    // Look at text before the current word, trimmed.
    let before_word = before[..word_start].trim_end();
    if before_word.is_empty() {
        return false;
    }

    // Must end with `|` but not `|=`, `!=`, `|~`, `!~`.
    let last = before_word.as_bytes()[before_word.len() - 1];
    if last != b'|' {
        return false;
    }

    // Check it's not part of `|=` or `|~` (those are filter operators, not pipeline).
    // Also ensure there's a `{...}` block somewhere before (it's a log query pipeline).
    if before_word.len() >= 2 {
        let second_last = before_word.as_bytes()[before_word.len() - 2];
        // `!=` or `!~` are handled by the last char being `=` or `~`, not `|`.
        // `|=` would have last char `=`, not `|`. So if last == `|`, it's a pipeline.
        // But double-check: `||` would be weird, just treat as pipeline.
        let _ = second_last; // no special handling needed
    }

    // Verify there's at least one `{` before this point (it's after a stream selector).
    count_brace_depth(&before[..before_word.len()]) == 0
        && before[..before_word.len()].contains('{')
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn detect_context_label_name_in_braces() {
        let ctx = detect_context("{jo", 3);
        assert!(
            matches!(ctx, CompletionCtx::LabelName { ref prefix, .. } if prefix == "jo"),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_label_value() {
        let ctx = detect_context(r#"{job="no"#, 8);
        assert!(
            matches!(ctx, CompletionCtx::LabelValue { ref label, ref prefix, .. }
                if label == "job" && prefix == "no"),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_after_operator_suppressed() {
        let ctx = detect_context("{job=", 5);
        assert!(matches!(ctx, CompletionCtx::None), "got {:?}", ctx);
    }

    #[test]
    fn detect_context_pipeline_stage() {
        let ctx = detect_context(r#"{job="varlogs"} | js"#, 20);
        assert!(
            matches!(ctx, CompletionCtx::PipelineStage { ref prefix } if prefix == "js"),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_pipeline_stage_empty_prefix() {
        let ctx = detect_context(r#"{job="varlogs"} | "#, 18);
        assert!(
            matches!(ctx, CompletionCtx::PipelineStage { ref prefix } if prefix.is_empty()),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_expression() {
        let ctx = detect_context("ra", 2);
        assert!(
            matches!(ctx, CompletionCtx::Expression { ref prefix } if prefix == "ra"),
            "got {:?}",
            ctx
        );
    }

    #[test]
    fn detect_context_empty_no_completions() {
        let ctx = detect_context("", 0);
        assert!(
            matches!(ctx, CompletionCtx::Expression { ref prefix } if prefix.is_empty())
        );
        let results = compute_completions(
            &ctx,
            &HashMap::new(),
            &HashMap::new(),
            10,
        );
        assert!(results.is_empty());
    }

    #[test]
    fn detect_context_bracket_suppresses() {
        let ctx = detect_context(r#"rate({job="varlogs"}[5"#, 22);
        assert!(matches!(ctx, CompletionCtx::None), "got {:?}", ctx);
    }

    #[test]
    fn completions_pipeline_filter() {
        let ctx = CompletionCtx::PipelineStage {
            prefix: "js".into(),
        };
        let results = compute_completions(&ctx, &HashMap::new(), &HashMap::new(), 10);
        assert!(results.contains(&"json".to_string()), "{results:?}");
        assert!(!results.contains(&"logfmt".to_string()));
    }

    #[test]
    fn completions_pipeline_all_shown_empty_prefix() {
        let ctx = CompletionCtx::PipelineStage {
            prefix: String::new(),
        };
        let results = compute_completions(&ctx, &HashMap::new(), &HashMap::new(), 20);
        assert!(results.contains(&"json".to_string()));
        assert!(results.contains(&"logfmt".to_string()));
        assert!(results.contains(&"unwrap".to_string()));
    }

    #[test]
    fn completions_expression_filter() {
        let ctx = CompletionCtx::Expression {
            prefix: "ra".into(),
        };
        let results = compute_completions(&ctx, &HashMap::new(), &HashMap::new(), 10);
        assert!(results.contains(&"rate".to_string()), "{results:?}");
    }

    #[test]
    fn completions_label_names() {
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        cache.insert(
            String::new(),
            vec!["job".into(), "instance".into(), "env".into()],
        );
        let ctx = CompletionCtx::LabelName {
            prefix: "j".into(),
            selector: String::new(),
        };
        let results = compute_completions(&ctx, &cache, &HashMap::new(), 10);
        assert_eq!(results, vec!["job"]);
    }

    #[test]
    fn completions_label_values() {
        let mut cache: HashMap<(String, String), Vec<String>> = HashMap::new();
        cache.insert(
            ("job".into(), String::new()),
            vec!["node".into(), "prometheus".into(), "alertmanager".into()],
        );
        let ctx = CompletionCtx::LabelValue {
            label: "job".into(),
            prefix: "pro".into(),
            selector: String::new(),
        };
        let results = compute_completions(&ctx, &HashMap::new(), &cache, 10);
        assert_eq!(results, vec!["prometheus"]);
    }
}
