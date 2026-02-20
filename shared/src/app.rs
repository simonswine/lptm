use std::collections::{BTreeSet, HashMap};

use crux_core::{
    macros::effect,
    render::{render, RenderOperation},
    App, Command,
};
use crux_http::{command::Http, protocol::HttpRequest};
use serde::{Deserialize, Serialize};

use crate::prometheus::{
    context::{char_to_byte, compute_completions, detect_context, word_boundary_byte},
    types::{
        PrometheusData, PrometheusResponse, PrometheusStringListResponse, PrometheusVectorItem,
    },
    CompletionCtx,
};

#[effect]
pub enum Effect {
    Render(RenderOperation),
    Http(HttpRequest),
}

// ── Domain types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Datasource {
    pub id: u64,
    pub uid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ds_type: String,
    pub url: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    pub access: String,
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Event {
    Configure { url: String, token: String },
    FetchDatasources,
    SelectNext,
    SelectPrevious,
    DatasourcesLoaded(crux_http::Result<crux_http::Response<Vec<Datasource>>>),
    Quit,

    // Query screen
    EnterQuery,
    QueryInput(char),
    QueryBackspace,
    QueryDelete,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    ExecuteQuery,
    QueryResultLoaded(crux_http::Result<crux_http::Response<PrometheusResponse>>),
    BackToDatasources,

    // Completion trigger (fired after the debounce timer in the TUI)
    TriggerCompletions,

    // Datasource filter
    DatasourceFilterInput(char),
    DatasourceFilterBackspace,
    DatasourceFilterClear,

    // Autocomplete
    MetricNamesLoaded(
        crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
    ),
    LabelNamesLoaded(
        String, // selector used when fetching (empty = unfiltered initial load)
        crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
    ),
    LabelValuesLoaded(
        String, // label name
        String, // selector used when fetching (empty = no filter)
        crux_http::Result<crux_http::Response<PrometheusStringListResponse>>,
    ),
    CompletionNext,
    CompletionPrev,
    CompletionAccept,
    CompletionDismiss,

    // History navigation (fired by the TUI's Up/Down keys when no completions)
    HistoryNavigate(String),
}

// ── Screen ────────────────────────────────────────────────────────────────────

#[derive(Default, PartialEq)]
pub enum Screen {
    #[default]
    DatasourceList,
    QueryMode,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Screen::DatasourceList => write!(f, "DatasourceList"),
            Screen::QueryMode => write!(f, "QueryMode"),
        }
    }
}

// ── Model ─────────────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct Model {
    pub grafana_url: String,
    pub grafana_token: String,
    pub datasources: Vec<Datasource>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_index: usize,

    pub datasource_filter: String,

    pub screen: Screen,
    pub query: String,
    pub cursor_pos: usize,
    pub query_loading: bool,
    pub query_error: Option<String>,
    pub query_results: Option<Vec<PrometheusVectorItem>>,

    // Autocomplete caches
    pub metric_names: Vec<String>,
    pub label_names_cache: HashMap<String, Vec<String>>, // key = selector ("" = unfiltered)
    pub label_values_cache: HashMap<(String, String), Vec<String>>,

    // Autocomplete UI state
    pub completion_index: Option<usize>,
    pub completion_dismissed: bool,
    pub metric_names_loading: bool,
    pub label_names_loading: bool,
}

