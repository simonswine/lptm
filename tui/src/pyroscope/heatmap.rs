use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use shared::pyroscope::{HeatmapSlot, HeatmapView, ProfileUnit};

use super::ui::{format_time_label, heatmap_color};

/// Data captured from a single heatmap cell click.
#[derive(Debug, Clone)]
pub struct HeatmapPopup {
    /// Screen position of the click (used to anchor the popup).
    pub screen_x: u16,
    pub screen_y: u16,
    /// Time range of the clicked slot.
    pub slot_start_ms: i64,
    pub slot_end_ms: i64,
    /// Value range of the clicked bucket.
    pub bucket_y_min: f64,
    pub bucket_y_max: f64,
    /// Raw count in this bucket/slot cell.
    pub count: i32,
}

impl HeatmapPopup {
    /// Construct from a click at `(screen_x, screen_y)`, given the layout,
    /// heatmap view, and raw slots.
    pub fn from_click(
        screen_x: u16,
        screen_y: u16,
        layout: &HeatmapLayout,
        hm: &HeatmapView,
        slots: &[HeatmapSlot],
    ) -> Option<Self> {
        let (slot_idx, bucket_idx) = layout.cell_to_slot_bucket(screen_x, screen_y, hm)?;
        let slot = slots.get(slot_idx)?;

        let count = *slot.counts.get(bucket_idx)?;
        let bucket_y_min = slot.y_min.get(bucket_idx).copied()?;
        let bucket_step = if slot.y_min.len() >= 2 {
            slot.y_min[1] - slot.y_min[0]
        } else {
            0.0
        };
        Some(HeatmapPopup {
            screen_x,
            screen_y,
            slot_start_ms: slot.timestamp_ms - hm.step_ms,
            slot_end_ms: slot.timestamp_ms,
            bucket_y_min,
            bucket_y_max: bucket_y_min + bucket_step,
            count,
        })
    }

    pub fn render(&self, frame: &mut Frame, frame_area: Rect, unit: ProfileUnit) {
        let lines = vec![
            Line::from(vec![
                Span::styled("time:  ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!(
                    "{} – {}",
                    format_time_label(self.slot_start_ms),
                    format_time_label(self.slot_end_ms)
                )),
            ]),
            Line::from(vec![
                Span::styled("value: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!(
                    "{} – {}",
                    unit.format(self.bucket_y_min),
                    unit.format(self.bucket_y_max)
                )),
            ]),
            Line::from(vec![
                Span::styled("count: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    self.count.to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        ];

        let popup_w = lines
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(20) as u16
            + 4; // padding + borders
        let popup_h = lines.len() as u16 + 2; // borders

        // Place popup to the right and below the click, flipping if it would
        // overflow the frame.
        let popup_x = if self.screen_x + popup_w + 1 <= frame_area.right() {
            self.screen_x + 1
        } else {
            self.screen_x.saturating_sub(popup_w)
        };
        let popup_y = if self.screen_y + popup_h <= frame_area.bottom() {
            self.screen_y
        } else {
            self.screen_y.saturating_sub(popup_h)
        };
        let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

        frame.render_widget(Clear, popup_area);
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" Cell "))
                .style(Style::default().fg(Color::White)),
            popup_area,
        );
    }
}

/// The pixel-space geometry of the graph area within a heatmap widget.
/// All coordinates are in terminal cells, relative to the frame origin.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatmapLayout {
    /// X coordinate of the left edge of the graph grid.
    pub graph_x: u16,
    /// Y coordinate of the top edge of the graph grid.
    pub graph_y: u16,
    /// Width of the graph grid in cells.
    pub graph_w: u16,
    /// Height of the graph grid in cells.
    pub graph_h: u16,
}

impl HeatmapLayout {
    /// Compute layout from the inner area (after the block border is removed).
    /// Returns `None` if the area is too small to render anything useful.
    pub fn from_inner(inner: Rect, y_label_w: u16) -> Option<Self> {
        // Left margin: y-axis labels + 1 for the │ tick character.
        // Bottom margin: 2 rows for the x-axis tick line + time labels.
        let graph_x = inner.x + y_label_w + 1;
        let graph_y = inner.y;
        let graph_w = inner.right().saturating_sub(graph_x);
        let graph_h = inner.height.saturating_sub(2);
        if graph_w == 0 || graph_h == 0 {
            return None;
        }
        Some(HeatmapLayout { graph_x, graph_y, graph_w, graph_h })
    }

    /// Map a timestamp to a pixel column using the **same** slot→pixel formula
    /// as the renderer (`slot = col * n_cols / w`), so the marker always falls
    /// inside the pixel range the renderer assigned to that slot.
    ///
    /// The renderer maps `col → slot` via `floor(col * n_cols / w)`.
    /// The first pixel column for slot `s` is therefore `ceil(s * w / n_cols)`
    /// (the smallest `c` where `floor(c * n_cols / w) == s`).
    /// We interpolate the sub-slot position within that range.
    pub fn ts_to_col(&self, ts_ms: i64, hm: &HeatmapView) -> u16 {
        let n_cols = hm.columns.len().max(1);
        let w = self.graph_w as usize;
        let step = hm.step_ms.max(1);

        let s = ((ts_ms - hm.start_ms) / step)
            .max(0)
            .min(n_cols as i64 - 1) as usize;

        // Sub-slot fraction: how far within [slot_xmin, slot_xmax) is ts?
        let sub_frac = ((ts_ms - hm.start_ms - s as i64 * step) as f64 / step as f64)
            .clamp(0.0, 1.0);

        // Pixel range [c_first, c_next) assigned to slot s by the renderer.
        // ceil(s * w / n_cols) = (s * w + n_cols - 1) / n_cols
        let c_first = (s * w + n_cols - 1) / n_cols;
        let c_next = ((s + 1) * w + n_cols - 1) / n_cols;
        let slot_px = c_next.saturating_sub(c_first).max(1);

        let col = c_first + (sub_frac * slot_px as f64) as usize;
        col.min(w.saturating_sub(1)) as u16
    }

