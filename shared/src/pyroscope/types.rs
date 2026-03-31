use serde::{Deserialize, Serialize};


/// Flamegraph from SelectMergeStacktraces.
/// Level encoding identical to flamebearer: groups of 4 in Level.values:
/// `[x_offset, total_samples, self_samples, name_index]`
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlameGraph {
    pub names: Vec<String>,
    pub levels: Vec<Level>,
    pub total: i64,
    #[serde(rename = "maxSelf", default)]
    pub max_self: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Level {
    pub values: Vec<i64>,
}

impl FlameGraph {
    /// Convert level values from Pyroscope's delta-encoded format to absolute x offsets.
    ///
    /// Pyroscope encodes each group of 4 as `[gap, total, self, name_idx]` where
    /// `gap` is the number of samples between the previous frame's end and this
    /// frame's start (not an absolute position).  After calling this, `values[i*4]`
    /// is the absolute x offset of frame `i`, matching the rest of the code.
    pub fn normalize_deltas(&mut self) {
        for level in &mut self.levels {
            let mut abs_x: i64 = 0;
            let count = level.values.len() / 4;
            for i in 0..count {
                let gap = level.values[i * 4];
                let width = level.values[i * 4 + 1];
                abs_x += gap;
                level.values[i * 4] = abs_x;
                abs_x += width;
            }
        }
    }
}

// ── Navigation state (not serialised – lives only in Model) ───────────────────

/// Zoom and selection state for flamegraph exploration.
#[derive(Debug, Clone)]
pub struct FlamegraphNav {
    /// Level index treated as the current root (anchored at the bottom of the view).
    pub root_level: usize,
    /// Frame index (group-of-4) within `root_level` that is the zoom root.
    pub root_frame: usize,
    /// Currently highlighted level (absolute index).
    pub sel_level: usize,
    /// Currently highlighted frame index (group-of-4) within `sel_level`.
    pub sel_frame: usize,
    /// Terminal width in characters — used to skip sub-pixel frames during navigation.
    pub viewport_chars: u64,
}

impl Default for FlamegraphNav {
    fn default() -> Self {
        FlamegraphNav { root_level: 0, root_frame: 0, sel_level: 0, sel_frame: 0, viewport_chars: 200 }
    }
}

impl FlamegraphNav {
    /// Minimum frame width (in samples) to be considered navigable.
    /// Frames narrower than this render as 0 characters and should be skipped.
    fn min_nav_width(&self, root_w: u64) -> u64 {
        if self.viewport_chars == 0 { return 1; }
        root_w / self.viewport_chars
    }

    /// `(x_start, width)` of the current root frame in samples.
    fn root_bounds(&self, fg: &FlameGraph) -> (u64, u64) {
        fg.levels
            .get(self.root_level)
            .and_then(|lv| {
                let i = self.root_frame * 4;
                if i + 1 < lv.values.len() {
                    Some((lv.values[i] as u64, lv.values[i + 1] as u64))
                } else {
                    None
                }
            })
            .unwrap_or((0, fg.total as u64))
    }

    /// `(x_start, x_end)` of the currently selected frame in samples.
    fn sel_bounds(&self, fg: &FlameGraph) -> Option<(u64, u64)> {
        let lv = fg.levels.get(self.sel_level)?;
        let i = self.sel_frame * 4;
        (i + 1 < lv.values.len()).then(|| {
            (lv.values[i] as u64, lv.values[i] as u64 + lv.values[i + 1] as u64)
        })
    }

    pub fn move_right(&mut self, fg: &FlameGraph) {
        let Some(lv) = fg.levels.get(self.sel_level) else { return };
        let (rx, rw) = self.root_bounds(fg);
        let min_w = self.min_nav_width(rw);
        for i in (self.sel_frame + 1)..(lv.values.len() / 4) {
            if frame_visible(&lv.values, i, rx, rw) && lv.values[i * 4 + 1] as u64 > min_w {
                self.sel_frame = i;
                return;
            }
        }
    }

    pub fn move_left(&mut self, fg: &FlameGraph) {
        let Some(lv) = fg.levels.get(self.sel_level) else { return };
        let (rx, rw) = self.root_bounds(fg);
        let min_w = self.min_nav_width(rw);
        for i in (0..self.sel_frame).rev() {
            if frame_visible(&lv.values, i, rx, rw) && lv.values[i * 4 + 1] as u64 > min_w {
                self.sel_frame = i;
                return;
            }
        }
    }

    /// Move selection to a child level (deeper into the call stack).
    pub fn move_up(&mut self, fg: &FlameGraph) {
        let new_level = self.sel_level + 1;
        if new_level >= fg.levels.len() {
            return;
        }
        let Some((sx, se)) = self.sel_bounds(fg) else { return };
        let (_, rw) = self.root_bounds(fg);
        let min_w = self.min_nav_width(rw);
        let lv = &fg.levels[new_level].values;
        for i in 0..(lv.len() / 4) {
            let off = lv[i * 4] as u64;
            let w = lv[i * 4 + 1] as u64;
            if w > min_w && off + w > sx && off < se {
                self.sel_level = new_level;
                self.sel_frame = i;
                return;
            }
        }
    }

    /// Move selection to the parent level (shallower in the call stack).
    pub fn move_down(&mut self, fg: &FlameGraph) {
        if self.sel_level == 0 || self.sel_level <= self.root_level {
            return;
        }
        let new_level = self.sel_level - 1;
        let Some((sx, se)) = self.sel_bounds(fg) else { return };
        let (rx, rw) = self.root_bounds(fg);
        let min_w = self.min_nav_width(rw);
        let lv = &fg.levels[new_level].values;
        // Prefer the frame that fully contains the selection.
        for i in 0..(lv.len() / 4) {
            let off = lv[i * 4] as u64;
            let w = lv[i * 4 + 1] as u64;
            if frame_visible(lv, i, rx, rw) && w > min_w && off <= sx && off + w >= se {
                self.sel_level = new_level;
                self.sel_frame = i;
                return;
            }
        }
        // Fallback: first visible frame in parent level.
        for i in 0..(lv.len() / 4) {
            if frame_visible(lv, i, rx, rw) && lv[i * 4 + 1] as u64 > min_w {
                self.sel_level = new_level;
                self.sel_frame = i;
                return;
            }
        }
    }

    /// Zoom into the currently selected frame.
    pub fn zoom_in(&mut self, fg: &FlameGraph) {
        self.root_level = self.sel_level;
        self.root_frame = self.sel_frame;
        self.move_up(fg);
    }

    /// Zoom out one level (or reset to full view if already at level 0).
    pub fn zoom_out(&mut self) {
        if self.root_level > 0 {
            self.root_level -= 1;
        }
        self.root_frame = 0;
        // Keep selection in bounds.
        if self.sel_level < self.root_level {
            self.sel_level = self.root_level;
            self.sel_frame = self.root_frame;
        }
    }
}

fn frame_visible(lv: &[i64], idx: usize, root_x: u64, root_w: u64) -> bool {
    let off = lv[idx * 4] as u64;
    let w = lv[idx * 4 + 1] as u64;
    w > 0 && off + w > root_x && off < root_x + root_w
}

// ── View model types (serialised, passed to the TUI layer) ────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlamegraphFrameView {
    pub name: String,
    /// X start in samples, relative to the viewport root's left edge.
    pub x_start: u64,
    /// Frame width in samples.
    pub width: u64,
    /// Self samples (time spent directly in this function, not in callees).
    pub self_samples: u64,
    /// Percentage of the viewport root's total samples.
    pub total_pct: f64,
    /// Percentage of viewport root's total samples spent as self.
    pub self_pct: f64,
    /// Whether this frame is currently highlighted.
    pub is_selected: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlamegraphLevelView {
    pub frames: Vec<FlamegraphFrameView>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlamegraphView {
    /// Visible levels, root first (index 0 = root / bottom of the flamegraph).
    pub levels: Vec<FlamegraphLevelView>,
    /// Total samples in the current zoom viewport.
    pub root_samples: u64,
    pub units: String,
    /// Precomputed info about the selected frame for the status line.
    pub selected_name: String,
    pub selected_total_pct: f64,
    pub selected_self_pct: f64,
}

// ── Sandwich view ─────────────────────────────────────────────────────────────

/// The three-panel "sandwich" view for a pinned function name, matching the
/// behaviour of the Grafana flamegraph panel's sandwich mode.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SandwichView {
    pub target_name: String,
    /// Total samples across all occurrences of the target function.
    pub target_samples: u64,
    /// Total samples in the full flamegraph (for "% of profile" label).
    pub root_samples: u64,
    /// Merged callers: outermost (root-side) first, direct caller last.
    pub callers: Vec<FlamegraphLevelView>,
    /// Merged callees: direct callee first, outermost last.
    pub callees: Vec<FlamegraphLevelView>,
    pub units: String,
}

/// Build a sandwich view for `target_name` from the flamegraph.
///
/// Finds all occurrences of `target_name`, then:
/// - Merges the ancestor chains into a callers tree (each ancestor is credited
///   with the occurrence's own sample count, as Grafana does).
/// - Merges the descendant trees into a callees tree (actual sample widths).
/// Frames at each merged level are sorted by total samples descending and laid
/// out consecutively from x=0.
pub fn build_sandwich_view(fg: &FlameGraph, target_name: &str, units: String) -> Option<SandwichView> {
    use std::collections::BTreeMap;

    let root_samples = fg.total as u64;

    // ── Find all occurrences ─────────────────────────────────────────────────
    // Stored as parallel vecs: (level_idx, x_off, x_end, width)
    let mut occ_level: Vec<usize> = Vec::new();
    let mut occ_x_off: Vec<u64> = Vec::new();
    let mut occ_x_end: Vec<u64> = Vec::new();
    let mut occ_width: Vec<u64> = Vec::new();

    for (li, lv) in fg.levels.iter().enumerate() {
        let raw = &lv.values;
        for i in 0..(raw.len() / 4) {
            let width = raw[i * 4 + 1] as u64;
            if width == 0 {
                continue;
            }
            let name_idx = raw[i * 4 + 3] as usize;
            if fg.names.get(name_idx).map(|n| n.as_str()) == Some(target_name) {
                let x_off = raw[i * 4] as u64;
                occ_level.push(li);
                occ_x_off.push(x_off);
                occ_x_end.push(x_off + width);
                occ_width.push(width);
            }
        }
    }
    if occ_level.is_empty() {
        return None;
    }
    let n_occs = occ_level.len();
    let target_samples: u64 = occ_width.iter().sum();

    // ── Build callees (what the target calls) ────────────────────────────────
    // depth_callees[d] = BTreeMap<name, (total_samples, self_samples)>
    let mut depth_callees: Vec<BTreeMap<String, (u64, u64)>> = Vec::new();

    for idx in 0..n_occs {
        let mut ranges: Vec<(u64, u64)> = vec![(occ_x_off[idx], occ_x_end[idx])];
        let mut d = 0usize;
        loop {
            if ranges.is_empty() {
                break;
            }
            let li = occ_level[idx] + 1 + d;
            if li >= fg.levels.len() {
                break;
            }
            if d == depth_callees.len() {
                depth_callees.push(BTreeMap::new());
            }
            let raw = &fg.levels[li].values;
            let mut next_ranges: Vec<(u64, u64)> = Vec::new();
            for i in 0..(raw.len() / 4) {
                let fx = raw[i * 4] as u64;
                let fw = raw[i * 4 + 1] as u64;
                if fw == 0 {
                    continue;
                }
                let fx_end = fx + fw;
                let fs = raw[i * 4 + 2] as u64;
                let mut clipped = 0u64;
                for &(rs, re) in &ranges {
                    if fx < re && fx_end > rs {
                        clipped += fx_end.min(re) - fx.max(rs);
                        next_ranges.push((fx.max(rs), fx_end.min(re)));
                    }
                }
                if clipped == 0 {
                    continue;
                }
                let name = fg.names.get(raw[i * 4 + 3] as usize).cloned().unwrap_or_default();
                let self_s = (fs as u128 * clipped as u128 / fw as u128) as u64;
                let e = depth_callees[d].entry(name).or_insert((0, 0));
                e.0 += clipped;
                e.1 += self_s;
            }
            ranges = next_ranges;
            d += 1;
        }
    }

    // ── Build callers (what calls the target) ────────────────────────────────
    // Each ancestor is credited with occ.width (Grafana's "trim to child" approach).
    // depth_callers[d] = BTreeMap<name, total_samples>  (d=0 = direct caller)
    let max_caller_depth = *occ_level.iter().max().unwrap_or(&0);
    let mut depth_callers: Vec<BTreeMap<String, u64>> =
        (0..max_caller_depth).map(|_| BTreeMap::new()).collect();

    for idx in 0..n_occs {
        for d in 0..occ_level[idx] {
            let li = occ_level[idx] - 1 - d;
            let raw = &fg.levels[li].values;
            for i in 0..(raw.len() / 4) {
                let fx = raw[i * 4] as u64;
                let fw = raw[i * 4 + 1] as u64;
                if fw == 0 {
                    continue;
                }
                if fx <= occ_x_off[idx] && fx + fw > occ_x_off[idx] {
                    let name =
                        fg.names.get(raw[i * 4 + 3] as usize).cloned().unwrap_or_default();
                    *depth_callers[d].entry(name).or_insert(0) += occ_width[idx];
                    break;
                }
            }
        }
    }

    // ── Convert to FlamegraphLevelView ────────────────────────────────────────
    // Frames sorted by total samples descending, laid out consecutively from x=0.
    let make_callee_level = |map: &BTreeMap<String, (u64, u64)>| -> FlamegraphLevelView {
        let mut v: Vec<_> = map.iter().map(|(n, &(t, s))| (n.as_str(), t, s)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        let mut x = 0u64;
        FlamegraphLevelView {
            frames: v
                .into_iter()
                .map(|(n, t, s)| {
                    let f = FlamegraphFrameView {
                        name: n.to_string(),
                        x_start: x,
                        width: t,
                        self_samples: s,
                        total_pct: t as f64 / target_samples as f64 * 100.0,
                        self_pct: s as f64 / target_samples as f64 * 100.0,
                        is_selected: false,
                    };
                    x += t;
                    f
                })
                .collect(),
        }
    };

    let make_caller_level = |map: &BTreeMap<String, u64>| -> FlamegraphLevelView {
        let mut v: Vec<_> = map.iter().map(|(n, &t)| (n.as_str(), t)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        let mut x = 0u64;
        FlamegraphLevelView {
            frames: v
                .into_iter()
                .map(|(n, t)| {
                    let f = FlamegraphFrameView {
                        name: n.to_string(),
                        x_start: x,
                        width: t,
                        self_samples: 0,
                        total_pct: t as f64 / target_samples as f64 * 100.0,
                        self_pct: 0.0,
                        is_selected: false,
                    };
                    x += t;
                    f
                })
                .collect(),
        }
    };

    let callees: Vec<_> = depth_callees.iter().map(|m| make_callee_level(m)).collect();
    let mut callers: Vec<_> = depth_callers.iter().map(|m| make_caller_level(m)).collect();
    callers.reverse(); // outermost (root-side) first for top-to-bottom rendering

    Some(SandwichView {
        target_name: target_name.to_string(),
        target_samples,
        root_samples,
        callers,
        callees,
        units,
    })
}

// ── View builder ──────────────────────────────────────────────────────────────

pub fn build_flamegraph_view(fg: &FlameGraph, nav: &FlamegraphNav) -> FlamegraphView {
    let (root_x, root_w) = nav.root_bounds(fg);

    if root_w == 0 {
        return FlamegraphView {
            levels: vec![],
            root_samples: 0,
            units: String::new(),
            selected_name: String::new(),
            selected_total_pct: 0.0,
            selected_self_pct: 0.0,
        };
    }

    let mut levels = Vec::new();
    let mut selected_name = String::new();
    let mut selected_total_pct = 0.0f64;
    let mut selected_self_pct = 0.0f64;

    for (level_idx, level) in fg.levels.iter().enumerate().skip(nav.root_level) {
        let raw = &level.values;
        let mut frames = Vec::new();
        let count = raw.len() / 4;

        for i in 0..count {
            let offset = raw[i * 4] as u64;
            let total = raw[i * 4 + 1] as u64;
            let self_samples = raw[i * 4 + 2] as u64;
            let name_idx = raw[i * 4 + 3] as usize;

            if !frame_visible(raw, i, root_x, root_w) {
                continue;
            }

            // Clip frame to the current viewport so partial frames don't bleed
            // outside the visible range (causes misalignment when zoomed in).
            let vis_start = offset.max(root_x);
            let vis_end = (offset + total).min(root_x + root_w);
            let x_start = vis_start - root_x;
            let clipped_width = vis_end.saturating_sub(vis_start);
            if clipped_width == 0 {
                continue;
            }

            let name = fg.names.get(name_idx).cloned().unwrap_or_default();
            // total_pct uses the unclipped sample count (represents true coverage).
            let total_pct = total as f64 / root_w as f64 * 100.0;
            let self_pct = self_samples as f64 / root_w as f64 * 100.0;
            let is_selected = level_idx == nav.sel_level && i == nav.sel_frame;

            if is_selected {
                selected_name = name.clone();
                selected_total_pct = total_pct;
                selected_self_pct = self_pct;
            }

            frames.push(FlamegraphFrameView {
                name,
                x_start,
                width: clipped_width,
                self_samples,
                total_pct,
                self_pct,
                is_selected,
            });
        }

        levels.push(FlamegraphLevelView { frames });
    }

    FlamegraphView {
        levels,
        root_samples: root_w,
        units: String::new(),
        selected_name,
        selected_total_pct,
        selected_self_pct,
    }
}

// ── Timeline ──────────────────────────────────────────────────────────────────

/// One individual exemplar delivered alongside a series.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TimelineExemplar {
    /// Exemplar-specific labels (pod, node, …).
    pub labels: Vec<(String, String)>,
    /// Unique profile identifier (UUID).
    pub profile_id: String,
    /// Span ID (non-empty when the exemplar was captured during a trace span).
    pub span_id: String,
    /// Total sample value (e.g. CPU nanoseconds).
    pub value: i64,
    pub timestamp_ms: i64,
}

/// One individual time-series returned by the SelectSeries API, with its full label set.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TimelineSeries {
    /// All label key-value pairs for this series (e.g. `[("pod", "pod-1"), ...]`).
    pub labels: Vec<(String, String)>,
    /// Aggregated data points for the chart.
    pub points: Vec<(f64, f64)>,
    /// Individual exemplars delivered alongside the series (populated in INDIVIDUAL mode).
    pub exemplars: Vec<TimelineExemplar>,
}

/// One time slot in a heatmap (raw API data).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HeatmapSlot {
    pub timestamp_ms: i64,
    /// Lower y bound of each bucket.
    pub y_min: Vec<f64>,
    /// Sample count per bucket.
    pub counts: Vec<i32>,
}

