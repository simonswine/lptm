mod core;
mod highlight;
mod http;

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
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table,
        TableState,
    },
    DefaultTerminal, Frame,
};
use shared::{Event, QueryResultsView, ScreenView, ViewModel};
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

async fn run(terminal: &mut DefaultTerminal, args: Args) -> Result<()> {
    let (render_tx, mut render_rx) = mpsc::unbounded_channel::<()>();
    let app_core = AppCore::new(render_tx);

    // Compute the URL hash before args are moved.
    let url_hash = grafana_url_hash(&args.grafana_url);

    // Bootstrap: configure and trigger initial fetch
    app_core.update(Event::Configure {
        url: args.grafana_url,
        token: args.grafana_token,
    });

    let mut reader = EventStream::new();

    // All persisted history entries (full JSON records).
    let mut all_history = load_history();
    // Filtered query strings for the currently selected datasource + Grafana URL.
    // Populated when the user enters QueryMode; empty until then.
    let mut history: Vec<String> = vec![];
    // None = live query; Some(i) = browsing history at index i.
    let mut history_pos: Option<usize> = None;
    // The query the user had typed before entering history navigation.
    let mut history_saved_query = String::new();

    // Completion debounce: fire TriggerCompletions after 250 ms of no typing.
    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
    let mut debounce_deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            // Debounce timer: fire completions after the user pauses typing.
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
                                        if !vm.datasource_filter.is_empty() {
                                            app_core.update(Event::DatasourceFilterClear);
                                        } else {
                                            break;
                                        }
                                    }
                                    (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                        app_core.update(Event::SelectNext);
                                    }
                                    (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                        app_core.update(Event::SelectPrevious);
                                    }
                                    (KeyCode::Backspace, _) => {
                                        app_core.update(Event::DatasourceFilterBackspace);
                                    }
                                    (KeyCode::Enter, _) => {
                                        history_pos = None;
                                        // Rebuild the filtered history for the selected datasource.
                                        if let Some(ds) = vm.datasources.get(vm.selected_index) {
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
                                        }
                                        app_core.update(Event::EnterQuery);
                                    }
                                    (KeyCode::Char(c), _) => {
                                        app_core.update(Event::DatasourceFilterInput(c));
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
                                    // Completion navigation (only when popup is visible).
                                    (KeyCode::Down, _) if !vm.completions.is_empty() => {
                                        app_core.update(Event::CompletionNext);
                                    }
                                    (KeyCode::Up, _) if !vm.completions.is_empty() => {
                                        app_core.update(Event::CompletionPrev);
                                    }
                                    // History navigation (only when popup is absent).
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
                        }
                    }
                    CrosstermEvent::Resize(_, _) => {
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

    // Reserve one line at the bottom for the footer in all modes.
    let outer = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let footer_text = match vm.screen {
        ScreenView::DatasourceList => {
            "Esc: Quit/Clear  j/↓: Next  k/↑: Prev  Enter: Select  Type: Filter"
        }
        ScreenView::QueryMode => {
            "Ctrl+C: Quit  Esc: Back  Enter: Execute  Tab: Complete  ↓/↑: History/Select  ←/→: Cursor"
        }
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
        outer[1],
    );

    match vm.screen {
        ScreenView::DatasourceList => {
            // The datasource dropdown expands to fill the whole content area.
            render_datasource_dropdown(frame, vm, outer[0]);
        }
        ScreenView::QueryMode => {
            // 3-line datasource bar at top, then query + results below.
            let split =
                Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(outer[0]);
            render_datasource_bar(frame, vm, split[0]);
            render_query_mode(frame, vm, split[1]);
        }
    }
}

fn render_datasource_dropdown(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let body_block = Block::default()
        .borders(Borders::ALL)
        .title(" Prometheus Datasources ");

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

    // Render the block border, then work inside the inner area.
    let inner = body_block.inner(area);
    frame.render_widget(body_block, area);

    // ── Filter input line ─────────────────────────────────────────────────────
    let [filter_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let filter_line = if vm.datasource_filter.is_empty() {
        Line::from(Span::styled(
            "/ type to filter…",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::DarkGray)),
            Span::raw(vm.datasource_filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    // ── Datasource table ──────────────────────────────────────────────────────
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
            let default_tag = if ds.is_default { " [default]" } else { "" };
            Row::new(vec![
                Cell::from(format!("{}{}", ds.name, default_tag)),
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

fn render_query_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    // ── Query input box ───────────────────────────────────────────────────────
    let query_block = Block::default()
        .borders(Borders::ALL)
        .title(" PromQL ");

    let query_line: Line = highlight::highlight_query(&vm.query, vm.cursor_pos);
    let query_text = Paragraph::new(query_line).block(query_block);
    frame.render_widget(query_text, split[0]);

    // ── Results area ──────────────────────────────────────────────────────────
    let results_area = split[1];
    let results_block = Block::default().borders(Borders::ALL).title(" Results ");

    if vm.query_loading {
        let loading = Paragraph::new("Executing query…").block(results_block);
        frame.render_widget(loading, results_area);
    } else if let Some(ref err) = vm.query_error {
        let error = Paragraph::new(format!("Error: {err}"))
            .style(Style::default().fg(Color::Red))
            .block(results_block);
        frame.render_widget(error, results_area);
    } else if let Some(ref results) = vm.query_results {
        if results.rows.is_empty() {
            let empty = Paragraph::new("No results.").block(results_block);
            frame.render_widget(empty, results_area);
        } else {
            render_results_table(frame, results, results_block, results_area);
        }
    } else {
        let hint = Paragraph::new("Type a PromQL expression and press Enter.")
            .style(Style::default().fg(Color::DarkGray))
            .block(results_block);
        frame.render_widget(hint, results_area);
    }

    // ── Completion popup (rendered on top of results area) ───────────────────
    render_completion_popup(frame, vm, results_area);
}

fn render_completion_popup(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let has_completions = !vm.completions.is_empty();
    let show_loading = vm.completions_loading && !has_completions;

    if !has_completions && !show_loading {
        return;
    }

    if show_loading {
        let popup_w = 20u16.min(area.width);
        let popup_h = 3u16.min(area.height);
        let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);
        frame.render_widget(Clear, popup_area);
        let loading = Paragraph::new("Loading…")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(loading, popup_area);
        return;
    }

    let max_len = vm.completions.iter().map(|s| s.len()).max().unwrap_or(10);
    let popup_w = ((max_len as u16) + 4).max(20).min(area.width);
    let popup_h = ((vm.completions.len() as u16) + 2).min(12).min(area.height);
    let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = vm
        .completions
        .iter()
        .map(|c| ListItem::new(c.as_str().to_owned()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(vm.completion_index);

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Completions "))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_results_table(
    frame: &mut Frame,
    results: &QueryResultsView,
    block: Block,
    area: Rect,
) {
    let header_cells: Vec<Cell> = results
        .columns
        .iter()
        .map(|col| {
            Cell::from(col.as_str().to_owned()).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    let header = Row::new(header_cells);

    let rows: Vec<Row> = results
        .rows
        .iter()
        .map(|row| Row::new(row.iter().map(|cell| Cell::from(cell.as_str().to_owned()))))
        .collect();

    let n = results.columns.len().max(1);
    let widths: Vec<Constraint> = (0..n).map(|_| Constraint::Fill(1)).collect();

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}