    /// Map a value to a pixel row using the **same** bucket→pixel formula as the
    /// renderer (`bucket_idx = row * n_buckets / h`, columns stored reversed).
    ///
    /// The renderer assigns bucket `b` (0 = lowest raw bucket) to display rows where
    /// `row * n_buckets / h == n_buckets - 1 - b` (columns are stored with `.rev()`).
    ///
    /// We find the raw bucket via binary search on `hm.bucket_bounds` (handles
    /// non-uniform/exponential spacing), then map to the centre of that bucket's
    /// pixel range — identical to how `ts_to_col` mirrors the slot mapping.
    pub fn value_to_row(&self, value: f64, hm: &HeatmapView) -> u16 {
        let h = self.graph_h as usize;
        let n = hm.n_buckets;

        if n == 0 || h == 0 {
            return 0;
        }

        // Find raw bucket index via binary search on lower bounds.
        let raw_bucket = if hm.bucket_bounds.len() == n {
            let b = hm.bucket_bounds.partition_point(|&lb| lb <= value);
            b.saturating_sub(1).min(n - 1)
        } else {
            // Fallback: linear interpolation (uniform buckets assumed).
            let y_span = (hm.y_max - hm.y_min).max(f64::EPSILON);
            ((value - hm.y_min) / y_span * n as f64) as usize
        }
        .min(n - 1);

        // Columns are stored reversed: col_bucket_idx = n - 1 - raw_bucket
        // (row 0 = col_bucket_idx 0 = raw bucket n-1, i.e. highest value).
        let col_bucket = n - 1 - raw_bucket;

        // First pixel row for col_bucket_idx b: ceil(b * h / n).
        let r_first = (col_bucket * h + n - 1) / n;
        let r_next  = ((col_bucket + 1) * h + n - 1) / n;
        let mid = r_first + r_next.saturating_sub(r_first) / 2;
        mid.min(h.saturating_sub(1)) as u16
    }

    /// Map a data-space `(timestamp_ms, value)` exemplar to a terminal cell
    /// `(col, row)` within the graph grid.
    pub fn exemplar_to_cell(&self, ts_ms: i64, value: f64, hm: &HeatmapView) -> (u16, u16) {
        (self.ts_to_col(ts_ms, hm), self.value_to_row(value, hm))
    }

    /// Map a terminal cell `(col, row)` (relative to `graph_x`/`graph_y`)
    /// back to data-space `(timestamp_ms, value)`. Used in tests to verify
    /// the round-trip.
    pub fn cell_to_data(&self, col: u16, row: u16, hm: &HeatmapView) -> (i64, f64) {
        let time_span = (hm.end_ms - hm.start_ms).max(1) as f64;
        let y_span = (hm.y_max - hm.y_min).max(f64::EPSILON);
        let w = self.graph_w as f64;
        let h = self.graph_h as f64;

        let ts_ms = hm.start_ms + (col as f64 / w * time_span) as i64;
        let value = hm.y_min + (1.0 - row as f64 / h) * y_span;
        (ts_ms, value)
    }

    /// Map a terminal cell to (slot_index, bucket_index) in the HeatmapView.
    /// Returns `None` if the cell is outside the graph area or the indices are
    /// out of range.
    /// Map a screen click to `(slot_index, bucket_index)` where both indices
    /// are into the **raw** `HeatmapSlot` arrays (`counts`, `y_min`), i.e.
    /// index 0 = lowest-value bucket, index n_buckets-1 = highest-value bucket.
    pub fn cell_to_slot_bucket(
        &self,
        screen_x: u16,
        screen_y: u16,
        hm: &HeatmapView,
    ) -> Option<(usize, usize)> {
        // Check that the click is within the graph area.
        if screen_x < self.graph_x
            || screen_x >= self.graph_x + self.graph_w
            || screen_y < self.graph_y
            || screen_y >= self.graph_y + self.graph_h
        {
            return None;
        }
        let col = (screen_x - self.graph_x) as usize;
        let row = (screen_y - self.graph_y) as usize;
        let w = self.graph_w as usize;
        let h = self.graph_h as usize;
        let n_cols = hm.columns.len();

        // Slot: same mapping as the renderer (col * n_cols / w).
        let slot_idx = (col * n_cols / w).min(n_cols.saturating_sub(1));

        // The renderer uses `bucket_idx = row * n_buckets / h` as an index into
        // `hm.columns[slot][bucket_idx]`. The columns were built with `.rev()` in
        // `build_heatmap_view`, so column index 0 = highest-value bucket and
        // column index n_buckets-1 = lowest-value bucket.
        //
        // `slot.counts` and `slot.y_min` are in forward order (index 0 = lowest).
        // We must therefore invert the bucket axis when mapping back to raw data.
        let col_bucket_idx = (row * hm.n_buckets / h).min(hm.n_buckets.saturating_sub(1));
        let raw_bucket_idx = hm.n_buckets.saturating_sub(1) - col_bucket_idx;

        Some((slot_idx, raw_bucket_idx))
    }
}

/// Compute the y-axis label width needed for a heatmap view.
pub fn y_label_width(hm: &HeatmapView, unit: ProfileUnit, max_width: u16) -> u16 {
    let top = unit.format(hm.y_max);
    let mid = unit.format((hm.y_min + hm.y_max) / 2.0);
    let bot = unit.format(hm.y_min);
    let w = top.len().max(mid.len()).max(bot.len()) as u16;
    w.min(max_width / 3)
}

