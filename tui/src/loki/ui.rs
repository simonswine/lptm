use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
    Frame,
};
use shared::{time_range::display_label, LokiDiagnosticView, ViewModel};
use tui_textarea::TextArea;

use super::lsp::CompletionItem;

// ── LokiUiState ──────────────────────────────────────────────────────────────

/// All per-session Loki UI state that lives outside the Crux model.
pub struct LokiUiState {
    /// The query text input widget.
    pub textarea: TextArea<'static>,
    /// Completions received from the LSP server.
    pub completions: Vec<CompletionItem>,
    /// Which completion is currently highlighted (None = none / first auto).
    pub completion_index: Option<usize>,
    /// True if the user dismissed the popup (reset on next keystroke).
    pub completion_dismissed: bool,
    /// Diagnostics received from `textDocument/publishDiagnostics`.
    pub diagnostics: Vec<LokiDiagnosticView>,
    /// True when the query has been changed since the last execution.
    pub query_dirty: bool,
}

/// Create a fresh textarea with the standard LogQL styling (border + title).
pub fn styled_textarea() -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_block(Block::default().borders(Borders::ALL).title(" LogQL "));
    ta.set_cursor_line_style(Style::default()); // no cursor-line highlight
    ta.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    ta
}

impl LokiUiState {
    pub fn new() -> Self {
        LokiUiState {
            textarea: styled_textarea(),
            completions: vec![],
            completion_index: None,
            completion_dismissed: false,
            diagnostics: vec![],
            query_dirty: false,
        }
    }

