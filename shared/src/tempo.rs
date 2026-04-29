use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TempoTrace {
    pub trace_id: String,
    pub root_service_name: String,
    pub root_trace_name: String,
    pub start_time_unix_nano: u64,
    pub duration_ms: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TempoSpan {
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub service_name: String,
    pub start_time_unix_nano: u64,
    pub duration_ns: u64,
    /// Tree depth computed after parsing (0 = root span).
    pub depth: usize,
    pub resource_attrs: Vec<(String, String)>,
    pub span_attrs: Vec<(String, String)>,
    /// OTLP SpanKind integer (0=unspecified,1=internal,2=server,3=client,4=producer,5=consumer).
    pub kind: i32,
    /// true when OTLP status.code == 2 (STATUS_CODE_ERROR).
    pub error: bool,
}

pub mod app {
    use super::{TempoSpan, TempoTrace};
    use crate::app::{fuzzy_match, Effect, Event, Model, Screen};
    use crux_core::{render::render, Command};

    pub fn handle_enter_tempo(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::TempoMode;
        model.tempo_query.clear();
        model.tempo_cursor_pos = 0;
        model.tempo_loading = false;
        model.tempo_error = None;
        model.tempo_results.clear();
        model.tempo_results_selected = 0;
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

    pub fn handle_tempo_cursor_home(model: &mut Model) -> Command<Effect, Event> {
        model.tempo_cursor_pos = 0;
        render()
    }

    pub fn handle_tempo_cursor_end(model: &mut Model) -> Command<Effect, Event> {
        model.tempo_cursor_pos = model.tempo_query.len();
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
        model.tempo_results_selected = 0;
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

    pub fn handle_tempo_select_next(model: &mut Model) -> Command<Effect, Event> {
        let count = model.tempo_results.len();
        if count > 0 {
            model.tempo_results_selected = (model.tempo_results_selected + 1) % count;
        }
        render()
    }

    pub fn handle_tempo_select_prev(model: &mut Model) -> Command<Effect, Event> {
        let count = model.tempo_results.len();
        if count > 0 {
            model.tempo_results_selected = (model.tempo_results_selected + count - 1) % count;
        }
        render()
    }

    pub fn handle_tempo_open_trace(model: &mut Model) -> Command<Effect, Event> {
        if let Some(trace) = model.tempo_results.get(model.tempo_results_selected) {
            model.trace_detail_trace_id = trace.trace_id.clone();
        }
        model.screen = Screen::TempoTraceDetail;
        model.trace_detail_loading = true;
        model.trace_detail_error = None;
        model.trace_detail_spans.clear();
        model.trace_detail_selected = 0;
        model.trace_detail_filter.clear();
        model.trace_detail_filter_focused = false;
        render()
    }

    pub fn handle_trace_detail_loaded(
        model: &mut Model,
        result: Result<Vec<TempoSpan>, String>,
    ) -> Command<Effect, Event> {
        model.trace_detail_loading = false;
        match result {
            Ok(spans) => {
                model.trace_detail_spans = spans;
                model.trace_detail_error = None;
            }
            Err(e) => {
                model.trace_detail_error = Some(e);
            }
        }
        model.trace_detail_selected = 0;
        render()
    }

    pub fn handle_trace_detail_next(model: &mut Model) -> Command<Effect, Event> {
        let visible = filtered_trace_detail_count(model);
        if visible > 0 {
            model.trace_detail_selected = (model.trace_detail_selected + 1) % visible;
        }
        model.trace_detail_attr_scroll = 0;
        render()
    }

    pub fn handle_trace_detail_prev(model: &mut Model) -> Command<Effect, Event> {
        let visible = filtered_trace_detail_count(model);
        if visible > 0 {
            model.trace_detail_selected = (model.trace_detail_selected + visible - 1) % visible;
        }
        model.trace_detail_attr_scroll = 0;
        render()
    }

    pub fn handle_trace_detail_filter_focus(model: &mut Model) -> Command<Effect, Event> {
        model.trace_detail_filter_focused = true;
        render()
    }

    pub fn handle_trace_detail_filter_blur(model: &mut Model) -> Command<Effect, Event> {
        model.trace_detail_filter_focused = false;
        render()
    }

    pub fn handle_trace_detail_filter_input(model: &mut Model, c: char) -> Command<Effect, Event> {
        model.trace_detail_filter.push(c);
        model.trace_detail_selected = 0;
        model.trace_detail_attr_scroll = 0;
        render()
    }

    pub fn handle_trace_detail_filter_backspace(model: &mut Model) -> Command<Effect, Event> {
        model.trace_detail_filter.pop();
        model.trace_detail_selected = 0;
        model.trace_detail_attr_scroll = 0;
        render()
    }

    pub fn handle_trace_detail_attr_scroll_down(model: &mut Model) -> Command<Effect, Event> {
        model.trace_detail_attr_scroll = model.trace_detail_attr_scroll.saturating_add(1);
        render()
    }

    pub fn handle_trace_detail_attr_scroll_up(model: &mut Model) -> Command<Effect, Event> {
        model.trace_detail_attr_scroll = model.trace_detail_attr_scroll.saturating_sub(1);
        render()
    }

    pub fn handle_back_from_trace_detail(model: &mut Model) -> Command<Effect, Event> {
        model.screen = Screen::TempoMode;
        render()
    }

    fn filtered_trace_detail_count(model: &Model) -> usize {
        if model.trace_detail_filter.is_empty() {
            return model.trace_detail_spans.len();
        }
        model
            .trace_detail_spans
            .iter()
            .filter(|s| fuzzy_match_in_span(s, &model.trace_detail_filter))
            .count()
    }

    fn fuzzy_match_in_span(span: &TempoSpan, needle: &str) -> bool {
        if fuzzy_match(&span.name, needle) {
            return true;
        }
        if fuzzy_match(&span.service_name, needle) {
            return true;
        }
        for (k, v) in span.resource_attrs.iter().chain(span.span_attrs.iter()) {
            if fuzzy_match(k, needle) || fuzzy_match(v, needle) {
                return true;
            }
        }
        false
    }
}
