use log::debug;
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

// ── Shared sandwich tree helpers (private) ────────────────────────────────────

struct SandwichNode {
    name: String,
    total: u64,
    self_s: u64, // 0 for caller nodes
    children: Vec<SandwichNode>,
}

/// Collect callee frames at `level` that intersect any of `ranges`, recursing into children.
fn build_callee_subtree(fg: &FlameGraph, level: usize, ranges: &[(u64, u64)]) -> Vec<SandwichNode> {
    if level >= fg.levels.len() || ranges.is_empty() {
        return Vec::new();
    }
    let raw = &fg.levels[level].values;
    let mut nodes: Vec<SandwichNode> = Vec::new();
    for i in 0..(raw.len() / 4) {
        let fx = raw[i * 4] as u64;
        let fw = raw[i * 4 + 1] as u64;
        if fw == 0 {
            continue;
        }
        let fx_end = fx + fw;
        let fs = raw[i * 4 + 2] as u64;
        let mut clipped_total = 0u64;
        let mut child_ranges: Vec<(u64, u64)> = Vec::new();
        for &(rs, re) in ranges {
            if fx < re && fx_end > rs {
                let start = fx.max(rs);
                let end = fx_end.min(re);
                clipped_total += end - start;
                child_ranges.push((start, end));
            }
        }
        if clipped_total == 0 {
            continue;
        }
        let self_s = (fs as u128 * clipped_total as u128 / fw as u128) as u64;
        let name = fg.names.get(raw[i * 4 + 3] as usize).cloned().unwrap_or_default();
        let children = build_callee_subtree(fg, level + 1, &child_ranges);
        nodes.push(SandwichNode { name, total: clipped_total, self_s, children });
    }
    nodes
}

/// Walk from `level` toward `occ_level`, following the single frame that
/// contains `occ_x_off`, returning a one-path chain credited with `occ_width`.
fn build_caller_chain(
    fg: &FlameGraph,
    level: usize,
    occ_level: usize,
    occ_x_off: u64,
    occ_width: u64,
) -> Vec<SandwichNode> {
    if level >= occ_level {
        return Vec::new();
    }
    let raw = &fg.levels[level].values;
    for i in 0..(raw.len() / 4) {
        let fx = raw[i * 4] as u64;
        let fw = raw[i * 4 + 1] as u64;
        if fw == 0 {
            continue;
        }
        if fx <= occ_x_off && fx + fw > occ_x_off {
            let name = fg.names.get(raw[i * 4 + 3] as usize).cloned().unwrap_or_default();
            let children = build_caller_chain(fg, level + 1, occ_level, occ_x_off, occ_width);
            return vec![SandwichNode { name, total: occ_width, self_s: 0, children }];
        }
    }
    Vec::new()
}

/// Merge nodes with the same name, summing samples and recursively merging children.
fn merge_nodes(nodes: Vec<SandwichNode>) -> Vec<SandwichNode> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<String, (u64, u64, Vec<SandwichNode>)> = BTreeMap::new();
    for node in nodes {
        let entry = by_name.entry(node.name).or_insert((0, 0, Vec::new()));
        entry.0 += node.total;
        entry.1 += node.self_s;
        entry.2.extend(node.children);
    }
    by_name
        .into_iter()
        .map(|(name, (total, self_s, children))| SandwichNode {
            name,
            total,
            self_s,
            children: merge_nodes(children),
        })
        .collect()
}

/// Maximum depth of the deepest leaf reachable from `nodes` (leaf = 1, empty = 0).
fn subtree_height(nodes: &[SandwichNode]) -> usize {
    nodes.iter().map(|n| 1 + subtree_height(&n.children)).max().unwrap_or(0)
}