/// Render the heatmap and return the `HeatmapLayout` so the caller can perform
/// hit-testing on subsequent mouse events. Returns `None` if the area is too
/// small to render.
pub fn render_heatmap(
    frame: &mut Frame,
    hm: &HeatmapView,
    block: Block,
    area: Rect,
    unit: ProfileUnit,
    highlight: Option<(i64, f64)>,
    blink_on: bool,
) -> Option<HeatmapLayout> {
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 || inner.width < 4 || hm.columns.is_empty() || hm.n_buckets == 0 {
        return None;
    }

    let y_top_label = unit.format(hm.y_max);
    let y_bot_label = unit.format(hm.y_min);
    let y_label_w = y_label_width(hm, unit, inner.width);

    let Some(layout) = HeatmapLayout::from_inner(inner, y_label_w) else { return None };
    let HeatmapLayout { graph_x, graph_y, graph_w, graph_h } = layout;

    let n_cols = hm.columns.len();
    let label_style = Style::default().fg(Color::DarkGray);

    // ── Y-axis labels (top, middle, bottom) ──────────────────────────────────
    let y_mid_label = unit.format((hm.y_min + hm.y_max) / 2.0);
    let mid_row = graph_h / 2;
    for (label, row) in [
        (y_top_label.as_str(), 0u16),
        (y_mid_label.as_str(), mid_row),
        (y_bot_label.as_str(), graph_h - 1),
    ] {
        // Only render the middle label if it won't overlap top or bottom.
        if row == mid_row && mid_row == 0 || row == mid_row && mid_row == graph_h - 1 {
            continue;
        }
        frame.render_widget(
            Paragraph::new(format!("{:>width$}", label, width = y_label_w as usize))
                .style(label_style),
            Rect::new(inner.x, graph_y + row, y_label_w, 1),
        );
    }

    // ── Y-axis line ───────────────────────────────────────────────────────────
    for r in 0..graph_h {
        frame.render_widget(
            Paragraph::new("│").style(label_style),
            Rect::new(inner.x + y_label_w, graph_y + r, 1, 1),
        );
    }

    // ── X-axis tick + labels ──────────────────────────────────────────────────
    let x_tick_y = graph_y + graph_h;
    let x_label_y = x_tick_y + 1;
    frame.render_widget(
        Paragraph::new("└".to_string() + &"─".repeat(graph_w as usize)).style(label_style),
        Rect::new(inner.x + y_label_w, x_tick_y, graph_w + 1, 1),
    );
    let mid_ms = hm.start_ms + (hm.end_ms - hm.start_ms) / 2;
    let time_labels: [(u16, String); 3] = [
        (0, format_time_label(hm.start_ms)),
        (graph_w / 2, format_time_label(mid_ms)),
        (graph_w.saturating_sub(8), format_time_label(hm.end_ms)),
    ];
    for (x_off, label) in &time_labels {
        let x = (graph_x + x_off).min(inner.right().saturating_sub(label.len() as u16));
        frame.render_widget(
            Paragraph::new(label.clone()).style(label_style),
            Rect::new(x, x_label_y, label.len() as u16, 1),
        );
    }

    // ── Heatmap grid ──────────────────────────────────────────────────────────
    // col → slot: slot_start = col * n_cols / w  (shared with ts_to_col inverse)
    // row → bucket_idx: bucket_idx = row * n_buckets / h  (shared with value_to_row inverse)
    let w = graph_w as usize;
    let usable_h = graph_h as usize;
    for row in 0..usable_h {
        let bucket_idx = row * hm.n_buckets / usable_h;
        let row_area = Rect::new(graph_x, graph_y + row as u16, graph_w, 1);
        let spans: Vec<Span> = (0..w)
            .map(|col| {
                let slot_start = col * n_cols / w;
                let slot_end = ((col + 1) * n_cols / w).max(slot_start + 1).min(n_cols);
                let intensity = hm.columns[slot_start..slot_end]
                    .iter()
                    .filter_map(|c| c.get(bucket_idx).copied())
                    .fold(0.0_f64, f64::max);
                Span::styled(" ", Style::default().bg(heatmap_color(intensity)))
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }

    // ── Blinking exemplar marker ──────────────────────────────────────────────
    // Uses ts_to_col / value_to_row which are the exact inverse of the grid's
    // col→slot and row→bucket_idx formulas, guaranteeing the marker lands in
    // the correct cell.
    if let Some((ts_ms, value)) = highlight {
        let col = layout.ts_to_col(ts_ms, hm);
        let row = layout.value_to_row(value, hm);
        if blink_on {
            frame.render_widget(
                Paragraph::new(" ").style(Style::default().bg(Color::Magenta)),
                Rect::new(graph_x + col, graph_y + row, 1, 1),
            );
        }
    }

    Some(layout)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use shared::pyroscope::{TimelineExemplar, build_heatmap_view};
    use super::*;
    use shared::pyroscope::{HeatmapSlot};

    /// Build a minimal HeatmapView from explicit slot data.
    fn make_heatmap(n_slots: usize, n_buckets: usize, step_ms: i64) -> HeatmapView {
        // slot.timestamp_ms is x_max; first slot x_max = start_ms + step_ms.
        let start_ms = 1_000_000i64;
        let slots: Vec<HeatmapSlot> = (0..n_slots)
            .map(|i| HeatmapSlot {
                timestamp_ms: start_ms + (i as i64 + 1) * step_ms,
                step_ms,
                y_min: (0..n_buckets).map(|b| b as f64 * 10.0).collect(),
                counts: vec![1i32; n_buckets],
                exemplars: vec![],
            })
            .collect();
        build_heatmap_view(&slots).unwrap()
    }

    fn make_layout(graph_w: u16, graph_h: u16) -> (HeatmapLayout, Rect) {
        // inner.width  = graph_w + y_label_w(10) + 1 (y-axis tick char)
        // inner.height = graph_h + 2 (x-axis tick + label rows)
        // This ensures from_inner() produces exactly the requested graph dimensions.
        const Y_LABEL_W: u16 = 10;
        let inner = Rect::new(0, 0, graph_w + Y_LABEL_W + 1, graph_h + 2);
        let layout = HeatmapLayout::from_inner(inner, Y_LABEL_W).unwrap();
        assert_eq!(layout.graph_w, graph_w, "make_layout: graph_w mismatch");
        assert_eq!(layout.graph_h, graph_h, "make_layout: graph_h mismatch");
        (layout, inner)
    }

    #[test]
    fn layout_graph_dimensions() {
        let inner = Rect::new(0, 0, 100, 30);
        let y_label_w = 8u16;
        let layout = HeatmapLayout::from_inner(inner, y_label_w).unwrap();
        // graph_x = 0 + 8 + 1 = 9
        assert_eq!(layout.graph_x, 9);
        assert_eq!(layout.graph_y, 0);
        // graph_w = 100 - 9 = 91
        assert_eq!(layout.graph_w, 91);
        // graph_h = 30 - 2 = 28
        assert_eq!(layout.graph_h, 28);
    }

    #[test]
    fn exemplar_top_left_maps_to_cell_zero() {
        let hm = make_heatmap(10, 20, 1000);
        let (layout, _) = make_layout(80, 20);
        // Exemplar at the very start of time and maximum value → top-left cell.
        let (col, row) = layout.exemplar_to_cell(hm.start_ms, hm.y_max - f64::EPSILON, &hm);
        assert_eq!(col, 0, "leftmost time should give col 0");
        assert_eq!(row, 0, "maximum value should give row 0");
    }

    #[test]
    fn exemplar_bottom_right_maps_to_last_cell() {
        let hm = make_heatmap(10, 20, 1000);
        let (layout, _) = make_layout(80, 20);
        // Exemplar at end of time and minimum value → bottom-right cell.
        let (col, row) = layout.exemplar_to_cell(hm.end_ms - 1, hm.y_min, &hm);
        assert_eq!(col, layout.graph_w - 1, "rightmost time should give last col");
        assert_eq!(row, layout.graph_h - 1, "minimum value should give last row");
    }

    #[test]
    fn exemplar_midpoint_maps_to_centre() {
        let hm = make_heatmap(100, 100, 1000);
        let (layout, _) = make_layout(100, 100);
        let mid_ms = hm.start_ms + (hm.end_ms - hm.start_ms) / 2;
        let mid_val = hm.y_min + (hm.y_max - hm.y_min) / 2.0;
        let (col, row) = layout.exemplar_to_cell(mid_ms, mid_val, &hm);
        // Allow ±1 cell for integer rounding.
        let expected_col = layout.graph_w / 2;
        let expected_row = layout.graph_h / 2;
        assert!(
            col.abs_diff(expected_col) <= 1,
            "col {col} should be near centre {expected_col}"
        );
        assert!(
            row.abs_diff(expected_row) <= 1,
            "row {row} should be near centre {expected_row}"
        );
    }

    #[test]
    fn exemplar_clamped_out_of_range() {
        let hm = make_heatmap(10, 20, 1000);
        let (layout, _) = make_layout(80, 20);
        // Way out of bounds should clamp, not panic.
        let (col, row) = layout.exemplar_to_cell(hm.start_ms - 999_999, -1e9, &hm);
        assert_eq!(col, 0);
        assert_eq!(row, layout.graph_h - 1); // below y_min → last row

        let (col2, row2) = layout.exemplar_to_cell(hm.end_ms + 999_999, 1e9, &hm);
        assert_eq!(col2, layout.graph_w - 1);
        assert_eq!(row2, 0); // above y_max → first row
    }

    #[test]
    fn heatmap_view_y_max_is_upper_bound_of_last_bucket() {
        // y_min stores lower bounds. With step 10.0 per bucket and 5 buckets:
        // lower bounds: 0, 10, 20, 30, 40  → last lower = 40, step = 10 → y_max = 50
        let slot = HeatmapSlot {
            timestamp_ms: 10_000,
            step_ms: 10_000,
            y_min: vec![0.0, 10.0, 20.0, 30.0, 40.0],
            counts: vec![1; 5],
            exemplars: vec![],
        };
        let hm = build_heatmap_view(&[slot]).unwrap();
        assert_eq!(hm.y_min, 0.0);
        assert_eq!(hm.y_max, 50.0, "y_max must be upper bound of last bucket");
    }

    #[test]
    fn heatmap_view_start_and_end_ms() {
        // slot.timestamp_ms is x_max. Two slots:
        //   slot 0: timestamp=5000 → x_min=0,    x_max=5000
        //   slot 1: timestamp=10000 → x_min=5000, x_max=10000
        // So hm.start_ms = 0, hm.end_ms = 10000.
        // step_ms comes from the slot field, not derived from timestamps.
        let step = 5_000i64;
        let slots = vec![
            HeatmapSlot { timestamp_ms: step,     step_ms: step, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
            HeatmapSlot { timestamp_ms: 2 * step, step_ms: step, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
        ];
        let hm = build_heatmap_view(&slots).unwrap();
        assert_eq!(hm.start_ms, 0,         "start_ms = first_slot.timestamp_ms - step_ms");
        assert_eq!(hm.end_ms,   2 * step,  "end_ms = last_slot.timestamp_ms (its x_max)");
        assert_eq!(hm.step_ms,  step);
    }

    #[test]
    fn heatmap_view_missing_slots_use_query_step() {
        // If slots are sparse (empty slots omitted), step_ms from the query
        // must be used — deriving from consecutive timestamps would give wrong results.
        let step = 5_000i64;
        let slots = vec![
            // slot at t=5000 (x_min=0)
            HeatmapSlot { timestamp_ms: step,      step_ms: step, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
            // slot at t=20000 (x_min=15000) — gap of 3 steps, slots 2 and 3 are missing
            HeatmapSlot { timestamp_ms: 4 * step,  step_ms: step, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
        ];
        let hm = build_heatmap_view(&slots).unwrap();
        assert_eq!(hm.step_ms, step, "step_ms must come from query, not consecutive timestamp diff");
        assert_eq!(hm.start_ms, 0,        "start_ms = first_slot.x_min");
        assert_eq!(hm.end_ms,   4 * step, "end_ms = last_slot.x_max");
    }

    #[test]
    fn bucket_reversal_top_row_is_highest_bucket() {
        // The renderer builds columns with .rev() so column[0] = highest bucket.
        // A click on row 0 (top) must map back to the highest raw bucket index.
        let n_buckets = 10usize;
        let hm = make_heatmap(5, n_buckets, 1000);
        let (layout, _) = make_layout(50, 20);

        // Click at the very top of the graph area.
        let screen_x = layout.graph_x;
        let screen_y = layout.graph_y;
        let (_slot, bucket) = layout.cell_to_slot_bucket(screen_x, screen_y, &hm).unwrap();
        assert_eq!(
            bucket,
            n_buckets - 1,
            "top row should map to highest raw bucket (index {}), got {}",
            n_buckets - 1,
            bucket
        );
    }

    #[test]
    fn bucket_reversal_bottom_row_is_lowest_bucket() {
        let n_buckets = 10usize;
        let hm = make_heatmap(5, n_buckets, 1000);
        let (layout, _) = make_layout(50, 20);

        // Click at the very bottom row of the graph area.
        let screen_x = layout.graph_x;
        let screen_y = layout.graph_y + layout.graph_h - 1;
        let (_slot, bucket) = layout.cell_to_slot_bucket(screen_x, screen_y, &hm).unwrap();
        assert_eq!(
            bucket,
            0,
            "bottom row should map to lowest raw bucket (index 0), got {}",
            bucket
        );
    }

    /// Build a HeatmapView with explicit non-uniform bucket bounds (like
    /// exponential latency buckets). Useful for value_to_row tests.
    fn make_heatmap_nonuniform_buckets() -> (HeatmapView, Vec<HeatmapSlot>) {
        // Exponential-ish latency buckets (nanoseconds): 1ms, 2ms, 5ms, 10ms, 20ms, 50ms, 100ms
        let bounds: Vec<f64> = vec![
            1_000_000.0,
            2_000_000.0,
            5_000_000.0,
            10_000_000.0,
            20_000_000.0,
            50_000_000.0,
            100_000_000.0,
        ];
        let n = bounds.len();
        let step_ms = 10_000i64;
        let exemplar = shared::pyroscope::TimelineExemplar {
            labels: vec![],
            profile_id: "p1".into(),
            span_id: String::new(),
            value: 8_000_000, // 8ms → falls in bucket 2 (bounds[2]=5ms..bounds[3]=10ms)
            timestamp_ms: step_ms / 2,
        };
        let slots = vec![HeatmapSlot {
            timestamp_ms: step_ms,
            step_ms,
            y_min: bounds.clone(),
            counts: vec![1; n],
            exemplars: vec![exemplar],
        }];
        let hm = build_heatmap_view(&slots).unwrap();
        (hm, slots)
    }

    #[test]
    fn value_to_row_uniform_buckets_matches_renderer() {
        // With uniform buckets, value_to_row must map to the same row that the
        // renderer assigns to that bucket.
        let hm = make_heatmap(1, 8, 10_000);
        let (layout, _) = make_layout(20, 16);
        let n = hm.n_buckets;
        let h = layout.graph_h as usize;

        for raw_b in 0..n {
            // Pick a value in the middle of this bucket's range.
            let bw = (hm.y_max - hm.y_min) / n as f64;
            let value = hm.y_min + (raw_b as f64 + 0.5) * bw;
            let row = layout.value_to_row(value, &hm) as usize;

            // What bucket does the renderer assign to this row?
            let col_bucket = row * n / h;
            let renderer_raw_bucket = n - 1 - col_bucket;

            assert_eq!(
                renderer_raw_bucket, raw_b,
                "value {value} in raw bucket {raw_b}: row={row} -> renderer bucket {renderer_raw_bucket}"
            );
        }
    }

    #[test]
    fn value_to_row_nonuniform_buckets_matches_renderer() {
        // With exponential buckets, linear interpolation would give wrong results.
        // value_to_row must use bucket_bounds lookup.
        let (hm, _) = make_heatmap_nonuniform_buckets();
        assert!(!hm.bucket_bounds.is_empty(), "bucket_bounds must be populated");
        let (layout, _) = make_layout(20, 14);
        let n = hm.n_buckets;
        let h = layout.graph_h as usize;

        // Verify every bucket: value at lower bound of bucket b maps to a row
        // the renderer assigns to bucket b.
        for raw_b in 0..n {
            let value = hm.bucket_bounds[raw_b] + 1.0; // just above lower bound
            let row = layout.value_to_row(value, &hm) as usize;
            let col_bucket = row * n / h;
            let renderer_raw_bucket = n - 1 - col_bucket;
            assert_eq!(
                renderer_raw_bucket, raw_b,
                "bucket {raw_b} (value {value}): row={row} -> renderer bucket {renderer_raw_bucket}"
            );
        }
    }

    #[test]
    fn value_to_row_exemplar_8ms_in_correct_bucket() {
        // The exemplar at 8ms must land in bucket 2 (5ms..10ms), not bucket 3.
        // Linear interpolation over [1ms, 100ms+] would place 8ms at ~7% of the
        // way up, which with 7 buckets gives bucket 0 or 1 — wrong.
        let (hm, _) = make_heatmap_nonuniform_buckets();
        let (layout, _) = make_layout(20, 14);
        let n = hm.n_buckets;
        let h = layout.graph_h as usize;

        let exemplar_value = 8_000_000.0_f64; // 8ms
        let row = layout.value_to_row(exemplar_value, &hm) as usize;
        let col_bucket = row * n / h;
        let renderer_raw_bucket = n - 1 - col_bucket;

        // 8ms is between bucket_bounds[2]=5ms and bucket_bounds[3]=10ms → raw bucket 2
        assert_eq!(renderer_raw_bucket, 2,
            "8ms exemplar must land in raw bucket 2 (5ms–10ms), got bucket {renderer_raw_bucket} at row {row}");
    }

    #[test]
    fn ts_to_col_matches_renderer_slot_mapping() {
        // For every slot s, any timestamp within [x_min, x_max) must map to a
        // pixel column that the renderer assigns to slot s via col*n_cols/w.
        let n_slots = 20usize;
        let step_ms = 1000i64;
        let hm = make_heatmap(n_slots, 4, step_ms);
        // Use a width that is NOT a multiple of n_slots to exercise the non-trivial case.
        let (layout, _) = make_layout(47, 20); // 47 is prime, ensures fractional slots
        let n_cols = hm.columns.len();
        let w = layout.graph_w as usize;

        for s in 0..n_slots {
            let slot_xmin = hm.start_ms + s as i64 * step_ms;
            // Check at x_min, quarter, midpoint, three-quarter, and just before x_max.
            for offset_num in [0i64, 1, 2, 3, step_ms - 1] {
                let ts = slot_xmin + offset_num * step_ms / (step_ms - 1).max(1);
                let ts = ts.min(slot_xmin + step_ms - 1);
                let col = layout.ts_to_col(ts, &hm) as usize;
                let slot_back = col * n_cols / w;
                assert_eq!(
                    slot_back, s,
                    "slot {s}: ts={ts} (offset {}ms) -> col={col} -> slot_back={slot_back} (expected {s})",
                    ts - slot_xmin,
                );
            }
        }
    }

    #[test]
    fn slot_to_column_alignment() {
        // With n_cols == graph_w each slot should map to exactly its own column.
        // slot.timestamp_ms is the x_min (left edge) of the slot; x_max = x_min + step_ms.
        let n_slots = 20usize;
        let step_ms = 1000i64;
        let hm = make_heatmap(n_slots, 10, step_ms);
        let graph_w = n_slots as u16;
        let (layout, _) = make_layout(graph_w, 20);
        let val = hm.y_min + (hm.y_max - hm.y_min) / 2.0;

        for i in 0..n_slots {
            let slot_x_min = hm.start_ms + i as i64 * step_ms;
            let slot_x_max = slot_x_min + step_ms;
            let expected_col = i as u16;

            // x_min (left edge) maps to this column.
            let (col, _) = layout.exemplar_to_cell(slot_x_min, val, &hm);
            assert_eq!(col, expected_col,
                "slot {i}: x_min={slot_x_min} should map to col {expected_col}, got {col}");

            // Midpoint also maps to this column.
            let mid_ts = slot_x_min + step_ms / 2;
            let (col, _) = layout.exemplar_to_cell(mid_ts, val, &hm);
            assert_eq!(col, expected_col,
                "slot {i}: mid_ts={mid_ts} should map to col {expected_col}, got {col}");

            // x_max - 1ms (just before right edge) still maps to this column, not the next.
            let just_before_x_max = slot_x_max - 1;
            let (col, _) = layout.exemplar_to_cell(just_before_x_max, val, &hm);
            assert_eq!(col, expected_col,
                "slot {i}: just_before_x_max={just_before_x_max} should still map to col {expected_col}, got {col}");

            // x_max itself (left edge of the next slot) should map to col+1
            // (or be clamped to last col for the final slot).
            if i + 1 < n_slots {
                let (col, _) = layout.exemplar_to_cell(slot_x_max, val, &hm);
                assert_eq!(col, expected_col + 1,
                    "slot {i}: x_max={slot_x_max} (= x_min of next slot) should map to col {}, got {col}", expected_col + 1);
            }
        }
    }

    // ── Integration tests: render into a TestBackend and inspect the buffer ───

    /// Build a small heatmap: 4 time slots, 3 value buckets, with one exemplar
    /// in the middle slot/bucket. Returns both the HeatmapView and the raw slots.
    fn make_heatmap_with_exemplar() -> (HeatmapView, Vec<HeatmapSlot>) {
        //  Buckets (y_min lower bounds): 0, 100, 200  →  y_max = 300
        //  step_ms = 1000; slot.timestamp_ms = x_max of that slot.
        //  Slot 1 (x_min=1000, x_max=2000) has hot bucket 1 (count=10)
        //  and an exemplar at ts=1500 (midpoint of that slot).
        let exemplar = TimelineExemplar {
            labels: vec![("pod".into(), "pod-a".into())],
            profile_id: "prof-001".into(),
            span_id: String::new(),
            value: 150,
            timestamp_ms: 1500, // within [1000, 2000)
        };
        let slots = vec![
            HeatmapSlot {
                timestamp_ms: 1000, step_ms: 1000, // x_min=0, x_max=1000
                y_min: vec![0.0, 100.0, 200.0],
                counts: vec![1, 1, 1],
                exemplars: vec![],
            },
            HeatmapSlot {
                timestamp_ms: 2000, step_ms: 1000, // x_min=1000, x_max=2000
                y_min: vec![0.0, 100.0, 200.0],
                counts: vec![1, 10, 1],  // bucket 1 is hot
                exemplars: vec![exemplar],
            },
            HeatmapSlot {
                timestamp_ms: 3000, step_ms: 1000, // x_min=2000, x_max=3000
                y_min: vec![0.0, 100.0, 200.0],
                counts: vec![1, 1, 1],
                exemplars: vec![],
            },
            HeatmapSlot {
                timestamp_ms: 4000, step_ms: 1000, // x_min=3000, x_max=4000
                y_min: vec![0.0, 100.0, 200.0],
                counts: vec![1, 1, 1],
                exemplars: vec![],
            },
        ];
        let hm = build_heatmap_view(&slots).unwrap();
        (hm, slots)
    }

    /// Render the heatmap into a TestBackend and return (terminal, layout).
    /// Terminal is 60 wide × 20 tall; the heatmap block occupies all of it.
    fn render_heatmap_to_terminal(
        hm: &HeatmapView,
        highlight: Option<(i64, f64)>,
        blink_on: bool,
    ) -> (Terminal<TestBackend>, HeatmapLayout) {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut layout_out = None;
        terminal.draw(|frame| {
            let area = frame.area();
            let block = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .title(" Heatmap ");
            let layout = render_heatmap(frame, hm, block, area, ProfileUnit::Nanoseconds, highlight, blink_on);
            layout_out = layout;
        }).unwrap();
        (terminal, layout_out.expect("heatmap layout must be returned"))
    }

    #[test]
    fn integration_hot_slot_has_brighter_background() {
        let (hm, _) = make_heatmap_with_exemplar();
        let (terminal, layout) = render_heatmap_to_terminal(&hm, None, false);
        let buf = terminal.backend().buffer().clone();

        // Slot 1 bucket 1 (count=10) is the global max → intensity = 1.0 → bright red.
        // All other cells have count=1 → intensity = 0.1 → near-black.
        let n_cols = hm.columns.len();
        let graph_w = layout.graph_w as usize;
        let graph_h = layout.graph_h as usize;

        // First terminal col that maps to slot 1 in the renderer.
        let hot_col = (1 * graph_w + n_cols - 1) / n_cols;

        // The renderer maps display row → column bucket index via:
        //   bucket_idx = row * n_buckets / graph_h
        // and the columns were built with .rev() so:
        //   column[bucket_idx] corresponds to raw bucket (n_buckets - 1 - bucket_idx)
        //
        // We want raw bucket 1 → col bucket_idx = n_buckets - 1 - 1 = 1.
        // First row r where (r * n_buckets / graph_h) == 1:
        //   r = ceil(1 * graph_h / n_buckets) = (graph_h + n_buckets - 1) / n_buckets
        let hot_row = (1 * graph_h + hm.n_buckets - 1) / hm.n_buckets;

        // Cold: slot 2, raw bucket 2 (highest) → col bucket_idx=0 → display row=0.
        let cold_col = (2 * graph_w + n_cols - 1) / n_cols;
        let cold_row = 0usize;

        let hot_cell = buf.get(layout.graph_x + hot_col as u16, layout.graph_y + hot_row as u16);
        let cold_cell = buf.get(layout.graph_x + cold_col as u16, layout.graph_y + cold_row as u16);

        assert_ne!(
            hot_cell.style().bg,
            cold_cell.style().bg,
            "hot cell bg={:?} should differ from cold cell bg={:?}",
            hot_cell.style().bg, cold_cell.style().bg,
        );
    }

    #[test]
    fn integration_axes_labels_present() {
        let (hm, _) = make_heatmap_with_exemplar();
        let (terminal, _layout) = render_heatmap_to_terminal(&hm, None, false);

        // Collect all rendered text from the buffer.
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        // Y-axis labels: y_min=0 and y_max=300 formatted as nanoseconds.
        assert!(text.contains("0ns") || text.contains("0 ns"), "y_min label missing: {text:?}");

        // X-axis should show time labels (hh:mm:ss format).
        assert!(text.contains("00:00:00"), "x-axis start time label missing");
    }

    #[test]
    fn integration_exemplar_table_shows_time_and_value() {
        let (hm, _) = make_heatmap_with_exemplar();

        // Render heatmap in upper 60% and exemplar table in lower 40%.
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| {
            let area = frame.area();
            let [vis_area, table_area] =
                ratatui::layout::Layout::vertical([
                    ratatui::layout::Constraint::Percentage(60),
                    ratatui::layout::Constraint::Percentage(40),
                ])
                .areas(area);

            let block = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .title(" Heatmap ");
            render_heatmap(frame, &hm, block, vis_area, ProfileUnit::Nanoseconds, None, false);

            let exemplars: Vec<_> = hm.exemplars.iter().collect();
            crate::pyroscope::ui::render_exemplars(
                frame,
                &exemplars,
                &hm.varying_label_keys,
                table_area,
                ProfileUnit::Nanoseconds,
                0, // first exemplar selected
                None,
                false,
            );
        }).unwrap();

        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();

        // The exemplar at ts=1000ms (00:00:01) with value=150ns should appear in the table.
        assert!(text.contains("00:00:01"), "exemplar time missing from table: {text:?}");
        assert!(text.contains("150"), "exemplar value missing from table: {text:?}");
        // The profile_id should appear.
        assert!(text.contains("prof-001"), "exemplar profile_id missing from table: {text:?}");
        // Column headers.
        assert!(text.contains("Time"), "Time header missing");
        assert!(text.contains("Value"), "Value header missing");
    }

    #[test]
    fn integration_selected_exemplar_row_has_highlight_style() {
        let (hm, _) = make_heatmap_with_exemplar();

        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| {
            let table_area = frame.area();
            let exemplars: Vec<_> = hm.exemplars.iter().collect();
            crate::pyroscope::ui::render_exemplars(
                frame,
                &exemplars,
                &hm.varying_label_keys,
                table_area,
                ProfileUnit::Nanoseconds,
                0, // row 0 selected
                None,
                false,
            );
        }).unwrap();

        let buf = terminal.backend().buffer().clone();
        // Find the ">>" highlight symbol — ratatui's TableState highlight_symbol.
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains(">>"), "selected row highlight symbol '>>' missing");

        // The selected row cells should have the magenta foreground style.
        let magenta_cell = buf.content().iter().find(|c| {
            c.style().fg == Some(ratatui::style::Color::Magenta)
        });
        assert!(magenta_cell.is_some(), "no cell with magenta fg found for selected row");
    }

    #[test]
    fn integration_popup_shows_correct_bucket_for_click() {
        let (hm, slots) = make_heatmap_with_exemplar();
        let (terminal, layout) = render_heatmap_to_terminal(&hm, None, false);
        let _ = terminal;

        // Click on the hot cell: slot 1, bucket 1.
        let n_cols = hm.columns.len();
        let graph_w = layout.graph_w as usize;
        let graph_h = layout.graph_h as usize;

        // First terminal col that maps to slot 1 in the renderer.
        let click_col = layout.graph_x + ((1 * graph_w + n_cols - 1) / n_cols) as u16;
        // Terminal row corresponding to raw bucket 1 in display coords.
        // First display row r where (r * n_buckets / graph_h) == col_bucket_idx=1:
        //   r = ceil(1 * graph_h / n_buckets)
        let display_row = (1 * graph_h + hm.n_buckets - 1) / hm.n_buckets;
        let click_row = layout.graph_y + display_row as u16;

        let popup = HeatmapPopup::from_click(click_col, click_row, &layout, &hm, &slots)
            .expect("click inside graph should produce a popup");

        // slot 1: timestamp_ms=2000 (x_max), so x_min = 2000 - step_ms(1000) = 1000.
        assert_eq!(popup.slot_start_ms, 1000, "popup slot_start_ms wrong");
        // bucket 1: y_min=100.0, y_max=200.0 (bucket_step=100).
        assert_eq!(popup.bucket_y_min, 100.0, "popup bucket_y_min wrong");
        assert_eq!(popup.bucket_y_max, 200.0, "popup bucket_y_max wrong");
        // count for slot 1 bucket 1 = 10.
        assert_eq!(popup.count, 10, "popup count wrong");
    }

    #[test]
    fn integration_count_levels_visually_distinct() {
        // Three slots: count=0, count=1, count=1_000_000.
        // global_max = 1_000_000, so intensities are 0.0, 0.000001, and 1.0.
        // All three must render as visibly different background colors:
        //   count=0        → intensity 0.0       → Color::Black
        //   count=1        → intensity ~0.000001 → rounds to Color::Black (idx 0)
        //   count=1_000_000 → intensity 1.0      → Color::Rgb(255,0,0)
        //
        // This means count=1 and count=0 will look the same when count=1_000_000
        // exists — that's the current behavior. The important guarantees are:
        //   1. count=0 is Black.
        //   2. count=1_000_000 (the global max) is the brightest color.
        //   3. count=1 vs count=1_000_000 are visually distinct.
        let slots = vec![
            HeatmapSlot {
                timestamp_ms: 1000, step_ms: 1000,
                y_min: vec![0.0, 100.0],
                counts: vec![0, 0],
                exemplars: vec![],
            },
            HeatmapSlot {
                timestamp_ms: 2000, step_ms: 1000,
                y_min: vec![0.0, 100.0],
                counts: vec![1, 1],
                exemplars: vec![],
            },
            HeatmapSlot {
                timestamp_ms: 3000, step_ms: 1000,
                y_min: vec![0.0, 100.0],
                counts: vec![1_000_000, 1_000_000],
                exemplars: vec![],
            },
        ];
        let hm = build_heatmap_view(&slots).unwrap();
        let (terminal, layout) = render_heatmap_to_terminal(&hm, None, false);
        let buf = terminal.backend().buffer().clone();

        let graph_w = layout.graph_w as usize;
        let n_cols = hm.columns.len();
        // The renderer maps terminal col → slot via: slot = col * n_cols / graph_w.
        // To find the first terminal col that maps to slot k, we need the smallest
        // col where col * n_cols / graph_w == k, i.e. col = ceil(k * graph_w / n_cols).
        let col_for_slot = |k: usize| (k * graph_w + n_cols - 1) / n_cols;
        let zero_col    = col_for_slot(0);
        let one_col     = col_for_slot(1);
        let million_col = col_for_slot(2);

        let zero_cell    = buf.get(layout.graph_x + zero_col as u16,    layout.graph_y);
        let one_cell     = buf.get(layout.graph_x + one_col as u16,     layout.graph_y);
        let million_cell = buf.get(layout.graph_x + million_col as u16, layout.graph_y);

        assert_eq!(
            zero_cell.style().bg,
            Some(ratatui::style::Color::Black),
            "count=0 must render as Black"
        );
        assert_eq!(
            million_cell.style().bg,
            Some(ratatui::style::Color::Rgb(255, 0, 0)),
            "count=1_000_000 (global max) must render as the brightest color Rgb(255,0,0)"
        );
        assert_ne!(
            one_cell.style().bg,
            Some(ratatui::style::Color::Black),
            "count=1 must not render as Black even when global_max is 1_000_000, got {:?}",
            one_cell.style().bg,
        );
        assert_ne!(
            one_cell.style().bg,
            million_cell.style().bg,
            "count=1 and count=1_000_000 must not render the same color"
        );
    }

    #[test]
    fn integration_click_outside_graph_returns_none() {
        let (hm, slots) = make_heatmap_with_exemplar();
        let (terminal, layout) = render_heatmap_to_terminal(&hm, None, false);
        let _ = terminal;

        // Click on the y-axis label area (left of graph_x).
        let popup = HeatmapPopup::from_click(
            layout.graph_x.saturating_sub(1), layout.graph_y, &layout, &hm, &slots,
        );
        assert!(popup.is_none(), "click on y-axis label should return None");

        // Click on x-axis label area (below graph).
        let popup = HeatmapPopup::from_click(
            layout.graph_x, layout.graph_y + layout.graph_h, &layout, &hm, &slots,
        );
        assert!(popup.is_none(), "click on x-axis row should return None");
    }
}
