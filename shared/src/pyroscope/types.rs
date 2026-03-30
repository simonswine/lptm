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
}

impl Default for FlamegraphNav {
    fn default() -> Self {
        FlamegraphNav { root_level: 0, root_frame: 0, sel_level: 0, sel_frame: 0 }
    }
}

impl FlamegraphNav {
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
        for i in (self.sel_frame + 1)..(lv.values.len() / 4) {
            if frame_visible(&lv.values, i, rx, rw) {
                self.sel_frame = i;
                return;
            }
        }
    }

    pub fn move_left(&mut self, fg: &FlameGraph) {
        let Some(lv) = fg.levels.get(self.sel_level) else { return };
        let (rx, rw) = self.root_bounds(fg);
        for i in (0..self.sel_frame).rev() {
            if frame_visible(&lv.values, i, rx, rw) {
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
        let lv = &fg.levels[new_level].values;
        for i in 0..(lv.len() / 4) {
            let off = lv[i * 4] as u64;
            let w = lv[i * 4 + 1] as u64;
            if w > 0 && off + w > sx && off < se {
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
        let lv = &fg.levels[new_level].values;
        // Prefer the frame that fully contains the selection.
        for i in 0..(lv.len() / 4) {
            let off = lv[i * 4] as u64;
            let w = lv[i * 4 + 1] as u64;
            if frame_visible(lv, i, rx, rw) && off <= sx && off + w >= se {
                self.sel_level = new_level;
                self.sel_frame = i;
                return;
            }
        }
        // Fallback: first visible frame in parent level.
        for i in 0..(lv.len() / 4) {
            if frame_visible(lv, i, rx, rw) {
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

            let name = fg.names.get(name_idx).cloned().unwrap_or_default();
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
                x_start: offset.saturating_sub(root_x),
                width: total,
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