    /// Current query text (single line).
    pub fn query(&self) -> &str {
        self.textarea
            .lines()
            .first()
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// Cursor column (0-based UTF-16 character offset, suitable for LSP).
    pub fn cursor_char_col(&self) -> u32 {
        let (_, col) = self.textarea.cursor();
        col as u32
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

pub fn render_loki_mode(frame: &mut Frame, vm: &ViewModel, area: Rect, loki_state: &LokiUiState) {
    if vm.loki_context_open {
        render_context_view(frame, vm, area);
        return;
    }

    // Layout: query box (3 lines) + diagnostics line (1) + results (rest)
    let has_diagnostics = !loki_state.diagnostics.is_empty();
    let info_height = if has_diagnostics { 1 } else { 0 };

    let split = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(info_height),
        Constraint::Min(0),
    ])
    .split(area);

    // ── Query input (tui-textarea) ────────────────────────────────────────────
    frame.render_widget(&loki_state.textarea, split[0]);

    // ── Diagnostic info line ──────────────────────────────────────────────────
    if has_diagnostics {
        let info_line = build_info_line(&loki_state.diagnostics, split[1].width);
        frame.render_widget(Paragraph::new(info_line), split[1]);
    }

    // ── Results area ─────────────────────────────────────────────────────────
    let results_area = split[2];
    let results_block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::raw(" Logs — "),
            Span::styled(
                display_label(&vm.time_range),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
        ]));

    if vm.loki_loading {
        frame.render_widget(
            Paragraph::new("Searching\u{2026}").block(results_block),
            results_area,
        );
        render_completion_popup(frame, loki_state, split[0], results_area);
        return;
    }

    if let Some(ref err) = vm.loki_error {
        frame.render_widget(
            Paragraph::new(format!("Error: {err}"))
                .style(Style::default().fg(Color::Red))
                .block(results_block),
            results_area,
        );
        render_completion_popup(frame, loki_state, split[0], results_area);
        return;
    }

    if vm.loki_results.is_empty() {
        let hint = if loki_state.query().trim().is_empty() {
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
        render_completion_popup(frame, loki_state, split[0], results_area);
        return;
    }

    let header = Row::new(["Timestamp", "Labels", "Log Line"].iter().map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }));

    let rows: Vec<Row> = vm
        .loki_results
        .iter()
        .flat_map(|stream| {
            stream.entries.iter().map(move |entry| {
                Row::new(vec![
                    Cell::from(entry.timestamp.clone()),
                    Cell::from(stream.labels.clone()).style(Style::default().fg(Color::Yellow)),
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

    render_completion_popup(frame, loki_state, split[0], results_area);
}

// ── Completion popup ──────────────────────────────────────────────────────────

fn render_completion_popup(
    frame: &mut Frame,
    loki_state: &LokiUiState,
    query_area: Rect,
    results_area: Rect,
) {
    if loki_state.completion_dismissed || loki_state.completions.is_empty() {
        return;
    }

    let items = &loki_state.completions;

    // Position popup just below the query box, aligned to cursor column.
    let (_, cursor_col) = loki_state.textarea.cursor();
    // +1 for the border, +2 for "kind_icon " prefix in the widget
    let popup_x = (query_area.x + 1 + cursor_col as u16)
        .min(query_area.x + query_area.width.saturating_sub(20));
    let popup_y = query_area.y + 3; // just below the 3-line query box

    // Width: longest label + detail + icon
    let max_label_len = items.iter().map(|c| c.label.len()).max().unwrap_or(10);
    let max_detail_len = items
        .iter()
        .filter_map(|c| c.detail.as_ref())
        .map(|d| d.len())
        .max()
        .unwrap_or(0);
    let content_w = 4
        + max_label_len
        + if max_detail_len > 0 {
            2 + max_detail_len
        } else {
            0
        };
    let popup_w = ((content_w as u16) + 4)
        .max(20)
        .min(results_area.x + results_area.width - popup_x);
    let popup_h = ((items.len() as u16) + 2)
        .min(12)
        .min(results_area.y + results_area.height - popup_y);

    if popup_w == 0 || popup_h == 0 {
        return;
    }

    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);
    frame.render_widget(Clear, popup_area);

    let list_items: Vec<ListItem> = items
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
    list_state.select(loki_state.completion_index);

    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Completions "),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::White));

    frame.render_stateful_widget(list, popup_area, &mut list_state);

    // Documentation panel below the popup.
    if let Some(idx) = loki_state.completion_index {
        if let Some(item) = items.get(idx) {
            if let Some(ref doc) = item.documentation {
                let doc_y = popup_area.y + popup_area.height;
                if doc_y < results_area.y + results_area.height {
                    let doc_w = popup_w
                        .max(doc.len() as u16 + 4)
                        .min(results_area.x + results_area.width - popup_x);
                    let doc_h = 3u16.min(results_area.y + results_area.height - doc_y);
                    let doc_area = Rect::new(popup_x, doc_y, doc_w, doc_h);
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

// ── Diagnostic info line ──────────────────────────────────────────────────────

fn build_info_line(diagnostics: &[LokiDiagnosticView], max_width: u16) -> Line<'static> {
    let mut parts: Vec<Span<'static>> = Vec::new();
    let max_w = max_width as usize;

    let diag = diagnostics.first();
    if let Some(diag) = diag {
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
        let icon_w = icon.chars().count();

        let count = diagnostics.len();
        let count_text = if count > 1 {
            format!(" [{count}]")
        } else {
            String::new()
        };
        let count_w = count_text.chars().count();

        let budget = max_w.saturating_sub(icon_w + count_w);
        let msg_chars: Vec<char> = diag.message.chars().collect();

        let display = if msg_chars.len() > budget && budget > 1 {
            let truncated: String = msg_chars[..budget - 1].iter().collect();
            format!("{icon}{truncated}\u{2026}")
        } else if budget == 0 {
            String::new()
        } else {
            format!("{icon}{}", diag.message)
        };

        if !display.is_empty() {
            parts.push(Span::styled(display, style));
        }

        if count > 1 {
            parts.push(Span::styled(
                count_text,
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    Line::from(parts)
}

// ── Context view ──────────────────────────────────────────────────────────────

fn render_context_view(frame: &mut Frame, vm: &ViewModel, area: Rect) {
    let title = format!(" Context: {} ", vm.loki_context_labels);
    let block = Block::default().borders(Borders::ALL).title(title);

    if vm.loki_context_loading {
        frame.render_widget(Paragraph::new("Loading context\u{2026}").block(block), area);
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
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
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
