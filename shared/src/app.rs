use std::collections::{BTreeSet, HashMap, HashSet};

use crux_core::{
    macros::effect,
    render::{render, RenderOperation},
    App, Command,
};
use crux_http::{command::Http, protocol::HttpRequest};
use serde::{Deserialize, Serialize};

use crate::loki::LokiStream;
use crate::prometheus::types::{
    PrometheusResponse, PrometheusStringListResponse, PrometheusVectorItem,
};
use crate::pyroscope::{
    build_flamegraph_view, build_sandwich_view, FlameGraph, FlamegraphNav, FlamegraphView,
    SandwichView,
};
use crate::tempo::{TempoSpan, TempoTrace};

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
    Configure {
        url: String,
        token: String,
    },
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
    DatasourceFilterFocus,
    DatasourceFilterBlur,
    DatasourceFilterInput(char),
    DatasourceFilterBackspace,
    DatasourceFilterClear,

    // Autocomplete
    MetricNamesLoaded(crux_http::Result<crux_http::Response<PrometheusStringListResponse>>),
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
    EnterPyroscope {
        now_unix_ms: i64,
    },
    /// History restore: skip series loading and go directly to the flamegraph
    /// for a known service + profile_type.
    PyroscopeDirectLoad {
        service_name: String,
        profile_type: String,
    },

    PyroscopeSeriesNext,
    PyroscopeSeriesPrev,
    PyroscopeSeriesLoaded(Result<Vec<(String, String)>, String>),

    // Time range picker (global, all 4 signals)
    TimeRangePickerOpen {
        now_unix_ms: i64,
    },
    TimeRangePickerClose,
    TimeRangePickerNext,
    TimeRangePickerPrev,
    TimeRangePickerToggleFocus,
    TimeRangePickerCustomInput(char),
    TimeRangePickerCustomBackspace,
    TimeRangePickerCursorLeft,
    TimeRangePickerCursorRight,
    TimeRangePickerCommit {
        now_unix_ms: i64,
    },

    PyroscopeProfileTypeNext,
    PyroscopeProfileTypePrev,
    PyroscopeProfileTypeDropdownOpen,
    PyroscopeProfileTypeDropdownClose,

    PyroscopeServiceFilterFocus,
    PyroscopeServiceFilterBlur,
    PyroscopeServiceFilterInput(char),
    PyroscopeServiceFilterBackspace,
    PyroscopeServiceFilterClear,

    PyroscopeSelectSeries {
        now_unix_ms: i64,
    },
    /// Restore a history entry: directly set the time-range without triggering a reload.
    PyroscopeSetTimeRange(String),
    /// After series are loaded, select the service+profile_type that matches a history entry.
    PyroscopeSelectByName {
        service_name: String,
        profile_type: String,
    },
    PyroscopeFlamegraphLoaded(Result<Option<FlameGraph>, String>),
    PyroscopeTimelineLoaded(Result<Vec<crate::pyroscope::TimelineSeries>, String>),
    PyroscopeHeatmapLoaded(Result<Vec<crate::pyroscope::HeatmapSlot>, String>),
    PyroscopeSpanHeatmapLoaded(Result<Vec<crate::pyroscope::HeatmapSlot>, String>),
    /// 0 = Flamegraph, 1 = Timeline, 2 = Heatmap
    PyroscopeSelectView(usize),

    ExemplarSelectNext,
    ExemplarSelectPrev,
    ExemplarDetailOpen,
    ExemplarDetailClose,

    // Span Heatmap → Tempo datasource picker
    SpanHeatmapOpenTempoPicker,
    SpanHeatmapCloseTempoPicker,
    SpanHeatmapTempoPickerNext,
    SpanHeatmapTempoPickerPrev,
    SpanHeatmapClearResults,
    SpanHeatmapTempoLoadingStarted,
    SpanHeatmapTempoLoadingDone,
    SpanHeatmapTempoResultLoaded {
        span_id: String,
        trace_id: String,
    },
    SpanHeatmapTempoBatchLoaded(HashMap<String, String>),

    BackToServiceList,
    BackFromPyroscope,

    FlameMoveLeft,
    FlameMoveRight,
    FlameMoveUp,
    FlameMoveDown,
    FlameZoomIn,
    FlameZoomOut,
    FlamegraphViewportChars(u64),
    /// Toggle sandwich view for the currently selected frame name.
    FlameSandwich,
    /// Clear sandwich view (return to normal flamegraph).
    FlameSandwichClear,

    // Favourites
    FavouritesLoaded(Vec<String>), // datasource UIDs
    ToggleFavourite(String),       // datasource UID

    // History panel (startup screen)
    HistoryEntriesLoaded(Vec<HistoryEntryView>),
    HistoryPanelFocus,
    HistoryPanelBlur,
    HistorySelectNext,
    HistorySelectPrev,
    /// Select a datasource by UID (preferred) with a name fallback for older history entries.
    SelectDatasource {
        uid: String,
        name: String,
    },

    // Tempo mode
    EnterTempo,
    TempoQueryInput(char),
    TempoQueryBackspace,
    TempoQueryCursorLeft,
    TempoQueryCursorRight,
    TempoExecuteQuery,
    TempoResultLoaded(Result<Vec<TempoTrace>, String>),
    BackFromTempo,
    TempoSelectNext,
    TempoSelectPrev,
    TempoOpenTrace,

    // Trace detail
    TempoTraceDetailLoaded(Result<Vec<TempoSpan>, String>),
    TempoTraceDetailNext,
    TempoTraceDetailPrev,
    TempoTraceDetailFilterFocus,
    TempoTraceDetailFilterBlur,
    TempoTraceDetailFilterInput(char),
    TempoTraceDetailFilterBackspace,
    TempoTraceDetailAttrScrollDown,
    TempoTraceDetailAttrScrollUp,
    BackFromTraceDetail,

    // Loki mode
    EnterLoki,
    LokiQueryChanged(String),
    LokiExecuteQuery,
    LokiResultLoaded(Result<Vec<LokiStream>, String>),
    LokiSelectNextRow,
    LokiSelectPrevRow,
    LokiOpenContext,
    LokiCloseContext,
    LokiContextLoaded(Result<(Vec<crate::loki::LokiEntry>, usize), String>),
    BackFromLoki,
    LokiDiagnosticsUpdated(Vec<LokiDiagnosticView>),
}

// ── Screen ────────────────────────────────────────────────────────────────────

