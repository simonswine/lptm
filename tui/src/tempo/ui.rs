use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};
use shared::ViewModel;

pub fn render_tempo_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    // ── Query input ───────────────────────────────────────────────────────────
    let query_block = Block::default().borders(Borders::ALL).title(" TraceQL ");

    let before = &vm.tempo_query[..vm.tempo_cursor_pos];
    let after = &vm.tempo_query[vm.tempo_cursor_pos..];
    let cursor_char = after.chars().next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let after_cursor: String = after.chars().skip(1).collect();

    let query_line = Line::from(vec![
        Span::raw(before.to_owned()),
        Span::styled(cursor_char, Style::default().fg(Color::Black).bg(Color::White)),
        Span::raw(after_cursor),
    ]);
    frame.render_widget(Paragraph::new(query_line).block(query_block), split[0]);

    // ── Results area ──────────────────────────────────────────────────────────
    let results_block = Block::default().borders(Borders::ALL).title(" Traces ");

    if vm.tempo_loading {
        frame.render_widget(
            Paragraph::new("Searching…").block(results_block),
            split[1],
        );
        return;
    }

    if let Some(ref err) = vm.tempo_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(results_block),
            split[1],
        );
        return;
    }

    if vm.tempo_results.is_empty() {
        let hint = if vm.tempo_query.trim().is_empty() {
            "Type a TraceQL expression and press Enter.  Example: { .service.name = \"my-svc\" }"
        } else {
            "No traces found."
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::default().fg(Color::DarkGray))
                .block(results_block),
            split[1],
        );
        return;
    }

    let header = Row::new(["TraceID", "Service", "Operation", "Duration"].iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    }));

    let rows: Vec<Row> = vm
        .tempo_results
        .iter()
        .map(|t| {
            let trace_id_short = if t.trace_id.len() > 16 {
                format!("{}…", &t.trace_id[..16])
            } else {
                t.trace_id.clone()
            };
            let duration = if t.duration_ms >= 1000 {
                format!("{:.1}s", t.duration_ms as f64 / 1000.0)
            } else {
                format!("{}ms", t.duration_ms)
            };
            Row::new(vec![
                Cell::from(trace_id_short),
                Cell::from(t.root_service_name.clone()),
                Cell::from(t.root_trace_name.clone()),
                Cell::from(duration),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(18),
        Constraint::Percentage(30),
        Constraint::Fill(1),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header).block(results_block);
    frame.render_widget(table, split[1]);
}
