use std::collections::BTreeSet;

use crux_core::{render::render, Command};

use crate::app::{
    filtered_datasource_indices, filtered_pyroscope_series_indices, Effect, Event, Model,
    PyroscopeSeriesItem, PyroscopeSubScreen, Screen,
};
use crate::pyroscope::{HeatmapSlot, TimelinePoint};
use crate::pyroscope::FlamegraphNav;

pub fn handle_enter_pyroscope(model: &mut Model) -> Command<Effect, Event> {
    let indices = filtered_datasource_indices(model);
    if indices.is_empty() {
        return render();
    }
    let abs_idx = indices[model.selected_index.min(indices.len() - 1)];
    model.selected_index = abs_idx;
    model.datasource_filter.clear();

    model.screen = Screen::PyroscopeMode;
    model.pyroscope_sub_screen = PyroscopeSubScreen::ServiceList;

    if model.pyroscope_time_range.is_empty() {
        model.pyroscope_time_range = "1h".into();
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

            // On first entry (profile_types was empty) prefer the cpu profile type.
            // On reload (time range change) preserve the current selection, just clamp.
            if was_empty {
                let preferred = "cpu:cpu:nanoseconds:cpu:nanoseconds";
                model.pyroscope_profile_type_index = model
                    .pyroscope_profile_types
                    .iter()
                    .position(|pt| pt == preferred)
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

pub fn handle_pyroscope_time_range_edit(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_time_range_editing = true;
    render()
}

pub fn handle_pyroscope_time_range_input(model: &mut Model, c: char) -> Command<Effect, Event> {
    model.pyroscope_time_range.push(c);
    render()
}

pub fn handle_pyroscope_time_range_backspace(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_time_range.pop();
    render()
}

pub fn handle_pyroscope_time_range_commit(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_time_range_editing = false;
    model.pyroscope_series_loading = true;
    model.pyroscope_series_error = None;
    render()
}

pub fn handle_pyroscope_time_range_abort(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_time_range_editing = false;
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
        model.pyroscope_profile_type_index =
            (model.pyroscope_profile_type_index + len - 1) % len;
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
    render()
}

pub fn handle_pyroscope_cycle_view(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_sub_screen = match model.pyroscope_sub_screen {
        PyroscopeSubScreen::Flamegraph => {
            // Trigger a timeline load only if we have no data and no error yet.
            if model.pyroscope_timeline.is_empty() && model.pyroscope_timeline_error.is_none() {
                model.pyroscope_timeline_loading = true;
            }
            PyroscopeSubScreen::Timeline
        }
        PyroscopeSubScreen::Timeline => {
            if model.pyroscope_heatmap.is_empty() && model.pyroscope_heatmap_error.is_none() {
                model.pyroscope_heatmap_loading = true;
            }
            PyroscopeSubScreen::Heatmap
        }
        PyroscopeSubScreen::Heatmap => PyroscopeSubScreen::Flamegraph,
        _ => return render(),
    };
    render()
}

pub fn handle_pyroscope_timeline_loaded(
    model: &mut Model,
    result: Result<Vec<TimelinePoint>, String>,
) -> Command<Effect, Event> {
    match result {
        Ok(points) => {
            model.pyroscope_timeline_loading = false;
            model.pyroscope_timeline = points;
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
        }
        Err(err) => {
            model.pyroscope_heatmap_loading = false;
            model.pyroscope_heatmap_error = Some(err);
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

pub fn handle_back_to_service_list(model: &mut Model) -> Command<Effect, Event> {
    model.pyroscope_sub_screen = PyroscopeSubScreen::ServiceList;
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

pub fn handle_flamegraph_viewport_chars(
    model: &mut Model,
    chars: u64,
) -> Command<Effect, Event> {
    model.flamegraph_nav.viewport_chars = chars;
    render()
}
