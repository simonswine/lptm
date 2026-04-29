use std::collections::BTreeSet;

use crux_core::{render::render, Command};

use crate::app::{
    filtered_pyroscope_series_indices, sorted_datasource_indices, Effect, Event, Model,
    PyroscopeSeriesItem, PyroscopeSubScreen, Screen,
};
use crate::pyroscope::FlamegraphNav;
use crate::pyroscope::{HeatmapSlot, TimelineSeries};

pub fn handle_enter_pyroscope(model: &mut Model) -> Command<Effect, Event> {
    // Use the favorites-sorted indices (same order as view()) so that
    // model.selected_index — which is always a *sorted* position — translates
    // correctly to the datasource the user actually has highlighted.
    let indices = sorted_datasource_indices(model);
    if indices.is_empty() {
        return render();
    }
    let clamped = model.selected_index.min(indices.len() - 1);
    // Capture the raw datasource index before clearing the filter; clearing
    // changes what sorted_datasource_indices returns, so we must re-map
    // selected_index to the matching position in the new unfiltered order.
    let raw_idx = indices[clamped];
    model.datasource_filter.clear();
    let new_sorted = sorted_datasource_indices(model);
    model.selected_index = new_sorted.iter().position(|&i| i == raw_idx).unwrap_or(0);

    model.screen = Screen::PyroscopeMode;
    model.pyroscope_sub_screen = PyroscopeSubScreen::ServiceList;

    if model.time_range.is_empty() {
        model.time_range = "1h".into();
    }

    model.pyroscope_series_loading = true;
    model.pyroscope_series_error = None;
    model.pyroscope_series.clear();
    model.pyroscope_profile_types.clear();
    model.pyroscope_series_index = 0;
    model.pyroscope_service_filter.clear();

    render()
}

pub fn handle_pyroscope_series_loaded(
    model: &mut Model,
    result: Result<Vec<(String, String)>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(series) => {
            model.pyroscope_series_loading = false;
            let was_empty = model.pyroscope_profile_types.is_empty();
            model.pyroscope_series = series
                .into_iter()
                .map(|(service, profile_type)| PyroscopeSeriesItem {
                    service_name: service,
                    profile_type_id: profile_type,
                })
                .collect();

            // Extract sorted unique profile types
            model.pyroscope_profile_types = model
                .pyroscope_series
                .iter()
                .map(|item| item.profile_type_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            // On first entry (profile_types was empty) prefer a CPU profile type.
            // On reload (time range change) preserve the current selection, just clamp.
            if was_empty {
                // Try known CPU profile type IDs in priority order, then fall back
                // to any type whose first segment is a CPU variant, then to index 0.
                model.pyroscope_profile_type_index = model
                    .pyroscope_profile_types
                    .iter()
                    .position(|pt| pt == "process_cpu:cpu:nanoseconds:cpu:nanoseconds")
                    .or_else(|| {
                        model.pyroscope_profile_types.iter().position(|pt| {
                            let seg = pt.split(':').next().unwrap_or("");
                            seg == "process_cpu" || seg == "cpu"
                        })
                    })
                    .unwrap_or(0);
            } else if model.pyroscope_profile_type_index >= model.pyroscope_profile_types.len() {
                model.pyroscope_profile_type_index = 0;
            }
            model.pyroscope_series_index = 0;
            render()
        }
        Err(err) => {
            model.pyroscope_series_loading = false;
            model.pyroscope_series_error = Some(err);
            render()
        }
    }
}

pub fn handle_pyroscope_series_next(model: &mut Model) -> Command<Effect, Event> {
    let count = filtered_pyroscope_series_indices(model).len();
    if count > 0 {
        model.pyroscope_series_index = (model.pyroscope_series_index + 1) % count;
    }
    render()
}

pub fn handle_pyroscope_series_prev(model: &mut Model) -> Command<Effect, Event> {
    let count = filtered_pyroscope_series_indices(model).len();
    if count > 0 {
        model.pyroscope_series_index = (model.pyroscope_series_index + count - 1) % count;
    }
    render()
}

