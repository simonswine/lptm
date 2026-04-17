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
        let info_line = build_info_line(vm, split[1].width);
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
///
/// Diagnostic underlines are rendered as colored underlines directly on the
/// syntax-highlighted text using `Modifier::UNDERLINED` + `underline_color`,
/// so highlighting and diagnostics compose on a single line.
fn build_query_line_with_diagnostics(
    query: &str,
    cursor_pos: usize,
    diagnostics: &[shared::LokiDiagnosticView],
) -> Line<'static> {
    let highlighted = super::highlight::highlight_loki_query(query, cursor_pos);

    if diagnostics.is_empty() || query.is_empty() {
        return highlighted;
    }

    // Map each query byte to an optional underline style
    let query_len = query.len();
    let mut ul_map: Vec<Option<Style>> = vec![None; query_len];
    for diag in diagnostics {
        let ul = if diag.severity == "error" {
            Style::default()
                .add_modifier(Modifier::UNDERLINED)
                .underline_color(Color::Red)
        } else {
            Style::default()
                .add_modifier(Modifier::UNDERLINED)
                .underline_color(Color::Yellow)
        };
        let start = diag.start_byte.min(query_len);
        let end = diag.end_byte.min(query_len);
        for pos in start..end {
            ul_map[pos] = Some(ul);
        }
    }

    // Walk highlighted spans, overlay diagnostic underline styles
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut byte_off = 0usize;

    for span in highlighted.spans {
        let base = span.style;
        let text = span.content.into_owned();
        let len = text.len();

        let overlap = if byte_off < query_len {
            len.min(query_len - byte_off)
        } else {
            0
        };

        if overlap == 0 {
            // Past query bytes (e.g. cursor-at-end space)
            out.push(Span::styled(text, base));
        } else {
            let mut i = 0;
            while i < overlap {
                let cur = ul_map[byte_off + i];
                let mut j = i + 1;
                while j < overlap && ul_map[byte_off + j] == cur {
                    j += 1;
                }
                let style = match cur {
                    Some(ul) => base.patch(ul),
                    None => base,
                };
                out.push(Span::styled(text[i..j].to_owned(), style));
                i = j;
            }
            if overlap < len {
                out.push(Span::styled(text[overlap..].to_owned(), base));
            }
        }

        byte_off += len;
    }

    Line::from(out)
}

