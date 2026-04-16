use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiStream {
    pub labels: String,
    pub entries: Vec<LokiEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiEntry {
    pub timestamp: String,
    /// Raw nanosecond timestamp string from the Loki API.
    pub timestamp_ns: String,
    pub line: String,
}

pub mod app {
    use crate::app::{Effect, Event, Model, Screen};
    use crux_core::{render::render, Command};

    use super::{LokiEntry, LokiStream};

    pub fn handle_enter_loki(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::LokiMode;
        model.loki_query.clear();
        model.loki_cursor_pos = 0;
        model.loki_loading = false;
        model.loki_error = None;
        model.loki_results.clear();
        model.loki_selected_row = 0;
        model.loki_query_dirty = true;
        model.loki_context_open = false;
        model.loki_context_loading = false;
        model.loki_context_error = None;
        model.loki_context_entries.clear();
        model.loki_context_highlight_index = 0;
        model.loki_context_labels.clear();
        render()
    }

    pub fn handle_loki_query_input(model: &mut Model, c: char) -> Command<Effect, Event> {
        let pos = model.loki_cursor_pos;
        model.loki_query.insert(pos, c);
        model.loki_cursor_pos += c.len_utf8();
        model.loki_query_dirty = true;
        render()
    }

    pub fn handle_loki_query_backspace(model: &mut Model) -> Command<Effect, Event> {
        let pos = model.loki_cursor_pos;
        if pos > 0 {
            let before = &model.loki_query[..pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                model.loki_query.remove(idx);
                model.loki_cursor_pos = idx;
                model.loki_query_dirty = true;
            }
        }
        render()
    }

    pub fn handle_loki_cursor_left(model: &mut Model) -> Command<Effect, Event> {
        if model.loki_cursor_pos > 0 {
            let before = &model.loki_query[..model.loki_cursor_pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                model.loki_cursor_pos = idx;
            }
        }
        render()
    }

    pub fn handle_loki_cursor_right(model: &mut Model) -> Command<Effect, Event> {
        let pos = model.loki_cursor_pos;
        if pos < model.loki_query.len() {
            if let Some(c) = model.loki_query[pos..].chars().next() {
                model.loki_cursor_pos += c.len_utf8();
            }
        }
        render()
    }

    pub fn handle_loki_execute_query(model: &mut Model) -> Command<Effect, Event> {
        if model.loki_query.trim().is_empty() {
            return render();
        }
        model.loki_loading = true;
        model.loki_error = None;
        model.loki_results.clear();
        model.loki_selected_row = 0;
        model.loki_query_dirty = false;
        render()
    }

    pub fn handle_loki_result_loaded(
        model: &mut Model,
        result: Result<Vec<LokiStream>, String>,
    ) -> Command<Effect, Event> {
        model.loki_loading = false;
        match result {
            Ok(streams) => {
                model.loki_results = streams;
                model.loki_error = None;
                model.loki_selected_row = 0;
            }
            Err(e) => {
                model.loki_error = Some(e);
            }
        }
        render()
    }

    /// Helper: returns the total number of log entries across all streams.
    fn loki_total_rows(model: &Model) -> usize {
        model.loki_results.iter().map(|s| s.entries.len()).sum()
    }

    /// Helper: given a flat row index, returns (stream_index, entry_index).
    pub fn loki_flat_entry(model: &Model, row: usize) -> Option<(usize, usize)> {
        let mut offset = 0;
        for (si, stream) in model.loki_results.iter().enumerate() {
            if row < offset + stream.entries.len() {
                return Some((si, row - offset));
            }
            offset += stream.entries.len();
        }
        None
    }

    pub fn handle_loki_select_next_row(model: &mut Model) -> Command<Effect, Event> {
        let total = loki_total_rows(model);
        if total > 0 {
            model.loki_selected_row = (model.loki_selected_row + 1) % total;
        }
        render()
    }

    pub fn handle_loki_select_prev_row(model: &mut Model) -> Command<Effect, Event> {
        let total = loki_total_rows(model);
        if total > 0 {
            model.loki_selected_row = (model.loki_selected_row + total - 1) % total;
        }
        render()
    }

    pub fn handle_loki_open_context(model: &mut Model) -> Command<Effect, Event> {
        let total = loki_total_rows(model);
        if total == 0 {
            return render();
        }
        if let Some((si, ei)) = loki_flat_entry(model, model.loki_selected_row) {
            let stream = &model.loki_results[si];
            model.loki_context_labels = stream.labels.clone();
            model.loki_context_open = true;
            model.loki_context_loading = true;
            model.loki_context_error = None;
            model.loki_context_entries.clear();
            model.loki_context_highlight_index = 0;
            // The TUI will read context_labels and the selected entry's timestamp_ns
            // from the VM to spawn the context query.
            let _ = (si, ei); // used by TUI via VM
        }
        render()
    }

    pub fn handle_loki_close_context(model: &mut Model) -> Command<Effect, Event> {
        model.loki_context_open = false;
        model.loki_context_loading = false;
        model.loki_context_error = None;
        model.loki_context_entries.clear();
        render()
    }

    pub fn handle_loki_context_loaded(
        model: &mut Model,
        result: Result<(Vec<LokiEntry>, usize), String>,
    ) -> Command<Effect, Event> {
        model.loki_context_loading = false;
        match result {
            Ok((entries, highlight_index)) => {
                model.loki_context_entries = entries;
                model.loki_context_highlight_index = highlight_index;
                model.loki_context_error = None;
            }
            Err(e) => {
                model.loki_context_error = Some(e);
            }
        }
        render()
    }

    pub fn handle_back_from_loki(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::DatasourceList;
        render()
    }
}