pub fn handle_pyroscope_profile_type_next(model: &mut Model) -> Command<Effect, Event> {
    let len = model.pyroscope_profile_types.len();
    if len > 0 {
        model.pyroscope_profile_type_index = (model.pyroscope_profile_type_index + 1) % len;
        model.pyroscope_series_index = 0;
    }
    render()
}

pub fn handle_pyroscope_profile_type_prev(model: &mut Model) -> Command<Effect, Event> {
    let len = model.pyroscope_profile_types.len();
    if len > 0 {
        model.pyroscope_profile_type_index = (model.pyroscope_profile_type_index + len - 1) % len;
        model.pyroscope_series_index = 0;
    }
    render()
}

pub fn handle_pyroscope_service_filter_input(model: &mut Model, c: char) -> Command<Effect, Event> {
    model.pyroscope_service_filter.push(c);
    model.pyroscope_series_index = 0;
    render()
}

pub fn handle_pyroscope_service_filter_backspace(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_service_filter.pop();
    model.pyroscope_series_index = 0;
    render()
}

pub fn handle_pyroscope_service_filter_clear(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_service_filter.clear();
    model.pyroscope_series_index = 0;
    render()
}

pub fn handle_pyroscope_select_series(model: &mut Model) -> Command<Effect, Event> {
    let indices = filtered_pyroscope_series_indices(model);
    let Some(&abs_idx) = indices.get(model.pyroscope_series_index) else {
        return render();
    };
    let item = &model.pyroscope_series[abs_idx];
    model.pyroscope_selected_service = item.service_name.clone();
    model.pyroscope_selected_profile_type = item.profile_type_id.clone();
    model.pyroscope_sub_screen = PyroscopeSubScreen::Flamegraph;
    model.pyroscope_flamegraph_loading = true;
    model.pyroscope_flamegraph_error = None;
    model.pyroscope_flamegraph = None;
    model.flamegraph_nav = FlamegraphNav::default();
    // Reset other views so stale data isn't shown if the user cycles later.
    model.pyroscope_timeline_loading = false;
    model.pyroscope_timeline_error = None;
    model.pyroscope_timeline.clear();
    model.pyroscope_heatmap_loading = false;
    model.pyroscope_heatmap_error = None;
    model.pyroscope_heatmap.clear();
    model.pyroscope_span_heatmap_loading = false;
    model.pyroscope_span_heatmap_error = None;
    model.pyroscope_span_heatmap.clear();
    render()
}

pub fn handle_pyroscope_select_view(model: &mut Model, idx: usize) -> Command<Effect, Event> {
    let target = match idx {
        0 => PyroscopeSubScreen::Flamegraph,
        1 => PyroscopeSubScreen::Timeline,
        2 => PyroscopeSubScreen::ProfileHeatmap,
        3 => PyroscopeSubScreen::SpanHeatmap,
        _ => return render(),
    };
    // Heatmap tabs need both heatmap data and timeline exemplars.
    match &target {
        PyroscopeSubScreen::Timeline => {
            if model.pyroscope_timeline.is_empty() && model.pyroscope_timeline_error.is_none() {
                model.pyroscope_timeline_loading = true;
            }
        }
        PyroscopeSubScreen::ProfileHeatmap => {
            if model.pyroscope_heatmap.is_empty() && model.pyroscope_heatmap_error.is_none() {
                model.pyroscope_heatmap_loading = true;
            }
            if model.pyroscope_timeline.is_empty() && model.pyroscope_timeline_error.is_none() {
                model.pyroscope_timeline_loading = true;
            }
        }
        PyroscopeSubScreen::SpanHeatmap => {
            if model.pyroscope_span_heatmap.is_empty()
                && model.pyroscope_span_heatmap_error.is_none()
            {
                model.pyroscope_span_heatmap_loading = true;
            }
        }
        _ => {}
    }
    model.pyroscope_sub_screen = target;
    render()
}