/// View model for the timeline chart.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TimelineView {
    /// Aggregated data points (sum across all series) for the chart widget.
    pub data: Vec<(f64, f64)>,
    pub value_min: f64,
    pub value_max: f64,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Per-series data (used to aggregate the chart).
    pub series: Vec<TimelineSeries>,
    /// Flattened exemplars from all series, sorted by value descending.
    pub exemplars: Vec<TimelineExemplar>,
    /// Label keys that vary across exemplars (non-common labels).
    pub varying_label_keys: Vec<String>,
}

/// View model for the heatmap.
/// `columns[time_col][bucket_row]` = normalised intensity in [0, 1].
/// Row 0 is the highest-value bucket; the last row is the lowest.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HeatmapView {
    pub columns: Vec<Vec<f64>>,
    pub n_buckets: usize,
    pub y_min: f64,
    pub y_max: f64,
    pub start_ms: i64,
    pub end_ms: i64,
}

pub fn build_timeline_view(series_list: &[TimelineSeries]) -> Option<TimelineView> {
    if series_list.is_empty() {
        return None;
    }
    // Aggregate all series for the chart (sorted by timestamp).
    let mut map: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
    for s in series_list {
        for &(ts, v) in &s.points {
            *map.entry(ts as i64).or_default() += v;
        }
    }
    if map.is_empty() {
        return None;
    }
    let data: Vec<(f64, f64)> = map.iter().map(|(&ts, &v)| (ts as f64, v)).collect();
    let value_min = data.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
    let value_max = data.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
    let start_ms = *map.keys().next().unwrap();
    let end_ms = *map.keys().next_back().unwrap();
    // Collect and sort all exemplars by value descending.
    let mut exemplars: Vec<TimelineExemplar> = series_list
        .iter()
        .flat_map(|s| s.exemplars.iter().cloned())
        .collect();
    exemplars.sort_by(|a, b| b.value.cmp(&a.value));

    let varying_label_keys = if exemplars.is_empty() {
        compute_varying_label_keys(series_list)
    } else {
        compute_varying_label_keys_exemplars(&exemplars)
    };
    Some(TimelineView {
        data,
        value_min,
        value_max,
        start_ms,
        end_ms,
        series: series_list.to_vec(),
        exemplars,
        varying_label_keys,
    })
}

