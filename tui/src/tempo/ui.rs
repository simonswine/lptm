use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::{TempoSpan, ViewModel};

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

    let table = Table::new(rows, widths)
        .header(header)
        .block(results_block)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(vm.tempo_results_selected));

    frame.render_stateful_widget(table, split[1], &mut table_state);
}

pub fn render_trace_detail_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    // ── Layout ────────────────────────────────────────────────────────────────
    let [summary_area, content_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    let [filter_area, table_area, detail_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(10),
    ])
    .areas(content_area);

    // ── Summary block ─────────────────────────────────────────────────────────
    let tid = &vm.trace_detail_trace_id;
    let span_count = vm.trace_detail_spans.len();
    let total_dur_ns: u64 = vm
        .trace_detail_spans
        .iter()
        .map(|s| s.start_time_unix_nano + s.duration_ns)
        .max()
        .unwrap_or(0)
        .saturating_sub(
            vm.trace_detail_spans
                .iter()
                .map(|s| s.start_time_unix_nano)
                .min()
                .unwrap_or(0),
        );
    let dur_str = format_duration_ns(total_dur_ns);
    let summary_text = format!("Trace: {tid}  ({span_count} spans, {dur_str})");
    let summary_block = Block::default().borders(Borders::ALL).title(" Trace Detail ");
    frame.render_widget(
        Paragraph::new(summary_text).block(summary_block),
        summary_area,
    );

    // ── Loading / error states ────────────────────────────────────────────────
    if vm.trace_detail_loading {
        frame.render_widget(
            Paragraph::new("Loading spans…").style(Style::default().fg(Color::DarkGray)),
            table_area,
        );
        return;
    }

    if let Some(ref err) = vm.trace_detail_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}")).style(Style::default().fg(Color::Red)),
            table_area,
        );
        return;
    }

    // ── Filter bar ────────────────────────────────────────────────────────────
    let filter_line = if vm.trace_detail_filter_focused {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Yellow)),
            Span::raw(vm.trace_detail_filter.clone()),
            Span::styled("_", Style::default().fg(Color::Black).bg(Color::White)),
        ])
    } else if vm.trace_detail_filter.is_empty() {
        Line::from(Span::styled(
            "/ to filter spans…",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Yellow)),
            Span::raw(vm.trace_detail_filter.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    // ── Compute trace time window for timeline bars ───────────────────────────
    let trace_start = vm
        .trace_detail_spans
        .iter()
        .map(|s| s.start_time_unix_nano)
        .min()
        .unwrap_or(0);
    let trace_end = vm
        .trace_detail_spans
        .iter()
        .map(|s| s.start_time_unix_nano + s.duration_ns)
        .max()
        .unwrap_or(trace_start + 1);
    let trace_total = trace_end.saturating_sub(trace_start).max(1);

    // ── Filtered spans ────────────────────────────────────────────────────────
    let filtered: Vec<&TempoSpan> = if vm.trace_detail_filter.is_empty() {
        vm.trace_detail_spans.iter().collect()
    } else {
        vm.trace_detail_spans
            .iter()
            .filter(|s| span_matches_filter(s, &vm.trace_detail_filter))
            .collect()
    };

    // ── Span table ────────────────────────────────────────────────────────────
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("No spans match the filter.")
                .style(Style::default().fg(Color::DarkGray)),
            table_area,
        );
    } else {
        let header = Row::new(
            ["Service", "Span Name", "Duration", "Timeline"].iter().map(|h| {
                Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            }),
        );

        const TIMELINE_WIDTH: usize = 26;

        let rows: Vec<Row> = filtered
            .iter()
            .map(|s| {
                let indent = "  ".repeat(s.depth);
                let prefix = if s.depth > 0 { "└─ " } else { "" };
                let span_name = format!("{indent}{prefix}{}", s.name);

                let dur = format_duration_ns(s.duration_ns);

                let timeline = render_timeline_bar(
                    s.start_time_unix_nano,
                    s.duration_ns,
                    trace_start,
                    trace_total,
                    TIMELINE_WIDTH,
                );

                let name_style = if s.error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(s.service_name.clone()),
                    Cell::from(span_name).style(name_style),
                    Cell::from(dur),
                    Cell::from(timeline).style(Style::default().fg(Color::Green)),
                ])
            })
            .collect();

        let widths = [
            Constraint::Percentage(18),
            Constraint::Fill(1),
            Constraint::Length(10),
            Constraint::Length(TIMELINE_WIDTH as u16 + 2),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .highlight_symbol(">> ")
            .row_highlight_style(
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            );

        let mut table_state = TableState::default();
        table_state.select(Some(vm.trace_detail_selected));

        frame.render_stateful_widget(table, table_area, &mut table_state);
    }

    // ── Span detail panel ─────────────────────────────────────────────────────
    let selected_span = filtered.get(vm.trace_detail_selected);

    if let Some(span) = selected_span {
        let kind_str = match span.kind {
            1 => "INTERNAL",
            2 => "SERVER",
            3 => "CLIENT",
            4 => "PRODUCER",
            5 => "CONSUMER",
            _ => "UNSPECIFIED",
        };
        let kind_color = match span.kind {
            2 => Color::Green,
            3 => Color::Blue,
            _ => Color::DarkGray,
        };

        let mut lines: Vec<Line> = Vec::new();

        // First line: span ID, kind, error badge
        let mut first_spans = vec![
            Span::styled("span.id", Style::default().fg(Color::DarkGray)),
            Span::raw(format!(": {}  ", span.span_id)),
            Span::styled(kind_str, Style::default().fg(kind_color)),
        ];
        if span.error {
            first_spans.push(Span::raw("  "));
            first_spans.push(Span::styled("ERROR", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
        }
        lines.push(Line::from(first_spans));

        for (k, v) in &span.resource_attrs {
            lines.push(Line::from(vec![
                Span::styled("[resource] ", Style::default().fg(Color::DarkGray)),
                Span::styled(k.clone(), Style::default().fg(Color::Cyan)),
                Span::raw(format!(": {v}")),
            ]));
        }
        for (k, v) in &span.span_attrs {
            lines.push(Line::from(vec![
                Span::styled("[span]     ", Style::default().fg(Color::DarkGray)),
                Span::styled(k.clone(), Style::default().fg(Color::Yellow)),
                Span::raw(format!(": {v}")),
            ]));
        }

        let total_lines = lines.len();
        let scroll = vm.trace_detail_attr_scroll.min(total_lines.saturating_sub(1));

        // Show ▼ in title when there is more content below
        let visible_lines = detail_area.height.saturating_sub(2) as usize; // -2 for borders
        let has_more = scroll + visible_lines < total_lines;
        let title = if has_more {
            " Span Attributes  J/K: scroll  ▼ more "
        } else {
            " Span Attributes "
        };
        let detail_block = Block::default().borders(Borders::ALL).title(title);

        frame.render_widget(
            Paragraph::new(lines)
                .block(detail_block)
                .scroll((scroll as u16, 0)),
            detail_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new("Select a span to view attributes.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(" Span Attributes ")),
            detail_area,
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn render_timeline_bar(
    span_start: u64,
    duration_ns: u64,
    trace_start: u64,
    trace_total: u64,
    width: usize,
) -> String {
    if width < 2 {
        return " ".repeat(width);
    }
    let w = width as f64;
    let bar_start =
        ((span_start.saturating_sub(trace_start)) as f64 / trace_total as f64 * w) as usize;
    let bar_end =
        ((span_start.saturating_sub(trace_start) + duration_ns) as f64 / trace_total as f64 * w)
            as usize;
    let bar_start = bar_start.min(width);
    let bar_end = bar_end.min(width).max(bar_start + 1);

    let mut bar: Vec<char> = vec!['-'; width];
    for c in bar.iter_mut().take(bar_end).skip(bar_start) {
        *c = '█';
    }
    bar.into_iter().collect()
}

fn format_duration_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2}s", ns as f64 / 1_000_000_000.0)
    } else if ns >= 1_000_000 {
        format!("{:.1}ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.1}µs", ns as f64 / 1_000.0)
    } else {
        format!("{ns}ns")
    }
}

fn span_matches_filter(span: &TempoSpan, needle: &str) -> bool {
    if fuzzy_match(&span.name, needle) {
        return true;
    }
    if fuzzy_match(&span.service_name, needle) {
        return true;
    }
    for (k, v) in span.resource_attrs.iter().chain(span.span_attrs.iter()) {
        if fuzzy_match(k, needle) || fuzzy_match(v, needle) {
            return true;
        }
    }
    false
}

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