pub fn handle_pyroscope_timeline_loaded(
    model: &mut Model,
    result: Result<Vec<TimelineSeries>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(points) => {
            model.pyroscope_timeline_loading = false;
            model.pyroscope_timeline = points;
            model.pyroscope_exemplar_index = 0;
        }
        Err(err) => {
            model.pyroscope_timeline_loading = false;
            model.pyroscope_timeline_error = Some(err);
        }
    }
    render()
}

pub fn handle_pyroscope_heatmap_loaded(
    model: &mut Model,
    result: Result<Vec<HeatmapSlot>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(slots) => {
            model.pyroscope_heatmap_loading = false;
            model.pyroscope_heatmap = slots;
            model.pyroscope_exemplar_index = 0;
        }
        Err(err) => {
            model.pyroscope_heatmap_loading = false;
            model.pyroscope_heatmap_error = Some(err);
        }
    }
    render()
}

pub fn handle_pyroscope_span_heatmap_loaded(
    model: &mut Model,
    result: Result<Vec<HeatmapSlot>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(slots) => {
            model.pyroscope_span_heatmap_loading = false;
            model.pyroscope_span_heatmap = slots;
            model.pyroscope_exemplar_index = 0;
        }
        Err(err) => {
            model.pyroscope_span_heatmap_loading = false;
            model.pyroscope_span_heatmap_error = Some(err);
        }
    }
    render()
}

pub fn handle_pyroscope_flamegraph_loaded(
    model: &mut Model,
    result: Result<Option<crate::pyroscope::FlameGraph>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(flamegraph) => {
            model.pyroscope_flamegraph_loading = false;
            model.pyroscope_flamegraph = flamegraph.map(|mut fg| {
                fg.normalize_deltas();
                fg
            });
            model.flamegraph_nav = FlamegraphNav::default();
            render()
        }
        Err(err) => {
            model.pyroscope_flamegraph_loading = false;
            model.pyroscope_flamegraph_error = Some(err);
            render()
        }
    }
}

/// History restore: enter PyroscopeMode and jump directly to the Flamegraph
/// sub-screen for `service_name` + `profile_type`, skipping series loading.
pub fn handle_pyroscope_direct_load(
    model: &mut Model,
    service_name: String,
    profile_type: String,
) -> Command<Effect, Event> {
    let indices = sorted_datasource_indices(model);
    if indices.is_empty() {
        return render();
    }
    model.selected_index = model.selected_index.min(indices.len() - 1);
    model.datasource_filter.clear();

    model.screen = Screen::PyroscopeMode;
    if model.time_range.is_empty() {
        model.time_range = "1h".into();
    }

    // Series are not needed — we already know what to load.
    model.pyroscope_series_loading = false;
    model.pyroscope_series_error = None;
    model.pyroscope_series.clear();
    model.pyroscope_profile_types.clear();
    model.pyroscope_series_index = 0;
    model.pyroscope_service_filter.clear();

    // Go directly to the flamegraph for the stored selection.
    model.pyroscope_selected_service = service_name;
    model.pyroscope_selected_profile_type = profile_type;
    model.pyroscope_sub_screen = PyroscopeSubScreen::Flamegraph;
    model.pyroscope_flamegraph_loading = true;
    model.pyroscope_flamegraph_error = None;
    model.pyroscope_flamegraph = None;
    model.flamegraph_nav = FlamegraphNav::default();
    model.pyroscope_timeline_loading = false;
    model.pyroscope_timeline_error = None;
    model.pyroscope_timeline.clear();
    model.pyroscope_heatmap_loading = false;
    model.pyroscope_heatmap_error = None;
    model.pyroscope_heatmap.clear();
    model.pyroscope_span_heatmap_loading = false;
    model.pyroscope_span_heatmap_error = None;
    model.pyroscope_span_heatmap.clear();

    render()
}

/// Set the time range directly without triggering a series reload.
/// Used when restoring a pyroscope history entry so EnterPyroscope preserves
/// the stored time range rather than defaulting to "1h".
pub fn handle_pyroscope_set_time_range(model: &mut Model, range: String) -> Command<Effect, Event> {
    model.time_range = range;
    render()
}