// ── ViewModel types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatasourceView {
    pub name: String,
    pub ds_type: String,
    pub url: String,
    pub is_default: bool,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub enum ScreenView {
    #[default]
    DatasourceList,
    QueryMode,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryResultsView {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ViewModel {
    pub datasources: Vec<DatasourceView>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_index: usize,

    pub datasource_filter: String,

    pub screen: ScreenView,
    pub query: String,
    pub cursor_pos: usize,
    pub query_loading: bool,
    pub query_error: Option<String>,
    pub query_results: Option<QueryResultsView>,
    pub selected_datasource_name: Option<String>,

    pub completions: Vec<String>,
    pub completion_index: Option<usize>,
    pub completions_loading: bool,
}

// ── App ───────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct ExploreTui;

impl App for ExploreTui {
    type Event = Event;
    type Model = Model;
    type ViewModel = ViewModel;
    type Capabilities = ();
    type Effect = Effect;

    fn update(
        &self,
        event: Self::Event,
        model: &mut Self::Model,
        _caps: &(),
    ) -> Command<Self::Effect, Self::Event> {
        match event {
            Event::Configure { url, token } => {
                model.grafana_url = url;
                model.grafana_token = token;
                Command::event(Event::FetchDatasources)
            }

            Event::FetchDatasources => {
                model.loading = true;
                model.error = None;
                let url = format!("{}/api/datasources", model.grafana_url);
                let token = model.grafana_token.clone();
                Command::all([
                    render(),
                    Http::<Effect, Event>::get(url)
                        .header("Authorization", format!("Bearer {token}"))
                        .expect_json::<Vec<Datasource>>()
                        .build()
                        .then_send(Event::DatasourcesLoaded),
                ])
            }

            Event::DatasourcesLoaded(Ok(mut response)) => {
                model.loading = false;
                let all = response.take_body().unwrap_or_default();
                model.datasources =
                    all.into_iter().filter(|ds| ds.ds_type == "prometheus").collect();
                model.datasource_filter.clear();
                model.selected_index = 0;
                render()
            }

            Event::DatasourcesLoaded(Err(err)) => {
                model.loading = false;
                model.error = Some(extract_error_message(&err));
                render()
            }

            Event::SelectNext => {
                if model.screen == Screen::DatasourceList {
                    let count = filtered_datasource_indices(model).len();
                    if count > 0 {
                        model.selected_index = (model.selected_index + 1) % count;
                    }
                }
                render()
            }

            Event::SelectPrevious => {
                if model.screen == Screen::DatasourceList {
                    let count = filtered_datasource_indices(model).len();
                    if count > 0 {
                        model.selected_index =
                            (model.selected_index + count - 1) % count;
                    }
                }
                render()
            }

            Event::Quit => Command::done(),

            Event::DatasourceFilterInput(c) => {
                model.datasource_filter.push(c);
                model.selected_index = 0;
                render()
            }

            Event::DatasourceFilterBackspace => {
                model.datasource_filter.pop();
                model.selected_index = 0;
                render()
            }

            Event::DatasourceFilterClear => {
                model.datasource_filter.clear();
                model.selected_index = 0;
                render()
            }

            Event::EnterQuery => {
                // Resolve the filtered-list index to an absolute datasource index,
                // then clear the filter so QueryMode always sees the full list.
                let indices = filtered_datasource_indices(model);
                if indices.is_empty() {
                    return render();
                }
                let abs_idx = indices[model.selected_index.min(indices.len() - 1)];
                model.selected_index = abs_idx;
                model.datasource_filter.clear();

                model.screen = Screen::QueryMode;
                model.query.clear();
                model.cursor_pos = 0;
                model.query_results = None;
                model.query_error = None;
                model.completion_dismissed = false;
                model.completion_index = None;

                // Clear caches and start fresh for the selected datasource.
                model.metric_names.clear();
                model.label_names_cache.clear();
                model.label_values_cache.clear();

                if model.datasources.is_empty() {
                    return render();
                }

                let ds = &model.datasources[model.selected_index];
                let base = format!(
                    "{}/api/datasources/proxy/{}",
                    model.grafana_url, ds.id
                );
                let token = model.grafana_token.clone();

                model.metric_names_loading = true;
                model.label_names_loading = true;

                Command::all([
                    render(),
                    Http::<Effect, Event>::get(format!(
                        "{base}/api/v1/label/__name__/values"
                    ))
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

            // ── Query text editing ────────────────────────────────────────────

            Event::QueryInput(c) => {
                let byte_pos = char_to_byte(&model.query, model.cursor_pos);
                model.query.insert(byte_pos, c);
                model.cursor_pos += 1;
                model.completion_dismissed = false;
                model.completion_index = None;
                render()
            }

            Event::QueryBackspace => {
                if model.cursor_pos > 0 {
                    let byte_pos = char_to_byte(&model.query, model.cursor_pos - 1);
                    model.query.remove(byte_pos);
                    model.cursor_pos -= 1;
                }
                model.completion_dismissed = false;
                model.completion_index = None;
                render()
            }

            // Fired by the TUI's debounce timer after a pause in typing.
            Event::TriggerCompletions => render_with_completion_cmds(model),

            Event::QueryDelete => {
                let char_count = model.query.chars().count();
                if model.cursor_pos < char_count {
                    let byte_pos = char_to_byte(&model.query, model.cursor_pos);
                    model.query.remove(byte_pos);
                }
                model.completion_dismissed = false;
                model.completion_index = None;
                render()
            }

            Event::CursorLeft => {
                if model.cursor_pos > 0 {
                    model.cursor_pos -= 1;
                }
                render()
            }

            Event::CursorRight => {
                if model.cursor_pos < model.query.chars().count() {
                    model.cursor_pos += 1;
                }
                render()
            }

            Event::CursorHome => {
                model.cursor_pos = 0;
                render()
            }

            Event::CursorEnd => {
                model.cursor_pos = model.query.chars().count();
                render()
            }

            // ── Autocomplete ──────────────────────────────────────────────────

            Event::MetricNamesLoaded(Ok(mut response)) => {
                model.metric_names_loading = false;
                model.metric_names =
                    response.take_body().map(|r| r.data).unwrap_or_default();
                model.metric_names.sort();
                render()
            }

            Event::MetricNamesLoaded(Err(_)) => {
                model.metric_names_loading = false;
                render()
            }

            Event::LabelNamesLoaded(selector, Ok(mut response)) => {
                if selector.is_empty() {
                    model.label_names_loading = false;
                }
                let mut names = response.take_body().map(|r| r.data).unwrap_or_default();
                names.sort();
                model.label_names_cache.insert(selector, names);
                render()
            }

            Event::LabelNamesLoaded(selector, Err(_)) => {
                if selector.is_empty() {
                    model.label_names_loading = false;
                }
                // Insert empty entry so we don't retry on every keystroke.
                model.label_names_cache.entry(selector).or_default();
                render()
            }

            Event::LabelValuesLoaded(label, selector, Ok(mut response)) => {
                let values = response.take_body().map(|r| r.data).unwrap_or_default();
                model.label_values_cache.insert((label, selector), values);
                render()
            }

            Event::LabelValuesLoaded(label, selector, Err(_)) => {
                // Insert empty entry so we don't retry on every keystroke.
                model.label_values_cache.entry((label, selector)).or_default();
                render()
            }

            Event::CompletionNext => {
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

            Event::CompletionPrev => {
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

            Event::CompletionAccept => {
                let completions = get_completions(model);
                // If nothing is selected, pick the first entry.
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
                // Recompute to show next-level completions (e.g. after accepting a
                // function name the user may want label completions inside {}).
                render_with_completion_cmds(model)
            }

            Event::CompletionDismiss => {
                model.completion_dismissed = true;
                model.completion_index = None;
                render()
            }

            // ── Query execution ───────────────────────────────────────────────

            Event::ExecuteQuery => {
                if model.datasources.is_empty() {
                    return render();
                }
                model.query_loading = true;
                model.query_error = None;
                model.query_results = None;
                model.completion_dismissed = true;
                model.completion_index = None;

                let ds = &model.datasources[model.selected_index];
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

            Event::QueryResultLoaded(Ok(mut response)) => {
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

            Event::QueryResultLoaded(Err(err)) => {
                model.query_loading = false;
                model.query_error = Some(extract_error_message(&err));
                render()
            }

            Event::BackToDatasources => {
                model.screen = Screen::DatasourceList;
                render()
            }

            Event::HistoryNavigate(query) => {
                model.cursor_pos = query.chars().count();
                model.query = query;
                model.completion_dismissed = true;
                model.completion_index = None;
                render()
            }
        }
    }

    fn view(&self, model: &Self::Model) -> Self::ViewModel {
        let screen = match model.screen {
            Screen::DatasourceList => ScreenView::DatasourceList,
            Screen::QueryMode => ScreenView::QueryMode,
        };

        // In DatasourceList mode the indices list reflects the active filter;
        // in QueryMode the filter has already been cleared so this is 0..len.
        let indices = filtered_datasource_indices(model);
        let selected_index = if indices.is_empty() {
            0
        } else {
            model.selected_index.min(indices.len() - 1)
        };

        let datasources: Vec<DatasourceView> = indices
            .iter()
            .map(|&i| {
                let ds = &model.datasources[i];
                DatasourceView {
                    name: ds.name.clone(),
                    ds_type: ds.ds_type.clone(),
                    url: ds.url.clone(),
                    is_default: ds.is_default,
                }
            })
            .collect();

        let selected_datasource_name = indices
            .get(selected_index)
            .and_then(|&i| model.datasources.get(i))
            .map(|ds| ds.name.clone());

        let query_results = model
            .query_results
            .as_ref()
            .map(|results| build_query_results_view(results));

        let completions = get_completions(model);
        let completions_loading = model.metric_names_loading || model.label_names_loading;

        ViewModel {
            datasource_filter: model.datasource_filter.clone(),
            datasources,
            loading: model.loading,
            error: model.error.clone(),
            selected_index,
            screen,
            query: model.query.clone(),
            cursor_pos: model.cursor_pos,
            query_loading: model.query_loading,
            query_error: model.query_error.clone(),
            query_results,
            selected_datasource_name,
            completions,
            completion_index: model.completion_index,
            completions_loading,
        }
    }
}

// ── Datasource filter helpers ─────────────────────────────────────────────────

/// All characters of `needle` must appear in `haystack` in order (case-insensitive).
fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut hi = haystack.chars().map(|c| c.to_ascii_lowercase());
    for nc in needle.chars().map(|c| c.to_ascii_lowercase()) {
        loop {
            match hi.next() {
                None => return false,
                Some(hc) if hc == nc => break,
                Some(_) => {}
            }
        }
    }
    true
}

/// Indices into `model.datasources` that survive the current filter.
fn filtered_datasource_indices(model: &Model) -> Vec<usize> {
    if model.datasource_filter.is_empty() {
        return (0..model.datasources.len()).collect();
    }
    model
        .datasources
        .iter()
        .enumerate()
        .filter(|(_, ds)| fuzzy_match(&ds.name, &model.datasource_filter))
        .map(|(i, _)| i)
        .collect()
}

// ── Completion helpers ────────────────────────────────────────────────────────

/// Compute filtered completions for the current query / cursor state.
/// Returns at most `MAX_COMPLETIONS` entries.
fn get_completions(model: &Model) -> Vec<String> {
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

/// If the cursor is inside `{…}` with a non-empty selector and label names for
/// that selector aren't cached yet, return an HTTP command to fetch them.
fn maybe_label_names_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds_id = model.datasources.get(model.selected_index)?.id;
    let ctx = detect_context(&model.query, model.cursor_pos);

    if let CompletionCtx::LabelName { ref selector, .. } = ctx {
        if selector.is_empty() {
            return None; // initial unfiltered fetch is handled by EnterQuery
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

/// Render the current frame and fire any label-name / label-value HTTP fetches
/// required by the current cursor context.
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

/// If the current context is a label-value context and the label's values are
/// not yet cached, return an HTTP command to fetch them.
fn maybe_label_values_cmd(model: &Model) -> Option<Command<Effect, Event>> {
    let ds_id = model.datasources.get(model.selected_index)?.id;
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
            // Pass existing matchers as a series selector so Prometheus scopes
            // the returned values to the already-constrained series.
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

// ── Utility helpers ───────────────────────────────────────────────────────────

/// Extract a human-readable error message from an `HttpError`.
///
/// For `HttpError::Http` errors the raw response body is available.  Prometheus
/// and Grafana both return JSON bodies with an `"error"` or `"message"` key on
/// 4xx/5xx responses, so we try to parse that out.  If parsing fails we fall
/// back to the HTTP status text.
fn extract_error_message(err: &crux_http::HttpError) -> String {
    if let crux_http::HttpError::Http { body: Some(body), .. } = err {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
            // Prometheus: {"status":"error","errorType":"...","error":"..."}
            if let Some(msg) = json.get("error").and_then(|v| v.as_str()) {
                return msg.to_string();
            }
            // Grafana admin errors: {"message":"..."}
            if let Some(msg) = json.get("message").and_then(|v| v.as_str()) {
                return msg.to_string();
            }
        }
        // Plain-text body fallback
        if let Ok(text) = std::str::from_utf8(body) {
            let text = text.trim();
            if !text.is_empty() {
                return format!("{err}: {text}");
            }
        }
    }
    err.to_string()
}

fn percent_encode(s: &str) -> String {
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

fn build_query_results_view(results: &[PrometheusVectorItem]) -> QueryResultsView {
    let mut key_set = BTreeSet::new();
    for item in results {
        for k in item.metric.keys() {
            key_set.insert(k.clone());
        }
    }
    let mut columns: Vec<String> = key_set.into_iter().collect();
    columns.push("value".to_string());

    let rows = results
        .iter()
        .map(|item| {
            let mut row: Vec<String> = columns[..columns.len() - 1]
                .iter()
                .map(|col| item.metric.get(col).cloned().unwrap_or_default())
                .collect();
            row.push(item.value.1.clone());
            row
        })
        .collect();

    QueryResultsView { columns, rows }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crux_core::Core;
    use crux_http::testing::ResponseBuilder;

    use crate::prometheus::types::{PrometheusStringListResponse, PrometheusVectorItem};

    fn make_core() -> Core<ExploreTui> {
        Core::new()
    }

    fn make_datasources() -> Vec<Datasource> {
        vec![
            Datasource {
                id: 1,
                uid: "uid1".into(),
                name: "Prometheus".into(),
                ds_type: "prometheus".into(),
                url: "http://prom:9090".into(),
                is_default: true,
                access: "proxy".into(),
            },
            Datasource {
                id: 2,
                uid: "uid2".into(),
                name: "Loki".into(),
                ds_type: "loki".into(),
                url: "http://loki:3100".into(),
                is_default: false,
                access: "proxy".into(),
            },
            Datasource {
                id: 3,
                uid: "uid3".into(),
                name: "Prometheus2".into(),
                ds_type: "prometheus".into(),
                url: "http://prom2:9090".into(),
                is_default: false,
                access: "proxy".into(),
            },
        ]
    }

    #[test]
    fn configure_triggers_fetch() {
        let core = make_core();
        let effects = core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        assert!(effects.iter().any(|e| matches!(e, Effect::Http(_))));
    }

    #[test]
    fn select_next_wraps_around() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        assert_eq!(core.view().selected_index, 0);
        core.process_event(Event::SelectNext);
        assert_eq!(core.view().selected_index, 1);
        core.process_event(Event::SelectNext);
        assert_eq!(core.view().selected_index, 0);
    }

    #[test]
    fn select_previous_wraps_around() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::SelectPrevious);
        assert_eq!(core.view().selected_index, 1);
    }

    #[test]
    fn error_state_on_failed_load() {
        let core = make_core();
        core.process_event(Event::DatasourcesLoaded(Err(crux_http::HttpError::Url(
            "connection refused".into(),
        ))));
        let vm = core.view();
        assert!(vm.error.is_some());
        assert!(!vm.loading);
    }

    #[test]
    fn view_reflects_loaded_datasources() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert_eq!(vm.datasources.len(), 2);
        assert_eq!(vm.datasources[0].name, "Prometheus");
        assert!(vm.datasources[0].is_default);
    }

    #[test]
    fn filters_non_prometheus_datasources() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert!(vm.datasources.iter().all(|ds| ds.ds_type == "prometheus"));
        assert!(vm.datasources.iter().all(|ds| ds.name != "Loki"));
    }

    #[test]
    fn percent_encode_basic() {
        assert_eq!(percent_encode("up"), "up");
        assert_eq!(
            percent_encode("rate(http_requests_total[5m])"),
            "rate%28http_requests_total%5B5m%5D%29"
        );
        assert_eq!(percent_encode("foo bar"), "foo%20bar");
    }

    #[test]
    fn build_query_results_view_basic() {
        let results = vec![PrometheusVectorItem {
            metric: {
                let mut m = HashMap::new();
                m.insert("job".into(), "node".into());
                m.insert("instance".into(), "localhost:9100".into());
                m
            },
            value: (1234567890.0, "1".into()),
        }];
        let view = build_query_results_view(&results);
        assert_eq!(view.columns, vec!["instance", "job", "value"]);
        assert_eq!(view.rows[0], vec!["localhost:9100", "node", "1"]);
    }

    #[test]
    fn enter_query_switches_screen() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        assert_eq!(core.view().screen, ScreenView::DatasourceList);
        core.process_event(Event::EnterQuery);
        assert_eq!(core.view().screen, ScreenView::QueryMode);
        core.process_event(Event::BackToDatasources);
        assert_eq!(core.view().screen, ScreenView::DatasourceList);
    }

    #[test]
    fn query_input_and_backspace() {
        let core = make_core();
        core.process_event(Event::EnterQuery);
        core.process_event(Event::QueryInput('u'));
        core.process_event(Event::QueryInput('p'));
        assert_eq!(core.view().query, "up");
        assert_eq!(core.view().cursor_pos, 2);
        core.process_event(Event::QueryBackspace);
        assert_eq!(core.view().query, "u");
        assert_eq!(core.view().cursor_pos, 1);
    }

    #[test]
    fn cursor_movement_and_insert() {
        let core = make_core();
        core.process_event(Event::EnterQuery);
        for c in ['a', 'b', 'c'] {
            core.process_event(Event::QueryInput(c));
        }
        assert_eq!(core.view().query, "abc");
        assert_eq!(core.view().cursor_pos, 3);
        core.process_event(Event::CursorLeft);
        core.process_event(Event::CursorLeft);
        assert_eq!(core.view().cursor_pos, 1);
        core.process_event(Event::QueryInput('X'));
        assert_eq!(core.view().query, "aXbc");
        assert_eq!(core.view().cursor_pos, 2);
        core.process_event(Event::CursorHome);
        assert_eq!(core.view().cursor_pos, 0);
        core.process_event(Event::CursorEnd);
        assert_eq!(core.view().cursor_pos, 4);
        core.process_event(Event::CursorRight);
        assert_eq!(core.view().cursor_pos, 4); // clamped
        core.process_event(Event::CursorHome);
        core.process_event(Event::CursorLeft);
        assert_eq!(core.view().cursor_pos, 0); // clamped
    }

    #[test]
    fn query_delete_forward() {
        let core = make_core();
        core.process_event(Event::EnterQuery);
        for c in ['a', 'b', 'c'] {
            core.process_event(Event::QueryInput(c));
        }
        core.process_event(Event::QueryDelete); // at end, no-op
        assert_eq!(core.view().query, "abc");
        core.process_event(Event::CursorHome);
        core.process_event(Event::CursorRight);
        core.process_event(Event::QueryDelete); // delete 'b'
        assert_eq!(core.view().query, "ac");
        assert_eq!(core.view().cursor_pos, 1);
    }

    #[test]
    fn completion_accept_replaces_word() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        // Inject datasources so EnterQuery can fire fetches
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::EnterQuery);

        // Simulate metric names loaded so completions are available
        let names_resp = ResponseBuilder::ok()
            .body(PrometheusStringListResponse {
                status: "success".into(),
                data: vec!["rate_total".into()],
            })
            .build();
        core.process_event(Event::MetricNamesLoaded(Ok(names_resp)));
        let labels_resp = ResponseBuilder::ok()
            .body(PrometheusStringListResponse {
                status: "success".into(),
                data: vec![],
            })
            .build();
        core.process_event(Event::LabelNamesLoaded(String::new(), Ok(labels_resp)));

        // Type "ra"
        core.process_event(Event::QueryInput('r'));
        core.process_event(Event::QueryInput('a'));
        assert_eq!(core.view().query, "ra");
        assert!(!core.view().completions.is_empty(), "expected completions");

        // Select second item with CompletionNext (first is a keyword like "rad")
        // Just accept whatever is first
        core.process_event(Event::CompletionNext);
        core.process_event(Event::CompletionAccept);

        // The word "ra" should have been replaced with the selected completion
        let vm = core.view();
        assert_ne!(vm.query, "ra", "query should be updated after accept");
        assert!(!vm.query.starts_with("ra") || vm.query.len() > 2);
    }
}
