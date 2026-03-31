use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::{pyroscope::{HeatmapView, ProfileUnit, TimelineView}, FlamegraphView, PyroscopeSubScreenView, ViewModel};

pub fn render_pyroscope_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    match vm.pyroscope_sub_screen {
        PyroscopeSubScreenView::ServiceList => render_pyroscope_service_list(frame, vm, area),
        PyroscopeSubScreenView::Flamegraph => render_pyroscope_flamegraph_screen(frame, vm, area),
        PyroscopeSubScreenView::Timeline => render_pyroscope_timeline_screen(frame, vm, area),
        PyroscopeSubScreenView::Heatmap => render_pyroscope_heatmap_screen(frame, vm, area),
    }
}

fn render_pyroscope_service_list(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let time_range_span = if vm.pyroscope_time_range_editing {
        Span::styled(
            format!("{}_", vm.pyroscope_time_range),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::styled(vm.pyroscope_time_range.clone(), Style::default().fg(Color::Yellow))
    };

    let mut title_spans = vec![Span::raw(" Services — Last ["), time_range_span, Span::raw("]")];
    if let Some(pt) = vm.pyroscope_profile_types.get(vm.pyroscope_profile_type_index) {
        let n = vm.pyroscope_profile_types.len();
        let idx = vm.pyroscope_profile_type_index + 1;
        title_spans.push(Span::raw("  ·  "));
        title_spans.push(Span::styled(pt.clone(), Style::default().fg(Color::Cyan)));
        if n > 1 {
            title_spans.push(Span::styled(
                format!(" ({idx}/{n})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    title_spans.push(Span::raw(" "));
    let title_line = Line::from(title_spans);

    let block = Block::default().borders(Borders::ALL).title(title_line);

    if vm.pyroscope_series_loading {
        frame.render_widget(
            Paragraph::new("Fetching services…").block(block),
            area,
        );
        return;
    }

    if let Some(ref err) = vm.pyroscope_series_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(block),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let split = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let filter_area = split[0];
    let list_area = split[1];

    // Filter line (like datasource page)
    let filter_line = if vm.pyroscope_service_filter_focused {
        if vm.pyroscope_service_filter.is_empty() {
            Line::from(Span::styled("/ type to filter…", Style::default().fg(Color::DarkGray)))
        } else {
            Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::DarkGray)),
                Span::raw(vm.pyroscope_service_filter.clone()),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        }
    } else if vm.pyroscope_service_filter.is_empty() {
        Line::from(Span::styled("/ to filter", Style::default().fg(Color::DarkGray)))
    } else {
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::DarkGray)),
            Span::raw(vm.pyroscope_service_filter.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    if vm.pyroscope_series.is_empty() {
        let msg = if vm.pyroscope_profile_types.is_empty() {
            "No services found."
        } else if vm.pyroscope_series_loading {
            "Fetching services…"
        } else if !vm.pyroscope_service_filter.is_empty() {
            "No matches."
        } else {
            "No services found."
        };
        frame.render_widget(
            Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
        return;
    }

    let header = Row::new([Cell::from("Service").style(
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )]);

    let rows: Vec<Row> = vm
        .pyroscope_series
        .iter()
        .map(|(service, _)| Row::new([Cell::from(service.clone())]))
        .collect();

    let table = Table::new(rows, [Constraint::Fill(1)])
        .header(header)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut table_state = TableState::default();
    table_state.select(Some(vm.pyroscope_series_index));

    frame.render_stateful_widget(table, list_area, &mut table_state);

    if vm.pyroscope_profile_type_dropdown_open {
        render_profile_type_dropdown(frame, vm, inner);
    }
}

fn render_profile_type_dropdown(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    if vm.pyroscope_profile_types.is_empty() {
        return;
    }
    let max_len = vm.pyroscope_profile_types.iter().map(|s| s.len()).max().unwrap_or(10);
    let popup_w = ((max_len as u16) + 4).min(area.width);
    let popup_h = ((vm.pyroscope_profile_types.len() as u16) + 2).min(area.height);
    let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = vm
        .pyroscope_profile_types
        .iter()
        .map(|pt| ListItem::new(pt.as_str().to_owned()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(vm.pyroscope_profile_type_index));

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Profile Type "))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_pyroscope_flamegraph_screen(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let [info_area, fg_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    render_profile_header(frame, vm, info_area);

    let fg_block = Block::default().borders(Borders::ALL).title(" Icicle Graph ");
    if vm.pyroscope_flamegraph_loading {
        frame.render_widget(
            Paragraph::new("Loading icicle graph…").block(fg_block),
            fg_area,
        );
    } else if let Some(ref err) = vm.pyroscope_flamegraph_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(fg_block),
            fg_area,
        );
    } else if let Some(ref fg) = vm.flamegraph {
        render_flamegraph(frame, fg, fg_block, fg_area);
    } else {
        frame.render_widget(
            Paragraph::new("No flamegraph data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(fg_block),
            fg_area,
        );
    }
}

fn render_flamegraph(frame: &mut Frame, fg: &FlamegraphView, block: Block, area: Rect) {
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width == 0 || fg.root_samples == 0 {
        return;
    }

    // Last row is the status line; remaining rows for icicle levels (root at top).
    let usable_h = inner.height - 1;
    let w = inner.width as u64;
    let n_visible = fg.levels.len().min(usable_h as usize);

    for li in 0..n_visible {
        let row_y = inner.y + li as u16;
        let row_area = Rect::new(inner.x, row_y, inner.width, 1);

        let mut spans: Vec<Span> = Vec::new();
        let mut x_cursor = 0u64;

        for frame_view in &fg.levels[li].frames {
            let x_char = frame_view.x_start * w / fg.root_samples;
            let x_char_end = (frame_view.x_start + frame_view.width) * w / fg.root_samples;
            let char_w = x_char_end.saturating_sub(x_char) as usize;
            if char_w == 0 {
                continue;
            }

            if x_char > x_cursor {
                spans.push(Span::raw(" ".repeat((x_char - x_cursor) as usize)));
            }

            let label = center_truncate(&frame_view.name, char_w);
            let base_style = Style::default().fg(Color::Black).bg(frame_color(&frame_view.name));
            let style = if frame_view.is_selected {
                base_style.add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                base_style
            };
            spans.push(Span::styled(label, style));
            x_cursor = x_char_end;
        }

        // Fill remainder with spaces.
        if x_cursor < w {
            spans.push(Span::raw(" ".repeat((w - x_cursor) as usize)));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }

    // Status line at the bottom of the inner area.
    let status = format!(
        " {} — {:.1}% total, {:.1}% self  [{}]",
        fg.selected_name, fg.selected_total_pct, fg.selected_self_pct, fg.units
    );
    let status_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
        status_area,
    );
}

fn render_profile_header(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let view_label = match vm.pyroscope_sub_screen {
        PyroscopeSubScreenView::Flamegraph => "Flamegraph",
        PyroscopeSubScreenView::Timeline => "Timeline",
        PyroscopeSubScreenView::Heatmap => "Heatmap",
        _ => "",
    };
    let block = Block::default().borders(Borders::ALL).title(" Profile ");
    let content = Line::from(vec![
        Span::styled(
            vm.pyroscope_selected_service.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  "),
        Span::styled(
            vm.pyroscope_selected_profile_type.clone(),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  ·  Last "),
        Span::styled(vm.pyroscope_time_range.clone(), Style::default().fg(Color::Yellow)),
        Span::raw("  ·  "),
        Span::styled(view_label, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(content).block(block), area);
}

fn render_pyroscope_timeline_screen(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let [info_area, chart_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
    render_profile_header(frame, vm, info_area);

    let block = Block::default().borders(Borders::ALL).title(" Timeline ");
    if vm.pyroscope_timeline_loading {
        frame.render_widget(Paragraph::new("Loading timeline…").block(block), chart_area);
        return;
    }
    if let Some(ref err) = vm.pyroscope_timeline_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(block),
            chart_area,
        );
        return;
    }
    if let Some(ref tl) = vm.timeline {
        let unit = ProfileUnit::from_profile_type_id(&vm.pyroscope_selected_profile_type);
        let [vis_area, table_area] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(chart_area);
        render_timeline(frame, tl, block, vis_area, unit);
        render_timeline_exemplars(frame, tl, table_area, unit);
    } else {
        frame.render_widget(
            Paragraph::new("No timeline data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_timeline(frame: &mut Frame, tl: &TimelineView, block: Block, area: Rect, unit: ProfileUnit) {
    let v_max = tl.value_max.max(1.0);
    let v_min = 0.0_f64.min(tl.value_min);

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Green))
        .data(&tl.data);

    let mid_ms = (tl.start_ms + tl.end_ms) / 2;
    let x_axis = Axis::default()
        .style(Style::default().fg(Color::DarkGray))
        .bounds([tl.start_ms as f64, tl.end_ms as f64])
        .labels([
            Span::styled(format_time_label(tl.start_ms), Style::default().fg(Color::DarkGray)),
            Span::styled(format_time_label(mid_ms), Style::default().fg(Color::DarkGray)),
            Span::styled(format_time_label(tl.end_ms), Style::default().fg(Color::DarkGray)),
        ]);

    let y_axis = Axis::default()
        .style(Style::default().fg(Color::DarkGray))
        .bounds([v_min, v_max])
        .labels([
            Span::styled(unit.format(v_min), Style::default().fg(Color::DarkGray)),
            Span::styled(unit.format(v_max / 2.0), Style::default().fg(Color::DarkGray)),
            Span::styled(unit.format(v_max), Style::default().fg(Color::DarkGray)),
        ]);

    let chart = Chart::new(vec![dataset])
        .block(block)
        .x_axis(x_axis)
        .y_axis(y_axis);

    frame.render_widget(chart, area);
}

fn format_time_label(ms: i64) -> String {
    let secs = ms / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn render_pyroscope_heatmap_screen(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let [info_area, chart_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
    render_profile_header(frame, vm, info_area);

    let block = Block::default().borders(Borders::ALL).title(" Heatmap ");
    if vm.pyroscope_heatmap_loading {
        frame.render_widget(Paragraph::new("Loading heatmap…").block(block), chart_area);
        return;
    }
    if let Some(ref err) = vm.pyroscope_heatmap_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(block),
            chart_area,
        );
        return;
    }
    if let Some(ref hm) = vm.heatmap {
        let unit = ProfileUnit::from_profile_type_id(&vm.pyroscope_selected_profile_type);
        let [vis_area, table_area] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(chart_area);
        render_heatmap(frame, hm, block, vis_area, unit);
        render_heatmap_exemplars(frame, hm, table_area, unit);
    } else {
        frame.render_widget(
            Paragraph::new("No heatmap data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_heatmap(frame: &mut Frame, hm: &HeatmapView, block: Block, area: Rect, unit: ProfileUnit) {
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width == 0 || hm.columns.is_empty() || hm.n_buckets == 0 {
        return;
    }

    let usable_h = (inner.height - 1) as usize;
    let w = inner.width as usize;
    let n_cols = hm.columns.len();

    for row in 0..usable_h {
        // Map terminal row → bucket index (row 0 = highest bucket).
        let bucket_idx = row * hm.n_buckets / usable_h;
        let row_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);

        let spans: Vec<Span> = (0..w)
            .map(|col| {
                let slot_start = col * n_cols / w;
                let slot_end = ((col + 1) * n_cols / w).max(slot_start + 1).min(n_cols);
                let intensity = hm.columns[slot_start..slot_end]
                    .iter()
                    .filter_map(|c| c.get(bucket_idx).copied())
                    .fold(0.0_f64, f64::max);
                Span::styled(" ", Style::default().bg(heatmap_color(intensity)))
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }

    let status = format!(" y: {} – {}", unit.format(hm.y_min), unit.format(hm.y_max));
    let status_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
        status_area,
    );
}

fn render_timeline_exemplars(frame: &mut Frame, tl: &TimelineView, area: Rect, unit: ProfileUnit) {
    let block = Block::default().borders(Borders::ALL).title(" Exemplars ");

    let varying = &tl.varying_label_keys;

    let rows_data: Vec<(i64, i64, Vec<String>)> = tl
        .exemplars
        .iter()
        .map(|e| {
            let label_vals = varying
                .iter()
                .map(|k| {
                    e.labels
                        .iter()
                        .find(|(lk, _)| lk == k)
                        .map(|(_, lv)| lv.clone())
                        .unwrap_or_default()
                })
                .collect();
            (e.timestamp_ms, e.value, label_vals)
        })
        .collect();

    let has_profile_id = tl.exemplars.iter().any(|e| !e.profile_id.is_empty());
    let has_span_id = tl.exemplars.iter().any(|e| !e.span_id.is_empty());

    let bold_cyan = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut header_cells = vec![
        Cell::from("Time").style(bold_cyan),
        Cell::from("Value").style(bold_cyan),
    ];
    for k in varying {
        header_cells.push(Cell::from(k.clone()).style(bold_cyan));
    }
    if has_profile_id {
        header_cells.push(Cell::from("Profile ID").style(bold_cyan));
    }
    if has_span_id {
        header_cells.push(Cell::from("Span ID").style(bold_cyan));
    }

    let rows: Vec<Row> = rows_data
        .iter()
        .zip(tl.exemplars.iter())
        .map(|((ts, v, labels), e)| {
            let mut cells = vec![
                Cell::from(format_time_label(*ts)),
                Cell::from(unit.format(*v as f64)),
            ];
            for lv in labels {
                cells.push(Cell::from(lv.clone()));
            }
            if has_profile_id {
                cells.push(Cell::from(e.profile_id.clone()).style(Style::default().fg(Color::DarkGray)));
            }
            if has_span_id {
                cells.push(Cell::from(e.span_id.clone()).style(Style::default().fg(Color::DarkGray)));
            }
            Row::new(cells)
        })
        .collect();

    let mut constraints = vec![Constraint::Fill(1), Constraint::Length(10)];
    for _ in varying {
        constraints.push(Constraint::Fill(1));
    }
    if has_profile_id {
        constraints.push(Constraint::Fill(2));
    }
    if has_span_id {
        constraints.push(Constraint::Length(18));
    }

    let table = Table::new(rows, constraints).header(Row::new(header_cells)).block(block);
    frame.render_widget(table, area);
}

fn render_heatmap_exemplars(frame: &mut Frame, hm: &HeatmapView, area: Rect, unit: ProfileUnit) {
    let block = Block::default().borders(Borders::ALL).title(" Top Values ");

    let n_cols = hm.columns.len();
    if n_cols == 0 || hm.n_buckets == 0 {
        frame.render_widget(Paragraph::new("No data.").block(block), area);
        return;
    }

    let time_span = hm.end_ms - hm.start_ms;
    let y_span = hm.y_max - hm.y_min;

    // One entry per column: find the bucket with the highest intensity.
    let mut entries: Vec<(f64, String, String)> = hm
        .columns
        .iter()
        .enumerate()
        .filter_map(|(c, col)| {
            let (b, &intensity) = col
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
            if intensity <= 0.0 {
                return None;
            }
            let ts_ms = hm.start_ms + c as i64 * time_span / n_cols as i64;
            let y_val = hm.y_min + b as f64 * y_span / hm.n_buckets as f64;
            Some((intensity, format_time_label(ts_ms), unit.format(y_val)))
        })
        .collect();

    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let header = Row::new([
        Cell::from("Time").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Cell::from("Value").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Cell::from("Intensity").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    ]);
    let rows: Vec<Row> = entries
        .iter()
        .map(|(intensity, time, y)| {
            Row::new([
                Cell::from(time.clone()),
                Cell::from(y.clone()),
                Cell::from(format!("{:.2}", intensity)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Fill(1), Constraint::Length(10), Constraint::Length(10)],
    )
    .header(header)
    .block(block);
    frame.render_widget(table, area);
}

fn heatmap_color(v: f64) -> Color {
    const COLORS: &[Color] = &[
        Color::Black,
        Color::Rgb(0, 0, 96),
        Color::Rgb(0, 0, 192),
        Color::Rgb(0, 96, 192),
        Color::Rgb(0, 160, 128),
        Color::Rgb(0, 192, 64),
        Color::Rgb(128, 192, 0),
        Color::Rgb(255, 192, 0),
        Color::Rgb(255, 96, 0),
        Color::Rgb(255, 0, 0),
    ];
    let idx = (v * (COLORS.len() - 1) as f64).round() as usize;
    COLORS[idx.min(COLORS.len() - 1)]
}

fn frame_color(name: &str) -> Color {
    const WARM: [Color; 8] = [
        Color::Red,
        Color::LightRed,
        Color::Rgb(255, 95, 0),
        Color::Rgb(215, 95, 0),
        Color::Rgb(215, 135, 0),
        Color::Rgb(175, 0, 0),
        Color::Yellow,
        Color::LightYellow,
    ];
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    WARM[(hash % WARM.len() as u64) as usize]
}

fn center_truncate(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len <= width {
        format!("{:^width$}", s, width = width)
    } else if width > 1 {
        let truncated: String = s.chars().take(width - 1).collect();
        format!("{}…", truncated)
    } else {
        s.chars().take(width).collect()
    }
}