#[derive(Default, PartialEq)]
pub enum Screen {
    #[default]
    DatasourceList,
    QueryMode,
    PyroscopeMode,
    TempoMode,
    TempoTraceDetail,
    LokiMode,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Screen::DatasourceList => write!(f, "DatasourceList"),
            Screen::QueryMode => write!(f, "QueryMode"),
            Screen::PyroscopeMode => write!(f, "PyroscopeMode"),
            Screen::TempoMode => write!(f, "TempoMode"),
            Screen::TempoTraceDetail => write!(f, "TempoTraceDetail"),
            Screen::LokiMode => write!(f, "LokiMode"),
        }
    }
}

#[derive(Default, Debug, PartialEq)]
pub enum PyroscopeSubScreen {
    #[default]
    ServiceList,
    Flamegraph,
    Timeline,
    ProfileHeatmap,
    SpanHeatmap,
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
    pub datasource_filter_focused: bool,
    pub favourites: HashSet<String>, // datasource UIDs

    pub history_entries: Vec<HistoryEntryView>,
    pub history_panel_focused: bool,
    pub history_selected_index: usize,

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

    // Time range picker (global — applies to all 4 signal screens)
    pub time_range: String,
    pub time_range_picker_open: bool,
    pub time_range_picker_index: usize,
    /// Focus: 0 = preset list, 1 = absolute From, 2 = absolute To
    pub time_range_picker_focus: u8,
    pub time_range_picker_abs_from: String, // "YYYY-MM-DD HH:MM:SS"
    pub time_range_picker_abs_to: String,   // "YYYY-MM-DD HH:MM:SS"
    pub time_range_picker_from_cursor: usize, // byte cursor within abs_from
    pub time_range_picker_to_cursor: usize, // byte cursor within abs_to

    // Pyroscope state
    pub pyroscope_sub_screen: PyroscopeSubScreen,
    pub pyroscope_series_loading: bool,
    pub pyroscope_series_error: Option<String>,
    pub pyroscope_series: Vec<PyroscopeSeriesItem>,
    pub pyroscope_profile_types: Vec<String>,
    pub pyroscope_profile_type_index: usize,
    pub pyroscope_profile_type_dropdown_open: bool,
    pub pyroscope_service_filter: String,
    pub pyroscope_service_filter_focused: bool,
    pub pyroscope_series_index: usize,
    pub pyroscope_selected_service: String,
    pub pyroscope_selected_profile_type: String,
    pub pyroscope_flamegraph_loading: bool,
    pub pyroscope_flamegraph_error: Option<String>,
    pub pyroscope_flamegraph: Option<FlameGraph>,
    pub flamegraph_nav: FlamegraphNav,
    /// When `Some`, the sandwich view is active for this function name.
    pub sandwich_name: Option<String>,

    pub pyroscope_timeline_loading: bool,
    pub pyroscope_timeline_error: Option<String>,
    pub pyroscope_timeline: Vec<crate::pyroscope::TimelineSeries>,

    pub pyroscope_heatmap_loading: bool,
    pub pyroscope_heatmap_error: Option<String>,
    pub pyroscope_heatmap: Vec<crate::pyroscope::HeatmapSlot>,

    // Span heatmap — fetched with HEATMAP_QUERY_TYPE_SPAN + EXEMPLAR_TYPE_SPAN.
    pub pyroscope_span_heatmap_loading: bool,
    pub pyroscope_span_heatmap_error: Option<String>,
    pub pyroscope_span_heatmap: Vec<crate::pyroscope::HeatmapSlot>,

    pub pyroscope_exemplar_index: usize,
    pub exemplar_detail_open: bool,

    // Span Heatmap → Tempo correlation
    pub tempo_datasource_picker_open: bool,
    pub tempo_picker_index: usize,
    pub span_trace_loading: bool,
    pub span_trace_lookup: HashMap<String, String>,

    // Tempo state
    pub tempo_query: String,
    pub tempo_cursor_pos: usize,
    pub tempo_loading: bool,
    pub tempo_error: Option<String>,
    pub tempo_results: Vec<TempoTrace>,
    pub tempo_results_selected: usize,

    // Trace detail state
    pub trace_detail_trace_id: String,
    pub trace_detail_spans: Vec<TempoSpan>,
    pub trace_detail_loading: bool,
    pub trace_detail_error: Option<String>,
    pub trace_detail_selected: usize,
    pub trace_detail_filter: String,
    pub trace_detail_filter_focused: bool,
    pub trace_detail_attr_scroll: usize,

    // Loki state
    pub loki_query: String,
    pub loki_loading: bool,
    pub loki_error: Option<String>,
    pub loki_results: Vec<LokiStream>,
    pub loki_selected_row: usize,
    pub loki_context_open: bool,
    pub loki_context_loading: bool,
    pub loki_context_error: Option<String>,
    pub loki_context_entries: Vec<crate::loki::LokiEntry>,
    pub loki_context_highlight_index: usize,
    pub loki_context_labels: String,

    // Loki diagnostics (pushed from LSP via LokiDiagnosticsUpdated)
    pub loki_diagnostics: Vec<LokiDiagnosticView>,
}

// ── ViewModel types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntryView {
    /// The query text: PromQL for Prometheus, "service:profile_type" for Pyroscope.
    pub query: String,
    pub datasource_name: String,
    pub datasource_uid: String,
    /// Normalised type: "prometheus" or "pyroscope".
    pub datasource_type: String,
    /// Human-readable age, e.g. "5m ago".
    pub time_ago: String,
    // Pyroscope-specific (empty string for Prometheus entries).
    pub service_name: String,
    pub profile_type: String,
    pub time_range: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatasourceView {
    pub id: u64,
    pub uid: String,
    pub name: String,
    pub ds_type: String,
    pub url: String,
    pub is_default: bool,
    pub is_favourite: bool,
}

