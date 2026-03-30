use std::collections::{BTreeSet, HashMap};

use crux_core::{
    macros::effect,
    render::{render, RenderOperation},
    App, Command,
};
use crux_http::{command::Http, protocol::HttpRequest};
use serde::{Deserialize, Serialize};

use crate::prometheus::types::{
    PrometheusResponse, PrometheusStringListResponse, PrometheusVectorItem,
};
use crate::pyroscope::{build_flamegraph_view, FlameGraph, FlamegraphNav, FlamegraphView};

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

#[derive(Debug, Clone)]
pub struct PyroscopeSeriesItem {
    pub service_name: String,
    pub profile_type_id: String,
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

    // Pyroscope mode
    EnterPyroscope { now_unix_ms: i64 },

    PyroscopeSeriesNext,
    PyroscopeSeriesPrev,
    PyroscopeSeriesLoaded(Result<Vec<(String, String)>, String>),

    PyroscopeTimeRangeEdit,
    PyroscopeTimeRangeInput(char),
    PyroscopeTimeRangeBackspace,
    PyroscopeTimeRangeCommit { now_unix_ms: i64 },
    PyroscopeTimeRangeAbort,

    PyroscopeSelectSeries { now_unix_ms: i64 },
    PyroscopeFlamegraphLoaded(Result<Option<FlameGraph>, String>),

    BackToServiceList,
    BackFromPyroscope,

    FlameMoveLeft,
    FlameMoveRight,
    FlameMoveUp,
    FlameMoveDown,
    FlameZoomIn,
    FlameZoomOut,
}

// ── Screen ────────────────────────────────────────────────────────────────────

#[derive(Default, PartialEq)]
pub enum Screen {
    #[default]
    DatasourceList,
    QueryMode,
    PyroscopeMode,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Screen::DatasourceList => write!(f, "DatasourceList"),
            Screen::QueryMode => write!(f, "QueryMode"),
            Screen::PyroscopeMode => write!(f, "PyroscopeMode"),
        }
    }
}

#[derive(Default, Debug, PartialEq)]
pub enum PyroscopeSubScreen {
    #[default]
    ServiceList,
    Flamegraph,
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
    pub label_names_cache: HashMap<String, Vec<String>>,
    pub label_values_cache: HashMap<(String, String), Vec<String>>,

    // Autocomplete UI state
    pub completion_index: Option<usize>,
    pub completion_dismissed: bool,
    pub metric_names_loading: bool,
    pub label_names_loading: bool,

    // Pyroscope state
    pub pyroscope_sub_screen: PyroscopeSubScreen,
    pub pyroscope_time_range: String,
    pub pyroscope_time_range_editing: bool,
    pub pyroscope_series_loading: bool,
    pub pyroscope_series_error: Option<String>,
    pub pyroscope_series: Vec<PyroscopeSeriesItem>,
    pub pyroscope_series_index: usize,
    pub pyroscope_selected_service: String,
    pub pyroscope_selected_profile_type: String,
    pub pyroscope_flamegraph_loading: bool,
    pub pyroscope_flamegraph_error: Option<String>,
    pub pyroscope_flamegraph: Option<FlameGraph>,
    pub flamegraph_nav: FlamegraphNav,
}

