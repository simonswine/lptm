mod core;
mod http;

use anyhow::Result;
use clap::Parser;
use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    DefaultTerminal, Frame,
};
use shared::{Event, ViewModel};
use tokio::sync::mpsc;

use crate::core::AppCore;

#[derive(Parser, Debug)]
#[command(name = "exploretui", about = "Grafana Explore for the terminal")]
struct Args {
    /// Grafana base URL (e.g. http://localhost:3000)
    #[arg(long, env = "GRAFANA_URL")]
    grafana_url: String,

    /// Grafana service account token
    #[arg(long, env = "GRAFANA_TOKEN")]
    grafana_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, args).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut DefaultTerminal, args: Args) -> Result<()> {
    let (render_tx, mut render_rx) = mpsc::unbounded_channel::<()>();
    let app_core = AppCore::new(render_tx);

    // Bootstrap: configure and trigger initial fetch
    app_core.update(Event::Configure {
        url: args.grafana_url,
        token: args.grafana_token,
    });

    let mut reader = EventStream::new();

    loop {
        tokio::select! {
            maybe_event = reader.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                match event {
                    CrosstermEvent::Key(key) => {
                        match (key.code, key.modifiers) {
                            (KeyCode::Char('q'), _)
                            | (KeyCode::Esc, _)
                            | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                break;
                            }
                            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                                app_core.update(Event::SelectNext);
                            }
                            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                                app_core.update(Event::SelectPrevious);
                            }
                            (KeyCode::Char('r'), _) => {
                                app_core.update(Event::FetchDatasources);
                            }
                            _ => {}
                        }
                    }
                    CrosstermEvent::Resize(_, _) => {
                        // Force a redraw on resize
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

    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    // ── Header ───────────────────────────────────────────────────────────────
    let header = Paragraph::new("Grafana Explore TUI")
        .style(Style::default().add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title(" exploretui "));
    frame.render_widget(header, layout[0]);

    // ── Body ─────────────────────────────────────────────────────────────────
    let body_block = Block::default()
        .borders(Borders::ALL)
        .title(" Datasources ");

    if vm.loading {
        let loading = Paragraph::new("Loading datasources…").block(body_block);
        frame.render_widget(loading, layout[1]);
    } else if let Some(ref err) = vm.error {
        let error = Paragraph::new(format!("Error: {err}"))
            .style(Style::default().fg(Color::Red))
            .block(body_block);
        frame.render_widget(error, layout[1]);
    } else if vm.datasources.is_empty() {
        let empty = Paragraph::new("No datasources found.").block(body_block);
        frame.render_widget(empty, layout[1]);
    } else {
        let items: Vec<ListItem> = vm
            .datasources
            .iter()
            .map(|ds| {
                let default_tag = if ds.is_default { " [default]" } else { "" };
                let line =
                    format!("{} ({})  {}{}", ds.name, ds.ds_type, ds.url, default_tag);
                ListItem::new(line)
            })
            .collect();

        let mut list_state = ListState::default();
        list_state.select(Some(vm.selected_index));

        let list = List::new(items)
            .block(body_block)
            .highlight_symbol(">> ")
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, layout[1], &mut list_state);
    }

    // ── Footer ────────────────────────────────────────────────────────────────
    let footer = Paragraph::new("q/Esc: Quit  j/↓: Next  k/↑: Prev  r: Refresh")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, layout[2]);
}
