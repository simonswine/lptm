use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem, ListState,
        Paragraph, Row, Table, TableState, Tabs,
    },
    Frame,
};
use shared::{
    pyroscope::{ProfileUnit, TimelineView},
    time_range::display_label,
    FlamegraphLevelView, FlamegraphView, PyroscopeSubScreenView, SandwichView, ViewModel,
};

/// Returns a 3-char wide spinner frame based on current time, or 3 spaces when not loading.
fn spinner(loading: bool) -> &'static str {
    if !loading {
        return "   ";
    }
    const FRAMES: &[&str] = &[
        " ⠋ ", " ⠙ ", " ⠹ ", " ⠸ ", " ⠼ ", " ⠴ ", " ⠦ ", " ⠧ ", " ⠇ ", " ⠏ ",
    ];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis() as usize;
    FRAMES[(ms / 100) % FRAMES.len()]
}

pub fn render_pyroscope_mode(frame: &mut Frame, vm: &ViewModel, area: Rect, blink_on: bool) {
    match vm.pyroscope_sub_screen {
        PyroscopeSubScreenView::ServiceList => render_pyroscope_service_list(frame, vm, area),
        PyroscopeSubScreenView::Flamegraph => render_pyroscope_flamegraph_screen(frame, vm, area),
        PyroscopeSubScreenView::Timeline => {
            render_pyroscope_timeline_screen(frame, vm, area, blink_on)
        }
        PyroscopeSubScreenView::ProfileHeatmap => {
            render_pyroscope_heatmap_screen(frame, vm, area, false, blink_on)
        }
        PyroscopeSubScreenView::SpanHeatmap => {
            render_pyroscope_heatmap_screen(frame, vm, area, true, blink_on)
        }
    }
}

fn render_pyroscope_service_list(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let time_range_span = Span::styled(
        display_label(&vm.time_range),
        Style::default().fg(Color::Yellow),
    );
    let mut title_spans = vec![Span::raw(" Services — "), time_range_span];
    if let Some(pt) = vm
        .pyroscope_profile_types
        .get(vm.pyroscope_profile_type_index)
    {
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
        frame.render_widget(Paragraph::new("Fetching services…").block(block), area);
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
            Line::from(Span::styled(
                "/ type to filter…",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::DarkGray)),
                Span::raw(vm.pyroscope_service_filter.clone()),
                Span::styled("_", Style::default().fg(Color::Yellow)),
            ])
        }
    } else if vm.pyroscope_service_filter.is_empty() {
        Line::from(Span::styled(
            "/ to filter",
            Style::default().fg(Color::DarkGray),
        ))
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
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
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
    let max_len = vm
        .pyroscope_profile_types
        .iter()
        .map(|s| s.len())
        .max()
        .unwrap_or(10);
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Profile Type "),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_view_tabs(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let selected = match vm.pyroscope_sub_screen {
        PyroscopeSubScreenView::Flamegraph => 0,
        PyroscopeSubScreenView::Timeline => 1,
        PyroscopeSubScreenView::ProfileHeatmap => 2,
        PyroscopeSubScreenView::SpanHeatmap => 3,
        _ => 0,
    };
    let tabs = Tabs::new(vec![
        "Flamegraph",
        "Timeline",
        "Profile Heatmap",
        "Span Heatmap",
    ])
    .select(selected)
    .block(Block::default().borders(Borders::ALL))
    .highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(tabs, area);
}

