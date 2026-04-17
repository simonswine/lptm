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

    // Layout: query box (3 lines) + diagnostics/type line (1) + results (rest)
    let has_diagnostics = !vm.loki_diagnostics.is_empty() || vm.loki_type_context.is_some();
    let info_height = if has_diagnostics { 1 } else { 0 };

    let split = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(info_height),
        Constraint::Min(0),
    ])
    .split(area);

    // ── Query input with diagnostic underlines ──────────────────────────────
    let query_block = Block::default().borders(Borders::ALL).title(" LogQL ");

    let query_line = build_query_line_with_diagnostics(
        &vm.loki_query,
        vm.loki_cursor_pos,
        &vm.loki_diagnostics,
    );
    frame.render_widget(Paragraph::new(query_line).block(query_block), split[0]);

    // ── Diagnostic / type context info line ──────────────────────────────────
    if has_diagnostics {
        let info_line = build_info_line(vm);
        frame.render_widget(Paragraph::new(info_line), split[1]);
    }

    // ── Results area ─────────────────────────────────────────────────────────
    let results_area = split[2];
    let results_block = Block::default().borders(Borders::ALL).title(" Logs ");

    if vm.loki_loading {
        frame.render_widget(
            Paragraph::new("Searching\u{2026}").block(results_block),
            results_area,
        );
        return;
    }

    if let Some(ref err) = vm.loki_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(results_block),
            results_area,
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
            results_area,
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
    frame.render_stateful_widget(table, results_area, &mut table_state);

    render_completion_popup(frame, vm, results_area);
}

/// Build the query line with syntax highlighting and diagnostic underlines.
fn build_query_line_with_diagnostics(
    query: &str,
    cursor_pos: usize,
    diagnostics: &[shared::LokiDiagnosticView],
) -> Vec<Line<'static>> {
    // First line: syntax-highlighted query
    let highlighted = super::highlight::highlight_loki_query(query, cursor_pos);

    if diagnostics.is_empty() || query.is_empty() {
        return vec![highlighted];
    }

    // Second line: diagnostic underlines (shown inside the query box border)
    let mut underline_chars: Vec<(char, Style)> = vec![(' ', Style::default()); query.len()];

    for diag in diagnostics {
        let style = if diag.severity == "error" {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let marker = if diag.severity == "error" { '^' } else { '~' };

        let start = diag.start_byte.min(query.len());
        let end = diag.end_byte.min(query.len());
        for i in start..end {
            underline_chars[i] = (marker, style);
        }
    }

    // Build underline spans
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut i = 0;
    while i < underline_chars.len() {
        let (ch, style) = underline_chars[i];
        let mut run = String::new();
        run.push(ch);
        let mut j = i + 1;
        while j < underline_chars.len() && underline_chars[j] == (ch, style) {
            run.push(ch);
            j += 1;
        }
        spans.push(Span::styled(run, style));
        i = j;
    }

    vec![highlighted, Line::from(spans)]
}

/// Build the info line showing type context and first diagnostic message.
fn build_info_line(vm: &ViewModel) -> Line<'static> {
    let mut parts: Vec<Span<'static>> = Vec::new();

    // Type context
    if let Some(ref tc) = vm.loki_type_context {
        parts.push(Span::styled(
            format!(" {tc} "),
            Style::default().fg(Color::Cyan),
        ));
    }

    // First diagnostic message
    if let Some(diag) = vm.loki_diagnostics.first() {
        if !parts.is_empty() {
            parts.push(Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)));
        }
        let style = if diag.severity == "error" {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let icon = if diag.severity == "error" {
            "\u{2717} "
        } else {
            "\u{26a0} "
        };
        parts.push(Span::styled(
            format!("{icon}{}", diag.message),
            style,
        ));
    }

    Line::from(parts)
}

fn render_completion_popup(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let has_completions = !vm.loki_completion_items.is_empty();
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

    // Compute popup width: kind_icon + " " + label + "  " + detail
    let max_label_len = vm
        .loki_completion_items
        .iter()
        .map(|c| c.label.len())
        .max()
        .unwrap_or(10);
    let max_detail_len = vm
        .loki_completion_items
        .iter()
        .filter_map(|c| c.detail.as_ref())
        .map(|d| d.len())
        .max()
        .unwrap_or(0);
    let content_w = 4 + max_label_len + if max_detail_len > 0 { 2 + max_detail_len } else { 0 };
    let popup_w = ((content_w as u16) + 4).max(20).min(area.width);
    let popup_h = ((vm.loki_completion_items.len() as u16) + 2).min(12).min(area.height);
    let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = vm
        .loki_completion_items
        .iter()
        .map(|c| {
            let mut spans = vec![
                Span::styled(
                    format!("{} ", c.kind_icon),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(c.label.clone()),
            ];
            if let Some(ref detail) = c.detail {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(vm.loki_completion_index);

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Completions "))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(list, popup_area, &mut list_state);

    // Show documentation for the selected item below the popup
    if let Some(idx) = vm.loki_completion_index {
        if let Some(item) = vm.loki_completion_items.get(idx) {
            if let Some(ref doc) = item.documentation {
                let doc_y = popup_area.y + popup_area.height;
                if doc_y < area.y + area.height {
                    let doc_w = popup_w.max(doc.len() as u16 + 4).min(area.width);
                    let doc_h = 3u16.min(area.y + area.height - doc_y);
                    let doc_area = Rect::new(popup_area.x, doc_y, doc_w, doc_h);
                    frame.render_widget(Clear, doc_area);
                    let doc_widget = Paragraph::new(doc.clone())
                        .block(Block::default().borders(Borders::ALL))
                        .style(Style::default().fg(Color::DarkGray));
                    frame.render_widget(doc_widget, doc_area);
                }
            }
        }
    }
}

fn render_context_view(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let title = format!(" Context: {} ", vm.loki_context_labels);
    let block = Block::default().borders(Borders::ALL).title(title);

    if vm.loki_context_loading {
        frame.render_widget(
            Paragraph::new("Loading context\u{2026}").block(block),
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
