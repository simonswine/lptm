use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::ViewModel;

pub fn render_loki_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    if vm.loki_context_open {
        render_context_view(frame, vm, area);
        return;
    }

    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    // ── Query input ───────────────────────────────────────────────────────────
    let query_block = Block::default().borders(Borders::ALL).title(" LogQL ");

    let before = &vm.loki_query[..vm.loki_cursor_pos];
    let after = &vm.loki_query[vm.loki_cursor_pos..];
    let cursor_char = after.chars().next().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
    let after_cursor: String = after.chars().skip(1).collect();

    let query_line = Line::from(vec![
        Span::raw(before.to_owned()),
        Span::styled(cursor_char, Style::default().fg(Color::Black).bg(Color::White)),
        Span::raw(after_cursor),
    ]);
    frame.render_widget(Paragraph::new(query_line).block(query_block), split[0]);

    // ── Results area ──────────────────────────────────────────────────────────
    let results_block = Block::default().borders(Borders::ALL).title(" Logs ");

    if vm.loki_loading {
        frame.render_widget(
            Paragraph::new("Searching…").block(results_block),
            split[1],
        );
        return;
    }

    if let Some(ref err) = vm.loki_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(results_block),
            split[1],
        );
        return;
    }

    if vm.loki_results.is_empty() {
        let hint = if vm.loki_query.trim().is_empty() {
            "Type a LogQL expression and press Enter.  Example: {job=\"varlogs\"}"
        } else {
            "No logs found."
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::default().fg(Color::DarkGray))
                .block(results_block),
            split[1],
        );
        return;
    }

    let header = Row::new(["Timestamp", "Labels", "Log Line"].iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    }));

    let rows: Vec<Row> = vm
        .loki_results
        .iter()
        .flat_map(|stream| {
            stream.entries.iter().map(move |entry| {
                Row::new(vec![
                    Cell::from(entry.timestamp.clone()),
                    Cell::from(stream.labels.clone())
                        .style(Style::default().fg(Color::Yellow)),
                    Cell::from(entry.line.clone()),
                ])
            })
        })
        .collect();

    let widths = [
        Constraint::Length(24),
        Constraint::Length(40),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(results_block)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(vm.loki_selected_row));
    frame.render_stateful_widget(table, split[1], &mut table_state);

    render_completion_popup(frame, vm, split[1]);
}

fn render_completion_popup(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let has_completions = !vm.loki_completions.is_empty();
    let show_loading = vm.loki_completions_loading && !has_completions;

    if !has_completions && !show_loading {
        return;
    }

    if show_loading {
        let popup_w = 20u16.min(area.width);
        let popup_h = 3u16.min(area.height);
        let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);
        frame.render_widget(Clear, popup_area);
        let loading = Paragraph::new("Loading\u{2026}")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(loading, popup_area);
        return;
    }

    let max_len = vm.loki_completions.iter().map(|s| s.len()).max().unwrap_or(10);
    let popup_w = ((max_len as u16) + 4).max(20).min(area.width);
    let popup_h = ((vm.loki_completions.len() as u16) + 2).min(12).min(area.height);
    let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = vm
        .loki_completions
        .iter()
        .map(|c| ListItem::new(c.as_str().to_owned()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(vm.loki_completion_index);

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Completions "))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_context_view(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let title = format!(" Context: {} ", vm.loki_context_labels);
    let block = Block::default().borders(Borders::ALL).title(title);

    if vm.loki_context_loading {
        frame.render_widget(
            Paragraph::new("Loading context…").block(block),
            area,
        );
        return;
    }

    if let Some(ref err) = vm.loki_context_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(block),
            area,
        );
        return;
    }

    if vm.loki_context_entries.is_empty() {
        frame.render_widget(
            Paragraph::new("No context lines found.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let header = Row::new(["Timestamp", "Log Line"].iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    }));

    let rows: Vec<Row> = vm
        .loki_context_entries
        .iter()
        .map(|entry| {
            Row::new(vec![
                Cell::from(entry.timestamp.clone()),
                Cell::from(entry.line.clone()),
            ])
        })
        .collect();

    let widths = [Constraint::Length(24), Constraint::Fill(1)];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(vm.loki_context_highlight_index));
    frame.render_stateful_widget(table, area, &mut table_state);
}
