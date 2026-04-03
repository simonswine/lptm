use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TempoTrace {
    pub trace_id: String,
    pub root_service_name: String,
    pub root_trace_name: String,
    pub start_time_unix_nano: u64,
    pub duration_ms: u32,
}

pub mod app {
    use super::TempoTrace;
    use crate::app::{Effect, Event, Model, Screen};
    use crux_core::{render::render, Command};

    pub fn handle_enter_tempo(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::TempoMode;
        model.tempo_query.clear();
        model.tempo_cursor_pos = 0;
        model.tempo_loading = false;
        model.tempo_error = None;
        model.tempo_results.clear();
        render()
    }

    pub fn handle_tempo_query_input(model: &mut Model, c: char) -> Command<Effect, Event> {
        let pos = model.tempo_cursor_pos;
        model.tempo_query.insert(pos, c);
        model.tempo_cursor_pos += c.len_utf8();
        render()
    }

    pub fn handle_tempo_query_backspace(model: &mut Model) -> Command<Effect, Event> {
        let pos = model.tempo_cursor_pos;
        if pos > 0 {
            let before = &model.tempo_query[..pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                model.tempo_query.remove(idx);
                model.tempo_cursor_pos = idx;
            }
        }
        render()
    }

    pub fn handle_tempo_cursor_left(model: &mut Model) -> Command<Effect, Event> {
        if model.tempo_cursor_pos > 0 {
            let before = &model.tempo_query[..model.tempo_cursor_pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                model.tempo_cursor_pos = idx;
            }
        }
        render()
    }

    pub fn handle_tempo_cursor_right(model: &mut Model) -> Command<Effect, Event> {
        let pos = model.tempo_cursor_pos;
        if pos < model.tempo_query.len() {
            if let Some(c) = model.tempo_query[pos..].chars().next() {
                model.tempo_cursor_pos += c.len_utf8();
            }
        }
        render()
    }

    pub fn handle_tempo_execute_query(model: &mut Model) -> Command<Effect, Event> {
        if model.tempo_query.trim().is_empty() {
            return render();
        }
        model.tempo_loading = true;
        model.tempo_error = None;
        model.tempo_results.clear();
        render()
    }

    pub fn handle_tempo_result_loaded(
        model: &mut Model,
        result: Result<Vec<TempoTrace>, String>,
    ) -> Command<Effect, Event> {
        model.tempo_loading = false;
        match result {
            Ok(traces) => {
                model.tempo_results = traces;
                model.tempo_error = None;
            }
            Err(e) => {
                model.tempo_error = Some(e);
            }
        }
        render()
    }

    pub fn handle_back_from_tempo(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::DatasourceList;
        render()
    }
}
