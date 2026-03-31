use crux_core::{render::render, Command};
use crux_http::command::Http;

use crate::app::{active_datasource, extract_error_message, sorted_datasource_indices, Effect, Event, Model, Screen};
use crate::prometheus::{
    context::{char_to_byte, compute_completions, detect_context, word_boundary_byte},
    types::{PrometheusData, PrometheusResponse, PrometheusStringListResponse},
    CompletionCtx,
};

pub fn handle_enter_query(model: &mut Model) -> Command<Effect, Event> {
    // Use the favorites-sorted indices (same order as view()) so that
    // model.selected_index — which is always a *sorted* position — translates
    // correctly to the datasource the user actually has highlighted.
    let indices = sorted_datasource_indices(model);
    if indices.is_empty() {
        return render();
    }
    // Clamp to valid range; do NOT overwrite selected_index — it remains the
    // sorted position and is used as such by view() and the TUI.
    model.selected_index = model.selected_index.min(indices.len() - 1);
    model.datasource_filter.clear();

    model.screen = Screen::QueryMode;
    model.query.clear();
    model.cursor_pos = 0;
    model.query_results = None;
    model.query_error = None;
    model.completion_dismissed = false;
    model.completion_index = None;

    model.metric_names.clear();
    model.label_names_cache.clear();
    model.label_values_cache.clear();

    if model.datasources.is_empty() {
        return render();
    }

    let ds = active_datasource(model).expect("selected datasource must exist after entering query mode");
    let base = format!("{}/api/datasources/proxy/{}", model.grafana_url, ds.id);
    let token = model.grafana_token.clone();

    model.metric_names_loading = true;
    model.label_names_loading = true;

    Command::all([
        render(),
        Http::<Effect, Event>::get(format!("{base}/api/v1/label/__name__/values"))
            .header("Authorization", format!("Bearer {token}"))
            .expect_json::<PrometheusStringListResponse>()
            .build()
            .then_send(Event::MetricNamesLoaded),
        Http::<Effect, Event>::get(format!("{base}/api/v1/labels"))
            .header("Authorization", format!("Bearer {token}"))
            .expect_json::<PrometheusStringListResponse>()
            .build()
            .then_send(|r| Event::LabelNamesLoaded(String::new(), r)),
    ])
}

pub fn handle_query_input(model: &mut Model, c: char) -> Command<Effect, Event> {
    let byte_pos = char_to_byte(&model.query, model.cursor_pos);
    model.query.insert(byte_pos, c);
    model.cursor_pos += 1;
    model.completion_dismissed = false;
    model.completion_index = None;
    render()
}

pub fn handle_query_backspace(model: &mut Model) -> Command<Effect, Event> {
    if model.cursor_pos > 0 {
        let byte_pos = char_to_byte(&model.query, model.cursor_pos - 1);
        model.query.remove(byte_pos);
        model.cursor_pos -= 1;
    }
    model.completion_dismissed = false;
    model.completion_index = None;
    render()
}

pub fn handle_query_delete(model: &mut Model) -> Command<Effect, Event> {
    let char_count = model.query.chars().count();
    if model.cursor_pos < char_count {
        let byte_pos = char_to_byte(&model.query, model.cursor_pos);
        model.query.remove(byte_pos);
    }
    model.completion_dismissed = false;
    model.completion_index = None;
    render()
}

pub fn handle_trigger_completions(model: &mut Model) -> Command<Effect, Event> {
    render_with_completion_cmds(model)
}

pub fn handle_cursor_left(model: &mut Model) -> Command<Effect, Event> {
    if model.cursor_pos > 0 {
        model.cursor_pos -= 1;
    }
    render()
}

pub fn handle_cursor_right(model: &mut Model) -> Command<Effect, Event> {
    if model.cursor_pos < model.query.chars().count() {
        model.cursor_pos += 1;
    }
    render()
}

pub fn handle_cursor_home(model: &mut Model) -> Command<Effect, Event> {
    model.cursor_pos = 0;
    render()
}

pub fn handle_cursor_end(model: &mut Model) -> Command<Effect, Event> {
    model.cursor_pos = model.query.chars().count();
    render()
}

pub fn handle_metric_names_loaded(
    model: &mut Model,
    resp: crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            model.metric_names_loading = false;
            model.metric_names = response.take_body().map(|r| r.data).unwrap_or_default();
            model.metric_names.sort();
            render()
        }
        Err(_) => {
            model.metric_names_loading = false;
            render()
        }
    }
}

pub fn handle_label_names_loaded(
    model: &mut Model,
    selector: String,
    resp: crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            if selector.is_empty() {
                model.label_names_loading = false;
            }
            let mut names = response.take_body().map(|r| r.data).unwrap_or_default();
            names.sort();
            model.label_names_cache.insert(selector, names);
            render()
        }
        Err(_) => {
            if selector.is_empty() {
                model.label_names_loading = false;
            }
            model.label_names_cache.entry(selector).or_default();
            render()
        }
    }
}

pub fn handle_label_values_loaded(
    model: &mut Model,
    label: String,
    selector: String,
    resp: crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            let values = response.take_body().map(|r| r.data).unwrap_or_default();
            model.label_values_cache.insert((label, selector), values);
            render()
        }
        Err(_) => {
            model.label_values_cache.entry((label, selector)).or_default();
            render()
        }
    }
}

