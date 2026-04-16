use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiStream {
    pub labels: String,
    pub entries: Vec<LokiEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LokiEntry {
    pub timestamp: String,
    pub line: String,
}

pub mod app {
    use crate::app::{Effect, Event, Model, Screen};
    use crux_core::{render::render, Command};

    use super::LokiStream;

    pub fn handle_enter_loki(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::LokiMode;
        model.loki_query.clear();
        model.loki_cursor_pos = 0;
        model.loki_loading = false;
        model.loki_error = None;
        model.loki_results.clear();
        render()
    }

    pub fn handle_loki_query_input(model: &mut Model, c: char) -> Command<Effect, Event> {
        let pos = model.loki_cursor_pos;
        model.loki_query.insert(pos, c);
        model.loki_cursor_pos += c.len_utf8();
        render()
    }

    pub fn handle_loki_query_backspace(model: &mut Model) -> Command<Effect, Event> {
        let pos = model.loki_cursor_pos;
        if pos > 0 {
            let before = &model.loki_query[..pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                model.loki_query.remove(idx);
                model.loki_cursor_pos = idx;
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
            }
            Err(e) => {
                model.loki_error = Some(e);
            }
        }
        render()
    }

    pub fn handle_back_from_loki(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::DatasourceList;
        render()
    }
}
