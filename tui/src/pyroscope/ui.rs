use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::{pyroscope::{HeatmapView, TimelineView}, FlamegraphView, PyroscopeSubScreenView, ViewModel};

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
        render_timeline(frame, tl, block, chart_area);
    } else {
        frame.render_widget(
            Paragraph::new("No timeline data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_timeline(frame: &mut Frame, tl: &TimelineView, block: Block, area: Rect) {
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if tl.data.is_empty() || inner.height < 3 || inner.width == 0 {
        return;
    }

    // Reserve bottom row for x-axis labels, second-to-last for status.
    let bar_rows = inner.height.saturating_sub(2) as usize;
    let w = inner.width as usize;
    let n = tl.data.len();

    let v_max = tl.value_max.max(1.0);
    let v_min = 0.0_f64.min(tl.value_min);
    let v_range = (v_max - v_min).max(1.0);

    // Resample: one bar per terminal column, take the max value in each bucket.
    let bars: Vec<f64> = (0..w)
        .map(|col| {
            let start = col * n / w;
            let end = ((col + 1) * n / w).max(start + 1).min(n);
            let max_v = tl.data[start..end]
                .iter()
                .map(|(_, v)| *v)
                .fold(f64::NEG_INFINITY, f64::max);
            ((max_v - v_min) / v_range).clamp(0.0, 1.0)
        })
        .collect();

    // ▁▂▃▄▅▆▇  (index 0 = empty, 1..7 = partial heights, full = █)
    const PARTIAL: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

    for row in 0..bar_rows {
        let row_from_bottom = bar_rows - 1 - row;
        let spans: Vec<Span> = bars
            .iter()
            .map(|&v| {
                let bar_h = v * bar_rows as f64;
                let full = bar_h.floor() as usize;
                let frac = bar_h - full as f64;
                let (ch, fg) = if row_from_bottom < full {
                    ('█', Color::Green)
                } else if row_from_bottom == full && frac > 0.0 {
                    let idx = ((frac * PARTIAL.len() as f64) as usize).min(PARTIAL.len() - 1);
                    (PARTIAL[idx], Color::Green)
                } else {
                    (' ', Color::Reset)
                };
                Span::styled(ch.to_string(), Style::default().fg(fg))
            })
            .collect();
        let row_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }

    // X-axis labels row.
    let start_lbl = format_time_label(tl.start_ms);
    let end_lbl = format_time_label(tl.end_ms);
    let mid_lbl = format_time_label((tl.start_ms + tl.end_ms) / 2);
    let x_row = Rect::new(inner.x, inner.y + bar_rows as u16, inner.width, 1);
    let pad_right = inner.width.saturating_sub(
        (start_lbl.len() + mid_lbl.len() + end_lbl.len()) as u16,
    ) / 2;
    let x_line = Line::from(vec![
        Span::styled(start_lbl, Style::default().fg(Color::DarkGray)),
        Span::raw(" ".repeat(pad_right as usize)),
        Span::styled(mid_lbl, Style::default().fg(Color::DarkGray)),
        Span::raw(" ".repeat(pad_right as usize)),
        Span::styled(end_lbl, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(x_line), x_row);

    // Status / y-range row.
    let status = format!(
        " 0 – {}  ({} points)",
        format_value_label(v_max),
        n,
    );
    let status_row = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
        status_row,
    );
}

fn format_time_label(ms: i64) -> String {
    let secs = ms / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_value_label(v: f64) -> String {
    let abs = v.abs();
    if abs >= 1_000_000_000.0 {
        format!("{:.1}G", v / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{v:.1}")
    }
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
        render_heatmap(frame, hm, block, chart_area);
    } else {
        frame.render_widget(
            Paragraph::new("No heatmap data.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            chart_area,
        );
    }
}

fn render_heatmap(frame: &mut Frame, hm: &HeatmapView, block: Block, area: Rect) {
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

    let status = format!(" y: {:.0} – {:.0}", hm.y_min, hm.y_max);
    let status_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Cyan)),
        status_area,
    );
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
