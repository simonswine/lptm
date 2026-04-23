use crux_core::{render::render, Command};

use crate::app::{Effect, Event, LokiDiagnosticView, Model, Screen};

use super::{LokiEntry, LokiStream};

// ── Enter / leave ────────────────────────────────────────────────────────────

pub fn handle_enter_loki(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::LokiMode;
    model.loki_query.clear();
    model.loki_loading = false;
    model.loki_error = None;
    model.loki_results.clear();
    model.loki_selected_row = 0;
    model.loki_context_open = false;
    model.loki_context_loading = false;
    model.loki_context_error = None;
    model.loki_context_entries.clear();
    model.loki_context_highlight_index = 0;
    model.loki_context_labels.clear();
    model.loki_diagnostics.clear();
    render()
}

pub fn handle_back_from_loki(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::DatasourceList;
    render()
}

// ── Query text (synced from tui-textarea via LokiQueryChanged) ───────────────

pub fn handle_loki_query_changed(model: &mut Model, query: String) -> Command<Effect, Event> {
    model.loki_query = query;
    render()
}

// ── Diagnostics (pushed from LSP via LokiDiagnosticsUpdated) ────────────────

pub fn handle_loki_diagnostics_updated(
    model: &mut Model,
    diagnostics: Vec<LokiDiagnosticView>,
) -> Command<Effect, Event> {
    model.loki_diagnostics = diagnostics;
    render()
}

// ── Query execution ──────────────────────────────────────────────────────────

pub fn handle_loki_execute_query(model: &mut Model) -> Command<Effect, Event> {
    if model.loki_query.trim().is_empty() {
        return render();
    }
    model.loki_loading = true;
    model.loki_error = None;
    model.loki_results.clear();
    model.loki_selected_row = 0;
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

// ── Result navigation ────────────────────────────────────────────────────────

fn loki_total_rows(model: &Model) -> usize {
    model.loki_results.iter().map(|s| s.entries.len()).sum()
}

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

// ── Context view ─────────────────────────────────────────────────────────────

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
        let _ = (si, ei);
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