fn render_pyroscope_flamegraph_screen(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let [info_area, tabs_area, fg_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    render_profile_header(frame, vm, info_area);
    render_view_tabs(frame, vm, tabs_area);

    if let Some(ref sw) = vm.sandwich_view {
        let title = format!(" Sandwich: {}  [s/Esc: exit] ", sw.target_name);
        let block = Block::default().borders(Borders::ALL).title(title);
        render_sandwich_view(frame, sw, block, fg_area);
        return;
    }

    let fg_block = Block::default()
        .borders(Borders::ALL)
        .title(" Icicle Graph ");
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
        render_level_row(
            frame,
            &fg.levels[li],
            w,
            fg.root_samples,
            inner.x,
            inner.y + li as u16,
        );
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

    // Skip outermost callers if they overflow the available height, preserving
    // the most direct callers (at the bottom of the callers slice).
    let n_callers = sw.callers.len() as u16;
    let caller_skip = n_callers.saturating_sub(usable_h.saturating_sub(3)) as usize;
    let mut row = 0u16;

    for level in sw.callers.iter().skip(caller_skip) {
        if row >= usable_h {
            break;
        }
        render_level_row(frame, level, w, samples, inner.x, inner.y + row);
        row += 1;
    }

    // Separator above target (labelled "Callers")
    if row < usable_h {
        let label = " Callers ";
        let dashes = "─".repeat((w as usize).saturating_sub(label.len()));
        let sep = format!("{label}{dashes}");
        frame.render_widget(
            Paragraph::new(sep).style(Style::default().fg(Color::Yellow)),
            Rect::new(inner.x, inner.y + row, inner.width, 1),
        );
        row += 1;
    }

    // Target bar (full width, bright yellow background)
    if row < usable_h {
        let label = center_truncate(&sw.target_name, w as usize);
        let style = Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(label, style))),
            Rect::new(inner.x, inner.y + row, inner.width, 1),
        );
        row += 1;
    }

    // Separator below target (labelled "Callees")
    if row < usable_h {
        let label = " Callees ";
        let dashes = "─".repeat((w as usize).saturating_sub(label.len()));
        let sep = format!("{label}{dashes}");
        frame.render_widget(
            Paragraph::new(sep).style(Style::default().fg(Color::Yellow)),
            Rect::new(inner.x, inner.y + row, inner.width, 1),
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
    let status = format!(
        " {} — {:.1}% of profile  [{}]",
        sw.target_name, pct, sw.units
    );
    let status_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
        status_area,
    );
}