fn compute_varying_label_keys_exemplars(exemplars: &[TimelineExemplar]) -> Vec<String> {
    if exemplars.len() <= 1 {
        return exemplars
            .first()
            .map(|e| e.labels.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
    }
    let mut key_values: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for e in exemplars {
        for (k, v) in &e.labels {
            key_values.entry(k.clone()).or_default().insert(v.clone());
        }
    }
    key_values
        .into_iter()
        .filter(|(_, vals)| vals.len() > 1)
        .map(|(k, _)| k)
        .collect()
}

fn compute_varying_label_keys(series_list: &[TimelineSeries]) -> Vec<String> {
    if series_list.len() <= 1 {
        return vec![];
    }
    let mut key_values: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for s in series_list {
        for (k, v) in &s.labels {
            key_values.entry(k.clone()).or_default().insert(v.clone());
        }
    }
    key_values
        .into_iter()
        .filter(|(_, vals)| vals.len() > 1)
        .map(|(k, _)| k)
        .collect()
}

pub fn build_heatmap_view(slots: &[HeatmapSlot]) -> Option<HeatmapView> {
    if slots.is_empty() {
        return None;
    }
    let global_max = slots
        .iter()
        .flat_map(|s| s.counts.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    let first = &slots[0];
    let n_buckets = first.counts.len();
    let y_min = first.y_min.first().copied().unwrap_or(0.0);
    let y_max = first.y_min.last().copied().unwrap_or(1.0);

    // Columns: each slot → one column; row 0 = highest bucket.
    let columns: Vec<Vec<f64>> = slots
        .iter()
        .map(|slot| {
            slot.counts
                .iter()
                .rev()
                .map(|&c| c as f64 / global_max)
                .collect()
        })
        .collect();

    Some(HeatmapView {
        columns,
        n_buckets,
        y_min,
        y_max,
        start_ms: slots.first().map(|s| s.timestamp_ms).unwrap_or(0),
        end_ms: slots.last().map(|s| s.timestamp_ms).unwrap_or(0),
    })
}

// ── Time range parser ─────────────────────────────────────────────────────────

/// Parses "15m", "1h", "6h", "24h", "7d" → seconds.
pub fn parse_time_range(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('h') {
        n.parse::<u64>().ok().map(|h| h * 3600)
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>().ok().map(|m| m * 60)
    } else if let Some(n) = s.strip_suffix('d') {
        n.parse::<u64>().ok().map(|d| d * 86400)
    } else {
        None
    }
}
