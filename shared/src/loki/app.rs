use crux_core::{render::render, Command};
use crux_http::command::Http;

use crate::app::{active_datasource, Effect, Event, Model, Screen};
use crate::prometheus::context::{char_to_byte, word_boundary_byte};
use crate::prometheus::app::percent_encode;
use crate::prometheus::types::PrometheusStringListResponse;

use super::context::{self, CompletionCtx};
use super::{LokiEntry, LokiStream};

// ── Enter / leave ────────────────────────────────────────────────────────────

pub fn handle_enter_loki(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::LokiMode;
    model.loki_query.clear();
    model.loki_cursor_pos = 0;
    model.loki_loading = false;
    model.loki_error = None;
    model.loki_results.clear();
    model.loki_selected_row = 0;
    model.loki_query_dirty = true;
    model.loki_context_open = false;
    model.loki_context_loading = false;
    model.loki_context_error = None;
    model.loki_context_entries.clear();
    model.loki_context_highlight_index = 0;
    model.loki_context_labels.clear();

    // Reset completion state
    model.loki_completion_index = None;
    model.loki_completion_dismissed = false;
    model.loki_label_names_cache.clear();
    model.loki_label_values_cache.clear();
    model.loki_label_names_loading = false;

    // Fetch initial label names from Loki
    if model.datasources.is_empty() {
        return render();
    }

    let ds = match active_datasource(model) {
        Some(ds) => ds,
        None => return render(),
    };
    let base = format!(
        "{}/api/datasources/proxy/{}/loki/api/v1",
        model.grafana_url, ds.id
    );
    let token = model.grafana_token.clone();
    model.loki_label_names_loading = true;

    Command::all([
        render(),
        Http::<Effect, Event>::get(format!("{base}/labels"))
            .header("Authorization", format!("Bearer {token}"))
            .expect_json::<PrometheusStringListResponse>()
            .build()
            .then_send(|r| Event::LokiLabelNamesLoaded(String::new(), r)),
    ])
}

pub fn handle_back_from_loki(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::DatasourceList;
    render()
}

// ── Query editing ────────────────────────────────────────────────────────────

pub fn handle_loki_query_input(model: &mut Model, c: char) -> Command<Effect, Event> {
    let pos = model.loki_cursor_pos;
    model.loki_query.insert(pos, c);
    model.loki_cursor_pos += c.len_utf8();
    model.loki_query_dirty = true;
    model.loki_completion_dismissed = false;
    model.loki_completion_index = None;
    render()
}

pub fn handle_loki_query_backspace(model: &mut Model) -> Command<Effect, Event> {
    let pos = model.loki_cursor_pos;
    if pos > 0 {
        let before = &model.loki_query[..pos];
        if let Some((idx, _)) = before.char_indices().next_back() {
            model.loki_query.remove(idx);
            model.loki_cursor_pos = idx;
            model.loki_query_dirty = true;
        }
    }
    model.loki_completion_dismissed = false;
    model.loki_completion_index = None;
    render()
}

pub fn handle_loki_cursor_left(model: &mut Model) -> Command<Effect, Event> {
    if model.loki_cursor_pos > 0 {
        let before = &model.loki_query[..model.loki_cursor_pos];
        if let Some((idx, _)) = before.char_indices().next_back() {
            model.loki_cursor_pos = idx;
        }
    }
    render()
}

pub fn handle_loki_cursor_right(model: &mut Model) -> Command<Effect, Event> {
    let pos = model.loki_cursor_pos;
    if pos < model.loki_query.len() {
        if let Some(c) = model.loki_query[pos..].chars().next() {
            model.loki_cursor_pos += c.len_utf8();
        }
    }
    render()
}

// ── Query execution ──────────────────────────────────────────────────────────

pub fn handle_loki_execute_query(model: &mut Model) -> Command<Effect, Event> {
    if model.loki_query.trim().is_empty() {
        return render();
    }
    model.loki_loading = true;
    model.loki_error = None;
    model.loki_results.clear();
    model.loki_selected_row = 0;
    model.loki_query_dirty = false;
    model.loki_completion_dismissed = true;
    model.loki_completion_index = None;
    render()
}