fn render_profile_header(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Profile ");
    let content = Line::from(vec![
        Span::styled(
            vm.pyroscope_selected_service.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  "),
        Span::styled(
            vm.pyroscope_selected_profile_type.clone(),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  ·  "),
        Span::styled(
            display_label(&vm.time_range),
            Style::default().fg(Color::Yellow),
        ),
    ]);
    frame.render_widget(Paragraph::new(content).block(block), area);
}

fn render_pyroscope_timeline_screen(frame: &mut Frame, vm: &ViewModel, area: Rect, blink_on: bool) {
    let [info_area, tabs_area, chart_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    render_profile_header(frame, vm, info_area);
    render_view_tabs(frame, vm, tabs_area);

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
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(chart_area);
        let sel_exemplar = tl.exemplars.get(vm.pyroscope_exemplar_index);
        let highlight = sel_exemplar.map(|e| (e.timestamp_ms as f64, e.value as f64));
        render_timeline(frame, tl, block, vis_area, unit, highlight, blink_on);
        render_timeline_exemplars(frame, tl, table_area, unit, vm.pyroscope_exemplar_index);
    } else {
        frame.render_widget(
            Paragraph::new("No timeline data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_timeline(
    frame: &mut Frame,
    tl: &TimelineView,
    block: Block,
    area: Rect,
    unit: ProfileUnit,
    highlight: Option<(f64, f64)>,
    blink_on: bool,
) {
    let v_max = tl.value_max.max(1.0);
    let v_min = 0.0_f64.min(tl.value_min);

    let y_labels = [
        unit.format(v_min),
        unit.format(v_max / 2.0),
        unit.format(v_max),
    ];
    let mid_ms = (tl.start_ms + tl.end_ms) / 2;

    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Green))
        .data(&tl.data);

    let x_axis = Axis::default()
        .style(Style::default().fg(Color::DarkGray))
        .bounds([tl.start_ms as f64, tl.end_ms as f64])
        .labels([
            Span::styled(
                format_time_label(tl.start_ms),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format_time_label(mid_ms),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format_time_label(tl.end_ms),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

    let y_axis = Axis::default()
        .style(Style::default().fg(Color::DarkGray))
        .bounds([v_min, v_max])
        .labels([
            Span::styled(y_labels[0].clone(), Style::default().fg(Color::DarkGray)),
            Span::styled(y_labels[1].clone(), Style::default().fg(Color::DarkGray)),
            Span::styled(y_labels[2].clone(), Style::default().fg(Color::DarkGray)),
        ]);

    let chart_area = block.inner(area);
    let chart = Chart::new(vec![dataset])
        .block(block)
        .x_axis(x_axis)
        .y_axis(y_axis);
    frame.render_widget(chart, area);

    // Overlay selected exemplar as a blinking 4-dot braille marker.
    if let Some((ts, value)) = highlight {
        // Replicate ratatui Chart::layout() to find the graph_area within chart_area.
        // Y-axis label width (capped at 1/3 of chart width), plus 1 for the axis tick char.
        let y_label_w = y_labels.iter().map(|s| s.len()).max().unwrap_or(0) as u16;
        let y_label_w = y_label_w.min(chart_area.width / 3);
        let graph_x = chart_area.x + y_label_w + 1; // +1 for y-axis tick
        let graph_y = chart_area.y;
        // X-axis occupies 2 rows at the bottom (1 tick + 1 label row).
        let graph_w = chart_area.right().saturating_sub(graph_x);
        let graph_h = chart_area.height.saturating_sub(2);

        if graph_w > 0 && graph_h > 0 {
            let time_span = (tl.end_ms - tl.start_ms).max(1) as f64;
            let val_span = (v_max - v_min).max(f64::EPSILON);
            let col = ((ts - tl.start_ms as f64) / time_span * graph_w as f64) as u16;
            let row = ((v_max - value) / val_span * graph_h as f64) as u16;
            let col = col.min(graph_w - 1);
            let row = row.min(graph_h - 1);
            if blink_on {
                frame.render_widget(
                    Paragraph::new(" ").style(Style::default().bg(Color::Magenta)),
                    Rect::new(graph_x + col, graph_y + row, 1, 1),
                );
            }
        }
    }
}

pub(super) fn format_time_label(ms: i64) -> String {
    let secs = ms / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn render_pyroscope_heatmap_screen(
    frame: &mut Frame,
    vm: &ViewModel,
    area: Rect,
    span: bool,
    blink_on: bool,
) {
    let [info_area, tabs_area, chart_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    render_profile_header(frame, vm, info_area);
    render_view_tabs(frame, vm, tabs_area);

    let (loading, error, heatmap_data, base_title) = if span {
        (
            vm.pyroscope_span_heatmap_loading,
            vm.pyroscope_span_heatmap_error.as_deref(),
            vm.span_heatmap.as_ref(),
            " Span Heatmap",
        )
    } else {
        (
            vm.pyroscope_heatmap_loading,
            vm.pyroscope_heatmap_error.as_deref(),
            vm.heatmap.as_ref(),
            " Profile Heatmap",
        )
    };

    let hm_title = format!("{}{}", base_title, spinner(loading));
    let block = Block::default().borders(Borders::ALL).title(hm_title);
    if loading {
        frame.render_widget(Paragraph::new("Loading…").block(block), chart_area);
        return;
    }
    if let Some(err) = error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(block),
            chart_area,
        );
        return;
    }
    if let Some(hm) = heatmap_data {
        let unit = ProfileUnit::from_profile_type_id(&vm.pyroscope_selected_profile_type);
        let exemplars: Vec<_> = hm.exemplars.iter().collect();
        let [vis_area, table_area] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(chart_area);
        let sel_exemplar = hm.exemplars.get(vm.pyroscope_exemplar_index);
        let hm_highlight = sel_exemplar.map(|e| (e.timestamp_ms, e.value as f64));
        super::heatmap::render_heatmap(frame, hm, block, vis_area, unit, hm_highlight, blink_on);
        let trace_lookup = if span {
            Some(&vm.span_trace_lookup)
        } else {
            None
        };
        let trace_loading = span && vm.span_trace_loading;
        render_exemplars(
            frame,
            &exemplars,
            &hm.varying_label_keys,
            table_area,
            unit,
            vm.pyroscope_exemplar_index,
            trace_lookup,
            trace_loading,
        );
        if span && vm.tempo_datasource_picker_open {
            render_tempo_datasource_picker(frame, vm, chart_area);
        }
        if span && vm.exemplar_detail_open {
            render_exemplar_detail(frame, vm, chart_area, unit);
        }
    } else {
        frame.render_widget(
            Paragraph::new("No heatmap data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_timeline_exemplars(
    frame: &mut Frame,
    tl: &TimelineView,
    area: Rect,
    unit: ProfileUnit,
    selected_idx: usize,
) {
    let exemplars: Vec<_> = tl.exemplars.iter().collect();
    render_exemplars(
        frame,
        &exemplars,
        &tl.varying_label_keys,
        area,
        unit,
        selected_idx,
        None,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_exemplars(
    frame: &mut Frame,
    exemplars: &[&shared::pyroscope::types::TimelineExemplar],
    varying_label_keys: &[String],
    area: Rect,
    unit: ProfileUnit,
    selected_idx: usize,
    trace_lookup: Option<&std::collections::HashMap<String, String>>,
    trace_loading: bool,
) {
    let title = format!(" Exemplars{}", spinner(trace_loading));
    let block = Block::default().borders(Borders::ALL).title(title);

    if exemplars.is_empty() {
        frame.render_widget(
            Paragraph::new("No exemplars.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let has_profile_id = exemplars.iter().any(|e| !e.profile_id.is_empty());
    let has_span_id = exemplars.iter().any(|e| !e.span_id.is_empty());
    let has_trace_id = trace_lookup.is_some_and(|m| !m.is_empty());

    let bold_cyan = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut header_cells = vec![
        Cell::from("Time").style(bold_cyan),
        Cell::from("Value").style(bold_cyan),
    ];
    for k in varying_label_keys {
        header_cells.push(Cell::from(k.clone()).style(bold_cyan));
    }
    if has_profile_id {
        header_cells.push(Cell::from("Profile ID").style(bold_cyan));
    }
    if has_span_id {
        header_cells.push(Cell::from("Span ID").style(bold_cyan));
    }
    if has_trace_id {
        header_cells.push(Cell::from("Trace ID").style(bold_cyan));
    }

    let rows: Vec<Row> = exemplars
        .iter()
        .map(|e| {
            let mut cells = vec![
                Cell::from(format_time_label(e.timestamp_ms)),
                Cell::from(unit.format(e.value as f64)),
            ];
            for k in varying_label_keys {
                let lv = e
                    .labels
                    .iter()
                    .find(|(lk, _)| lk == k)
                    .map(|(_, lv)| lv.clone())
                    .unwrap_or_default();
                cells.push(Cell::from(lv));
            }
            if has_profile_id {
                cells.push(
                    Cell::from(e.profile_id.clone()).style(Style::default().fg(Color::DarkGray)),
                );
            }
            if has_span_id {
                cells.push(
                    Cell::from(e.span_id.clone()).style(Style::default().fg(Color::DarkGray)),
                );
            }
            if has_trace_id {
                let trace_id = trace_lookup
                    .and_then(|m| m.get(&e.span_id))
                    .cloned()
                    .unwrap_or_default();
                let style = if trace_id.is_empty() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                cells.push(Cell::from(trace_id).style(style));
            }
            Row::new(cells)
        })
        .collect();

    let mut constraints = vec![Constraint::Fill(1), Constraint::Length(10)];
    for _ in varying_label_keys {
        constraints.push(Constraint::Fill(1));
    }
    if has_profile_id {
        constraints.push(Constraint::Fill(2));
    }
    if has_span_id {
        constraints.push(Constraint::Length(18));
    }
    if has_trace_id {
        constraints.push(Constraint::Length(32));
    }

    let clamped = selected_idx.min(exemplars.len().saturating_sub(1));
    let table = Table::new(rows, constraints)
        .header(Row::new(header_cells))
        .block(block)
        .highlight_symbol(">> ")
        .row_highlight_style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    let mut table_state = TableState::default();
    table_state.select(Some(clamped));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_exemplar_detail(frame: &mut Frame, vm: &ViewModel, area: Rect, unit: ProfileUnit) {
    let hm = match vm.span_heatmap.as_ref() {
        Some(h) => h,
        None => return,
    };
    let exemplar = match hm.exemplars.get(vm.pyroscope_exemplar_index) {
        Some(e) => e,
        None => return,
    };

    let trace_id = vm
        .span_trace_lookup
        .get(&exemplar.span_id)
        .cloned()
        .unwrap_or_default();
    let has_trace = !trace_id.is_empty();
    let has_profile = !exemplar.profile_id.is_empty();

    let dim = Style::default().fg(Color::DarkGray);
    let val = Style::default().fg(Color::White);
    let cyan = Style::default().fg(Color::Cyan);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("time:       ", dim),
            Span::styled(format_time_label(exemplar.timestamp_ms), val),
        ]),
        Line::from(vec![
            Span::styled("value:      ", dim),
            Span::styled(unit.format(exemplar.value as f64), val),
        ]),
    ];
    for (k, v) in &exemplar.labels {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", k), dim),
            Span::styled(v.clone(), val),
        ]));
    }
    if !exemplar.span_id.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("span id:    ", dim),
            Span::styled(exemplar.span_id.clone(), val),
        ]));
    }
    if has_trace {
        lines.push(Line::from(vec![
            Span::styled("trace id:   ", dim),
            Span::styled(trace_id, cyan),
        ]));
    }
    if has_profile {
        lines.push(Line::from(vec![
            Span::styled("profile id: ", dim),
            Span::styled(exemplar.profile_id.clone(), val),
        ]));
    }

    // Action hints
    lines.push(Line::from(""));
    if has_trace {
        lines.push(Line::from(Span::styled(
            "Ctrl+T  open trace in Tempo",
            Style::default().fg(Color::Yellow),
        )));
    }
    if has_profile {
        lines.push(Line::from(Span::styled(
            "Ctrl+P  open span profile",
            Style::default().fg(Color::Yellow),
        )));
    }

    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(20) as u16;
    let popup_w = (content_w + 4).min(area.width.saturating_sub(4)).max(36);
    let popup_h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Exemplar Detail "),
            )
            .style(Style::default().fg(Color::White)),
        popup_area,
    );
}

fn render_tempo_datasource_picker(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    if vm.tempo_datasources.is_empty() {
        return;
    }

    let max_name_len = vm
        .tempo_datasources
        .iter()
        .map(|d| d.name.len())
        .max()
        .unwrap_or(10);
    let popup_w = (max_name_len as u16 + 6)
        .min(area.width.saturating_sub(4))
        .max(24);
    let popup_h = (vm.tempo_datasources.len() as u16 + 2)
        .min(area.height.saturating_sub(4))
        .max(3);
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let items: Vec<ListItem> = vm
        .tempo_datasources
        .iter()
        .map(|ds| {
            let fav = if ds.is_favourite { "★ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(fav, Style::default().fg(Color::Yellow)),
                Span::raw(ds.name.clone()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Tempo Datasource "),
        )
        .highlight_symbol(">> ")
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = ListState::default();
    list_state.select(Some(vm.tempo_picker_index));

    frame.render_widget(Clear, popup_area);
    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

pub(super) fn heatmap_color(v: f64) -> Color {
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
    let without_args = if let Some(p) = label.find('(') {
        &label[..p]
    } else {
        label
    };
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
        (223, 139, 83),  // h:24  s:69%  l:60%
        (224, 173, 108), // h:34  s:65%  l:65%
        (104, 183, 207), // h:194 s:52%  l:61%
        (89, 192, 163),  // h:163 s:45%  l:55%
        (104, 151, 202), // h:211 s:48%  l:60%
        (137, 130, 201), // h:246 s:40%  l:65%
        (235, 168, 230), // h:305 s:63%  l:79%
        (255, 225, 117), // h:47  s:100% l:73%
        (183, 219, 171),
        (244, 213, 152),
        (78, 146, 249),
        (249, 186, 143),
        (242, 145, 145),
        (130, 181, 216),
        (229, 168, 226),
        (174, 162, 224),
        (154, 196, 138),
        (242, 201, 109),
        (101, 197, 219),
        (249, 147, 78),
        (234, 100, 96),
        (81, 149, 206),
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
