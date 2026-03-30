mod core;
mod highlight;
mod http;
mod prometheus;
mod pyroscope;

use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;
use log::info;
use serde::{Deserialize, Serialize};
use simplelog::{Config as LogConfig, LevelFilter, WriteLogger};
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Paragraph, Row, Table, TableState,
    },
    DefaultTerminal, Frame,
};
use shared::{Event, PyroscopeSubScreenView, ScreenView, ViewModel};
use tokio::{sync::mpsc, time::Instant};

use crate::core::AppCore;

#[derive(Parser, Debug)]
#[command(name = "grafex", about = "Explore Grafana data from your terminal")]
struct Args {
    /// Grafana base URL (e.g. http://localhost:3000)
    #[arg(long, env = "GRAFANA_URL")]
    grafana_url: String,

    /// Grafana service account token
    #[arg(long, env = "GRAFANA_TOKEN")]
    grafana_token: String,
}

/// Initialise file logging under `~/.config/grafex/`.
/// Errors are non-fatal: the app runs without logging if setup fails.
fn grafex_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".config").join("grafex"))
}

fn setup_logging() {
    let Some(dir) = grafex_dir() else {
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
    grafex_dir().map(|d| d.join("history"))
}

fn favourites_path() -> Option<std::path::PathBuf> {
    grafex_dir().map(|d| d.join("favourites.json"))
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
    grafana_url_hash: String,
    /// Unix timestamp (seconds since epoch) when the query was executed.
    #[serde(default)]
    timestamp: u64,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

    info!("grafex starting – url={}", args.grafana_url);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, args).await;
    ratatui::restore();
    info!("grafex exiting");
    result
}

enum PyroscopeMsg {
    Series(Result<Vec<(String, String)>, String>),
    Flamegraph(Result<Option<shared::pyroscope::FlameGraph>, String>),
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
    let mut history: Vec<String> = vec![];
    let mut history_pos: Option<usize> = None;
    let mut history_saved_query = String::new();

    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
    let mut debounce_deadline: Option<Instant> = None;

    let (pyroscope_tx, mut pyroscope_rx) = mpsc::unbounded_channel::<PyroscopeMsg>();

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
                }
            }
            () = async {
                match debounce_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None    => std::future::pending().await,
                }
            } => {
                debounce_deadline = None;
                app_core.update(Event::TriggerCompletions);
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
                                        if vm.datasource_filter_focused
                                            || !vm.datasource_filter.is_empty()
                                        {
                                            app_core.update(Event::DatasourceFilterClear);
                                        } else {
                                            break;
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
                                                        datasource_type: ds.ds_type.clone(),
                                                        grafana_url_hash: url_hash.clone(),
                                                        timestamp: now_unix_secs(),
                                                    };
                                                    history.push(query.clone());
                                                    append_history(&entry);
                                                    all_history.push(entry);
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
                                                    if !vm.pyroscope_service_filter.is_empty() {
                                                        app_core.update(Event::PyroscopeServiceFilterClear);
                                                    } else {
                                                        app_core.update(Event::BackFromPyroscope);
                                                    }
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
                                                KeyCode::Char('[') => {
                                                    app_core.update(Event::PyroscopeProfileTypePrev);
                                                }
                                                KeyCode::Char(']') => {
                                                    app_core.update(Event::PyroscopeProfileTypeNext);
                                                }
                                                KeyCode::Backspace => {
                                                    app_core.update(Event::PyroscopeServiceFilterBackspace);
                                                }
                                                KeyCode::Enter => {
                                                    let now = now_unix_secs() as i64 * 1000;
                                                    // Read the selected series before firing the event.
                                                    let selected = vm.pyroscope_series
                                                        .get(vm.pyroscope_series_index)
                                                        .cloned();
                                                    let ds_id = vm.datasources.get(vm.selected_index).map(|d| d.id);
                                                    let time_range = vm.pyroscope_time_range.clone();
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
                                                KeyCode::Char(c) => {
                                                    app_core.update(Event::PyroscopeServiceFilterInput(c));
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    PyroscopeSubScreenView::Flamegraph => {
                                        match key.code {
                                            KeyCode::Esc | KeyCode::Char('q') => {
                                                app_core.update(Event::BackToServiceList);
                                            }
                                            KeyCode::Left | KeyCode::Char('h') => {
                                                app_core.update(Event::FlameMoveLeft);
                                            }
                                            KeyCode::Right | KeyCode::Char('l') => {
                                                app_core.update(Event::FlameMoveRight);
                                            }
                                            KeyCode::Up | KeyCode::Char('k') => {
                                                app_core.update(Event::FlameMoveDown);
                                            }
                                            KeyCode::Down | KeyCode::Char('j') => {
                                                app_core.update(Event::FlameMoveUp);
                                            }
                                            KeyCode::Enter | KeyCode::Char('z') => {
                                                app_core.update(Event::FlameZoomIn);
                                            }
                                            KeyCode::Backspace | KeyCode::Char('o') => {
                                                app_core.update(Event::FlameZoomOut);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                    CrosstermEvent::Resize(w, _) => {
                        app_core.update(Event::FlamegraphViewportChars(w.saturating_sub(2) as u64));
                        let vm = app_core.core.view();
                        terminal.draw(|frame| ui(frame, &vm))?;
                    }
                    _ => {}
                }
            }
            Some(()) = render_rx.recv() => {
                let vm = app_core.core.view();
                terminal.draw(|frame| ui(frame, &vm))?;
            }
        }
    }

    Ok(())
}

fn ui(frame: &mut Frame, vm: &ViewModel) {
    let area = frame.area();

    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let footer_text = match vm.screen {
        ScreenView::DatasourceList => {
            "Esc: Quit/Clear  j/↓: Next  k/↑: Prev  Enter: Select  f: Favourite  /: Filter"
        }
        ScreenView::QueryMode => {
            "Ctrl+C: Quit  Esc: Back  Enter: Execute  Tab: Complete  ↓/↑: History/Select  ←/→: Cursor"
        }
        ScreenView::PyroscopeMode => match vm.pyroscope_sub_screen {
            PyroscopeSubScreenView::ServiceList if vm.pyroscope_time_range_editing => {
                "Esc: Cancel  Enter: Apply"
            }
            PyroscopeSubScreenView::ServiceList => {
                "Esc: Back/Clear  j/k: Next/Prev  Enter: Select  t: Time Range  [/]: Profile Type  Type: Filter"
            }
            PyroscopeSubScreenView::Flamegraph => {
                "Esc: List  ←/h: Left  →/l: Right  ↓/j: Callee  ↑/k: Caller  Enter/z: Zoom In  o: Zoom Out"
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
            pyroscope::render_pyroscope_mode(frame, vm, split[1]);
        }
    }
}

fn render_datasource_dropdown(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let body_block = Block::default()
        .borders(Borders::ALL)
        .title(" Datasources ");

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
            Row::new(vec![
                Cell::from(format!("{}{}{}", fav, ds.name, default_tag)),
                Cell::from(ds.ds_type.clone()),
                Cell::from(ds.url.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(20),
        Constraint::Percentage(50),
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