/// After a series load, select the service + profile_type matching a history entry.
/// Finds the right profile_type_index and series_index then delegates to the
/// normal select-series logic (which navigates to the Flamegraph sub-screen).
pub fn handle_pyroscope_select_by_name(
    model: &mut Model,
    service_name: String,
    profile_type: String,
) -> Command<Effect, Event> {
    let Some(pt_idx) = model
        .pyroscope_profile_types
        .iter()
        .position(|pt| *pt == profile_type)
    else {
        return render();
    };
    model.pyroscope_profile_type_index = pt_idx;

    let indices = filtered_pyroscope_series_indices(model);
    if let Some(series_idx) = indices
        .iter()
        .position(|&abs_i| model.pyroscope_series[abs_i].service_name == service_name)
    {
        model.pyroscope_series_index = series_idx;
    }

    handle_pyroscope_select_series(model)
}

pub fn handle_back_to_service_list(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_sub_screen = PyroscopeSubScreen::ServiceList;
    // When arriving via PyroscopeDirectLoad the series were never fetched.
    // Trigger a load now so the service list is populated.
    if model.pyroscope_series.is_empty() && !model.pyroscope_series_loading {
        model.pyroscope_series_loading = true;
        model.pyroscope_series_error = None;
    }
    render()
}

pub fn handle_back_from_pyroscope(model: &mut Model) -> Command<Effect, Event> {
    model.screen = Screen::DatasourceList;
    render()
}

pub fn handle_flame_move_left(model: &mut Model) -> Command<Effect, Event> {
    if let Some(ref fg) = model.pyroscope_flamegraph {
        model.flamegraph_nav.move_left(fg);
    }
    render()
}

pub fn handle_flame_move_right(model: &mut Model) -> Command<Effect, Event> {
    if let Some(ref fg) = model.pyroscope_flamegraph {
        model.flamegraph_nav.move_right(fg);
    }
    render()
}

pub fn handle_flame_move_up(model: &mut Model) -> Command<Effect, Event> {
    if let Some(ref fg) = model.pyroscope_flamegraph {
        model.flamegraph_nav.move_up(fg);
    }
    render()
}

pub fn handle_flame_move_down(model: &mut Model) -> Command<Effect, Event> {
    if let Some(ref fg) = model.pyroscope_flamegraph {
        model.flamegraph_nav.move_down(fg);
    }
    render()
}

pub fn handle_flame_zoom_in(model: &mut Model) -> Command<Effect, Event> {
    if let Some(ref fg) = model.pyroscope_flamegraph {
        model.flamegraph_nav.zoom_in(fg);
    }
    render()
}

pub fn handle_flame_zoom_out(model: &mut Model) -> Command<Effect, Event> {
    model.flamegraph_nav.zoom_out();
    render()
}

pub fn handle_flamegraph_viewport_chars(model: &mut Model, chars: u64) -> Command<Effect, Event> {
    model.flamegraph_nav.viewport_chars = chars;
    render()
}

pub fn handle_exemplar_select_next(model: &mut Model) -> Command<Effect, Event> {
    let count = exemplar_count(model);
    if count > 0 {
        model.pyroscope_exemplar_index = (model.pyroscope_exemplar_index + 1) % count;
    }
    render()
}

pub fn handle_exemplar_select_prev(model: &mut Model) -> Command<Effect, Event> {
    let count = exemplar_count(model);
    if count > 0 {
        model.pyroscope_exemplar_index = (model.pyroscope_exemplar_index + count - 1) % count;
    }
    render()
}

fn exemplar_count(model: &Model) -> usize {
    match model.pyroscope_sub_screen {
        PyroscopeSubScreen::Timeline => model
            .pyroscope_timeline
            .iter()
            .flat_map(|s| s.exemplars.iter())
            .count(),
        PyroscopeSubScreen::ProfileHeatmap => model
            .pyroscope_heatmap
            .iter()
            .flat_map(|s| s.exemplars.iter())
            .count(),
        PyroscopeSubScreen::SpanHeatmap => model
            .pyroscope_span_heatmap
            .iter()
            .flat_map(|s| s.exemplars.iter())
            .count(),
        _ => 0,
    }
}
