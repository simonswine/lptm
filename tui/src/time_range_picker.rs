use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use shared::{
    time_range::PRESETS,
    time_range_picker::{FOCUS_ABS_FROM, FOCUS_ABS_TO, FOCUS_PRESETS, DATETIME_TEMPLATE},
    ViewModel,
};

const TITLE: &str = " Time Range  ↑↓/jk: preset · Tab: From/To · ←/→: cursor · Enter: apply · Esc: cancel ";

pub fn render_time_range_picker(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    if !vm.time_range_picker_open {
        return;
    }

    let popup_w = (TITLE.len() as u16 + 2).max(44).min(area.width);
    // 2 input rows + 1 blank separator + PRESETS rows + 2 borders
    let popup_h = (PRESETS.len() as u16 + 4).min(area.height);
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let block = Block::default().borders(Borders::ALL).title(TITLE);
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let [inputs_area, _sep, list_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    let [from_area, to_area] =
        Layout::vertical([Constraint::Length(1); 2]).areas(inputs_area);

    render_datetime_row(
        frame,
        from_area,
        "From:",
        &vm.time_range_picker_abs_from,
        vm.time_range_picker_focus == FOCUS_ABS_FROM,
        vm.time_range_picker_from_cursor,
    );
    render_datetime_row(
        frame,
        to_area,
        "To:  ",
        &vm.time_range_picker_abs_to,
        vm.time_range_picker_focus == FOCUS_ABS_TO,
        vm.time_range_picker_to_cursor,
    );

    let items: Vec<ListItem> = PRESETS
        .iter()
        .map(|&p| {
            let label = format!("Last {p}");
            if p == vm.time_range.as_str() {
                ListItem::new(label).style(Style::default().fg(Color::Cyan))
            } else {
                ListItem::new(label)
            }
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    let mut list_state = ListState::default();
    if vm.time_range_picker_focus == FOCUS_PRESETS {
        list_state.select(Some(vm.time_range_picker_index));
    }

    frame.render_stateful_widget(list, list_area, &mut list_state);
}

fn render_datetime_row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: &str,
    focused: bool,
    cursor: usize,
) {
    let label_span = Span::styled(label, Style::default().fg(Color::DarkGray));

    if !focused {
        // Not focused: show the value (or template if empty) in plain style.
        let display = if value.is_empty() {
            DATETIME_TEMPLATE
        } else {
            value
        };
        let line = Line::from(vec![
            label_span,
            Span::raw(" "),
            Span::styled(display.to_owned(), Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // Focused: render character-by-character, highlighting the cursor position.
    // Use the template for any missing characters.
    let display: Vec<char> = {
        let val_chars: Vec<char> = value.chars().collect();
        DATETIME_TEMPLATE
            .chars()
            .enumerate()
            .map(|(i, tmpl_c)| val_chars.get(i).copied().unwrap_or(tmpl_c))
            .collect()
    };

    let cursor_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(Color::Yellow);
    let sep_style = Style::default().fg(Color::DarkGray);

    let mut spans = vec![label_span, Span::raw(" ")];
    for (i, ch) in display.iter().enumerate() {
        let s = String::from(*ch);
        let span = if i == cursor {
            Span::styled(s, cursor_style)
        } else if matches!(i, 4 | 7 | 10 | 13 | 16) {
            Span::styled(s, sep_style)
        } else {
            Span::styled(s, normal_style)
        };
        spans.push(span);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