// ── ViewModel types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatasourceView {
    pub id: u64,
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
    PyroscopeMode,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub enum PyroscopeSubScreenView {
    #[default]
    ServiceList,
    Flamegraph,
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

    // Pyroscope
    pub pyroscope_sub_screen: PyroscopeSubScreenView,
    pub pyroscope_time_range: String,
    pub pyroscope_time_range_editing: bool,
    pub pyroscope_series_loading: bool,
    pub pyroscope_series_error: Option<String>,
    pub pyroscope_series: Vec<(String, String)>, // (service_name, profile_type_id)
    pub pyroscope_series_index: usize,
    pub pyroscope_selected_service: String,
    pub pyroscope_selected_profile_type: String,
    pub pyroscope_flamegraph_loading: bool,
    pub pyroscope_flamegraph_error: Option<String>,
    pub flamegraph: Option<FlamegraphView>,
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
                model.datasources = all
                    .into_iter()
                    .filter(|ds| {
                        ds.ds_type == "prometheus"
                            || ds.ds_type == "grafana-pyroscope-datasource"
                            || ds.ds_type == "phlare"
                    })
                    .collect();
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

            // ── Prometheus ────────────────────────────────────────────────────

            Event::EnterQuery => crate::prometheus::app::handle_enter_query(model),
            Event::QueryInput(c) => crate::prometheus::app::handle_query_input(model, c),
            Event::QueryBackspace => crate::prometheus::app::handle_query_backspace(model),
            Event::QueryDelete => crate::prometheus::app::handle_query_delete(model),
            Event::TriggerCompletions => crate::prometheus::app::handle_trigger_completions(model),
            Event::CursorLeft => crate::prometheus::app::handle_cursor_left(model),
            Event::CursorRight => crate::prometheus::app::handle_cursor_right(model),
            Event::CursorHome => crate::prometheus::app::handle_cursor_home(model),
            Event::CursorEnd => crate::prometheus::app::handle_cursor_end(model),
            Event::MetricNamesLoaded(resp) => {
                crate::prometheus::app::handle_metric_names_loaded(model, resp)
            }
            Event::LabelNamesLoaded(sel, resp) => {
                crate::prometheus::app::handle_label_names_loaded(model, sel, resp)
            }
            Event::LabelValuesLoaded(lbl, sel, resp) => {
                crate::prometheus::app::handle_label_values_loaded(model, lbl, sel, resp)
            }
            Event::CompletionNext => crate::prometheus::app::handle_completion_next(model),
            Event::CompletionPrev => crate::prometheus::app::handle_completion_prev(model),
            Event::CompletionAccept => crate::prometheus::app::handle_completion_accept(model),
            Event::CompletionDismiss => crate::prometheus::app::handle_completion_dismiss(model),
            Event::ExecuteQuery => crate::prometheus::app::handle_execute_query(model),
            Event::QueryResultLoaded(resp) => {
                crate::prometheus::app::handle_query_result_loaded(model, resp)
            }
            Event::BackToDatasources => crate::prometheus::app::handle_back_to_datasources(model),
            Event::HistoryNavigate(query) => {
                crate::prometheus::app::handle_history_navigate(model, query)
            }

            // ── Pyroscope ─────────────────────────────────────────────────────

            Event::EnterPyroscope { .. } => crate::pyroscope::app::handle_enter_pyroscope(model),
            Event::PyroscopeSeriesLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_series_loaded(model, result)
            }
            Event::PyroscopeSeriesNext => {
                crate::pyroscope::app::handle_pyroscope_series_next(model)
            }
            Event::PyroscopeSeriesPrev => {
                crate::pyroscope::app::handle_pyroscope_series_prev(model)
            }
            Event::PyroscopeTimeRangeEdit => {
                crate::pyroscope::app::handle_pyroscope_time_range_edit(model)
            }
            Event::PyroscopeTimeRangeInput(c) => {
                crate::pyroscope::app::handle_pyroscope_time_range_input(model, c)
            }
            Event::PyroscopeTimeRangeBackspace => {
                crate::pyroscope::app::handle_pyroscope_time_range_backspace(model)
            }
            Event::PyroscopeTimeRangeCommit { .. } => {
                crate::pyroscope::app::handle_pyroscope_time_range_commit(model)
            }
            Event::PyroscopeTimeRangeAbort => {
                crate::pyroscope::app::handle_pyroscope_time_range_abort(model)
            }
            Event::PyroscopeSelectSeries { .. } => {
                crate::pyroscope::app::handle_pyroscope_select_series(model)
            }
            Event::PyroscopeFlamegraphLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_flamegraph_loaded(model, result)
            }
            Event::BackToServiceList => crate::pyroscope::app::handle_back_to_service_list(model),
            Event::BackFromPyroscope => crate::pyroscope::app::handle_back_from_pyroscope(model),
            Event::FlameMoveLeft => crate::pyroscope::app::handle_flame_move_left(model),
            Event::FlameMoveRight => crate::pyroscope::app::handle_flame_move_right(model),
            Event::FlameMoveUp => crate::pyroscope::app::handle_flame_move_up(model),
            Event::FlameMoveDown => crate::pyroscope::app::handle_flame_move_down(model),
            Event::FlameZoomIn => crate::pyroscope::app::handle_flame_zoom_in(model),
            Event::FlameZoomOut => crate::pyroscope::app::handle_flame_zoom_out(model),
        }
    }

    fn view(&self, model: &Self::Model) -> Self::ViewModel {
        let screen = match model.screen {
            Screen::DatasourceList => ScreenView::DatasourceList,
            Screen::QueryMode => ScreenView::QueryMode,
            Screen::PyroscopeMode => ScreenView::PyroscopeMode,
        };

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
                    id: ds.id,
                    name: ds.name.clone(),
                    ds_type: normalize_ds_type(&ds.ds_type).to_string(),
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

        let completions = crate::prometheus::app::get_completions(model);
        let completions_loading = model.metric_names_loading || model.label_names_loading;

        let flamegraph = model
            .pyroscope_flamegraph
            .as_ref()
            .map(|fg| build_flamegraph_view(fg, &model.flamegraph_nav));

        let pyroscope_sub_screen = match model.pyroscope_sub_screen {
            PyroscopeSubScreen::ServiceList => PyroscopeSubScreenView::ServiceList,
            PyroscopeSubScreen::Flamegraph => PyroscopeSubScreenView::Flamegraph,
        };

        let pyroscope_series: Vec<(String, String)> = model
            .pyroscope_series
            .iter()
            .map(|item| (item.service_name.clone(), item.profile_type_id.clone()))
            .collect();

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
            pyroscope_sub_screen,
            pyroscope_time_range: model.pyroscope_time_range.clone(),
            pyroscope_time_range_editing: model.pyroscope_time_range_editing,
            pyroscope_series_loading: model.pyroscope_series_loading,
            pyroscope_series_error: model.pyroscope_series_error.clone(),
            pyroscope_series,
            pyroscope_series_index: model.pyroscope_series_index,
            pyroscope_selected_service: model.pyroscope_selected_service.clone(),
            pyroscope_selected_profile_type: model.pyroscope_selected_profile_type.clone(),
            pyroscope_flamegraph_loading: model.pyroscope_flamegraph_loading,
            pyroscope_flamegraph_error: model.pyroscope_flamegraph_error.clone(),
            flamegraph,
        }
    }
}