/// Build the info line showing type context and the cursor-relevant diagnostic.
///
/// When the cursor is inside a diagnostic span, that diagnostic's message is
/// shown. Otherwise the first diagnostic is used as a fallback. Messages are
/// truncated with an ellipsis when the terminal is too narrow, and a count
/// badge is appended when multiple diagnostics exist.
fn build_info_line(vm: &ViewModel, max_width: u16) -> Line<'static> {
    let mut parts: Vec<Span<'static>> = Vec::new();
    let max_w = max_width as usize;
    let mut used = 0usize;

    // Type context
    if let Some(ref tc) = vm.loki_type_context {
        let text = format!(" {tc} ");
        used += text.chars().count();
        parts.push(Span::styled(text, Style::default().fg(Color::Cyan)));
    }

    // Find the diagnostic under the cursor, fall back to the first
    let diag = vm
        .loki_diagnostics
        .iter()
        .find(|d| vm.loki_cursor_pos >= d.start_byte && vm.loki_cursor_pos < d.end_byte)
        .or_else(|| vm.loki_diagnostics.first());

    if let Some(diag) = diag {
        if !parts.is_empty() {
            parts.push(Span::styled(
                " \u{2502} ",
                Style::default().fg(Color::DarkGray),
            ));
            used += 3;
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
        let icon_w = icon.chars().count();

        let count = vm.loki_diagnostics.len();
        let count_text = if count > 1 {
            format!(" [{count}]")
        } else {
            String::new()
        };
        let count_w = count_text.chars().count();

        let budget = max_w.saturating_sub(used + icon_w + count_w);
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
            parts.push(Span::styled(count_text, Style::default().fg(Color::DarkGray)));
        }
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

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use shared::LokiDiagnosticView;

    fn plain_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Check whether the span covering `byte_pos` has the UNDERLINED modifier.
    fn has_underline(line: &Line, byte_pos: usize) -> bool {
        let mut offset = 0;
        for span in &line.spans {
            let len = span.content.len();
            if byte_pos >= offset && byte_pos < offset + len {
                return span.style.add_modifier.contains(Modifier::UNDERLINED);
            }
            offset += len;
        }
        false
    }

    /// Return the underline color of the span covering `byte_pos`.
    fn underline_color_at(line: &Line, byte_pos: usize) -> Option<Color> {
        let mut offset = 0;
        for span in &line.spans {
            let len = span.content.len();
            if byte_pos >= offset && byte_pos < offset + len {
                return span.style.underline_color;
            }
            offset += len;
        }
        None
    }

    // ── build_query_line_with_diagnostics ────────────────────────────────────

    #[test]
    fn no_diagnostics_no_underline() {
        let line = build_query_line_with_diagnostics("rate", 4, &[]);
        assert!(!has_underline(&line, 0));
        assert!(!has_underline(&line, 3));
    }

    #[test]
    fn error_diagnostic_red_underline() {
        let diag = LokiDiagnosticView {
            severity: "error".into(),
            message: "unexpected token".into(),
            start_byte: 0,
            end_byte: 4,
        };
        let line = build_query_line_with_diagnostics("rate", 4, &[diag]);
        assert!(has_underline(&line, 0));
        assert!(has_underline(&line, 3));
        assert_eq!(underline_color_at(&line, 0), Some(Color::Red));
    }

    #[test]
    fn warning_diagnostic_yellow_underline() {
        let query = r#"{job="app"}"#;
        let diag = LokiDiagnosticView {
            severity: "warning".into(),
            message: "unused label".into(),
            start_byte: 1,
            end_byte: 4,
        };
        let line = build_query_line_with_diagnostics(query, query.len(), &[diag]);
        assert!(!has_underline(&line, 0)); // '{' — no diagnostic
        assert!(has_underline(&line, 1));  // 'j'
        assert!(has_underline(&line, 3));  // 'b'
        assert_eq!(underline_color_at(&line, 1), Some(Color::Yellow));
    }

    #[test]
    fn multiple_diagnostics_different_spans() {
        let query = "rate error";
        let diags = vec![
            LokiDiagnosticView {
                severity: "error".into(),
                message: "err1".into(),
                start_byte: 0,
                end_byte: 2,
            },
            LokiDiagnosticView {
                severity: "warning".into(),
                message: "warn1".into(),
                start_byte: 5,
                end_byte: 8,
            },
        ];
        let line = build_query_line_with_diagnostics(query, query.len(), &diags);
        assert_eq!(underline_color_at(&line, 0), Some(Color::Red));
        assert_eq!(underline_color_at(&line, 1), Some(Color::Red));
        assert!(!has_underline(&line, 3)); // between diagnostics
        assert_eq!(underline_color_at(&line, 5), Some(Color::Yellow));
        assert_eq!(underline_color_at(&line, 7), Some(Color::Yellow));
    }

    #[test]
    fn diagnostics_preserve_query_text() {
        let query = r#"rate({job="app"}[5m])"#;
        let diag = LokiDiagnosticView {
            severity: "error".into(),
            message: "test".into(),
            start_byte: 0,
            end_byte: 4,
        };
        for pos in 0..=query.len() {
            if !query.is_char_boundary(pos) {
                continue;
            }
            let line = build_query_line_with_diagnostics(query, pos, &[diag.clone()]);
            let text = plain_text(&line);
            if pos < query.len() {
                assert_eq!(text, query, "text mismatch at pos={pos}");
            } else {
                assert_eq!(text, format!("{query} "), "end cursor at pos={pos}");
            }
        }
    }

    #[test]
    fn diagnostic_on_cursor_position_composes() {
        let query = "rate";
        let diag = LokiDiagnosticView {
            severity: "error".into(),
            message: "bad".into(),
            start_byte: 0,
            end_byte: 4,
        };
        // Cursor at byte 2 (inside the diagnostic span)
        let line = build_query_line_with_diagnostics(query, 2, &[diag]);
        assert_eq!(plain_text(&line), "rate");
        // The cursor character should still have underline
        assert!(has_underline(&line, 2));
    }

    // ── build_info_line ─────────────────────────────────────────────────────

    #[test]
    fn info_line_cursor_inside_diagnostic() {
        let vm = ViewModel {
            loki_diagnostics: vec![
                LokiDiagnosticView {
                    severity: "error".into(),
                    message: "first error".into(),
                    start_byte: 0,
                    end_byte: 3,
                },
                LokiDiagnosticView {
                    severity: "warning".into(),
                    message: "second warning".into(),
                    start_byte: 5,
                    end_byte: 8,
                },
            ],
            loki_cursor_pos: 6, // inside second diagnostic
            ..Default::default()
        };
        let line = build_info_line(&vm, 80);
        let text = plain_text(&line);
        assert!(text.contains("second warning"), "got: {text}");
        assert!(!text.contains("first error"), "got: {text}");
    }

    #[test]
    fn info_line_falls_back_to_first_diagnostic() {
        let vm = ViewModel {
            loki_diagnostics: vec![
                LokiDiagnosticView {
                    severity: "error".into(),
                    message: "first error".into(),
                    start_byte: 0,
                    end_byte: 3,
                },
                LokiDiagnosticView {
                    severity: "warning".into(),
                    message: "second warning".into(),
                    start_byte: 5,
                    end_byte: 8,
                },
            ],
            loki_cursor_pos: 4, // not inside any diagnostic
            ..Default::default()
        };
        let line = build_info_line(&vm, 80);
        let text = plain_text(&line);
        assert!(text.contains("first error"), "got: {text}");
    }

    #[test]
    fn info_line_narrow_terminal_truncates() {
        let vm = ViewModel {
            loki_diagnostics: vec![LokiDiagnosticView {
                severity: "error".into(),
                message: "this is a very long error message that should be truncated".into(),
                start_byte: 0,
                end_byte: 5,
            }],
            loki_cursor_pos: 0,
            ..Default::default()
        };
        let line = build_info_line(&vm, 20);
        let text = plain_text(&line);
        assert!(text.contains('\u{2026}'), "expected ellipsis, got: {text}");
        assert!(text.chars().count() <= 20, "too wide: {}", text.chars().count());
    }

    #[test]
    fn info_line_multiple_diagnostics_shows_count() {
        let vm = ViewModel {
            loki_diagnostics: vec![
                LokiDiagnosticView {
                    severity: "error".into(),
                    message: "err".into(),
                    start_byte: 0,
                    end_byte: 2,
                },
                LokiDiagnosticView {
                    severity: "warning".into(),
                    message: "warn".into(),
                    start_byte: 5,
                    end_byte: 8,
                },
            ],
            loki_cursor_pos: 0,
            ..Default::default()
        };
        let line = build_info_line(&vm, 80);
        let text = plain_text(&line);
        assert!(text.contains("[2]"), "expected count badge, got: {text}");
    }

    #[test]
    fn info_line_single_diagnostic_no_count() {
        let vm = ViewModel {
            loki_diagnostics: vec![LokiDiagnosticView {
                severity: "error".into(),
                message: "only one".into(),
                start_byte: 0,
                end_byte: 3,
            }],
            loki_cursor_pos: 0,
            ..Default::default()
        };
        let line = build_info_line(&vm, 80);
        let text = plain_text(&line);
        assert!(!text.contains('['), "unexpected count badge, got: {text}");
    }

    #[test]
    fn info_line_type_context_with_diagnostic() {
        let vm = ViewModel {
            loki_type_context: Some("log stream".into()),
            loki_diagnostics: vec![LokiDiagnosticView {
                severity: "error".into(),
                message: "parse error".into(),
                start_byte: 0,
                end_byte: 3,
            }],
            loki_cursor_pos: 0,
            ..Default::default()
        };
        let line = build_info_line(&vm, 80);
        let text = plain_text(&line);
        assert!(text.contains("log stream"), "got: {text}");
        assert!(text.contains("parse error"), "got: {text}");
        assert!(text.contains('\u{2502}'), "expected separator, got: {text}");
    }
}