pub fn handle_loki_result_loaded(
    model: &mut Model,
    result: Result<Vec<LokiStream>, String>,
) -> Command<Effect, Event> {
    model.loki_loading = false;
    match result {
        Ok(streams) => {
            model.loki_results = streams;
            model.loki_error = None;
            model.loki_selected_row = 0;
        }
        Err(e) => {
            model.loki_error = Some(e);
        }
    }
    render()
}

// ── Result navigation ────────────────────────────────────────────────────────

fn loki_total_rows(model: &Model) -> usize {
    model.loki_results.iter().map(|s| s.entries.len()).sum()
}

pub fn loki_flat_entry(model: &Model, row: usize) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (si, stream) in model.loki_results.iter().enumerate() {
        if row < offset + stream.entries.len() {
            return Some((si, row - offset));
        }
        offset += stream.entries.len();
    }
    None
}

pub fn handle_loki_select_next_row(model: &mut Model) -> Command<Effect, Event> {
    let total = loki_total_rows(model);
    if total > 0 {
        model.loki_selected_row = (model.loki_selected_row + 1) % total;
    }
    render()
}

pub fn handle_loki_select_prev_row(model: &mut Model) -> Command<Effect, Event> {
    let total = loki_total_rows(model);
    if total > 0 {
        model.loki_selected_row = (model.loki_selected_row + total - 1) % total;
    }
    render()
}

// ── Context view ─────────────────────────────────────────────────────────────

pub fn handle_loki_open_context(model: &mut Model) -> Command<Effect, Event> {
    let total = loki_total_rows(model);
    if total == 0 {
        return render();
    }
    if let Some((si, ei)) = loki_flat_entry(model, model.loki_selected_row) {
        let stream = &model.loki_results[si];
        model.loki_context_labels = stream.labels.clone();
        model.loki_context_open = true;
        model.loki_context_loading = true;
        model.loki_context_error = None;
        model.loki_context_entries.clear();
        model.loki_context_highlight_index = 0;
        let _ = (si, ei);
    }
    render()
}

pub fn handle_loki_close_context(model: &mut Model) -> Command<Effect, Event> {
    model.loki_context_open = false;
    model.loki_context_loading = false;
    model.loki_context_error = None;
    model.loki_context_entries.clear();
    render()
}

pub fn handle_loki_context_loaded(
    model: &mut Model,
    result: Result<(Vec<LokiEntry>, usize), String>,
) -> Command<Effect, Event> {
    model.loki_context_loading = false;
    match result {
        Ok((entries, highlight_index)) => {
            model.loki_context_entries = entries;
            model.loki_context_highlight_index = highlight_index;
            model.loki_context_error = None;
        }
        Err(e) => {
            model.loki_context_error = Some(e);
        }
    }
    render()
}

// ── Completion ───────────────────────────────────────────────────────────────

pub fn handle_loki_trigger_completions(model: &mut Model) -> Command<Effect, Event> {
    render_with_completion_cmds(model)
}

pub fn handle_loki_completion_next(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_loki_completions(model);
    if !completions.is_empty() {
        model.loki_completion_dismissed = false;
        model.loki_completion_index = Some(match model.loki_completion_index {
            None => 0,
            Some(i) => (i + 1) % completions.len(),
        });
    }
    render()
}

pub fn handle_loki_completion_prev(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_loki_completions(model);
    if !completions.is_empty() {
        model.loki_completion_dismissed = false;
        let len = completions.len();
        model.loki_completion_index = Some(match model.loki_completion_index {
            None => len - 1,
            Some(0) => len - 1,
            Some(i) => i - 1,
        });
    }
    render()
}

pub fn handle_loki_completion_accept(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_loki_completions(model);
    let idx = model
        .loki_completion_index
        .or(if completions.is_empty() { None } else { Some(0) });

    if let Some(i) = idx {
        if let Some(completion) = completions.get(i) {
            let completion = completion.clone();
            let cursor_byte = char_to_byte(&model.loki_query, model.loki_cursor_pos);
            let before = &model.loki_query[..cursor_byte];
            let word_start = word_boundary_byte(before);

            let mut new_query = model.loki_query[..word_start].to_string();
            new_query.push_str(&completion);
            new_query.push_str(&model.loki_query[cursor_byte..]);

            model.loki_cursor_pos = word_start + completion.len();
            model.loki_query = new_query;
        }
    }

    model.loki_completion_index = None;
    model.loki_completion_dismissed = false;
    render_with_completion_cmds(model)
}