// ── Datasource type helpers ───────────────────────────────────────────────────

fn normalize_ds_type(ds_type: &str) -> &str {
    match ds_type {
        "grafana-pyroscope-datasource" | "phlare" => "pyroscope",
        other => other,
    }
}

// ── Datasource filter helpers ─────────────────────────────────────────────────

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

pub(crate) fn filtered_datasource_indices(model: &Model) -> Vec<usize> {
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

// ── Utility helpers ───────────────────────────────────────────────────────────

pub(crate) fn extract_error_message(err: &crux_http::HttpError) -> String {
    if let crux_http::HttpError::Http { body: Some(body), .. } = err {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(msg) = json.get("error").and_then(|v| v.as_str()) {
                return msg.to_string();
            }
            if let Some(msg) = json.get("message").and_then(|v| v.as_str()) {
                return msg.to_string();
            }
        }
        if let Ok(text) = std::str::from_utf8(body) {
            let text = text.trim();
            if !text.is_empty() {
                return format!("{err}: {text}");
            }
        }
    }
    err.to_string()
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

    use crate::prometheus::app::percent_encode;
    use crate::prometheus::types::{PrometheusStringListResponse, PrometheusVectorItem};
    use crate::pyroscope::{FlameGraph, Level};

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

    fn make_datasources_with_pyroscope() -> Vec<Datasource> {
        let mut ds = make_datasources();
        ds.push(Datasource {
            id: 4,
            uid: "uid4".into(),
            name: "Pyroscope".into(),
            ds_type: "grafana-pyroscope-datasource".into(),
            url: "http://pyroscope:4040".into(),
            is_default: false,
            access: "proxy".into(),
        });
        ds.push(Datasource {
            id: 5,
            uid: "uid5".into(),
            name: "Phlare".into(),
            ds_type: "phlare".into(),
            url: "http://phlare:4100".into(),
            is_default: false,
            access: "proxy".into(),
        });
        ds
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
        assert!(vm.datasources.iter().all(|ds| {
            ds.ds_type == "prometheus" || ds.ds_type == "pyroscope"
        }));
        assert!(vm.datasources.iter().all(|ds| ds.name != "Loki"));
    }

    #[test]
    fn pyroscope_filter_includes_pyroscope_datasources() {
        let core = make_core();
        let response =
            ResponseBuilder::ok().body(make_datasources_with_pyroscope()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert!(vm
            .datasources
            .iter()
            .any(|ds| ds.ds_type == "pyroscope"));
    }

    #[test]
    fn enter_pyroscope_series_loaded() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        let response =
            ResponseBuilder::ok().body(make_datasources_with_pyroscope()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        // Select the pyroscope datasource (index 2 after filtering: Prometheus, Prometheus2, Pyroscope)
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);

        core.process_event(Event::EnterPyroscope { now_unix_ms: 1_000_000 });
        assert_eq!(core.view().screen, ScreenView::PyroscopeMode);

        let series = vec![
            ("svc-a".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            ("svc-a".into(), "memory:alloc_objects:count:space:bytes".into()),
        ];
        core.process_event(Event::PyroscopeSeriesLoaded(Ok(series)));

        let vm = core.view();
        assert_eq!(vm.pyroscope_series.len(), 2);
    }

    #[test]
    fn build_flamegraph_view_basic() {
        use crate::pyroscope::{build_flamegraph_view, FlamegraphNav};

        let fg = FlameGraph {
            names: vec!["total".into(), "func_a".into(), "func_b".into()],
            levels: vec![
                Level { values: vec![0, 100, 0, 0] },
                Level { values: vec![0, 60, 10, 1, 60, 40, 5, 2] },
            ],
            total: 100,
            max_self: 10,
        };
        let nav = FlamegraphNav::default();
        let view = build_flamegraph_view(&fg, &nav);

        assert_eq!(view.root_samples, 100);
        assert_eq!(view.levels.len(), 2);
        // Root level has 1 visible frame (the total frame)
        assert_eq!(view.levels[0].frames.len(), 1);
        assert_eq!(view.levels[0].frames[0].name, "total");
        // Level 1 has 2 frames
        assert_eq!(view.levels[1].frames.len(), 2);
        // Selected frame is root level frame 0 (sel_level=0, sel_frame=0)
        assert!(view.levels[0].frames[0].is_selected);
        assert!((view.levels[0].frames[0].total_pct - 100.0).abs() < 1e-9);
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
        assert_eq!(core.view().cursor_pos, 4);
        core.process_event(Event::CursorHome);
        core.process_event(Event::CursorLeft);
        assert_eq!(core.view().cursor_pos, 0);
    }

    #[test]
    fn query_delete_forward() {
        let core = make_core();
        core.process_event(Event::EnterQuery);
        for c in ['a', 'b', 'c'] {
            core.process_event(Event::QueryInput(c));
        }
        core.process_event(Event::QueryDelete);
        assert_eq!(core.view().query, "abc");
        core.process_event(Event::CursorHome);
        core.process_event(Event::CursorRight);
        core.process_event(Event::QueryDelete);
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
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::EnterQuery);

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

        core.process_event(Event::QueryInput('r'));
        core.process_event(Event::QueryInput('a'));
        assert_eq!(core.view().query, "ra");
        assert!(!core.view().completions.is_empty(), "expected completions");

        core.process_event(Event::CompletionNext);
        core.process_event(Event::CompletionAccept);

        let vm = core.view();
        assert_ne!(vm.query, "ra", "query should be updated after accept");
        assert!(!vm.query.starts_with("ra") || vm.query.len() > 2);
    }
}
