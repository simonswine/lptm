use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table},
    Frame,
};
use shared::{QueryResultsView, ViewModel};

use crate::highlight;

pub fn render_query_mode(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let split = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    let query_block = Block::default()
        .borders(Borders::ALL)
        .title(" PromQL ");

    let query_line: Line = highlight::highlight_query(&vm.query, vm.cursor_pos);
    let query_text = Paragraph::new(query_line).block(query_block);
    frame.render_widget(query_text, split[0]);

    let results_area = split[1];
    let results_block = Block::default().borders(Borders::ALL).title(" Results ");

    if vm.query_loading {
        let loading = Paragraph::new("Executing query…").block(results_block);
        frame.render_widget(loading, results_area);
    } else if let Some(ref err) = vm.query_error {
        let error = Paragraph::new(format!("Error: {err}"))
            .style(Style::default().fg(Color::Red))
            .block(results_block);
        frame.render_widget(error, results_area);
    } else if let Some(ref results) = vm.query_results {
        if results.rows.is_empty() {
            let empty = Paragraph::new("No results.").block(results_block);
            frame.render_widget(empty, results_area);
        } else {
            render_results_table(frame, results, results_block, results_area);
        }
    } else {
        let hint = Paragraph::new("Type a PromQL expression and press Enter.")
            .style(Style::default().fg(Color::DarkGray))
            .block(results_block);
        frame.render_widget(hint, results_area);
    }

    render_completion_popup(frame, vm, results_area);
}

fn render_completion_popup(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let has_completions = !vm.completions.is_empty();
    let show_loading = vm.completions_loading && !has_completions;

    if !has_completions && !show_loading {
        return;
    }

    if show_loading {
        let popup_w = 20u16.min(area.width);
        let popup_h = 3u16.min(area.height);
        let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);
        frame.render_widget(Clear, popup_area);
        let loading = Paragraph::new("Loading…")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(loading, popup_area);
        return;
    }

    let max_len = vm.completions.iter().map(|s| s.len()).max().unwrap_or(10);
    let popup_w = ((max_len as u16) + 4).max(20).min(area.width);
    let popup_h = ((vm.completions.len() as u16) + 2).min(12).min(area.height);
    let popup_area = Rect::new(area.x, area.y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = vm
        .completions
        .iter()
        .map(|c| ListItem::new(c.as_str().to_owned()))
        .collect();

    let mut list_state = ListState::default();
    list_state.select(vm.completion_index);

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Completions "))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}

fn render_results_table(
    frame: &mut Frame,
    results: &QueryResultsView,
    block: Block,
    area: Rect,
) {
    let header_cells: Vec<Cell> = results
        .columns
        .iter()
        .map(|col| {
            Cell::from(col.as_str().to_owned()).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    let header = Row::new(header_cells);

    let rows: Vec<Row> = results
        .rows
        .iter()
        .map(|row| Row::new(row.iter().map(|cell| Cell::from(cell.as_str().to_owned()))))
        .collect();

    let n = results.columns.len().max(1);
    let widths: Vec<Constraint> = (0..n).map(|_| Constraint::Fill(1)).collect();

    let table = Table::new(rows, widths).header(header).block(block);
    frame.render_widget(table, area);
}
