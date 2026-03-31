use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::{
    pyroscope::{HeatmapView, ProfileUnit, TimelineView},
    FlamegraphLevelView, FlamegraphView, PyroscopeSubScreenView, SandwichView, ViewModel,
};

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

    if let Some(ref sw) = vm.sandwich_view {
        let title = format!(" Sandwich: {}  [s/Esc: exit] ", sw.target_name);
        let block = Block::default().borders(Borders::ALL).title(title);
        render_sandwich_view(frame, sw, block, fg_area);
        return;
    }

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

/// Render one level row into the terminal at `(x, y)`, normalised to `root_samples`.
fn render_level_row(
    frame: &mut Frame,
    level: &FlamegraphLevelView,
    w: u64,
    root_samples: u64,
    x: u16,
    y: u16,
) {
    let row_area = Rect::new(x, y, w as u16, 1);
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = 0u64;

    for fv in &level.frames {
        let xc = fv.x_start * w / root_samples;
        let xc_end = (fv.x_start + fv.width) * w / root_samples;
        let cw = xc_end.saturating_sub(xc) as usize;
        if cw == 0 {
            continue;
        }
        if xc > cursor {
            spans.push(Span::raw(" ".repeat((xc - cursor) as usize)));
        }
        let label = center_truncate(&fv.name, cw);
        let base_style = Style::default().fg(Color::Black).bg(frame_color(&fv.name));
        let style = if fv.is_selected {
            base_style.add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            base_style
        };
        spans.push(Span::styled(label, style));
        cursor = xc_end;
    }
    if cursor < w {
        spans.push(Span::raw(" ".repeat((w - cursor) as usize)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
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
        render_level_row(frame, &fg.levels[li], w, fg.root_samples, inner.x, inner.y + li as u16);
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

fn render_sandwich_view(frame: &mut Frame, sw: &SandwichView, block: Block, area: Rect) {
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width == 0 || sw.target_samples == 0 {
        return;
    }

    let usable_h = inner.height - 1; // last row = status line
    let w = inner.width as u64;
    let samples = sw.target_samples;
    let mut row = 0u16;

    // Callers (outermost first = top of screen)
    for level in &sw.callers {
        if row >= usable_h {
            break;
        }
        render_level_row(frame, level, w, samples, inner.x, inner.y + row);
        row += 1;
    }

    // Target bar (full width, highlighted)
    if row < usable_h {
        let label = center_truncate(&sw.target_name, w as usize);
        let style = Style::default()
            .fg(Color::Black)
            .bg(frame_color(&sw.target_name))
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        let row_area = Rect::new(inner.x, inner.y + row, inner.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(label, style))),
            row_area,
        );
        row += 1;
    }

    // Callees (direct callee first = just below target)
    for level in &sw.callees {
        if row >= usable_h {
            break;
        }
        render_level_row(frame, level, w, samples, inner.x, inner.y + row);
        row += 1;
    }

    // Status line
    let pct = sw.target_samples as f64 / sw.root_samples as f64 * 100.0;
    let status = format!(" {} — {:.1}% of profile  [{}]", sw.target_name, pct, sw.units);
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

/// Port of murmurhash3_32_gc from @grafana/flamegraph/murmur3.ts (seed=0).
fn murmurhash3_32(key: &str, seed: u32) -> u32 {
    let bytes = key.as_bytes();
    let len = bytes.len();
    let n_blocks = len / 4;

    let mut h1: u32 = seed;
    const C1: u32 = 0xcc9e2d51;
    const C2: u32 = 0x1b873593;

    let mut i = 0;
    for _ in 0..n_blocks {
        let k1 = (bytes[i] as u32)
            | ((bytes[i + 1] as u32) << 8)
            | ((bytes[i + 2] as u32) << 16)
            | ((bytes[i + 3] as u32) << 24);
        i += 4;
        let k1 = k1.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h1 ^= k1;
        h1 = h1.rotate_left(13);
        h1 = h1.wrapping_mul(5).wrapping_add(0xe6546b64);
    }

    let mut k1: u32 = 0;
    let remainder = len & 3;
    if remainder >= 3 {
        k1 ^= (bytes[i + 2] as u32) << 16;
    }
    if remainder >= 2 {
        k1 ^= (bytes[i + 1] as u32) << 8;
    }
    if remainder >= 1 {
        k1 ^= bytes[i] as u32;
    }
    let k1 = k1.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
    h1 ^= k1;

    h1 ^= len as u32;
    h1 ^= h1 >> 16;
    h1 = h1.wrapping_mul(0x85ebca6b);
    h1 ^= h1 >> 13;
    h1 = h1.wrapping_mul(0xc2b2ae35);
    h1 ^= h1 >> 16;
    h1
}

/// Extract the package name from a profiling symbol label, mirroring the
/// logic in @grafana/flamegraph/FlameGraph/colors.ts → getPackageName().
fn get_package_name(label: &str) -> &str {
    // PHP / Python / Ruby: directory portion before the source file
    for ext in &[".php", ".py", ".rb"] {
        if let Some(pos) = label.find(ext) {
            if let Some(slash) = label[..pos].rfind('/') {
                return &label[..slash + 1];
            }
        }
    }

    // Rust / C++: "crate::module::func" → "crate"
    if let Some(pos) = label.find("::") {
        return &label[..pos];
    }

    // Node.js: "(./node_modules/)pkg/file.js:func:line"
    if label.contains(':') {
        if let Some(colon) = label.find(':') {
            let path = &label[..colon];
            let path = path.strip_prefix("./node_modules/").unwrap_or(path);
            if let Some(slash) = path.find('/') {
                return &path[..slash];
            }
            return path;
        }
    }

    // Go / Java: "github.com/pkg/foo.Func" → "github.com/pkg/foo."
    if label.contains('/') {
        if let Some(last_slash) = label.rfind('/') {
            let after = &label[last_slash + 1..];
            if let Some(dot) = after.find('.') {
                return &label[..last_slash + 1 + dot + 1];
            }
            return &label[..last_slash + 1];
        }
    }

    // dotnet / simple Go: "runtime.goroutine" → "runtime."
    let without_args = if let Some(p) = label.find('(') { &label[..p] } else { label };
    if let Some(dot) = without_args.find('.') {
        return &label[..dot + 1];
    }

    label
}

/// Color a flamegraph frame using the same palette as @grafana/flamegraph.
/// Colors are keyed by package name (murmurhash3 % 24).
fn frame_color(name: &str) -> Color {
    // Palette from @grafana/flamegraph/FlameGraph/colors.ts → packageColors
    // HSL entries converted to RGB; RGB entries taken directly.
    const PALETTE: [(u8, u8, u8); 24] = [
        (223, 139,  83), // h:24  s:69%  l:60%
        (224, 173, 108), // h:34  s:65%  l:65%
        (104, 183, 207), // h:194 s:52%  l:61%
        ( 89, 192, 163), // h:163 s:45%  l:55%
        (104, 151, 202), // h:211 s:48%  l:60%
        (137, 130, 201), // h:246 s:40%  l:65%
        (235, 168, 230), // h:305 s:63%  l:79%
        (255, 225, 117), // h:47  s:100% l:73%
        (183, 219, 171),
        (244, 213, 152),
        ( 78, 146, 249),
        (249, 186, 143),
        (242, 145, 145),
        (130, 181, 216),
        (229, 168, 226),
        (174, 162, 224),
        (154, 196, 138),
        (242, 201, 109),
        (101, 197, 219),
        (249, 147,  78),
        (234, 100,  96),
        ( 81, 149, 206),
        (214, 131, 206),
        (128, 110, 183),
    ];
    let pkg = get_package_name(name);
    let hash = murmurhash3_32(pkg, 0);
    let (r, g, b) = PALETTE[hash as usize % PALETTE.len()];
    Color::Rgb(r, g, b)
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