/// A diagnostic (parse error or semantic warning) for the Loki query box.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiDiagnosticView {
    pub severity: String,
    pub message: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub enum ScreenView {
    #[default]
    DatasourceList,
    QueryMode,
    PyroscopeMode,
    TempoMode,
    TempoTraceDetail,
    LokiMode,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub enum PyroscopeSubScreenView {
    #[default]
    ServiceList,
    Flamegraph,
    Timeline,
    ProfileHeatmap,
    SpanHeatmap,
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
    pub datasource_filter_focused: bool,

    pub history_entries: Vec<HistoryEntryView>,
    pub history_panel_focused: bool,
    pub history_selected_index: usize,

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

    // Time range picker (global)
    pub time_range: String,
    pub time_range_picker_open: bool,
    pub time_range_picker_index: usize,
    /// Focus: 0 = preset list, 1 = absolute From, 2 = absolute To
    pub time_range_picker_focus: u8,
    pub time_range_picker_abs_from: String,
    pub time_range_picker_abs_to: String,
    pub time_range_picker_from_cursor: usize,
    pub time_range_picker_to_cursor: usize,

    // Pyroscope
    pub pyroscope_sub_screen: PyroscopeSubScreenView,
    pub pyroscope_series_loading: bool,
    pub pyroscope_series_error: Option<String>,
    pub pyroscope_series: Vec<(String, String)>, // filtered by profile type + service filter
    pub pyroscope_profile_types: Vec<String>,
    pub pyroscope_profile_type_index: usize,
    pub pyroscope_profile_type_dropdown_open: bool,
    pub pyroscope_service_filter: String,
    pub pyroscope_service_filter_focused: bool,
    pub pyroscope_series_index: usize,
    pub pyroscope_selected_service: String,
    pub pyroscope_selected_profile_type: String,
    pub pyroscope_flamegraph_loading: bool,
    pub pyroscope_flamegraph_error: Option<String>,
    pub flamegraph: Option<FlamegraphView>,
    pub sandwich_view: Option<SandwichView>,

    pub pyroscope_timeline_loading: bool,
    pub pyroscope_timeline_error: Option<String>,
    pub timeline: Option<crate::pyroscope::TimelineView>,

    pub pyroscope_heatmap_loading: bool,
    pub pyroscope_heatmap_error: Option<String>,
    pub heatmap: Option<crate::pyroscope::HeatmapView>,

    pub pyroscope_span_heatmap_loading: bool,
    pub pyroscope_span_heatmap_error: Option<String>,
    pub span_heatmap: Option<crate::pyroscope::HeatmapView>,

    pub pyroscope_exemplar_index: usize,
    pub exemplar_detail_open: bool,

    // Span Heatmap → Tempo correlation
    pub tempo_datasource_picker_open: bool,
    pub tempo_picker_index: usize,
    pub tempo_datasources: Vec<DatasourceView>,
    pub span_trace_loading: bool,
    pub span_trace_lookup: HashMap<String, String>,

    // Tempo state
    pub tempo_query: String,
    pub tempo_cursor_pos: usize,
    pub tempo_loading: bool,
    pub tempo_error: Option<String>,
    pub tempo_results: Vec<TempoTrace>,
    pub tempo_results_selected: usize,

    // Trace detail
    pub trace_detail_trace_id: String,
    pub trace_detail_spans: Vec<TempoSpan>,
    pub trace_detail_loading: bool,
    pub trace_detail_error: Option<String>,
    pub trace_detail_selected: usize,
    pub trace_detail_filter: String,
    pub trace_detail_filter_focused: bool,
    pub trace_detail_attr_scroll: usize,

    // Loki state
    pub loki_loading: bool,
    pub loki_error: Option<String>,
    pub loki_results: Vec<LokiStream>,
    pub loki_selected_row: usize,
    pub loki_context_open: bool,
    pub loki_context_loading: bool,
    pub loki_context_error: Option<String>,
    pub loki_context_entries: Vec<crate::loki::LokiEntry>,
    pub loki_context_highlight_index: usize,
    pub loki_context_labels: String,

    // Loki diagnostics (pushed from LSP)
    pub loki_diagnostics: Vec<LokiDiagnosticView>,
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
                if model.time_range.is_empty() {
                    model.time_range = "1h".to_string();
                }
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
                            || ds.ds_type == "tempo"
                            || ds.ds_type == "loki"
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
                        model.selected_index = (model.selected_index + count - 1) % count;
                    }
                }
                render()
            }

            Event::Quit => Command::done(),

            Event::DatasourceFilterFocus => {
                model.datasource_filter_focused = true;
                render()
            }

            Event::DatasourceFilterBlur => {
                model.datasource_filter_focused = false;
                render()
            }

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
                model.datasource_filter_focused = false;
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
            Event::PyroscopeDirectLoad {
                service_name,
                profile_type,
            } => crate::pyroscope::app::handle_pyroscope_direct_load(
                model,
                service_name,
                profile_type,
            ),
            Event::PyroscopeSeriesLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_series_loaded(model, result)
            }
            Event::PyroscopeSeriesNext => {
                crate::pyroscope::app::handle_pyroscope_series_next(model)
            }
            Event::PyroscopeSeriesPrev => {
                crate::pyroscope::app::handle_pyroscope_series_prev(model)
            }
            Event::TimeRangePickerOpen { now_unix_ms } => {
                crate::time_range_picker::handle_open(model, now_unix_ms)
            }
            Event::TimeRangePickerClose => crate::time_range_picker::handle_close(model),
            Event::TimeRangePickerNext => crate::time_range_picker::handle_next(model),
            Event::TimeRangePickerPrev => crate::time_range_picker::handle_prev(model),
            Event::TimeRangePickerToggleFocus => {
                crate::time_range_picker::handle_toggle_focus(model)
            }
            Event::TimeRangePickerCustomInput(c) => {
                crate::time_range_picker::handle_custom_input(model, c)
            }
            Event::TimeRangePickerCustomBackspace => {
                crate::time_range_picker::handle_custom_backspace(model)
            }
            Event::TimeRangePickerCursorLeft => crate::time_range_picker::handle_cursor_left(model),
            Event::TimeRangePickerCursorRight => {
                crate::time_range_picker::handle_cursor_right(model)
            }
            Event::TimeRangePickerCommit { now_unix_ms } => {
                crate::time_range_picker::handle_commit(model, now_unix_ms)
            }
            Event::PyroscopeProfileTypeNext => {
                crate::pyroscope::app::handle_pyroscope_profile_type_next(model)
            }
            Event::PyroscopeProfileTypePrev => {
                crate::pyroscope::app::handle_pyroscope_profile_type_prev(model)
            }
            Event::PyroscopeProfileTypeDropdownOpen => {
                model.pyroscope_profile_type_dropdown_open = true;
                render()
            }
            Event::PyroscopeProfileTypeDropdownClose => {
                model.pyroscope_profile_type_dropdown_open = false;
                render()
            }
            Event::PyroscopeServiceFilterFocus => {
                model.pyroscope_service_filter_focused = true;
                render()
            }
            Event::PyroscopeServiceFilterBlur => {
                model.pyroscope_service_filter_focused = false;
                render()
            }
            Event::PyroscopeServiceFilterInput(c) => {
                crate::pyroscope::app::handle_pyroscope_service_filter_input(model, c)
            }
            Event::PyroscopeServiceFilterBackspace => {
                crate::pyroscope::app::handle_pyroscope_service_filter_backspace(model)
            }
            Event::PyroscopeServiceFilterClear => {
                model.pyroscope_service_filter_focused = false;
                crate::pyroscope::app::handle_pyroscope_service_filter_clear(model)
            }
            Event::PyroscopeSelectSeries { .. } => {
                crate::pyroscope::app::handle_pyroscope_select_series(model)
            }
            Event::PyroscopeSetTimeRange(range) => {
                crate::pyroscope::app::handle_pyroscope_set_time_range(model, range)
            }
            Event::PyroscopeSelectByName {
                service_name,
                profile_type,
            } => crate::pyroscope::app::handle_pyroscope_select_by_name(
                model,
                service_name,
                profile_type,
            ),
            Event::PyroscopeFlamegraphLoaded(result) => {
                model.sandwich_name = None;
                crate::pyroscope::app::handle_pyroscope_flamegraph_loaded(model, result)
            }
            Event::PyroscopeTimelineLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_timeline_loaded(model, result)
            }
            Event::PyroscopeHeatmapLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_heatmap_loaded(model, result)
            }
            Event::PyroscopeSpanHeatmapLoaded(result) => {
                crate::pyroscope::app::handle_pyroscope_span_heatmap_loaded(model, result)
            }
            Event::PyroscopeSelectView(idx) => {
                crate::pyroscope::app::handle_pyroscope_select_view(model, idx)
            }
            Event::ExemplarSelectNext => crate::pyroscope::app::handle_exemplar_select_next(model),
            Event::ExemplarSelectPrev => crate::pyroscope::app::handle_exemplar_select_prev(model),
            Event::ExemplarDetailOpen => {
                model.exemplar_detail_open = true;
                render()
            }
            Event::ExemplarDetailClose => {
                model.exemplar_detail_open = false;
                render()
            }
            Event::SpanHeatmapClearResults => {
                model.span_trace_lookup.clear();
                render()
            }
            Event::SpanHeatmapTempoLoadingStarted => {
                model.span_trace_loading = true;
                render()
            }
            Event::SpanHeatmapTempoLoadingDone => {
                model.span_trace_loading = false;
                render()
            }
            Event::SpanHeatmapOpenTempoPicker => {
                model.tempo_datasource_picker_open = true;
                model.tempo_picker_index = 0;
                render()
            }
            Event::SpanHeatmapCloseTempoPicker => {
                model.tempo_datasource_picker_open = false;
                render()
            }
            Event::SpanHeatmapTempoPickerNext => {
                let n = model
                    .datasources
                    .iter()
                    .filter(|d| d.ds_type == "tempo")
                    .count();
                if n > 0 {
                    model.tempo_picker_index = (model.tempo_picker_index + 1) % n;
                }
                render()
            }
            Event::SpanHeatmapTempoPickerPrev => {
                let n = model
                    .datasources
                    .iter()
                    .filter(|d| d.ds_type == "tempo")
                    .count();
                if n > 0 {
                    model.tempo_picker_index = (model.tempo_picker_index + n - 1) % n;
                }
                render()
            }
            Event::SpanHeatmapTempoResultLoaded { span_id, trace_id } => {
                if !trace_id.is_empty() {
                    model.span_trace_lookup.insert(span_id, trace_id);
                }
                render()
            }
            Event::SpanHeatmapTempoBatchLoaded(map) => {
                model.span_trace_lookup.extend(map);
                render()
            }
            Event::BackToServiceList => crate::pyroscope::app::handle_back_to_service_list(model),
            Event::BackFromPyroscope => crate::pyroscope::app::handle_back_from_pyroscope(model),
            Event::FlameMoveLeft => crate::pyroscope::app::handle_flame_move_left(model),
            Event::FlameMoveRight => crate::pyroscope::app::handle_flame_move_right(model),
            Event::FlameMoveUp => crate::pyroscope::app::handle_flame_move_up(model),
            Event::FlameMoveDown => crate::pyroscope::app::handle_flame_move_down(model),
            Event::FlameZoomIn => crate::pyroscope::app::handle_flame_zoom_in(model),
            Event::FlameZoomOut => crate::pyroscope::app::handle_flame_zoom_out(model),
            Event::FlamegraphViewportChars(chars) => {
                crate::pyroscope::app::handle_flamegraph_viewport_chars(model, chars)
            }
            Event::FlameSandwich => {
                if let Some(ref fg) = model.pyroscope_flamegraph {
                    let name = fg
                        .levels
                        .get(model.flamegraph_nav.sel_level)
                        .and_then(|lv| {
                            let i = model.flamegraph_nav.sel_frame * 4 + 3;
                            lv.values
                                .get(i)
                                .and_then(|&ni| fg.names.get(ni as usize))
                                .cloned()
                        });
                    if let Some(name) = name {
                        if model.sandwich_name.as_deref() == Some(&name) {
                            model.sandwich_name = None;
                        } else {
                            model.sandwich_name = Some(name);
                        }
                    }
                }
                render()
            }
            Event::FlameSandwichClear => {
                model.sandwich_name = None;
                render()
            }

            Event::FavouritesLoaded(uids) => {
                model.favourites = uids.into_iter().collect();
                render()
            }
            Event::ToggleFavourite(uid) => {
                if !model.favourites.remove(&uid) {
                    model.favourites.insert(uid);
                }
                render()
            }

            Event::HistoryEntriesLoaded(entries) => {
                model.history_entries = entries;
                model.history_selected_index = 0;
                render()
            }

            Event::HistoryPanelFocus => {
                model.history_panel_focused = true;
                model.datasource_filter_focused = false;
                render()
            }

            Event::HistoryPanelBlur => {
                model.history_panel_focused = false;
                render()
            }

            Event::HistorySelectNext => {
                let len = model.history_entries.len();
                if len > 0 {
                    model.history_selected_index = (model.history_selected_index + 1) % len;
                }
                render()
            }

            Event::HistorySelectPrev => {
                let len = model.history_entries.len();
                if len > 0 {
                    model.history_selected_index = (model.history_selected_index + len - 1) % len;
                }
                render()
            }

            // ── Tempo ─────────────────────────────────────────────────────────
            Event::EnterTempo => crate::tempo::app::handle_enter_tempo(model),
            Event::TempoQueryInput(c) => crate::tempo::app::handle_tempo_query_input(model, c),
            Event::TempoQueryBackspace => crate::tempo::app::handle_tempo_query_backspace(model),
            Event::TempoQueryCursorLeft => crate::tempo::app::handle_tempo_cursor_left(model),
            Event::TempoQueryCursorRight => crate::tempo::app::handle_tempo_cursor_right(model),
            Event::TempoExecuteQuery => crate::tempo::app::handle_tempo_execute_query(model),
            Event::TempoResultLoaded(result) => {
                crate::tempo::app::handle_tempo_result_loaded(model, result)
            }
            Event::BackFromTempo => crate::tempo::app::handle_back_from_tempo(model),
            Event::TempoSelectNext => crate::tempo::app::handle_tempo_select_next(model),
            Event::TempoSelectPrev => crate::tempo::app::handle_tempo_select_prev(model),
            Event::TempoOpenTrace => crate::tempo::app::handle_tempo_open_trace(model),
            Event::TempoTraceDetailLoaded(result) => {
                crate::tempo::app::handle_trace_detail_loaded(model, result)
            }
            Event::TempoTraceDetailNext => crate::tempo::app::handle_trace_detail_next(model),
            Event::TempoTraceDetailPrev => crate::tempo::app::handle_trace_detail_prev(model),
            Event::TempoTraceDetailFilterFocus => {
                crate::tempo::app::handle_trace_detail_filter_focus(model)
            }
            Event::TempoTraceDetailFilterBlur => {
                crate::tempo::app::handle_trace_detail_filter_blur(model)
            }
            Event::TempoTraceDetailFilterInput(c) => {
                crate::tempo::app::handle_trace_detail_filter_input(model, c)
            }
            Event::TempoTraceDetailFilterBackspace => {
                crate::tempo::app::handle_trace_detail_filter_backspace(model)
            }
            Event::TempoTraceDetailAttrScrollDown => {
                crate::tempo::app::handle_trace_detail_attr_scroll_down(model)
            }
            Event::TempoTraceDetailAttrScrollUp => {
                crate::tempo::app::handle_trace_detail_attr_scroll_up(model)
            }
            Event::BackFromTraceDetail => crate::tempo::app::handle_back_from_trace_detail(model),

            // ── Loki ─────────────────────────────────────────────────────────
            Event::EnterLoki => crate::loki::app::handle_enter_loki(model),
            Event::LokiQueryChanged(q) => crate::loki::app::handle_loki_query_changed(model, q),
            Event::LokiExecuteQuery => crate::loki::app::handle_loki_execute_query(model),
            Event::LokiResultLoaded(result) => {
                crate::loki::app::handle_loki_result_loaded(model, result)
            }
            Event::LokiSelectNextRow => crate::loki::app::handle_loki_select_next_row(model),
            Event::LokiSelectPrevRow => crate::loki::app::handle_loki_select_prev_row(model),
            Event::LokiOpenContext => crate::loki::app::handle_loki_open_context(model),
            Event::LokiCloseContext => crate::loki::app::handle_loki_close_context(model),
            Event::LokiContextLoaded(result) => {
                crate::loki::app::handle_loki_context_loaded(model, result)
            }
            Event::BackFromLoki => crate::loki::app::handle_back_from_loki(model),
            Event::LokiDiagnosticsUpdated(diags) => {
                crate::loki::app::handle_loki_diagnostics_updated(model, diags)
            }

            Event::SelectDatasource { uid, name } => {
                model.datasource_filter.clear();
                model.datasource_filter_focused = false;
                model.history_panel_focused = false;
                // Try by UID first; fall back to name for older history entries that lack a uid.
                // Find the raw index first, then convert to the sorted position that view() uses,
                // so that model.selected_index always reflects the sorted display order.
                let raw_pos = if !uid.is_empty() {
                    model.datasources.iter().position(|ds| ds.uid == uid)
                } else {
                    None
                }
                .or_else(|| {
                    if !name.is_empty() {
                        model.datasources.iter().position(|ds| ds.name == name)
                    } else {
                        None
                    }
                });
                if let Some(raw) = raw_pos {
                    // sorted_datasource_indices is computed after the filter is cleared above.
                    let sorted = sorted_datasource_indices(model);
                    if let Some(sorted_pos) = sorted.iter().position(|&i| i == raw) {
                        model.selected_index = sorted_pos;
                    }
                }
                render()
            }
        }
    }

    fn view(&self, model: &Self::Model) -> Self::ViewModel {
        let screen = match model.screen {
            Screen::DatasourceList => ScreenView::DatasourceList,
            Screen::QueryMode => ScreenView::QueryMode,
            Screen::PyroscopeMode => ScreenView::PyroscopeMode,
            Screen::TempoMode => ScreenView::TempoMode,
            Screen::TempoTraceDetail => ScreenView::TempoTraceDetail,
            Screen::LokiMode => ScreenView::LokiMode,
        };

        let mut indices = filtered_datasource_indices(model);
        // Favourites bubble to the top (stable sort preserves relative order within groups).
        indices.sort_by_key(|&i| {
            if model.favourites.contains(&model.datasources[i].uid) {
                0u8
            } else {
                1u8
            }
        });
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
                    uid: ds.uid.clone(),
                    name: ds.name.clone(),
                    ds_type: normalize_ds_type(&ds.ds_type).to_string(),
                    url: ds.url.clone(),
                    is_default: ds.is_default,
                    is_favourite: model.favourites.contains(&ds.uid),
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

        let units = crate::pyroscope::ProfileUnit::from_profile_type_id(
            &model.pyroscope_selected_profile_type,
        )
        .label()
        .to_string();

        let flamegraph = model.pyroscope_flamegraph.as_ref().map(|fg| {
            let mut fgv = build_flamegraph_view(fg, &model.flamegraph_nav);
            fgv.units = units.clone();
            fgv
        });

        let sandwich_view = model.sandwich_name.as_ref().and_then(|name| {
            model
                .pyroscope_flamegraph
                .as_ref()
                .and_then(|fg| build_sandwich_view(fg, name, units))
        });

        let pyroscope_sub_screen = match model.pyroscope_sub_screen {
            PyroscopeSubScreen::ServiceList => PyroscopeSubScreenView::ServiceList,
            PyroscopeSubScreen::Flamegraph => PyroscopeSubScreenView::Flamegraph,
            PyroscopeSubScreen::Timeline => PyroscopeSubScreenView::Timeline,
            PyroscopeSubScreen::ProfileHeatmap => PyroscopeSubScreenView::ProfileHeatmap,
            PyroscopeSubScreen::SpanHeatmap => PyroscopeSubScreenView::SpanHeatmap,
        };

        let timeline = if model.pyroscope_timeline.is_empty() {
            None
        } else {
            crate::pyroscope::build_timeline_view(&model.pyroscope_timeline)
        };

        let heatmap = crate::pyroscope::build_heatmap_view(&model.pyroscope_heatmap);

        let span_heatmap = crate::pyroscope::build_heatmap_view(&model.pyroscope_span_heatmap);

        let pyroscope_filtered = filtered_pyroscope_series_indices(model);
        let pyroscope_series_index = if pyroscope_filtered.is_empty() {
            0
        } else {
            model
                .pyroscope_series_index
                .min(pyroscope_filtered.len() - 1)
        };
        let pyroscope_series: Vec<(String, String)> = pyroscope_filtered
            .iter()
            .map(|&i| {
                let item = &model.pyroscope_series[i];
                (item.service_name.clone(), item.profile_type_id.clone())
            })
            .collect();

        let history_selected_index = if model.history_entries.is_empty() {
            0
        } else {
            model
                .history_selected_index
                .min(model.history_entries.len() - 1)
        };

        let mut tempo_datasources: Vec<DatasourceView> = model
            .datasources
            .iter()
            .filter(|ds| ds.ds_type == "tempo")
            .map(|ds| DatasourceView {
                id: ds.id,
                uid: ds.uid.clone(),
                name: ds.name.clone(),
                ds_type: normalize_ds_type(&ds.ds_type).to_string(),
                url: ds.url.clone(),
                is_default: ds.is_default,
                is_favourite: model.favourites.contains(&ds.uid),
            })
            .collect();
        tempo_datasources.sort_by_key(|ds| if ds.is_favourite { 0u8 } else { 1u8 });
        let tempo_picker_index = if tempo_datasources.is_empty() {
            0
        } else {
            model.tempo_picker_index.min(tempo_datasources.len() - 1)
        };

        ViewModel {
            datasource_filter: model.datasource_filter.clone(),
            datasource_filter_focused: model.datasource_filter_focused,
            history_entries: model.history_entries.clone(),
            history_panel_focused: model.history_panel_focused,
            history_selected_index,
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
            time_range: model.time_range.clone(),
            time_range_picker_open: model.time_range_picker_open,
            time_range_picker_index: model.time_range_picker_index,
            time_range_picker_focus: model.time_range_picker_focus,
            time_range_picker_abs_from: model.time_range_picker_abs_from.clone(),
            time_range_picker_abs_to: model.time_range_picker_abs_to.clone(),
            time_range_picker_from_cursor: model.time_range_picker_from_cursor,
            time_range_picker_to_cursor: model.time_range_picker_to_cursor,
            pyroscope_sub_screen,
            pyroscope_series_loading: model.pyroscope_series_loading,
            pyroscope_series_error: model.pyroscope_series_error.clone(),
            pyroscope_series,
            pyroscope_profile_types: model.pyroscope_profile_types.clone(),
            pyroscope_profile_type_index: model.pyroscope_profile_type_index,
            pyroscope_profile_type_dropdown_open: model.pyroscope_profile_type_dropdown_open,
            pyroscope_service_filter: model.pyroscope_service_filter.clone(),
            pyroscope_service_filter_focused: model.pyroscope_service_filter_focused,
            pyroscope_series_index,
            pyroscope_selected_service: model.pyroscope_selected_service.clone(),
            pyroscope_selected_profile_type: model.pyroscope_selected_profile_type.clone(),
            pyroscope_flamegraph_loading: model.pyroscope_flamegraph_loading,
            pyroscope_flamegraph_error: model.pyroscope_flamegraph_error.clone(),
            flamegraph,
            sandwich_view,
            pyroscope_timeline_loading: model.pyroscope_timeline_loading,
            pyroscope_timeline_error: model.pyroscope_timeline_error.clone(),
            timeline,
            pyroscope_heatmap_loading: model.pyroscope_heatmap_loading,
            pyroscope_heatmap_error: model.pyroscope_heatmap_error.clone(),
            heatmap,
            pyroscope_span_heatmap_loading: model.pyroscope_span_heatmap_loading,
            pyroscope_span_heatmap_error: model.pyroscope_span_heatmap_error.clone(),
            span_heatmap,
            pyroscope_exemplar_index: model.pyroscope_exemplar_index,
            exemplar_detail_open: model.exemplar_detail_open,
            tempo_datasource_picker_open: model.tempo_datasource_picker_open,
            tempo_picker_index,
            tempo_datasources,
            span_trace_loading: model.span_trace_loading,
            span_trace_lookup: model.span_trace_lookup.clone(),
            tempo_query: model.tempo_query.clone(),
            tempo_cursor_pos: model.tempo_cursor_pos,
            tempo_loading: model.tempo_loading,
            tempo_error: model.tempo_error.clone(),
            tempo_results: model.tempo_results.clone(),
            tempo_results_selected: model.tempo_results_selected,
            trace_detail_trace_id: model.trace_detail_trace_id.clone(),
            trace_detail_spans: model.trace_detail_spans.clone(),
            trace_detail_loading: model.trace_detail_loading,
            trace_detail_error: model.trace_detail_error.clone(),
            trace_detail_selected: model.trace_detail_selected,
            trace_detail_filter: model.trace_detail_filter.clone(),
            trace_detail_filter_focused: model.trace_detail_filter_focused,
            trace_detail_attr_scroll: model.trace_detail_attr_scroll,
            loki_loading: model.loki_loading,
            loki_error: model.loki_error.clone(),
            loki_results: model.loki_results.clone(),
            loki_selected_row: model.loki_selected_row,
            loki_context_open: model.loki_context_open,
            loki_context_loading: model.loki_context_loading,
            loki_context_error: model.loki_context_error.clone(),
            loki_context_entries: model.loki_context_entries.clone(),
            loki_context_highlight_index: model.loki_context_highlight_index,
            loki_context_labels: model.loki_context_labels.clone(),
            loki_diagnostics: model.loki_diagnostics.clone(),
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

pub(crate) fn fuzzy_match(haystack: &str, needle: &str) -> bool {
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

pub(crate) fn filtered_pyroscope_series_indices(model: &Model) -> Vec<usize> {
    let profile_type = model
        .pyroscope_profile_types
        .get(model.pyroscope_profile_type_index)
        .map(|s| s.as_str())
        .unwrap_or("");
    model
        .pyroscope_series
        .iter()
        .enumerate()
        .filter(|(_, item)| item.profile_type_id == profile_type)
        .filter(|(_, item)| fuzzy_match(&item.service_name, &model.pyroscope_service_filter))
        .map(|(i, _)| i)
        .collect()
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

/// Returns filtered datasource indices sorted with favourites first, matching
/// the order used by `view()` to build `ViewModel::datasources`.
pub(crate) fn sorted_datasource_indices(model: &Model) -> Vec<usize> {
    let mut indices = filtered_datasource_indices(model);
    indices.sort_by_key(|&i| {
        if model.favourites.contains(&model.datasources[i].uid) {
            0u8
        } else {
            1u8
        }
    });
    indices
}

/// Returns the datasource that `model.selected_index` (a sorted position) points to.
pub(crate) fn active_datasource(model: &Model) -> Option<&Datasource> {
    let raw = *sorted_datasource_indices(model).get(model.selected_index)?;
    model.datasources.get(raw)
}

// ── Utility helpers ───────────────────────────────────────────────────────────

pub(crate) fn extract_error_message(err: &crux_http::HttpError) -> String {
    if let crux_http::HttpError::Http {
        body: Some(body), ..
    } = err
    {
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
        assert_eq!(core.view().selected_index, 2);
        core.process_event(Event::SelectNext);
        assert_eq!(core.view().selected_index, 0);
    }

    #[test]
    fn select_previous_wraps_around() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::SelectPrevious);
        assert_eq!(core.view().selected_index, 2);
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
        assert_eq!(vm.datasources.len(), 3);
        assert_eq!(vm.datasources[0].name, "Prometheus");
        assert!(vm.datasources[0].is_default);
    }

    #[test]
    fn filters_supported_datasources() {
        let core = make_core();
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert!(vm.datasources.iter().all(|ds| {
            ds.ds_type == "prometheus"
                || ds.ds_type == "pyroscope"
                || ds.ds_type == "loki"
                || ds.ds_type == "tempo"
        }));
        assert!(vm.datasources.iter().any(|ds| ds.ds_type == "loki"));
    }

    #[test]
    fn pyroscope_filter_includes_pyroscope_datasources() {
        let core = make_core();
        let response = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        let vm = core.view();
        assert!(vm.datasources.iter().any(|ds| ds.ds_type == "pyroscope"));
    }

    #[test]
    fn enter_pyroscope_series_loaded() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        let response = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        // Select the pyroscope datasource (index 3 after filtering: Prometheus, Loki, Prometheus2, Pyroscope, Phlare)
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);

        core.process_event(Event::EnterPyroscope {
            now_unix_ms: 1_000_000,
        });
        assert_eq!(core.view().screen, ScreenView::PyroscopeMode);

        let series = vec![
            ("svc-a".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            (
                "svc-a".into(),
                "memory:alloc_objects:count:space:bytes".into(),
            ),
        ];
        core.process_event(Event::PyroscopeSeriesLoaded(Ok(series)));

        let vm = core.view();
        // Two distinct profile types extracted from the series
        assert_eq!(vm.pyroscope_profile_types.len(), 2);
        // Filtered to the first profile type (alphabetically), so 1 visible service
        assert_eq!(vm.pyroscope_series.len(), 1);
    }

    #[test]
    fn pyroscope_profile_type_cycling() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        let response = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);
        core.process_event(Event::EnterPyroscope {
            now_unix_ms: 1_000_000,
        });

        let series = vec![
            ("svc-a".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            ("svc-b".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            (
                "svc-a".into(),
                "memory:alloc_objects:count:space:bytes".into(),
            ),
        ];
        core.process_event(Event::PyroscopeSeriesLoaded(Ok(series)));

        // First profile type selected (alphabetically: cpu:...)
        let vm = core.view();
        assert_eq!(vm.pyroscope_profile_type_index, 0);
        assert_eq!(vm.pyroscope_series.len(), 2); // svc-a and svc-b under cpu

        // Cycle to next profile type
        core.process_event(Event::PyroscopeProfileTypeNext);
        let vm = core.view();
        assert_eq!(vm.pyroscope_profile_type_index, 1);
        assert_eq!(vm.pyroscope_series.len(), 1); // only svc-a under memory

        // Cycle back
        core.process_event(Event::PyroscopeProfileTypePrev);
        assert_eq!(core.view().pyroscope_profile_type_index, 0);
    }

    #[test]
    fn pyroscope_service_filter() {
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        let response = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);
        core.process_event(Event::SelectNext);
        core.process_event(Event::EnterPyroscope {
            now_unix_ms: 1_000_000,
        });

        let series = vec![
            ("alpha".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            ("beta".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
            ("gamma".into(), "cpu:cpu:nanoseconds:cpu:nanoseconds".into()),
        ];
        core.process_event(Event::PyroscopeSeriesLoaded(Ok(series)));

        assert_eq!(core.view().pyroscope_series.len(), 3);

        // Type filter
        core.process_event(Event::PyroscopeServiceFilterInput('b'));
        assert_eq!(core.view().pyroscope_series.len(), 1);
        assert_eq!(core.view().pyroscope_series[0].0, "beta");

        // Clear filter
        core.process_event(Event::PyroscopeServiceFilterClear);
        assert_eq!(core.view().pyroscope_series.len(), 3);
    }

    #[test]
    fn build_flamegraph_view_basic() {
        use crate::pyroscope::{build_flamegraph_view, FlamegraphNav};

        let fg = FlameGraph {
            names: vec!["total".into(), "func_a".into(), "func_b".into()],
            levels: vec![
                Level {
                    values: vec![0, 100, 0, 0],
                },
                Level {
                    values: vec![0, 60, 10, 1, 60, 40, 5, 2],
                },
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
    fn enter_query_with_filter_opens_correct_datasource() {
        // Regression: when a datasource filter is active, EnterQuery must open
        // the highlighted (filtered) datasource, not the one at the same index
        // in the unfiltered list.
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        // Datasources: [Prometheus(0), Loki(1), Prometheus2(2)]
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        // Filter to only "Prometheus2" — index 0 in the filtered view maps to
        // raw index 2, not raw index 0 (Prometheus).
        core.process_event(Event::DatasourceFilterInput('2'));
        assert_eq!(core.view().datasources.len(), 1);
        assert_eq!(core.view().datasources[0].name, "Prometheus2");
        assert_eq!(core.view().selected_index, 0);

        core.process_event(Event::EnterQuery);

        // After entering query mode the selected datasource must be Prometheus2.
        let vm = core.view();
        assert_eq!(vm.screen, ScreenView::QueryMode);
        assert_eq!(vm.selected_datasource_name.as_deref(), Some("Prometheus2"));
    }

    #[test]
    fn enter_pyroscope_with_filter_opens_correct_datasource() {
        // Regression: same index-remapping bug as enter_query, but for Pyroscope.
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://localhost:3000".into(),
            token: "test-token".into(),
        });
        // Datasources: [Prometheus(0), Loki(1), Prometheus2(2), Pyroscope(3), Phlare(4)]
        let response = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        // Filter to show only "Pyroscope" — the single result sits at selected_index 0,
        // which in the unfiltered list would incorrectly resolve to Prometheus.
        core.process_event(Event::DatasourceFilterInput('P'));
        core.process_event(Event::DatasourceFilterInput('y'));
        core.process_event(Event::DatasourceFilterInput('r'));
        assert_eq!(core.view().datasources[0].name, "Pyroscope");
        assert_eq!(core.view().selected_index, 0);

        core.process_event(Event::EnterPyroscope {
            now_unix_ms: 1_000_000,
        });

        let vm = core.view();
        assert_eq!(vm.screen, ScreenView::PyroscopeMode);
        assert_eq!(vm.selected_datasource_name.as_deref(), Some("Pyroscope"));
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

    // Helper: extract HTTP request URLs from a list of effects.
    fn http_urls(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| {
                if let Effect::Http(req) = e {
                    Some(req.operation.url.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn pyroscope_history_restore_sets_expected_state() {
        // Simulate restoring a pyroscope history entry via the direct-load path.
        // Expected outcome:
        //   - datasource: Pyroscope (id=4, uid=uid4)
        //   - time range: "2h" (from history, not the default "1h")
        //   - selected service: "my-svc"
        //   - selected profile type: "process_cpu:cpu:nanoseconds:cpu:nanoseconds"
        //   - sub-screen: Flamegraph (no series load required)
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://grafana".into(),
            token: "tok".into(),
        });
        let resp = ResponseBuilder::ok()
            .body(make_datasources_with_pyroscope())
            .build();
        core.process_event(Event::DatasourcesLoaded(Ok(resp)));

        // Step 1: select the right datasource
        core.process_event(Event::SelectDatasource {
            uid: "uid4".into(),
            name: "Pyroscope".into(),
        });

        // Step 2: restore the stored time range
        core.process_event(Event::PyroscopeSetTimeRange("2h".into()));

        // Step 3: jump directly to flamegraph — no series fetch needed
        core.process_event(Event::PyroscopeDirectLoad {
            service_name: "my-svc".into(),
            profile_type: "process_cpu:cpu:nanoseconds:cpu:nanoseconds".into(),
        });

        let vm = core.view();
        // Correct datasource active
        assert_eq!(vm.datasources[vm.selected_index].uid, "uid4");
        assert_eq!(vm.datasources[vm.selected_index].id, 4);
        // Time range from history preserved
        assert_eq!(vm.time_range, "2h");
        // Landed directly on Flamegraph sub-screen
        assert_eq!(vm.pyroscope_sub_screen, PyroscopeSubScreenView::Flamegraph);
        // Flamegraph is loading (TUI will spawn the fetch)
        assert!(vm.pyroscope_flamegraph_loading);
        // Series are NOT loading
        assert!(!vm.pyroscope_series_loading);
        // Correct service and profile type
        assert_eq!(vm.pyroscope_selected_service, "my-svc");
        assert_eq!(
            vm.pyroscope_selected_profile_type,
            "process_cpu:cpu:nanoseconds:cpu:nanoseconds"
        );
    }

    #[test]
    fn select_datasource_by_uid_uses_correct_ds_despite_favourites() {
        // Regression: when a favourite reorders the sorted view, restoring a
        // history entry by UID must still fire HTTP requests to the correct
        // datasource, not to whichever datasource happens to sit at the same
        // *sorted* position.
        //
        // Setup: after filtering, model.datasources = [prom1(id=1), prom2(id=3)].
        // prom2 (uid3) is marked as favourite → sorted view = [prom2, prom1].
        // SelectDatasource { uid: "uid1" } targets prom1 (raw index 0).
        // EnterQuery must produce HTTP requests to /proxy/1/…, not /proxy/3/…
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://grafana".into(),
            token: "tok".into(),
        });
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));
        // Make prom2 the favourite — it now appears first in the sorted view.
        core.process_event(Event::FavouritesLoaded(vec!["uid3".into()]));

        core.process_event(Event::SelectDatasource {
            uid: "uid1".into(),
            name: "Prometheus".into(),
        });
        let urls = http_urls(&core.process_event(Event::EnterQuery));

        assert!(
            urls.iter().any(|u| u.contains("/proxy/1/")),
            "expected requests to prom1 (id=1), got: {urls:?}"
        );
        assert!(
            urls.iter().all(|u| !u.contains("/proxy/3/")),
            "must not query prom2 (id=3), got: {urls:?}"
        );
    }

    #[test]
    fn select_datasource_name_fallback_for_empty_uid() {
        // Regression: history entries written before uid tracking was added have
        // an empty datasource_uid.  SelectDatasource must fall back to name
        // matching so the correct datasource is used.
        //
        // The cursor starts on prom1 (index 0).  We request prom2 by name with
        // an empty uid.  EnterQuery must target prom2 (id=3).
        let core = make_core();
        core.process_event(Event::Configure {
            url: "http://grafana".into(),
            token: "tok".into(),
        });
        let response = ResponseBuilder::ok().body(make_datasources()).build();
        core.process_event(Event::DatasourcesLoaded(Ok(response)));

        core.process_event(Event::SelectDatasource {
            uid: String::new(), // empty – simulates a pre-uid history entry
            name: "Prometheus2".into(),
        });
        let urls = http_urls(&core.process_event(Event::EnterQuery));

        assert!(
            urls.iter().any(|u| u.contains("/proxy/3/")),
            "expected prom2 (id=3) via name fallback, got: {urls:?}"
        );
        assert!(
            urls.iter().all(|u| !u.contains("/proxy/1/")),
            "must not query prom1 (id=1), got: {urls:?}"
        );
    }
}