pub fn handle_completion_next(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_completions(model);
    if !completions.is_empty() {
        model.completion_dismissed = false;
        model.completion_index = Some(match model.completion_index {
            None => 0,
            Some(i) => (i + 1) % completions.len(),
        });
    }
    render()
}

pub fn handle_completion_prev(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_completions(model);
    if !completions.is_empty() {
        model.completion_dismissed = false;
        let len = completions.len();
        model.completion_index = Some(match model.completion_index {
            None => len - 1,
            Some(0) => len - 1,
            Some(i) => i - 1,
        });
    }
    render()
}

pub fn handle_completion_accept(model: &mut Model) -> Command<Effect, Event> {
    let completions = get_completions(model);
    let idx = model
        .completion_index
        .or(if completions.is_empty() { None } else { Some(0) });

    if let Some(i) = idx {
        if let Some(completion) = completions.get(i) {
            let completion = completion.clone();
            let cursor_byte = char_to_byte(&model.query, model.cursor_pos);
            let before = &model.query[..cursor_byte];
            let word_start = word_boundary_byte(before);
            let word_start_char = model.query[..word_start].chars().count();

            let mut new_query = model.query[..word_start].to_string();
            new_query.push_str(&completion);
            new_query.push_str(&model.query[cursor_byte..]);

            model.cursor_pos = word_start_char + completion.chars().count();
            model.query = new_query;
        }
    }

    model.completion_index = None;
    model.completion_dismissed = false;
    render_with_completion_cmds(model)
}

pub fn handle_completion_dismiss(model: &mut Model) -> Command<Effect, Event> {
    model.completion_dismissed = true;
    model.completion_index = None;
    render()
}

pub fn handle_execute_query(model: &mut Model) -> Command<Effect, Event> {
    if model.datasources.is_empty() {
        return render();
    }
    model.query_loading = true;
    model.query_error = None;
    model.query_results = None;
    model.completion_dismissed = true;
    model.completion_index = None;

    let ds = active_datasource(model).expect("selected datasource must exist in query mode");
    let url = format!(
        "{}/api/datasources/proxy/{}/api/v1/query?query={}",
        model.grafana_url,
        ds.id,
        percent_encode(&model.query)
    );
    let token = model.grafana_token.clone();
    Command::all([
        render(),
        Http::<Effect, Event>::get(url)
            .header("Authorization", format!("Bearer {token}"))
            .expect_json::<PrometheusResponse>()
            .build()
            .then_send(Event::QueryResultLoaded),
    ])
}

pub fn handle_query_result_loaded(
    model: &mut Model,
    resp: crux_http::Result<crux_http::Response<PrometheusResponse>>,
) -> Command<Effect, Event> {
    match resp {
        Ok(mut response) => {
            model.query_loading = false;
            let prom = response.take_body().unwrap_or(PrometheusResponse {
                status: "error".into(),
                data: PrometheusData {
                    result_type: "vector".into(),
                    result: vec![],
                },
            });
            model.query_results = Some(prom.data.result);
            render()
        }
        Err(err) => {
            model.query_loading = false;
            model.query_error = Some(extract_error_message(&err));
            render()
        }
    }
}

pub fn handle_back_to_datasources(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::DatasourceList;
    render()
}

pub fn handle_history_navigate(model: &mut Model, query: String) -> Command<Effect, Event> {
    model.cursor_pos = query.chars().count();
    model.query = query;
    model.completion_dismissed = true;
    model.completion_index = None;
    render()
}

// ── Completion helpers ────────────────────────────────────────────────────────

pub fn get_completions(model: &Model) -> Vec<String> {
    const MAX_COMPLETIONS: usize = 10;

    if model.screen != Screen::QueryMode || model.completion_dismissed {
        return vec![];
    }

    let ctx = detect_context(&model.query, model.cursor_pos);
    compute_completions(
        &ctx,
        &model.metric_names,
        &model.label_names_cache,
        &model.label_values_cache,
        MAX_COMPLETIONS,
    )
}

fn maybe_label_names_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds_id = active_datasource(model)?.id;
    let ctx = detect_context(&model.query, model.cursor_pos);

    if let CompletionCtx::LabelName { ref selector, .. } = ctx {
        if selector.is_empty() {
            return None;
        }
        if !model.label_names_cache.contains_key(selector.as_str()) {
            let url = format!(
                "{}/api/datasources/proxy/{}/api/v1/labels?match[]={}",
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
                    .then_send(move |r| Event::LabelNamesLoaded(selector_clone.clone(), r)),
            );
        }
    }

    None
}

fn maybe_label_values_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds_id = active_datasource(model)?.id;
    let ctx = detect_context(&model.query, model.cursor_pos);

    if let CompletionCtx::LabelValue { ref label, ref selector, .. } = ctx {
        let cache_key = (label.clone(), selector.clone());
        if !model.label_values_cache.contains_key(&cache_key) {
            let base = format!(
                "{}/api/datasources/proxy/{}/api/v1/label/{}/values",
                model.grafana_url,
                ds_id,
                percent_encode(label)
            );
            let url = if selector.is_empty() {
                base
            } else {
                format!("{}?match[]={}", base, percent_encode(selector))
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
                        Event::LabelValuesLoaded(label_clone.clone(), selector_clone.clone(), r)
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

// ── Utility ───────────────────────────────────────────────────────────────────

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}