pub fn handle_loki_completion_dismiss(model: &mut Model) -> Command<Effect, Event> {
    model.loki_completion_dismissed = true;
    model.loki_completion_index = None;
    render()
}

// ── Label data loading ───────────────────────────────────────────────────────

pub fn handle_loki_label_names_loaded(
    model: &mut Model,
    selector: String,
    resp: crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            if selector.is_empty() {
                model.loki_label_names_loading = false;
            }
            let mut names = response.take_body().map(|r| r.data).unwrap_or_default();
            names.sort();
            model.loki_label_names_cache.insert(selector, names);
            render()
        }
        Err(_) => {
            if selector.is_empty() {
                model.loki_label_names_loading = false;
            }
            model.loki_label_names_cache.entry(selector).or_default();
            render()
        }
    }
}

pub fn handle_loki_label_values_loaded(
    model: &mut Model,
    label: String,
    selector: String,
    resp: crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            let values = response.take_body().map(|r| r.data).unwrap_or_default();
            model.loki_label_values_cache.insert((label, selector), values);
            render()
        }
        Err(_) => {
            model
                .loki_label_values_cache
                .entry((label, selector))
                .or_default();
            render()
        }
    }
}

// ── Completion helpers ───────────────────────────────────────────────────────

pub fn get_loki_completions(model: &Model) -> Vec<String> {
    const MAX_COMPLETIONS: usize = 10;

    if model.screen != Screen::LokiMode || model.loki_completion_dismissed {
        return vec![];
    }

    let ctx = context::detect_context(&model.loki_query, model.loki_cursor_pos);
    context::compute_completions(
        &ctx,
        &model.loki_label_names_cache,
        &model.loki_label_values_cache,
        MAX_COMPLETIONS,
    )
}

fn maybe_label_names_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds = active_datasource(model)?;
    let ds_id = ds.id;
    let ctx = context::detect_context(&model.loki_query, model.loki_cursor_pos);

    if let CompletionCtx::LabelName { ref selector, .. } = ctx {
        if !selector.is_empty() && !model.loki_label_names_cache.contains_key(selector.as_str()) {
            let url = format!(
                "{}/api/datasources/proxy/{}/loki/api/v1/labels?query={}",
                model.grafana_url,
                ds_id,
                percent_encode(selector)
            );
            let token = model.grafana_token.clone();
            let selector_clone = selector.clone();
            return Some(
                Http::<Effect, Event>::get(url)
                    .header("Authorization", format!("Bearer {token}"))
                    .expect_json::<PrometheusStringListResponse>()
                    .build()
                    .then_send(move |r| Event::LokiLabelNamesLoaded(selector_clone.clone(), r)),
            );
        }
    }

    None
}

fn maybe_label_values_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds = active_datasource(model)?;
    let ds_id = ds.id;
    let ctx = context::detect_context(&model.loki_query, model.loki_cursor_pos);

    if let CompletionCtx::LabelValue {
        ref label,
        ref selector,
        ..
    } = ctx
    {
        let cache_key = (label.clone(), selector.clone());
        if !model.loki_label_values_cache.contains_key(&cache_key) {
            let base = format!(
                "{}/api/datasources/proxy/{}/loki/api/v1/label/{}/values",
                model.grafana_url,
                ds_id,
                percent_encode(label)
            );
            let url = if selector.is_empty() {
                base
            } else {
                format!("{}?query={}", base, percent_encode(selector))
            };
            let token = model.grafana_token.clone();
            let label_clone = label.clone();
            let selector_clone = selector.clone();
            return Some(
                Http::<Effect, Event>::get(url)
                    .header("Authorization", format!("Bearer {token}"))
                    .expect_json::<PrometheusStringListResponse>()
                    .build()
                    .then_send(move |r| {
                        Event::LokiLabelValuesLoaded(label_clone.clone(), selector_clone.clone(), r)
                    }),
            );
        }
    }

    None
}

fn render_with_completion_cmds(model: &Model) -> Command<Effect, Event> {
    let extra: Vec<_> = [maybe_label_names_cmd(model), maybe_label_values_cmd(model)]
        .into_iter()
        .flatten()
        .collect();
    if extra.is_empty() {
        render()
    } else {
        Command::all(std::iter::once(render()).chain(extra))
    }
}
