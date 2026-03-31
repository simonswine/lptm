use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use shared::{FlamegraphView, PyroscopeSubScreenView, ViewModel};

pub fn render_pyroscope_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    match vm.pyroscope_sub_screen {
        PyroscopeSubScreenView::ServiceList => render_pyroscope_service_list(frame, vm, area),
        PyroscopeSubScreenView::Flamegraph => render_pyroscope_flamegraph_screen(frame, vm, area),
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

    let info_block = Block::default().borders(Borders::ALL).title(" Profile ");
    let info_content = Line::from(vec![
        Span::styled(
            vm.pyroscope_selected_service.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  "),
        Span::styled(
            vm.pyroscope_selected_profile_type.clone(),
            Style::default().fg(Color::Cyan),
        ),
    ]);
    frame.render_widget(Paragraph::new(info_content).block(info_block), info_area);

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
