mod core;
mod highlight;
mod http;
mod loki;
mod prometheus;
mod pyroscope;
mod tempo;

use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;
use log::info;
use serde::{Deserialize, Serialize};
use simplelog::{Config as LogConfig, LevelFilter, WriteLogger};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CrosstermEvent, EventStream, KeyCode,
    KeyModifiers,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
    DefaultTerminal, Frame,
};
use shared::{Event, HistoryEntryView, PyroscopeSubScreenView, ScreenView, ViewModel};
use tui_textarea;
use tokio::{sync::mpsc, time::Instant};

use crate::core::AppCore;

#[derive(Parser, Debug)]
#[command(name = "lptm", about = "Explore Grafana data from your terminal")]
struct Args {
    /// Grafana base URL (e.g. http://localhost:3000)
    #[arg(long, env = "GRAFANA_URL")]
    grafana_url: String,

    /// Grafana service account token
    #[arg(long, env = "GRAFANA_TOKEN")]
    grafana_token: String,
}

/// Initialise file logging under `~/.config/lptm/`.
/// Errors are non-fatal: the app runs without logging if setup fails.
fn lptm_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("lptm"))
}

fn setup_logging() {
    let Some(dir) = lptm_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let log_path = dir.join("debug.log");
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = WriteLogger::init(LevelFilter::Debug, LogConfig::default(), file);
    }
}

fn history_path() -> Option<std::path::PathBuf> {
    lptm_dir().map(|d| d.join("history"))
}

fn favourites_path() -> Option<std::path::PathBuf> {
    lptm_dir().map(|d| d.join("favourites.json"))
}

fn load_favourites(url_hash: &str) -> Vec<String> {
    let Some(path) = favourites_path() else { return vec![] };
    let Ok(content) = std::fs::read_to_string(&path) else { return vec![] };
    let Ok(map) = serde_json::from_str::<HashMap<String, Vec<String>>>(&content) else {
        return vec![];
    };
    map.get(url_hash).cloned().unwrap_or_default()
}