/// Lay out callee nodes top-down: depth 0 = first row, children below their parent.
fn flatten_tree(
    nodes: &mut Vec<SandwichNode>,
    x_start: u64,
    target_samples: u64,
    depth: usize,
    levels: &mut Vec<FlamegraphLevelView>,
) {
    if nodes.is_empty() {
        return;
    }
    nodes.sort_by(|a, b| b.total.cmp(&a.total));
    while levels.len() <= depth {
        levels.push(FlamegraphLevelView { frames: Vec::new() });
    }
    let mut x = x_start;
    for node in nodes.iter_mut() {
        levels[depth].frames.push(FlamegraphFrameView {
            name: node.name.clone(),
            x_start: x,
            width: node.total,
            self_samples: node.self_s,
            total_pct: node.total as f64 / target_samples as f64 * 100.0,
            self_pct: node.self_s as f64 / target_samples as f64 * 100.0,
            is_selected: false,
        });
        flatten_tree(&mut node.children, x, target_samples, depth + 1, levels);
        x += node.total;
    }
}

/// Lay out caller nodes bottom-aligned: each node's row is determined by its
/// subtree height so that every direct caller (leaf) lands at `total_depth - 1`
/// regardless of how deep its path goes. Shorter paths get empty space at the top.
fn flatten_caller_tree(
    nodes: &mut Vec<SandwichNode>,
    x_start: u64,
    target_samples: u64,
    total_depth: usize,
    levels: &mut Vec<FlamegraphLevelView>,
) {
    if nodes.is_empty() {
        return;
    }
    nodes.sort_by(|a, b| b.total.cmp(&a.total));
    let mut x = x_start;
    for node in nodes.iter_mut() {
        let h = 1 + subtree_height(&node.children);
        let row = total_depth - h;
        while levels.len() <= row {
            levels.push(FlamegraphLevelView { frames: Vec::new() });
        }
        levels[row].frames.push(FlamegraphFrameView {
            name: node.name.clone(),
            x_start: x,
            width: node.total,
            self_samples: node.self_s,
            total_pct: node.total as f64 / target_samples as f64 * 100.0,
            self_pct: node.self_s as f64 / target_samples as f64 * 100.0,
            is_selected: false,
        });
        flatten_caller_tree(&mut node.children, x, target_samples, total_depth, levels);
        x += node.total;
    }
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

    // ── Build callees (tree-based, preserving parent-child x-positioning) ────
    let mut callee_roots: Vec<SandwichNode> = Vec::new();
    for idx in 0..n_occs {
        callee_roots.extend(build_callee_subtree(
            fg,
            occ_level[idx] + 1,
            &[(occ_x_off[idx], occ_x_end[idx])],
        ));
    }
    let mut callee_roots = merge_nodes(callee_roots);
    let mut callees: Vec<FlamegraphLevelView> = Vec::new();
    flatten_tree(&mut callee_roots, 0, target_samples, 0, &mut callees);

    // ── Build callers (tree-based, preserving parent-child x-positioning) ─────
    // Each ancestor is credited with occ.width (Grafana's "trim to child" approach).
    let mut caller_roots: Vec<SandwichNode> = Vec::new();
    for idx in 0..n_occs {
        caller_roots.extend(build_caller_chain(
            fg,
            0,
            occ_level[idx],
            occ_x_off[idx],
            occ_width[idx],
        ));
    }
    let mut caller_roots = merge_nodes(caller_roots);
    let total_caller_depth = subtree_height(&caller_roots);
    let mut callers: Vec<FlamegraphLevelView> = Vec::new();
    flatten_caller_tree(&mut caller_roots, 0, target_samples, total_caller_depth, &mut callers);
    // Ensure all levels up to total_caller_depth exist (intermediate rows may be empty).
    while callers.len() < total_caller_depth {
        callers.push(FlamegraphLevelView { frames: Vec::new() });
    }

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
    /// Timestamp of the right edge (x_max) of this slot. slot covers [timestamp_ms - step_ms, timestamp_ms).
    pub timestamp_ms: i64,
    /// Step duration in milliseconds from the query. Must not be derived from consecutive timestamps
    /// because empty slots may be omitted from the response.
    #[serde(default)]
    pub step_ms: i64,
    /// Lower y bound of each bucket.
    pub y_min: Vec<f64>,
    /// Sample count per bucket.
    pub counts: Vec<i32>,
    /// Exemplars attached to this slot (populated when exemplar_type is set).
    #[serde(default)]
    pub exemplars: Vec<TimelineExemplar>,
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
    /// Lower bound of the lowest bucket.
    pub y_min: f64,
    /// Upper bound of the highest bucket (= y_min[-1] + bucket_step).
    pub y_max: f64,
    /// Timestamp of the left edge of the first column (= first slot timestamp).
    pub start_ms: i64,
    /// Timestamp of the right edge of the last column (= last slot timestamp + step_ms).
    pub end_ms: i64,
    /// Duration of each time slot in milliseconds.
    #[serde(default)]
    pub step_ms: i64,
    /// Exemplars from timeline series (populated in app::view when timeline data is available).
    #[serde(default)]
    pub exemplars: Vec<TimelineExemplar>,
    /// Label keys that vary across exemplars.
    #[serde(default)]
    pub varying_label_keys: Vec<String>,
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

pub fn compute_varying_label_keys_exemplars(exemplars: &[TimelineExemplar]) -> Vec<String> {
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

    // step_ms comes from the query (stored on each slot), not derived from timestamps
    // because empty slots may be omitted from the API response.
    let step_ms = slots.iter().map(|s| s.step_ms).find(|&s| s > 0).unwrap_or(0);

    // Multiple HeatmapSeries may contribute slots at the same timestamp.
    // Sort by timestamp then merge same-timestamp slots by summing counts
    // and pooling exemplars.
    let slots: Vec<HeatmapSlot> = {
        let mut sorted = slots.to_vec();
        sorted.sort_by_key(|s| s.timestamp_ms);
        let mut merged: Vec<HeatmapSlot> = Vec::with_capacity(sorted.len());
        for slot in sorted {
            if let Some(last) = merged.last_mut() {
                if last.timestamp_ms == slot.timestamp_ms {
                    for (a, b) in last.counts.iter_mut().zip(slot.counts.iter()) {
                        *a = a.saturating_add(*b);
                    }
                    last.exemplars.extend(slot.exemplars);
                    continue;
                }
            }
            merged.push(slot);
        }
        merged
    };
    let slots = slots.as_slice();

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
    // y_max is the upper bound of the highest bucket.
    // y_min stores the lower bound of each bucket; the upper bound of the last
    // bucket is lower_bound[-1] + bucket_step.
    let y_max_lower = first.y_min.last().copied().unwrap_or(1.0);
    let bucket_step = if first.y_min.len() >= 2 {
        first.y_min[1] - first.y_min[0]
    } else {
        y_max_lower - y_min
    };
    let y_max = y_max_lower + bucket_step;

    // Columns: each slot → one column; row 0 = highest bucket.
    // count=0  → intensity 0.0  (transparent/black, no activity).
    // count>0  → intensity in (0, 1]: linearly normalised but clamped to
    //            at least 1/(PALETTE_SIZE-1) so that even a single sample
    //            always renders as the first non-black colour regardless of
    //            how large the global maximum is.
    const MIN_NONZERO: f64 = 1.0 / 9.0; // 1 / (palette size − 1)
    let columns: Vec<Vec<f64>> = slots
        .iter()
        .map(|slot| {
            slot.counts
                .iter()
                .rev()
                .map(|&c| {
                    if c == 0 {
                        0.0
                    } else {
                        (c as f64 / global_max).max(MIN_NONZERO)
                    }
                })
                .collect()
        })
        .collect();

    let mut exemplars: Vec<TimelineExemplar> = slots
        .iter()
        .flat_map(|s| s.exemplars.iter().cloned())
        .collect();
    exemplars.sort_by(|a, b| b.value.cmp(&a.value));
    let varying_label_keys = compute_varying_label_keys_exemplars(&exemplars);

    // slot.timestamp_ms is the x_max (right edge) of the slot interval.
    // x_min = timestamp_ms - step_ms, x_max = timestamp_ms.
    let start_ms = slots.first().map(|s| s.timestamp_ms).unwrap_or(0) - step_ms;
    let end_ms = slots.last().map(|s| s.timestamp_ms).unwrap_or(0);

    debug!(
        "build_heatmap_view: n_slots={} step_ms={} start_ms={} end_ms={}",
        slots.len(), step_ms, start_ms, end_ms,
    );
    for (i, slot) in slots.iter().enumerate() {
        if !slot.exemplars.is_empty() {
            debug!(
                "  slot[{}] timestamp_ms={} (x_min={} x_max={}) exemplars:",
                i, slot.timestamp_ms, slot.timestamp_ms - step_ms, slot.timestamp_ms,
            );
            for e in &slot.exemplars {
                let offset_ms = e.timestamp_ms - (slot.timestamp_ms - step_ms);
                debug!(
                    "    exemplar ts={} offset_from_x_min={}ms value={}",
                    e.timestamp_ms, offset_ms, e.value,
                );
            }
        }
    }

    Some(HeatmapView {
        columns,
        n_buckets,
        y_min,
        y_max,
        start_ms,
        end_ms,
        step_ms,
        exemplars,
        varying_label_keys,
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

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a FlameGraph with pre-normalised (absolute) x offsets.
    fn make_fg(total: i64, names: Vec<&str>, levels: Vec<Vec<i64>>) -> FlameGraph {
        FlameGraph {
            names: names.into_iter().map(|s| s.to_string()).collect(),
            levels: levels.into_iter().map(|v| Level { values: v }).collect(),
            total,
            max_self: 0,
        }
    }

    // ── Callee positioning ────────────────────────────────────────────────────

    /// Callees at depth >1 must be positioned under their parent frame, not
    /// all left-aligned from x=0.
    ///
    /// Layout:
    ///   L0: root       (0..100)
    ///   L1: target     (0..100)
    ///   L2: funcA      (0..60),  funcB     (60..100)
    ///   L3: child_of_A (0..40),  child_of_B(60..80)
    ///
    /// child_of_B is under funcB and must appear at x=60, not x=40.
    #[test]
    fn test_callee_positioning() {
        let graph = make_fg(
            100,
            vec!["root", "target", "funcA", "funcB", "child_of_A", "child_of_B"],
            vec![
                vec![0, 100, 0, 0],               // L0
                vec![0, 100, 0, 1],               // L1: target
                vec![0, 60, 10, 2, 60, 40, 5, 3], // L2: funcA, funcB
                vec![0, 40, 40, 4, 60, 20, 20, 5], // L3: child_of_A, child_of_B
            ],
        );

        let sw = build_sandwich_view(&graph, "target", "samples".into()).unwrap();

        assert_eq!(sw.target_samples, 100);
        assert_eq!(sw.callees.len(), 2);

        // Depth 0: funcA(60) then funcB(40), sorted by total desc.
        let l0 = &sw.callees[0].frames;
        assert_eq!(l0.len(), 2);
        assert_eq!(l0[0].name, "funcA");
        assert_eq!(l0[0].x_start, 0);
        assert_eq!(l0[0].width, 60);
        assert_eq!(l0[1].name, "funcB");
        assert_eq!(l0[1].x_start, 60);
        assert_eq!(l0[1].width, 40);

        // Depth 1: child_of_A under funcA, child_of_B under funcB.
        let l1 = &sw.callees[1].frames;
        assert_eq!(l1.len(), 2);
        let a = l1.iter().find(|f| f.name == "child_of_A").unwrap();
        let b = l1.iter().find(|f| f.name == "child_of_B").unwrap();
        assert_eq!(a.x_start, 0);
        assert_eq!(a.width, 40);
        // Key: child_of_B must sit under funcB (x=60), not left-aligned at x=40.
        assert_eq!(b.x_start, 60, "child_of_B must be positioned under funcB");
        assert_eq!(b.width, 20);
    }

    // ── Caller positioning ────────────────────────────────────────────────────

    /// Callers at depth >1 must be positioned under their parent frame, not all
    /// left-aligned from x=0.
    ///
    /// Layout:
    ///   L0: root    (0..100)
    ///   L1: callerA (0..60),  callerB (60..100)
    ///   L2: target  (0..60),  target  (60..100)  ← two occurrences
    ///
    /// callerB is under the right half of root and must appear at x=60, not x=0.
    #[test]
    fn test_caller_positioning() {
        let graph = make_fg(
            100,
            vec!["root", "callerA", "callerB", "target"],
            vec![
                vec![0, 100, 0, 0],                   // L0: root
                vec![0, 60, 0, 1, 60, 40, 0, 2],      // L1: callerA, callerB
                vec![0, 60, 60, 3, 60, 40, 40, 3],    // L2: target x2
            ],
        );

        let sw = build_sandwich_view(&graph, "target", "samples".into()).unwrap();

        assert_eq!(sw.target_samples, 100);
        assert_eq!(sw.callers.len(), 2);

        // Depth 0 (outermost): root credited with both occurrences.
        let l0 = &sw.callers[0].frames;
        assert_eq!(l0.len(), 1);
        assert_eq!(l0[0].name, "root");
        assert_eq!(l0[0].x_start, 0);
        assert_eq!(l0[0].width, 100);

        // Depth 1 (direct callers): callerA (60) and callerB (40).
        let l1 = &sw.callers[1].frames;
        assert_eq!(l1.len(), 2);
        let a = l1.iter().find(|f| f.name == "callerA").unwrap();
        let b = l1.iter().find(|f| f.name == "callerB").unwrap();
        assert_eq!(a.x_start, 0);
        assert_eq!(a.width, 60);
        // Key: callerB must sit under root's right portion (x=60), not at x=0.
        assert_eq!(b.x_start, 60, "callerB must be positioned within root's x-range");
        assert_eq!(b.width, 40);
    }

    // ── Merging ───────────────────────────────────────────────────────────────

    /// Same-name callee frames from different target occurrences must be merged
    /// into a single frame with summed widths and self-samples.
    ///
    /// Layout:
    ///   L0: root   (0..100)
    ///   L1: pathA  (0..60),  pathB   (60..100)
    ///   L2: target (0..60),  target  (60..100)
    ///   L3: common (0..60),  common  (60..100)
    #[test]
    fn test_callee_merge_same_name() {
        let graph = make_fg(
            100,
            vec!["root", "pathA", "pathB", "target", "common"],
            vec![
                vec![0, 100, 0, 0],                   // L0
                vec![0, 60, 0, 1, 60, 40, 0, 2],      // L1: pathA, pathB
                vec![0, 60, 0, 3, 60, 40, 0, 3],      // L2: target x2
                vec![0, 60, 60, 4, 60, 40, 40, 4],    // L3: common x2
            ],
        );

        let sw = build_sandwich_view(&graph, "target", "samples".into()).unwrap();

        assert_eq!(sw.target_samples, 100);
        assert_eq!(sw.callees.len(), 1);

        let l0 = &sw.callees[0].frames;
        assert_eq!(l0.len(), 1, "both 'common' frames should merge into one");
        assert_eq!(l0[0].name, "common");
        assert_eq!(l0[0].width, 100, "merged width should be 60+40");
        assert_eq!(l0[0].self_samples, 100, "merged self-samples should be 60+40");
    }

    // ── Caller bottom-alignment ───────────────────────────────────────────────

    /// When two parallel call paths have different depths, all direct callers
    /// must appear at the same (bottom) level regardless of path length.
    ///
    /// Layout:
    ///   L0: root  (0..100)
    ///   L1: longPath (0..60), shortPath (60..100)
    ///   L2: longL2   (0..60), target2   (60..100)  ← shortPath's target at L2
    ///   L3: longL3   (0..60)
    ///   L4: longL4   (0..60)
    ///   L5: target1  (0..60)                        ← longPath's target at L5
    ///
    /// Both targets are "target". Direct callers: longL4 (for L5 target), longL3... no wait,
    /// let me redo with clearer names.
    ///
    /// Direct caller of target1 (L5): longL4 at L4
    /// Direct caller of target2 (L2): shortPath at L1
    ///
    /// Both longL4 and shortPath must be in callers[last] (same bottom level).
    #[test]
    fn test_caller_bottom_alignment() {
        // L0: root (0..100)
        // L1: longPath (0..60), shortPath (60..100)
        // L2: longL2 (0..60),   target (60..100)   <- occurrence 1 (occ_level=2)
        // L3: longL3 (0..60)
        // L4: target (0..60)                       <- occurrence 2 (occ_level=4)
        let graph = make_fg(
            100,
            vec!["root", "longPath", "shortPath", "longL2", "target", "longL3"],
            vec![
                vec![0, 100, 0, 0],                    // L0: root
                vec![0, 60, 0, 1, 60, 40, 0, 2],       // L1: longPath, shortPath
                vec![0, 60, 0, 3, 60, 40, 40, 4],      // L2: longL2, target(occ1)
                vec![0, 60, 0, 5],                     // L3: longL3
                vec![0, 60, 60, 4],                    // L4: target(occ2)
            ],
        );

        let sw = build_sandwich_view(&graph, "target", "samples".into()).unwrap();

        // target_samples = 40 (occ1) + 60 (occ2) = 100
        assert_eq!(sw.target_samples, 100);

        // The deepest path (longPath) has 4 levels of callers (root→longPath→longL2→longL3).
        // The shallow path (shortPath) has 2 levels of callers (root→shortPath).
        // With bottom-alignment both paths' direct callers must be at callers[last].
        let last = sw.callers.len() - 1;
        let direct_callers = &sw.callers[last];

        // longL3 is the direct caller of target at L4 (occ2)
        assert!(
            direct_callers.frames.iter().any(|f| f.name == "longL3"),
            "longL3 must be a direct caller at the bottom level"
        );
        // shortPath is the direct caller of target at L2 (occ1)
        assert!(
            direct_callers.frames.iter().any(|f| f.name == "shortPath"),
            "shortPath must be a direct caller at the bottom level, same row as longL3"
        );
    }

    // ── Caller merging ────────────────────────────────────────────────────────

    /// Same-name caller frames from different target occurrences must be merged.
    ///
    /// Layout:
    ///   L0: shared  (0..100)  ← same name appears once at root
    ///   L1: pathA   (0..60),  pathB  (60..100)
    ///   L2: target  (0..60),  target (60..100)
    ///
    /// "shared" is credited with 100 samples (60+40), merged into one caller frame.
    #[test]
    fn test_caller_merge_same_name() {
        let graph = make_fg(
            100,
            vec!["shared", "pathA", "pathB", "target"],
            vec![
                vec![0, 100, 0, 0],                   // L0: shared (root)
                vec![0, 60, 0, 1, 60, 40, 0, 2],      // L1: pathA, pathB
                vec![0, 60, 60, 3, 60, 40, 40, 3],    // L2: target x2
            ],
        );

        let sw = build_sandwich_view(&graph, "target", "samples".into()).unwrap();

        assert_eq!(sw.target_samples, 100);
        assert_eq!(sw.callers.len(), 2);

        // Outermost level: shared should appear once, credited with 100.
        let l0 = &sw.callers[0].frames;
        assert_eq!(l0.len(), 1, "'shared' should appear as a single merged caller");
        assert_eq!(l0[0].name, "shared");
        assert_eq!(l0[0].width, 100);
    }
}