fn save_favourites(url_hash: &str, uids: &[String]) {
    let Some(path) = favourites_path() else { return };
    let mut map: HashMap<String, Vec<String>> =
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
    if uids.is_empty() {
        map.remove(url_hash);
    } else {
        map.insert(url_hash.to_string(), uids.to_vec());
    }
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, json);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    query: String,
    datasource_name: String,
    datasource_type: String,
    #[serde(default)]
    datasource_uid: String,
    grafana_url_hash: String,
    /// Unix timestamp (seconds since epoch) when the query was executed.
    #[serde(default)]
    timestamp: u64,
    // Pyroscope-specific (absent for Prometheus entries).
    #[serde(default)]
    service_name: Option<String>,
    #[serde(default)]
    profile_type: Option<String>,
    #[serde(default)]
    time_range: Option<String>,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_time_ago(ts: u64) -> String {
    let now = now_unix_secs();
    let secs = now.saturating_sub(ts);
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn history_views(all: &[HistoryEntry], url_hash: &str) -> Vec<HistoryEntryView> {
    all.iter()
        .rev()
        .filter(|e| e.grafana_url_hash == url_hash)
        .take(100)
        .map(|e| HistoryEntryView {
            query: e.query.clone(),
            datasource_name: e.datasource_name.clone(),
            datasource_uid: e.datasource_uid.clone(),
            datasource_type: e.datasource_type.clone(),
            time_ago: format_time_ago(e.timestamp),
            service_name: e.service_name.clone().unwrap_or_default(),
            profile_type: e.profile_type.clone().unwrap_or_default(),
            time_range: e.time_range.clone().unwrap_or_default(),
        })
        .collect()
}

/// FNV-1a hash of a URL — stable, no extra dependencies.
fn grafana_url_hash(url: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in url.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn load_history() -> Vec<HistoryEntry> {
    let Some(path) = history_path() else {
        return vec![];
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        Err(_) => vec![],
    }
}

fn append_history(entry: &HistoryEntry) {
    if entry.query.trim().is_empty() {
        return;
    }
    let Some(path) = history_path() else {
        return;
    };
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{}", line);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();
    let args = Args::parse();

    info!("lptm starting – url={}", args.grafana_url);
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stderr(), EnableMouseCapture);
    let result = run(&mut terminal, args).await;
    let _ = execute!(std::io::stderr(), DisableMouseCapture);
    ratatui::restore();
    info!("lptm exiting");
    result
}

enum TempoMsg {
    Result(Result<Vec<shared::TempoTrace>, String>),
    #[allow(dead_code)]
    SpanTraceResult { span_id: String, trace_id: Option<String> },
    SpanTraceBatch(std::collections::HashMap<String, String>),
    TraceDetail(Result<Vec<shared::TempoSpan>, String>),
    SpanTraceLoadingDone,
}

enum LokiMsg {
    Result(Result<Vec<shared::loki::LokiStream>, String>),
    ContextResult(Result<(Vec<shared::loki::LokiEntry>, usize), String>),
}

enum PyroscopeMsg {
    Series(Result<Vec<(String, String)>, String>),
    Flamegraph(Result<Option<shared::pyroscope::FlameGraph>, String>),
    Timeline(Result<Vec<shared::pyroscope::TimelineSeries>, String>),
    Heatmap(Result<Vec<shared::pyroscope::HeatmapSlot>, String>),
    SpanHeatmap(Result<Vec<shared::pyroscope::HeatmapSlot>, String>),
}

async fn run(terminal: &mut DefaultTerminal, args: Args) -> Result<()> {
    let (render_tx, mut render_rx) = mpsc::unbounded_channel::<()>();
    let app_core = AppCore::new(render_tx);

    let url_hash = grafana_url_hash(&args.grafana_url);

    let grafana_url = args.grafana_url.clone();
    let grafana_token = args.grafana_token.clone();

    app_core.update(Event::Configure {
        url: args.grafana_url,
        token: args.grafana_token,
    });
    app_core.update(Event::FavouritesLoaded(load_favourites(&url_hash)));

    let mut reader = EventStream::new();

    let mut all_history = load_history();
    app_core.update(Event::HistoryEntriesLoaded(history_views(&all_history, &url_hash)));
    let mut history: Vec<String> = vec![];
    let mut history_pos: Option<usize> = None;
    let mut history_saved_query = String::new();

    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
    let mut debounce_deadline: Option<Instant> = None;

    let (pyroscope_tx, mut pyroscope_rx) = mpsc::unbounded_channel::<PyroscopeMsg>();
    let (tempo_tx, mut tempo_rx) = mpsc::unbounded_channel::<TempoMsg>();
    let (loki_tx, mut loki_rx) = mpsc::unbounded_channel::<LokiMsg>();

    // Loki LSP + UI state (created on first EnterLoki).
    let mut loki_ui_state: Option<loki::LokiUiState> = None;
    let mut loki_lsp: Option<loki::LspClient> = None;
    let mut loki_lsp_notif_rx: Option<mpsc::UnboundedReceiver<loki::LspNotification>> = None;
    // Channel carrying fetched label data back to the main loop.
    let (loki_labels_tx, mut loki_labels_rx) =
        mpsc::unbounded_channel::<(String, Vec<String>)>();          // (selector, names)
    let (loki_label_values_tx, mut loki_label_values_rx) =
        mpsc::unbounded_channel::<(String, String, Vec<String>)>();  // (label, selector, values)
    // The URI we use for the single Loki query document.
    let loki_doc_uri = "file:///loki/query.logql";

    macro_rules! draw {
        ($term:expr, $vm:expr, $loki_state:expr) => {{
            let ls: Option<&loki::LokiUiState> = $loki_state;
            $term.draw(|frame| { ui(frame, $vm, ls); })?;
        }};
    }

    loop {
        tokio::select! {
            Some(msg) = pyroscope_rx.recv() => {
                match msg {
                    PyroscopeMsg::Series(result) => {
                        app_core.update(Event::PyroscopeSeriesLoaded(result));
                    }
                    PyroscopeMsg::Flamegraph(result) => {
                        app_core.update(Event::PyroscopeFlamegraphLoaded(result));
                    }
                    PyroscopeMsg::Timeline(result) => {
                        app_core.update(Event::PyroscopeTimelineLoaded(result));
                    }
                    PyroscopeMsg::Heatmap(result) => {
                        app_core.update(Event::PyroscopeHeatmapLoaded(result));
                    }
                    PyroscopeMsg::SpanHeatmap(result) => {
                        app_core.update(Event::PyroscopeSpanHeatmapLoaded(result));
                    }
                }
            }
            Some(msg) = tempo_rx.recv() => {
                match msg {
                    TempoMsg::Result(result) => {
                        app_core.update(Event::TempoResultLoaded(result));
                    }
                    TempoMsg::SpanTraceResult { span_id, trace_id } => {
                        app_core.update(Event::SpanHeatmapTempoResultLoaded {
                            span_id,
                            trace_id: trace_id.unwrap_or_default(),
                        });
                    }
                    TempoMsg::SpanTraceBatch(map) => {
                        app_core.update(Event::SpanHeatmapTempoBatchLoaded(map));
                    }
                    TempoMsg::SpanTraceLoadingDone => {
                        app_core.update(Event::SpanHeatmapTempoLoadingDone);
                    }
                    TempoMsg::TraceDetail(result) => {
                        app_core.update(Event::TempoTraceDetailLoaded(result));
                    }
                }
            }
            Some(msg) = loki_rx.recv() => {
                match msg {
                    LokiMsg::Result(result) => {
                        app_core.update(Event::LokiResultLoaded(result));
                    }
                    LokiMsg::ContextResult(result) => {
                        app_core.update(Event::LokiContextLoaded(result));
                    }
                }
            }
            // Label names fetched from the Loki API — forward to LSP server.
            Some((selector, names)) = loki_labels_rx.recv() => {
                if let Some(ref lsp) = loki_lsp {
                    lsp.push_labels(&selector, names).await;
                }
            }
            // Label values fetched from the Loki API — forward to LSP server.
            Some((label, selector, values)) = loki_label_values_rx.recv() => {
                if let Some(ref lsp) = loki_lsp {
                    lsp.push_label_values(&label, &selector, values).await;
                }
            }
            // LSP notifications from the language server (diagnostics etc.).
            Some(notif) = async {
                match loki_lsp_notif_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match notif {
                    loki::LspNotification::Diagnostics { diagnostics, .. } => {
                        if let Some(ref mut state) = loki_ui_state {
                            state.diagnostics = diagnostics.clone();
                        }
                        app_core.update(Event::LokiDiagnosticsUpdated(diagnostics));
                    }
                }
            }
            () = async {
                match debounce_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None    => std::future::pending().await,
                }
            } => {
                debounce_deadline = None;
                let vm = app_core.core.view();
                match vm.screen {
                    ScreenView::LokiMode => {
                        // Completions are requested inline on each keystroke via LSP.
                    }
                    _ => {
                        app_core.update(Event::TriggerCompletions);
                    }
                }
                drop(vm);
            }
            maybe_event = reader.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                match event {
                    CrosstermEvent::Key(key) => {
                        let vm = app_core.core.view();
                        match vm.screen {
                            ScreenView::DatasourceList => {
                                match (key.code, key.modifiers) {
                                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                        break;
                                    }
                                    (KeyCode::Esc, _) => {
                                        if vm.history_panel_focused {
                                            app_core.update(Event::HistoryPanelBlur);
                                        } else if vm.datasource_filter_focused
                                            || !vm.datasource_filter.is_empty()
                                        {
                                            app_core.update(Event::DatasourceFilterClear);
                                        } else {
                                            break;
                                        }
                                    }
                                    // ── History panel toggle ─────────────────
                                    (KeyCode::Tab, _) => {
                                        if vm.history_panel_focused {
                                            app_core.update(Event::HistoryPanelBlur);
                                        } else {
                                            app_core.update(Event::HistoryPanelFocus);
                                        }
                                    }
                                    // ── History panel navigation ─────────────
                                    _ if vm.history_panel_focused => {
                                        match key.code {
                                            KeyCode::Char('j') | KeyCode::Down => {
                                                app_core.update(Event::HistorySelectNext);
                                            }
                                            KeyCode::Char('k') | KeyCode::Up => {
                                                app_core.update(Event::HistorySelectPrev);
                                            }
                                            KeyCode::Enter => {
                                                let vm2 = app_core.core.view();
                                                if let Some(entry) = vm2
                                                    .history_entries
                                                    .get(vm2.history_selected_index)
                                                    .cloned()
                                                {
                                                    // SelectDatasource clears any active
                                                    // datasource filter, so the full list is
                                                    // available in the view() taken afterwards.
                                                    app_core.update(Event::SelectDatasource {
                                                        uid: entry.datasource_uid.clone(),
                                                        name: entry.datasource_name.clone(),
                                                    });
                                                    if entry.datasource_type == "pyroscope" {
                                                        let now = now_unix_secs() as i64 * 1000;
                                                        if !entry.time_range.is_empty() {
                                                            app_core.update(Event::PyroscopeSetTimeRange(
                                                                entry.time_range.clone(),
                                                            ));
                                                        }
                                                        if !entry.service_name.is_empty()
                                                            && !entry.profile_type.is_empty()
                                                        {
                                                            // We know exactly what to load — skip
                                                            // the series fetch and go straight to
                                                            // the flamegraph.
                                                            app_core.update(Event::PyroscopeDirectLoad {
                                                                service_name: entry.service_name.clone(),
                                                                profile_type: entry.profile_type.clone(),
                                                            });
                                                        } else {
                                                            // Old history entry without
                                                            // service/profile — fall back to series
                                                            // list.
                                                            app_core.update(Event::EnterPyroscope {
                                                                now_unix_ms: now,
                                                            });
                                                        }
                                                        // Look up the datasource after events
                                                        // (filter is cleared by now).
                                                        let vm3 = app_core.core.view();
                                                        let ds = vm3.datasources.iter().find(|d| {
                                                            (!entry.datasource_uid.is_empty()
                                                                && d.uid == entry.datasource_uid)
                                                                || (entry.datasource_uid.is_empty()
                                                                    && d.name
                                                                        == entry.datasource_name)
                                                        }).cloned();
                                                        if let Some(ds) = ds {
                                                            if !entry.service_name.is_empty()
                                                                && !entry.profile_type.is_empty()
                                                            {
                                                                if let Ok(size) = terminal.size() {
                                                                    app_core.update(Event::FlamegraphViewportChars(
                                                                        size.width.saturating_sub(2) as u64,
                                                                    ));
                                                                }
                                                                spawn_flamegraph_fetch(
                                                                    &pyroscope_tx,
                                                                    grafana_url.clone(),
                                                                    ds.id,
                                                                    grafana_token.clone(),
                                                                    entry.profile_type.clone(),
                                                                    entry.service_name.clone(),
                                                                    vm3.pyroscope_time_range.clone(),
                                                                    now,
                                                                );
                                                            } else {
                                                                spawn_series_fetch(
                                                                    &pyroscope_tx,
                                                                    grafana_url.clone(),
                                                                    ds.id,
                                                                    grafana_token.clone(),
                                                                    vm3.pyroscope_time_range.clone(),
                                                                    now,
                                                                );
                                                            }
                                                        }
                                                    } else if entry.datasource_type == "tempo" {
                                                        app_core.update(Event::EnterTempo);
                                                        if !entry.query.is_empty() {
                                                            for c in entry.query.chars() {
                                                                app_core.update(Event::TempoQueryInput(c));
                                                            }
                                                            let ds_uid = app_core.core.view()
                                                                .datasources
                                                                .iter()
                                                                .find(|d| {
                                                                    (!entry.datasource_uid.is_empty() && d.uid == entry.datasource_uid)
                                                                        || (entry.datasource_uid.is_empty() && d.name == entry.datasource_name)
                                                                })
                                                                .map(|d| d.uid.clone());
                                                            if let Some(uid) = ds_uid {
                                                                app_core.update(Event::TempoExecuteQuery);
                                                                spawn_tempo_search(&tempo_tx, grafana_url.clone(), uid, grafana_token.clone(), entry.query.clone());
                                                            }
                                                        }
                                                    } else if entry.datasource_type == "loki" {
                                                        app_core.update(Event::EnterLoki);
                                                        init_loki_state(
                                                            &mut loki_ui_state,
                                                            &mut loki_lsp,
                                                            &mut loki_lsp_notif_rx,
                                                            loki_doc_uri,
                                                            &grafana_url,
                                                            &grafana_token,
                                                            app_core.core.view().selected_index,
                                                            &app_core.core.view().datasources,
                                                            &loki_labels_tx,
                                                        ).await;
                                                        if !entry.query.is_empty() {
                                                            if let Some(ref mut state) = loki_ui_state {
                                                                state.textarea = loki::styled_textarea();
                                                                for c in entry.query.chars() {
                                                                    state.textarea.insert_char(c);
                                                                }
                                                                state.query_dirty = true;
                                                                app_core.update(Event::LokiQueryChanged(entry.query.clone()));
                                                            }
                                                            let ds_id = app_core.core.view()
                                                                .datasources
                                                                .iter()
                                                                .find(|d| {
                                                                    (!entry.datasource_uid.is_empty() && d.uid == entry.datasource_uid)
                                                                        || (entry.datasource_uid.is_empty() && d.name == entry.datasource_name)
                                                                })
                                                                .map(|d| d.id);
                                                            if let Some(ds_id) = ds_id {
                                                                if let Some(ref mut state) = loki_ui_state {
                                                                    state.query_dirty = false;
                                                                }
                                                                app_core.update(Event::LokiExecuteQuery);
                                                                spawn_loki_query(&loki_tx, grafana_url.clone(), ds_id, grafana_token.clone(), entry.query.clone());
                                                            }
                                                        }
                                                    } else {
                                                        // Prometheus: enter query mode with
                                                        // the historical query pre-filled.
                                                        history_pos = None;
                                                        history = all_history
                                                            .iter()
                                                            .filter(|e| {
                                                                e.grafana_url_hash == url_hash
                                                                    && e.datasource_name
                                                                        == entry.datasource_name
                                                                    && e.datasource_type
                                                                        == entry.datasource_type
                                                            })
                                                            .map(|e| e.query.clone())
                                                            .collect();
                                                        app_core.update(Event::EnterQuery);
                                                        app_core.update(Event::HistoryNavigate(
                                                            entry.query.clone(),
                                                        ));
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    // ── Filter focused mode ──────────────────
                                    _ if vm.datasource_filter_focused => {
                                        match key.code {
                                            KeyCode::Enter => {
                                                app_core.update(Event::DatasourceFilterBlur);
                                            }
                                            KeyCode::Backspace => {
                                                app_core.update(Event::DatasourceFilterBackspace);
                                            }
                                            KeyCode::Char(c) => {
                                                app_core.update(Event::DatasourceFilterInput(c));
                                            }
                                            _ => {}
                                        }
                                    }
                                    // ── Navigation mode ──────────────────────
                                    (KeyCode::Char('/'), _) => {
                                        app_core.update(Event::DatasourceFilterFocus);
                                    }
                                    (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                        app_core.update(Event::SelectNext);
                                    }
                                    (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                        app_core.update(Event::SelectPrevious);
                                    }
                                    (KeyCode::Char('f'), _) => {
                                        if let Some(ds) = vm.datasources.get(vm.selected_index) {
                                            let uid = ds.uid.clone();
                                            app_core.update(Event::ToggleFavourite(uid));
                                            let vm2 = app_core.core.view();
                                            let fav_uids: Vec<String> = vm2
                                                .datasources
                                                .iter()
                                                .filter(|d| d.is_favourite)
                                                .map(|d| d.uid.clone())
                                                .collect();
                                            save_favourites(&url_hash, &fav_uids);
                                        }
                                    }
                                    (KeyCode::Enter, _) => {
                                        if let Some(ds) = vm.datasources.get(vm.selected_index) {
                                            if ds.ds_type == "pyroscope" {
                                                let now = now_unix_secs() as i64 * 1000;
                                                app_core.update(Event::EnterPyroscope { now_unix_ms: now });
                                                // After the event the model has set pyroscope_time_range.
                                                let vm2 = app_core.core.view();
                                                spawn_series_fetch(
                                                    &pyroscope_tx,
                                                    grafana_url.clone(),
                                                    ds.id,
                                                    grafana_token.clone(),
                                                    vm2.pyroscope_time_range.clone(),
                                                    now,
                                                );
                                            } else if ds.ds_type == "tempo" {
                                                app_core.update(Event::EnterTempo);
                                            } else if ds.ds_type == "loki" {
                                                app_core.update(Event::EnterLoki);
                                                init_loki_state(
                                                    &mut loki_ui_state,
                                                    &mut loki_lsp,
                                                    &mut loki_lsp_notif_rx,
                                                    loki_doc_uri,
                                                    &grafana_url,
                                                    &grafana_token,
                                                    app_core.core.view().selected_index,
                                                    &app_core.core.view().datasources,
                                                    &loki_labels_tx,
                                                ).await;
                                            } else {
                                                history_pos = None;
                                                let ds_name = ds.name.clone();
                                                let ds_type = ds.ds_type.clone();
                                                history = all_history
                                                    .iter()
                                                    .filter(|e| {
                                                        e.grafana_url_hash == url_hash
                                                            && e.datasource_name == ds_name
                                                            && e.datasource_type == ds_type
                                                    })
                                                    .map(|e| e.query.clone())
                                                    .collect();
                                                app_core.update(Event::EnterQuery);
                                            }
                                        } else {
                                            history_pos = None;
                                            app_core.update(Event::EnterQuery);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ScreenView::QueryMode => {
                                match (key.code, key.modifiers) {
                                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                        break;
                                    }
                                    (KeyCode::Esc, _) => {
                                        history_pos = None;
                                        if !vm.completions.is_empty() {
                                            app_core.update(Event::CompletionDismiss);
                                        } else {
                                            app_core.update(Event::BackToDatasources);
                                        }
                                    }
                                    (KeyCode::Tab, _) => {
                                        app_core.update(Event::CompletionAccept);
                                    }
                                    (KeyCode::Down, _) if !vm.completions.is_empty() => {
                                        app_core.update(Event::CompletionNext);
                                    }
                                    (KeyCode::Up, _) if !vm.completions.is_empty() => {
                                        app_core.update(Event::CompletionPrev);
                                    }
                                    (KeyCode::Up, _) => {
                                        if !history.is_empty() {
                                            let new_pos = match history_pos {
                                                None => {
                                                    history_saved_query = vm.query.clone();
                                                    history.len() - 1
                                                }
                                                Some(0) => 0,
                                                Some(i) => i - 1,
                                            };
                                            history_pos = Some(new_pos);
                                            app_core.update(Event::HistoryNavigate(
                                                history[new_pos].clone(),
                                            ));
                                        }
                                    }
                                    (KeyCode::Down, _) => {
                                        if let Some(pos) = history_pos {
                                            if pos + 1 < history.len() {
                                                let new_pos = pos + 1;
                                                history_pos = Some(new_pos);
                                                app_core.update(Event::HistoryNavigate(
                                                    history[new_pos].clone(),
                                                ));
                                            } else {
                                                history_pos = None;
                                                app_core.update(Event::HistoryNavigate(
                                                    history_saved_query.clone(),
                                                ));
                                            }
                                        }
                                    }
                                    (KeyCode::Enter, _) => {
                                        let query = vm.query.clone();
                                        if !query.trim().is_empty() {
                                            let is_dup = history
                                                .last()
                                                .map(|s| s == &query)
                                                .unwrap_or(false);
                                            if !is_dup {
                                                if let Some(ds) =
                                                    vm.datasources.get(vm.selected_index)
                                                {
                                                    let entry = HistoryEntry {
                                                        query: query.clone(),
                                                        datasource_name: ds.name.clone(),
                                                        datasource_uid: ds.uid.clone(),
                                                        datasource_type: ds.ds_type.clone(),
                                                        grafana_url_hash: url_hash.clone(),
                                                        timestamp: now_unix_secs(),
                                                        service_name: None,
                                                        profile_type: None,
                                                        time_range: None,
                                                    };
                                                    history.push(query.clone());
                                                    append_history(&entry);
                                                    all_history.push(entry);
                                                    app_core.update(Event::HistoryEntriesLoaded(
                                                        history_views(&all_history, &url_hash),
                                                    ));
                                                }
                                            }
                                        }
                                        history_pos = None;
                                        app_core.update(Event::ExecuteQuery);
                                    }
                                    (KeyCode::Backspace, _) => {
                                        history_pos = None;
                                        app_core.update(Event::QueryBackspace);
                                        debounce_deadline = Some(Instant::now() + DEBOUNCE);
                                    }
                                    (KeyCode::Delete, _) => {
                                        history_pos = None;
                                        app_core.update(Event::QueryDelete);
                                    }
                                    (KeyCode::Left, _) => {
                                        app_core.update(Event::CursorLeft);
                                    }
                                    (KeyCode::Right, _) => {
                                        app_core.update(Event::CursorRight);
                                    }
                                    (KeyCode::Home, _) => {
                                        app_core.update(Event::CursorHome);
                                    }
                                    (KeyCode::End, _) => {
                                        app_core.update(Event::CursorEnd);
                                    }
                                    (KeyCode::Char(c), _) => {
                                        history_pos = None;
                                        app_core.update(Event::QueryInput(c));
                                        debounce_deadline = Some(Instant::now() + DEBOUNCE);
                                    }
                                    _ => {}
                                }
                            }
                            ScreenView::PyroscopeMode => {
                                match (key.code, key.modifiers) {
                                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                                    _ => {}
                                }
                                match vm.pyroscope_sub_screen {
                                    PyroscopeSubScreenView::ServiceList => {
                                        if vm.pyroscope_time_range_editing {
                                            match key.code {
                                                KeyCode::Esc => {
                                                    app_core.update(Event::PyroscopeTimeRangeAbort);
                                                }
                                                KeyCode::Enter => {
                                                    let now = now_unix_secs() as i64 * 1000;
                                                    app_core.update(Event::PyroscopeTimeRangeCommit { now_unix_ms: now });
                                                    // After commit the model has the updated time range.
                                                    if let Some(ds) = app_core.core.view().datasources.get(app_core.core.view().selected_index) {
                                                        let vm2 = app_core.core.view();
                                                        spawn_series_fetch(
                                                            &pyroscope_tx,
                                                            grafana_url.clone(),
                                                            ds.id,
                                                            grafana_token.clone(),
                                                            vm2.pyroscope_time_range.clone(),
                                                            now,
                                                        );
                                                    }
                                                }
                                                KeyCode::Backspace => {
                                                    app_core.update(Event::PyroscopeTimeRangeBackspace);
                                                }
                                                KeyCode::Char(c) => {
                                                    app_core.update(Event::PyroscopeTimeRangeInput(c));
                                                }
                                                _ => {}
                                            }
                                        } else {
                                            match key.code {
                                                KeyCode::Esc => {
                                                    if vm.pyroscope_service_filter_focused
                                                        || !vm.pyroscope_service_filter.is_empty()
                                                    {
                                                        app_core.update(Event::PyroscopeServiceFilterClear);
                                                    } else {
                                                        app_core.update(Event::BackFromPyroscope);
                                                    }
                                                }
                                                // ── Profile type dropdown open ───────
                                                _ if vm.pyroscope_profile_type_dropdown_open => {
                                                    match key.code {
                                                        KeyCode::Char('j') | KeyCode::Down => {
                                                            app_core.update(Event::PyroscopeProfileTypeNext);
                                                        }
                                                        KeyCode::Char('k') | KeyCode::Up => {
                                                            app_core.update(Event::PyroscopeProfileTypePrev);
                                                        }
                                                        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('p') => {
                                                            app_core.update(Event::PyroscopeProfileTypeDropdownClose);
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                                // ── Filter focused mode ──────────────
                                                _ if vm.pyroscope_service_filter_focused => {
                                                    match key.code {
                                                        KeyCode::Enter => {
                                                            app_core.update(Event::PyroscopeServiceFilterBlur);
                                                        }
                                                        KeyCode::Backspace => {
                                                            app_core.update(Event::PyroscopeServiceFilterBackspace);
                                                        }
                                                        KeyCode::Char(c) => {
                                                            app_core.update(Event::PyroscopeServiceFilterInput(c));
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                                // ── Navigation mode ──────────────────
                                                KeyCode::Char('/') => {
                                                    app_core.update(Event::PyroscopeServiceFilterFocus);
                                                }
                                                KeyCode::Char('j') | KeyCode::Down => {
                                                    app_core.update(Event::PyroscopeSeriesNext);
                                                }
                                                KeyCode::Char('k') | KeyCode::Up => {
                                                    app_core.update(Event::PyroscopeSeriesPrev);
                                                }
                                                KeyCode::Char('t') => {
                                                    app_core.update(Event::PyroscopeTimeRangeEdit);
                                                }
                                                KeyCode::Char('p') => {
                                                    app_core.update(Event::PyroscopeProfileTypeDropdownOpen);
                                                }
                                                KeyCode::Enter => {
                                                    let now = now_unix_secs() as i64 * 1000;
                                                    // Read the selected series before firing the event.
                                                    let selected = vm.pyroscope_series
                                                        .get(vm.pyroscope_series_index)
                                                        .cloned();
                                                    let ds = vm.datasources.get(vm.selected_index).cloned();
                                                    let ds_id = ds.as_ref().map(|d| d.id);
                                                    let time_range = vm.pyroscope_time_range.clone();

                                                    // Record history for Pyroscope selection.
                                                    if let (Some((ref service, ref pt)), Some(ref ds)) = (&selected, &ds) {
                                                        let entry = HistoryEntry {
                                                            query: format!("{}:{}", service, pt),
                                                            datasource_name: ds.name.clone(),
                                                            datasource_uid: ds.uid.clone(),
                                                            datasource_type: ds.ds_type.clone(),
                                                            grafana_url_hash: url_hash.clone(),
                                                            timestamp: now_unix_secs(),
                                                            service_name: Some(service.clone()),
                                                            profile_type: Some(pt.clone()),
                                                            time_range: Some(time_range.clone()),
                                                        };
                                                        append_history(&entry);
                                                        all_history.push(entry);
                                                        app_core.update(Event::HistoryEntriesLoaded(
                                                            history_views(&all_history, &url_hash),
                                                        ));
                                                    }

                                                    app_core.update(Event::PyroscopeSelectSeries { now_unix_ms: now });
                                                    if let Ok(size) = terminal.size() {
                                                        app_core.update(Event::FlamegraphViewportChars(size.width.saturating_sub(2) as u64));
                                                    }
                                                    if let (Some((service, profile_type)), Some(ds_id)) = (selected, ds_id) {
                                                        spawn_flamegraph_fetch(
                                                            &pyroscope_tx,
                                                            grafana_url.clone(),
                                                            ds_id,
                                                            grafana_token.clone(),
                                                            profile_type,
                                                            service,
                                                            time_range,
                                                            now,
                                                        );
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    PyroscopeSubScreenView::Flamegraph
                                    | PyroscopeSubScreenView::Timeline
                                    | PyroscopeSubScreenView::ProfileHeatmap
                                    | PyroscopeSubScreenView::SpanHeatmap => {
                                        // Tempo datasource picker intercepts all keys when open.
                                        if vm.tempo_datasource_picker_open {
                                            match key.code {
                                                KeyCode::Esc => {
                                                    app_core.update(Event::SpanHeatmapCloseTempoPicker);
                                                }
                                                KeyCode::Char('j') | KeyCode::Down => {
                                                    app_core.update(Event::SpanHeatmapTempoPickerNext);
                                                }
                                                KeyCode::Char('k') | KeyCode::Up => {
                                                    app_core.update(Event::SpanHeatmapTempoPickerPrev);
                                                }
                                                KeyCode::Enter => {
                                                    let vm2 = app_core.core.view();
                                                    if let Some(ds) = vm2.tempo_datasources.get(vm2.tempo_picker_index).cloned() {
                                                        if let Some(hm) = vm2.span_heatmap.as_ref() {
                                                            let mut span_ids: Vec<String> = hm.exemplars.iter()
                                                                .map(|e| e.span_id.clone())
                                                                .filter(|s| !s.is_empty())
                                                                .collect();
                                                            span_ids.sort_unstable();
                                                            span_ids.dedup();
                                                            if !span_ids.is_empty() {
                                                                let query = format!(
                                                                    "{{{}}}",
                                                                    span_ids.iter()
                                                                        .map(|id| format!("span:id = \"{id}\""))
                                                                        .collect::<Vec<_>>()
                                                                        .join(" || ")
                                                                );
                                                                info!("tempo span lookup: {query}");
                                                                app_core.update(Event::SpanHeatmapClearResults);
                                                                app_core.update(Event::SpanHeatmapTempoLoadingStarted);
                                                                spawn_span_traces_fetch(
                                                                    &tempo_tx,
                                                                    grafana_url.clone(),
                                                                    ds.uid.clone(),
                                                                    grafana_token.clone(),
                                                                    span_ids,
                                                                    hm.start_ms,
                                                                    hm.end_ms,
                                                                );
                                                            }
                                                        }
                                                    }
                                                    app_core.update(Event::SpanHeatmapCloseTempoPicker);
                                                }
                                                _ => {}
                                            }
                                        } else if vm.pyroscope_sub_screen == PyroscopeSubScreenView::SpanHeatmap
                                            && vm.exemplar_detail_open
                                        {
                                            match (key.code, key.modifiers) {
                                                (KeyCode::Esc, _) | (KeyCode::Enter, _) => {
                                                    app_core.update(Event::ExemplarDetailClose);
                                                }
                                                (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                                                    let vm2 = app_core.core.view();
                                                    if let Some(exemplar) = vm2.span_heatmap.as_ref()
                                                        .and_then(|h| h.exemplars.get(vm2.pyroscope_exemplar_index))
                                                    {
                                                        let trace_id = vm2.span_trace_lookup.get(&exemplar.span_id).cloned().unwrap_or_default();
                                                        if !trace_id.is_empty() {
                                                            if let Some(ds) = vm2.tempo_datasources.first().cloned() {
                                                                app_core.update(Event::ExemplarDetailClose);
                                                                app_core.update(Event::SelectDatasource { uid: ds.uid.clone(), name: ds.name.clone() });
                                                                app_core.update(Event::EnterTempo);
                                                                for c in trace_id.chars() {
                                                                    app_core.update(Event::TempoQueryInput(c));
                                                                }
                                                                let vm3 = app_core.core.view();
                                                                app_core.update(Event::TempoExecuteQuery);
                                                                spawn_tempo_search(&tempo_tx, grafana_url.clone(), ds.uid, grafana_token.clone(), vm3.tempo_query.clone());
                                                            }
                                                        }
                                                    }
                                                }
                                                (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                                    let vm2 = app_core.core.view();
                                                    if let Some(exemplar) = vm2.span_heatmap.as_ref()
                                                        .and_then(|h| h.exemplars.get(vm2.pyroscope_exemplar_index))
                                                    {
                                                        if !exemplar.profile_id.is_empty() {
                                                            let ds_id = vm2.datasources.get(vm2.selected_index).map(|d| d.id);
                                                            let profile_id = exemplar.profile_id.clone();
                                                            let profile_type = vm2.pyroscope_selected_profile_type.clone();
                                                            let service = vm2.pyroscope_selected_service.clone();
                                                            let time_range = vm2.pyroscope_time_range.clone();
                                                            let now = now_unix_secs() as i64 * 1000;
                                                            app_core.update(Event::ExemplarDetailClose);
                                                            app_core.update(Event::PyroscopeDirectLoad {
                                                                service_name: service.clone(),
                                                                profile_type: profile_type.clone(),
                                                            });
                                                            if let Ok(size) = terminal.size() {
                                                                app_core.update(Event::FlamegraphViewportChars(size.width.saturating_sub(2) as u64));
                                                            }
                                                            if let Some(ds_id) = ds_id {
                                                                spawn_flamegraph_by_profile_id(
                                                                    &pyroscope_tx,
                                                                    grafana_url.clone(),
                                                                    ds_id,
                                                                    grafana_token.clone(),
                                                                    profile_type,
                                                                    service,
                                                                    time_range,
                                                                    now,
                                                                    profile_id,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                _ => {}
                                            }
                                        } else if vm.pyroscope_sub_screen == PyroscopeSubScreenView::SpanHeatmap
                                            && key.code == KeyCode::Char('t')
                                            && key.modifiers.contains(KeyModifiers::CONTROL)
                                        {
                                            if !vm.tempo_datasources.is_empty() {
                                                app_core.update(Event::SpanHeatmapOpenTempoPicker);
                                            }
                                        } else if vm.pyroscope_sub_screen == PyroscopeSubScreenView::SpanHeatmap
                                            && key.code == KeyCode::Enter
                                        {
                                            app_core.update(Event::ExemplarDetailOpen);
                                        } else {
                                        match (&vm.pyroscope_sub_screen, key.code) {
                                            // Back to service list
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Esc) if vm.sandwich_view.is_some() => {
                                                app_core.update(Event::FlameSandwichClear);
                                            }
                                            (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
                                                let ds_id = vm.datasources.get(vm.selected_index).map(|d| d.id);
                                                let time_range = vm.pyroscope_time_range.clone();
                                                let now = now_unix_secs() as i64 * 1000;
                                                app_core.update(Event::BackToServiceList);
                                                let vm2 = app_core.core.view();
                                                if vm2.pyroscope_series_loading {
                                                    if let Some(ds_id) = ds_id {
                                                        spawn_series_fetch(&pyroscope_tx, grafana_url.clone(), ds_id, grafana_token.clone(), time_range, now);
                                                    }
                                                }
                                            }
                                            // Tab cycles forward through views
                                            (_, KeyCode::Tab) => {
                                                let current = match vm.pyroscope_sub_screen {
                                                    PyroscopeSubScreenView::Flamegraph => 0,
                                                    PyroscopeSubScreenView::Timeline => 1,
                                                    PyroscopeSubScreenView::ProfileHeatmap => 2,
                                                    _ => 3,
                                                };
                                                let next = (current + 1) % 4;
                                                let ds_id = vm.datasources.get(vm.selected_index).map(|d| d.id);
                                                let service = vm.pyroscope_selected_service.clone();
                                                let profile_type = vm.pyroscope_selected_profile_type.clone();
                                                let time_range = vm.pyroscope_time_range.clone();
                                                let now = now_unix_secs() as i64 * 1000;
                                                app_core.update(Event::PyroscopeSelectView(next));
                                                let vm2 = app_core.core.view();
                                                let w = terminal.size().map(|s| s.width.saturating_sub(2)).unwrap_or(120);
                                                if vm2.pyroscope_timeline_loading {
                                                    if let Some(ds_id) = ds_id {
                                                        spawn_timeline_fetch(&pyroscope_tx, grafana_url.clone(), ds_id, grafana_token.clone(), profile_type.clone(), service.clone(), time_range.clone(), now, w);
                                                    }
                                                }
                                                // For heatmaps subtract the y-axis margin (label + tick)
                                                // so the step matches the actual graph width.
                                                let hm_w = w.saturating_sub(12);
                                                if vm2.pyroscope_heatmap_loading {
                                                    if let Some(ds_id) = ds_id {
                                                        spawn_heatmap_fetch(&pyroscope_tx, grafana_url.clone(), ds_id, grafana_token.clone(), profile_type.clone(), service.clone(), time_range.clone(), now, hm_w);
                                                    }
                                                }
                                                if vm2.pyroscope_span_heatmap_loading {
                                                    if let Some(ds_id) = ds_id {
                                                        spawn_span_heatmap_fetch(&pyroscope_tx, grafana_url.clone(), ds_id, grafana_token.clone(), profile_type, service, time_range, now, hm_w);
                                                    }
                                                }
                                            }
                                            // Flamegraph-only keys
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Left)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('h')) => {
                                                app_core.update(Event::FlameMoveLeft);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Right)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('l')) => {
                                                app_core.update(Event::FlameMoveRight);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Up)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('k')) => {
                                                app_core.update(Event::FlameMoveDown);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Down)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('j')) => {
                                                app_core.update(Event::FlameMoveUp);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Enter)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('z')) => {
                                                app_core.update(Event::FlameZoomIn);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Backspace)
                                            | (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('o')) => {
                                                app_core.update(Event::FlameZoomOut);
                                            }
                                            (PyroscopeSubScreenView::Flamegraph, KeyCode::Char('s')) => {
                                                app_core.update(Event::FlameSandwich);
                                            }
                                            // Exemplar navigation on Timeline/Heatmap screens
                                            (PyroscopeSubScreenView::Timeline, KeyCode::Down)
                                            | (PyroscopeSubScreenView::Timeline, KeyCode::Char('j'))
                                            | (PyroscopeSubScreenView::ProfileHeatmap, KeyCode::Down)
                                            | (PyroscopeSubScreenView::ProfileHeatmap, KeyCode::Char('j'))
                                            | (PyroscopeSubScreenView::SpanHeatmap, KeyCode::Down)
                                            | (PyroscopeSubScreenView::SpanHeatmap, KeyCode::Char('j')) => {
                                                app_core.update(Event::ExemplarSelectNext);
                                            }
                                            (PyroscopeSubScreenView::Timeline, KeyCode::Up)
                                            | (PyroscopeSubScreenView::Timeline, KeyCode::Char('k'))
                                            | (PyroscopeSubScreenView::ProfileHeatmap, KeyCode::Up)
                                            | (PyroscopeSubScreenView::ProfileHeatmap, KeyCode::Char('k'))
                                            | (PyroscopeSubScreenView::SpanHeatmap, KeyCode::Up)
                                            | (PyroscopeSubScreenView::SpanHeatmap, KeyCode::Char('k')) => {
                                                app_core.update(Event::ExemplarSelectPrev);
                                            }
                                            _ => {}
                                        }
                                        } // close else { match }
                                    }
                                }
                            }
                            ScreenView::TempoMode => {
                                match (key.code, key.modifiers) {
                                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                                    (KeyCode::Esc, _) => {
                                        app_core.update(Event::BackFromTempo);
                                    }
                                    // Results navigation — only when results exist
                                    (KeyCode::Char('j'), _) | (KeyCode::Down, _)
                                        if !vm.tempo_results.is_empty() =>
                                    {
                                        app_core.update(Event::TempoSelectNext);
                                    }
                                    (KeyCode::Char('k'), _) | (KeyCode::Up, _)
                                        if !vm.tempo_results.is_empty() =>
                                    {
                                        app_core.update(Event::TempoSelectPrev);
                                    }
                                    (KeyCode::Enter, _) if !vm.tempo_results.is_empty() => {
                                        // Open the selected trace in detail view
                                        app_core.update(Event::TempoOpenTrace);
                                        let vm3 = app_core.core.view();
                                        let ds_uid = vm3
                                            .datasources
                                            .get(vm3.selected_index)
                                            .map(|d| d.uid.clone());
                                        if let Some(uid) = ds_uid {
                                            spawn_trace_detail_fetch(
                                                &tempo_tx,
                                                grafana_url.clone(),
                                                uid,
                                                grafana_token.clone(),
                                                vm3.trace_detail_trace_id.clone(),
                                            );
                                        }
                                    }
                                    (KeyCode::Enter, _) => {
                                        let vm2 = app_core.core.view();
                                        if !vm2.tempo_query.trim().is_empty() {
                                            let ds_uid = vm2
                                                .datasources
                                                .get(vm2.selected_index)
                                                .map(|d| d.uid.clone());
                                            let query = vm2.tempo_query.clone();

                                            // Record history
                                            if let Some(ds) = vm2.datasources.get(vm2.selected_index) {
                                                let is_dup = all_history
                                                    .last()
                                                    .map(|e| e.query == query && e.datasource_uid == ds.uid)
                                                    .unwrap_or(false);
                                                if !is_dup {
                                                    let entry = HistoryEntry {
                                                        query: query.clone(),
                                                        datasource_name: ds.name.clone(),
                                                        datasource_uid: ds.uid.clone(),
                                                        datasource_type: ds.ds_type.clone(),
                                                        grafana_url_hash: url_hash.clone(),
                                                        timestamp: now_unix_secs(),
                                                        service_name: None,
                                                        profile_type: None,
                                                        time_range: None,
                                                    };
                                                    append_history(&entry);
                                                    all_history.push(entry);
                                                    app_core.update(Event::HistoryEntriesLoaded(
                                                        history_views(&all_history, &url_hash),
                                                    ));
                                                }
                                            }

                                            app_core.update(Event::TempoExecuteQuery);
                                            if let Some(uid) = ds_uid {
                                                spawn_tempo_search(&tempo_tx, grafana_url.clone(), uid, grafana_token.clone(), query);
                                            }
                                        }
                                    }
                                    (KeyCode::Backspace, _) => {
                                        app_core.update(Event::TempoQueryBackspace);
                                    }
                                    (KeyCode::Left, _) => {
                                        app_core.update(Event::TempoQueryCursorLeft);
                                    }
                                    (KeyCode::Right, _) => {
                                        app_core.update(Event::TempoQueryCursorRight);
                                    }
                                    (KeyCode::Char(c), _) => {
                                        app_core.update(Event::TempoQueryInput(c));
                                    }
                                    _ => {}
                                }
                            }
                            ScreenView::TempoTraceDetail => {
                                if vm.trace_detail_filter_focused {
                                    match key.code {
                                        KeyCode::Esc | KeyCode::Enter => {
                                            app_core.update(Event::TempoTraceDetailFilterBlur);
                                        }
                                        KeyCode::Backspace => {
                                            app_core.update(Event::TempoTraceDetailFilterBackspace);
                                        }
                                        KeyCode::Char(c) => {
                                            app_core.update(Event::TempoTraceDetailFilterInput(c));
                                        }
                                        _ => {}
                                    }
                                } else {
                                    match (key.code, key.modifiers) {
                                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                                        (KeyCode::Esc, _) => {
                                            app_core.update(Event::BackFromTraceDetail);
                                        }
                                        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                            app_core.update(Event::TempoTraceDetailNext);
                                        }
                                        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                            app_core.update(Event::TempoTraceDetailPrev);
                                        }
                                        (KeyCode::Char('J'), _) => {
                                            app_core.update(Event::TempoTraceDetailAttrScrollDown);
                                        }
                                        (KeyCode::Char('K'), _) => {
                                            app_core.update(Event::TempoTraceDetailAttrScrollUp);
                                        }
                                        (KeyCode::Char('/'), _) => {
                                            app_core.update(Event::TempoTraceDetailFilterFocus);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            ScreenView::LokiMode => {
                                let vm2 = app_core.core.view();
                                let context_open = vm2.loki_context_open;
                                let has_results = !vm2.loki_results.is_empty();
                                drop(vm2);

                                let has_completions = loki_ui_state
                                    .as_ref()
                                    .map(|s| !s.completions.is_empty() && !s.completion_dismissed)
                                    .unwrap_or(false);
                                let query_dirty = loki_ui_state
                                    .as_ref()
                                    .map(|s| s.query_dirty)
                                    .unwrap_or(true);

                                if context_open {
                                    match (key.code, key.modifiers) {
                                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                                        (KeyCode::Esc, _) => {
                                            app_core.update(Event::LokiCloseContext);
                                        }
                                        _ => {}
                                    }
                                } else {
                                    match (key.code, key.modifiers) {
                                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,

                                        (KeyCode::Esc, _) => {
                                            if has_completions {
                                                if let Some(ref mut s) = loki_ui_state {
                                                    s.completion_dismissed = true;
                                                }
                                            } else {
                                                app_core.update(Event::BackFromLoki);
                                            }
                                        }

                                        // Tab: accept top/selected completion.
                                        (KeyCode::Tab, _) => {
                                            if let Some(ref mut s) = loki_ui_state {
                                                accept_loki_completion(s);
                                                let text = s.query().to_string();
                                                app_core.update(Event::LokiQueryChanged(text.clone()));
                                                // Notify LSP of updated text.
                                                if let Some(ref lsp) = loki_lsp {
                                                    lsp.did_change(loki_doc_uri, &text, 0).await;
                                                }
                                            }
                                        }

                                        (KeyCode::Down, _) if has_completions => {
                                            if let Some(ref mut s) = loki_ui_state {
                                                let len = s.completions.len();
                                                s.completion_index = Some(
                                                    match s.completion_index {
                                                        None => 0,
                                                        Some(i) => (i + 1) % len,
                                                    },
                                                );
                                            }
                                        }
                                        (KeyCode::Up, _) if has_completions => {
                                            if let Some(ref mut s) = loki_ui_state {
                                                let len = s.completions.len();
                                                s.completion_index = Some(
                                                    match s.completion_index {
                                                        None => len - 1,
                                                        Some(0) => len - 1,
                                                        Some(i) => i - 1,
                                                    },
                                                );
                                            }
                                        }
                                        (KeyCode::Down, _) => {
                                            app_core.update(Event::LokiSelectNextRow);
                                        }
                                        (KeyCode::Up, _) => {
                                            app_core.update(Event::LokiSelectPrevRow);
                                        }

                                        (KeyCode::Enter, _) => {
                                            let query = loki_ui_state
                                                .as_ref()
                                                .map(|s| s.query().to_string())
                                                .unwrap_or_default();
                                            let query_empty = query.trim().is_empty();

                                            if has_results && !query_dirty {
                                                // Open context for the selected row.
                                                let vm2 = app_core.core.view();
                                                let selected_row = vm2.loki_selected_row;
                                                let mut entry_info = None;
                                                let mut flat_idx = 0;
                                                for stream in &vm2.loki_results {
                                                    for entry in &stream.entries {
                                                        if flat_idx == selected_row {
                                                            entry_info = Some((
                                                                stream.labels.clone(),
                                                                entry.timestamp_ns.clone(),
                                                            ));
                                                            break;
                                                        }
                                                        flat_idx += 1;
                                                    }
                                                    if entry_info.is_some() { break; }
                                                }
                                                let ds_id = vm2.datasources
                                                    .get(vm2.selected_index)
                                                    .map(|d| d.id);
                                                drop(vm2);

                                                if let (Some((labels, ts_ns)), Some(ds_id)) =
                                                    (entry_info, ds_id)
                                                {
                                                    app_core.update(Event::LokiOpenContext);
                                                    spawn_loki_context_query(
                                                        &loki_tx,
                                                        grafana_url.clone(),
                                                        ds_id,
                                                        grafana_token.clone(),
                                                        labels,
                                                        ts_ns,
                                                    );
                                                }
                                            } else if !query_empty {
                                                // Execute query.
                                                let vm2 = app_core.core.view();
                                                let ds_id = vm2.datasources
                                                    .get(vm2.selected_index)
                                                    .map(|d| d.id);

                                                if let Some(ds) = vm2.datasources.get(vm2.selected_index) {
                                                    let is_dup = all_history
                                                        .last()
                                                        .map(|e| e.query == query && e.datasource_uid == ds.uid)
                                                        .unwrap_or(false);
                                                    if !is_dup {
                                                        let entry = HistoryEntry {
                                                            query: query.clone(),
                                                            datasource_name: ds.name.clone(),
                                                            datasource_uid: ds.uid.clone(),
                                                            datasource_type: ds.ds_type.clone(),
                                                            grafana_url_hash: url_hash.clone(),
                                                            timestamp: now_unix_secs(),
                                                            service_name: None,
                                                            profile_type: None,
                                                            time_range: None,
                                                        };
                                                        append_history(&entry);
                                                        all_history.push(entry);
                                                        app_core.update(Event::HistoryEntriesLoaded(
                                                            history_views(&all_history, &url_hash),
                                                        ));
                                                    }
                                                }
                                                drop(vm2);

                                                if let Some(ref mut s) = loki_ui_state {
                                                    s.query_dirty = false;
                                                }
                                                app_core.update(Event::LokiQueryChanged(query.clone()));
                                                app_core.update(Event::LokiExecuteQuery);
                                                if let Some(ds_id) = ds_id {
                                                    spawn_loki_query(
                                                        &loki_tx,
                                                        grafana_url.clone(),
                                                        ds_id,
                                                        grafana_token.clone(),
                                                        query,
                                                    );
                                                }
                                            }
                                        }

                                        // All other keys go to tui-textarea.
                                        _ => {
                                            if let Some(ref mut s) = loki_ui_state {
                                                let modified = s.textarea.input(key);
                                                if modified {
                                                    s.query_dirty = true;
                                                    s.completion_dismissed = false;
                                                    s.completion_index = None;
                                                    let text = s.query().to_string();
                                                    app_core.update(Event::LokiQueryChanged(text.clone()));
                                                    // Notify LSP + request completions.
                                                    if let Some(ref lsp) = loki_lsp {
                                                        let col = s.cursor_char_col();
                                                        lsp.did_change(loki_doc_uri, &text, 0).await;
                                                        let completions = lsp
                                                            .completion(loki_doc_uri, 0, col)
                                                            .await;
                                                        s.completions = completions;
                                                        // Trigger label value fetch for value context.
                                                        maybe_fetch_label_values(
                                                            &text,
                                                            col as usize,
                                                            &grafana_url,
                                                            &grafana_token,
                                                            app_core.core.view().selected_index,
                                                            &app_core.core.view().datasources,
                                                            &loki_label_values_tx,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // Loki key events may change local state (completion index, dismissal,
                        // textarea cursor) without going through Crux, so always redraw.
                        if matches!(vm.screen, ScreenView::LokiMode) {
                            let vm2 = app_core.core.view();
                            draw!(terminal, &vm2, loki_ui_state.as_ref());
                        }
                    }
                    CrosstermEvent::Resize(w, _) => {
                        app_core.update(Event::FlamegraphViewportChars(w.saturating_sub(2) as u64));
                        let vm = app_core.core.view();
                        draw!(terminal, &vm, loki_ui_state.as_ref());
                    }
                    _ => {}
                }
            }
            Some(()) = render_rx.recv() => {
                let vm = app_core.core.view();
                draw!(terminal, &vm, loki_ui_state.as_ref());
            }
            _ = {
                let vm = app_core.core.view();
                let needs_fast = vm.span_trace_loading
                    || vm.pyroscope_span_heatmap_loading
                    || vm.pyroscope_heatmap_loading
                    || vm.pyroscope_timeline_loading
                    || vm.pyroscope_flamegraph_loading;
                let ms = if needs_fast { 100 } else { 500 };
                tokio::time::sleep(std::time::Duration::from_millis(ms))
            } => {
                // Drive spinner animation and blinking exemplar marker.
                let vm = app_core.core.view();
                let needs_redraw = vm.span_trace_loading
                    || matches!(
                        vm.pyroscope_sub_screen,
                        PyroscopeSubScreenView::Timeline
                        | PyroscopeSubScreenView::ProfileHeatmap
                        | PyroscopeSubScreenView::SpanHeatmap
                    ) && (vm.timeline.as_ref().map_or(false, |t| !t.exemplars.is_empty())
                        || vm.heatmap.as_ref().map_or(false, |h| !h.exemplars.is_empty())
                        || vm.span_heatmap.as_ref().map_or(false, |h| !h.exemplars.is_empty()));
                if needs_redraw {
                    draw!(terminal, &vm, loki_ui_state.as_ref());
                }
            }
        }
    }

    Ok(())
}

// ── Loki helpers ─────────────────────────────────────────────────────────────

/// Initialize or re-initialize the Loki LSP server and UI state.
async fn init_loki_state(
    loki_ui_state: &mut Option<loki::LokiUiState>,
    loki_lsp: &mut Option<loki::LspClient>,
    loki_lsp_notif_rx: &mut Option<mpsc::UnboundedReceiver<loki::LspNotification>>,
    doc_uri: &str,
    grafana_url: &str,
    grafana_token: &str,
    selected_index: usize,
    datasources: &[shared::DatasourceView],
    labels_tx: &mpsc::UnboundedSender<(String, Vec<String>)>,
) {
    // Only initialize once per session.
    if loki_lsp.is_some() {
        // Reset textarea for re-entry.
        if let Some(ref mut state) = loki_ui_state {
            state.textarea = loki::styled_textarea();
            state.completions.clear();
            state.completion_index = None;
            state.completion_dismissed = false;
            state.diagnostics.clear();
            state.query_dirty = false;
        }
        return;
    }

    match loki::LspClient::new().await {
        Ok((lsp, notif_rx)) => {
            lsp.did_open(doc_uri, "").await;
            *loki_lsp = Some(lsp);
            *loki_lsp_notif_rx = Some(notif_rx);
        }
        Err(e) => {
            log::warn!("Failed to initialize LogQL LSP: {e}");
        }
    }

    *loki_ui_state = Some(loki::LokiUiState::new());

    // Kick off initial label name fetch in the background.
    if let Some(ds) = datasources.get(selected_index) {
        let url = grafana_url.to_string();
        let token = grafana_token.to_string();
        let ds_id = ds.id;
        let tx = labels_tx.clone();
        tokio::spawn(async move {
            let client = loki::LokiClient::new(url, ds_id, token);
            match client.fetch_label_names("").await {
                Ok(names) => { let _ = tx.send((String::new(), names)); }
                Err(e) => { log::warn!("label fetch failed: {e}"); }
            }
        });
    }
}

/// Accept the selected (or first) completion into the textarea.
fn accept_loki_completion(state: &mut loki::LokiUiState) {
    let idx = state.completion_index.unwrap_or(0);
    if let Some(item) = state.completions.get(idx) {
        // Apply the LSP text edit: replace [replace_start_char..replace_end_char] with insert_text.
        let line = state.textarea.lines().first().cloned().unwrap_or_default();
        let chars: Vec<char> = line.chars().collect();
        let start = (item.replace_start_char as usize).min(chars.len());
        let end = (item.replace_end_char as usize).min(chars.len());
        let new_line: String = chars[..start]
            .iter()
            .chain(item.insert_text.chars().collect::<Vec<_>>().iter())
            .chain(chars[end..].iter())
            .collect();
        let new_col = start + item.insert_text.chars().count();
        state.textarea = loki::styled_textarea();
        for c in new_line.chars() {
            state.textarea.insert_char(c);
        }
        use tui_textarea::CursorMove;
        state.textarea.move_cursor(CursorMove::Jump(0, new_col as u16));
        state.completions.clear();
        state.completion_index = None;
        state.completion_dismissed = false;
    }
}

/// If the cursor is in a label-value context, spawn a background fetch for
/// the values so the LSP server can offer them in subsequent completions.
fn maybe_fetch_label_values(
    text: &str,
    cursor_char: usize,
    grafana_url: &str,
    grafana_token: &str,
    selected_index: usize,
    datasources: &[shared::DatasourceView],
    tx: &mpsc::UnboundedSender<(String, String, Vec<String>)>,
) {
    use logql_core::completions::{detect_cursor_context, CursorContext};
    let ctx = detect_cursor_context(text, cursor_char);
    if let CursorContext::LabelValue { label, selector, .. } = ctx {
        if label.is_empty() { return; }
        if let Some(ds) = datasources.get(selected_index) {
            let url = grafana_url.to_string();
            let token = grafana_token.to_string();
            let ds_id = ds.id;
            let tx = tx.clone();
            tokio::spawn(async move {
                let client = loki::LokiClient::new(url, ds_id, token);
                match client.fetch_label_values(&label, &selector).await {
                    Ok(values) => { let _ = tx.send((label, selector, values)); }
                    Err(e) => { log::warn!("label values fetch failed: {e}"); }
                }
            });
        }
    }
}

fn blink_on() -> bool {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis()
        < 500
}

fn ui(frame: &mut Frame, vm: &ViewModel, loki_state: Option<&loki::LokiUiState>) {
    let area = frame.area();

    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let footer_text = match vm.screen {
        ScreenView::DatasourceList => {
            "Esc: Quit/Clear  j/k: Next/Prev  Enter: Select  f: Fav  /: Filter  Tab: History"
        }
        ScreenView::QueryMode => {
            "Ctrl+C: Quit  Esc: Back  Enter: Execute  Tab: Complete  ↓/↑: History/Select  ←/→: Cursor"
        }
        ScreenView::TempoMode => {
            if vm.tempo_results.is_empty() {
                "Ctrl+C: Quit  Esc: Back  Enter: Execute  ←/→: Cursor"
            } else {
                "Ctrl+C: Quit  Esc: Back  Enter: Open Trace  j/k: Navigate  ←/→: Cursor"
            }
        }
        ScreenView::TempoTraceDetail if vm.trace_detail_filter_focused => {
            "Enter/Esc: Stop filtering  Backspace: Delete"
        }
        ScreenView::TempoTraceDetail => {
            "Ctrl+C: Quit  Esc: Back  j/k: Span  J/K: Scroll attrs  /: Filter"
        }
        ScreenView::LokiMode => {
            if vm.loki_context_open {
                "Ctrl+C: Quit  Esc: Back to results"
            } else {
                "Ctrl+C: Quit  Esc: Back/Dismiss  Tab: Complete  ↑/↓: Select/Navigate  Enter: Execute/Context"
            }
        }
        ScreenView::PyroscopeMode => match vm.pyroscope_sub_screen {
            PyroscopeSubScreenView::ServiceList if vm.pyroscope_profile_type_dropdown_open => {
                "Esc/Enter/p: Close  j/k: Select profile type"
            }
            PyroscopeSubScreenView::ServiceList if vm.pyroscope_time_range_editing => {
                "Esc: Cancel  Enter: Apply"
            }
            PyroscopeSubScreenView::ServiceList => {
                "Esc: Back/Clear  j/k: Next/Prev  Enter: Select  t: Time Range  p: Profile Type  /: Filter"
            }
            PyroscopeSubScreenView::Flamegraph if vm.sandwich_view.is_some() => {
                "Esc/s: Exit Sandwich  Tab: Switch View  ←/h: Left  →/l: Right  ↓/j: Callee  ↑/k: Caller"
            }
            PyroscopeSubScreenView::Flamegraph => {
                "Esc: List  Tab: Switch View  ←/h: Left  →/l: Right  ↓/j: Callee  ↑/k: Caller  Enter/z: Zoom In  o: Zoom Out  s: Sandwich"
            }
            PyroscopeSubScreenView::Timeline | PyroscopeSubScreenView::ProfileHeatmap => {
                "Esc: List  Tab: Switch View  j/k: Exemplar Next/Prev"
            }
            PyroscopeSubScreenView::SpanHeatmap if vm.tempo_datasource_picker_open => {
                "Esc: Cancel  j/k: Select  Enter: Query Tempo"
            }
            PyroscopeSubScreenView::SpanHeatmap if vm.exemplar_detail_open => {
                "Esc/Enter: Close  Ctrl+T: Open Trace  Ctrl+P: Open Profile"
            }
            PyroscopeSubScreenView::SpanHeatmap => {
                "Esc: List  Tab: Switch View  j/k: Exemplar Next/Prev  Enter: Detail  Ctrl+T: Lookup Trace"
            }
        },
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
        outer[1],
    );

    match vm.screen {
        ScreenView::DatasourceList => {
            render_datasource_dropdown(frame, vm, outer[0]);
        }
        ScreenView::QueryMode => {
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            prometheus::render_query_mode(frame, vm, split[1]);
        }
        ScreenView::PyroscopeMode => {
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            pyroscope::render_pyroscope_mode(frame, vm, split[1], blink_on());
        }
        ScreenView::TempoMode => {
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            tempo::render_tempo_mode(frame, vm, split[1]);
        }
        ScreenView::TempoTraceDetail => {
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            tempo::render_trace_detail_mode(frame, vm, split[1]);
        }
        ScreenView::LokiMode => {
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            if let Some(state) = loki_state {
                loki::render_loki_mode(frame, vm, split[1], state);
            }
        }
    }
}

fn render_datasource_dropdown(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let (ds_area, hist_area) = (chunks[0], chunks[1]);

    render_datasource_list_panel(frame, vm, ds_area);
    render_history_panel(frame, vm, hist_area);
}

fn render_datasource_list_panel(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let dim = vm.history_panel_focused;
    let border_style = if dim {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let body_block = Block::default()
        .borders(Borders::ALL)
        .title(" Datasources ")
        .border_style(border_style);

    if vm.loading {
        let loading = Paragraph::new("Loading datasources…").block(body_block);
        frame.render_widget(loading, area);
        return;
    }
    if let Some(ref err) = vm.error {
        let error = Paragraph::new(format!("Error: {err}"))
            .style(Style::default().fg(Color::Red))
            .block(body_block);
        frame.render_widget(error, area);
        return;
    }

    let inner = body_block.inner(area);
    frame.render_widget(body_block, area);

    let [filter_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let filter_line = if vm.datasource_filter_focused {
        if vm.datasource_filter.is_empty() {
            Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::DarkGray)),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        } else {
            Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::DarkGray)),
                Span::raw(vm.datasource_filter.clone()),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        }
    } else if vm.datasource_filter.is_empty() {
        Line::from(Span::styled(
            "/ to filter…",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::DarkGray)),
            Span::raw(vm.datasource_filter.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    if vm.datasources.is_empty() {
        let msg = if vm.datasource_filter.is_empty() {
            "No datasources found."
        } else {
            "No matches."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
        return;
    }

    let header_cells = ["Name", "Type", "URL"].iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells);

    let rows: Vec<Row> = vm
        .datasources
        .iter()
        .map(|ds| {
            let fav = if ds.is_favourite { "★ " } else { "  " };
            let default_tag = if ds.is_default { " [default]" } else { "" };
            let type_color = match ds.ds_type.as_str() {
                "pyroscope" => Color::Magenta,
                "prometheus" => Color::Green,
                "tempo" => Color::Cyan,
                "loki" => Color::Yellow,
                _ => Color::DarkGray,
            };
            Row::new(vec![
                Cell::from(format!("{}{}{}", fav, ds.name, default_tag)),
                Cell::from(ds.ds_type.clone()).style(Style::default().fg(type_color)),
                Cell::from(ds.url.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(40),
        Constraint::Percentage(20),
        Constraint::Percentage(40),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(vm.selected_index));

    frame.render_stateful_widget(table, list_area, &mut table_state);
}

fn render_history_panel(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let focused = vm.history_panel_focused;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" History ")
        .border_style(border_style);

    if vm.history_entries.is_empty() {
        frame.render_widget(
            Paragraph::new("No history yet.\nStart querying datasources.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = vm
        .history_entries
        .iter()
        .map(|e| {
            let ds_short = truncate_str(&e.datasource_name, 12);
            let type_color = match e.datasource_type.as_str() {
                "pyroscope" => Color::Magenta,
                "tempo" => Color::Cyan,
                "loki" => Color::Yellow,
                _ => Color::Green,
            };
            let line = Line::from(vec![
                Span::styled(
                    format!("{:>9}  ", e.time_ago),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("[{}] ", e.datasource_type),
                    Style::default().fg(type_color),
                ),
                Span::styled(
                    format!("{:<12}  ", ds_short),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(e.query.clone()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_symbol(">> ")
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let mut list_state = ListState::default();
    list_state.select(Some(vm.history_selected_index));

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

fn render_datasource_bar(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Datasource ");

    let content = if let Some(ds) = vm.datasources.get(vm.selected_index) {
        let default_tag = if ds.is_default { "  [default]" } else { "" };
        Line::from(vec![
            Span::styled(ds.name.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("  ·  "),
            Span::styled(ds.ds_type.clone(), Style::default().fg(Color::Cyan)),
            Span::raw("  ·  "),
            Span::styled(ds.url.clone(), Style::default().fg(Color::DarkGray)),
            Span::styled(default_tag, Style::default().fg(Color::Green)),
        ])
    } else {
        Line::from(Span::styled("No datasource selected", Style::default().fg(Color::DarkGray)))
    };

    frame.render_widget(Paragraph::new(content).block(block), area);
}

// ── Pyroscope async helpers ───────────────────────────────────────────────────

fn spawn_series_fetch(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    time_range: String,
    now_ms: i64,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let window_ms = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as i64 * 1000;
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client.series(now_ms - window_ms, now_ms).await
            .map_err(|e| e.to_string());
        let _ = tx.send(PyroscopeMsg::Series(result));
    });
}

fn spawn_flamegraph_fetch(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    profile_type: String,
    service: String,
    time_range: String,
    now_ms: i64,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let window_ms = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as i64 * 1000;
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client
            .select_merge_stacktraces(&profile_type, &service, now_ms - window_ms, now_ms)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(PyroscopeMsg::Flamegraph(result));
    });
}

fn spawn_flamegraph_by_profile_id(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    profile_type: String,
    service: String,
    time_range: String,
    now_ms: i64,
    profile_id: String,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let window_ms = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as i64 * 1000;
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client
            .select_merge_stacktraces_by_profile_id(
                &profile_type, &service, now_ms - window_ms, now_ms, &profile_id,
            )
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(PyroscopeMsg::Flamegraph(result));
    });
}

fn spawn_timeline_fetch(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    profile_type: String,
    service: String,
    time_range: String,
    now_ms: i64,
    chart_width: u16,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let window_s = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as f64;
        let window_ms = (window_s * 1000.0) as i64;
        let cols = chart_width.max(20) as f64;
        let step_s = (window_s / cols).max(15.0);
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client
            .select_series(&profile_type, &service, now_ms - window_ms, now_ms, step_s, false)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(PyroscopeMsg::Timeline(result));
    });
}

fn spawn_heatmap_fetch(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    profile_type: String,
    service: String,
    time_range: String,
    now_ms: i64,
    chart_width: u16,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let window_s = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as f64;
        let window_ms = (window_s * 1000.0) as i64;
        let cols = chart_width.max(20) as f64;
        let step_s = (window_s / cols).max(15.0);
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client
            .select_heatmap(&profile_type, &service, now_ms - window_ms, now_ms, step_s, false)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(PyroscopeMsg::Heatmap(result));
    });
}

fn spawn_span_heatmap_fetch(
    tx: &mpsc::UnboundedSender<PyroscopeMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    profile_type: String,
    service: String,
    time_range: String,
    now_ms: i64,
    chart_width: u16,
) {
    let tx2 = tx.clone();
    tokio::spawn(async move {
        let window_s = shared::pyroscope::parse_time_range(&time_range).unwrap_or(3600) as f64;
        let window_ms = (window_s * 1000.0) as i64;
        let cols = chart_width.max(20) as f64;
        let step_s = (window_s / cols).max(15.0);
        let client = pyroscope::PyroscopeClient::new(grafana_url, ds_id, token);
        let result = client
            .select_heatmap(&profile_type, &service, now_ms - window_ms, now_ms, step_s, true)
            .await
            .map_err(|e| e.to_string());
        let _ = tx2.send(PyroscopeMsg::SpanHeatmap(result));
    });
}

// ── Tempo async helpers ───────────────────────────────────────────────────────

fn spawn_span_traces_fetch(
    tx: &mpsc::UnboundedSender<TempoMsg>,
    grafana_url: String,
    uid: String,
    token: String,
    span_ids: Vec<String>,
    start_ms: i64,
    end_ms: i64,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let start_s = (start_ms / 1000).max(0) as u64;
        let end_s = (end_ms / 1000).max(0) as u64;
        let client = tempo::TempoClient::new(grafana_url, uid, token);
        let tx2 = tx.clone();
        let result = client.traces_for_span_ids(&span_ids, start_s, end_s, |batch| {
            let _ = tx2.send(TempoMsg::SpanTraceBatch(batch));
        }).await;
        if let Err(e) = result {
            log::warn!("span→trace batch lookup failed: {e}");
        }
        let _ = tx.send(TempoMsg::SpanTraceLoadingDone);
    });
}

fn spawn_trace_detail_fetch(
    tx: &mpsc::UnboundedSender<TempoMsg>,
    grafana_url: String,
    uid: String,
    token: String,
    trace_id: String,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let client = tempo::TempoClient::new(grafana_url, uid, token);
        let result = client.fetch_trace(&trace_id).await.map_err(|e| e.to_string());
        let _ = tx.send(TempoMsg::TraceDetail(result));
    });
}

fn spawn_tempo_search(
    tx: &mpsc::UnboundedSender<TempoMsg>,
    grafana_url: String,
    uid: String,
    token: String,
    query: String,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let start_s = now_s.saturating_sub(3600); // last 1 hour
        let client = tempo::TempoClient::new(grafana_url, uid, token);
        let result = client.search(&query, start_s, now_s).await.map_err(|e| e.to_string());
        let _ = tx.send(TempoMsg::Result(result));
    });
}

// ── Loki async helpers ───────────────────────────────────────────────────────

fn spawn_loki_query(
    tx: &mpsc::UnboundedSender<LokiMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    query: String,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let start_ns = now_ns.saturating_sub(3600 * 1_000_000_000); // last 1 hour
        let client = loki::LokiClient::new(grafana_url, ds_id, token);
        let result = client
            .query_range(&query, start_ns, now_ns, 1000, "backward")
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(LokiMsg::Result(result));
    });
}

fn spawn_loki_context_query(
    tx: &mpsc::UnboundedSender<LokiMsg>,
    grafana_url: String,
    ds_id: u64,
    token: String,
    labels: String,
    timestamp_ns: String,
) {
    let tx = tx.clone();
    let context_lines: u32 = 50;
    tokio::spawn(async move {
        let ts: u64 = timestamp_ns.parse().unwrap_or(0);
        let client = loki::LokiClient::new(grafana_url, ds_id, token);

        // Query lines BEFORE (inclusive of selected): end=ts, direction=backward
        let before_result = client
            .query_range(&labels, ts.saturating_sub(3600 * 1_000_000_000), ts, context_lines, "backward")
            .await;

        // Query lines AFTER (inclusive of selected): start=ts, direction=forward
        let far_future = ts.saturating_add(3600 * 1_000_000_000);
        let after_result = client
            .query_range(&labels, ts, far_future, context_lines + 1, "forward")
            .await;

        let result = match (before_result, after_result) {
            (Ok(before_streams), Ok(after_streams)) => {
                // Flatten and collect entries from the before query (newest-first → reverse to oldest-first)
                let mut before_entries: Vec<shared::loki::LokiEntry> = before_streams
                    .into_iter()
                    .flat_map(|s| s.entries)
                    .collect();
                before_entries.reverse();

                // Flatten after entries (already oldest-first in forward mode)
                let after_entries: Vec<shared::loki::LokiEntry> = after_streams
                    .into_iter()
                    .flat_map(|s| s.entries)
                    .collect();

                // The selected line appears in both results (at ts boundary).
                // before_entries ends with the selected line, after_entries starts with it.
                // Deduplicate: skip the first entry from after if it matches the last from before.
                let highlight_index = before_entries.len().saturating_sub(1);
                let mut merged = before_entries;

                let skip = if !merged.is_empty() && !after_entries.is_empty() {
                    let last = &merged[merged.len() - 1];
                    let first = &after_entries[0];
                    last.timestamp_ns == first.timestamp_ns && last.line == first.line
                } else {
                    false
                };

                if skip {
                    merged.extend(after_entries.into_iter().skip(1));
                } else {
                    merged.extend(after_entries);
                }

                Ok((merged, highlight_index))
            }
            (Err(e), _) | (_, Err(e)) => Err(e.to_string()),
        };

        let _ = tx.send(LokiMsg::ContextResult(result));
    });
}
