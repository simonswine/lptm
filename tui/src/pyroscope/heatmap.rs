use log::debug;
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

        debug!(
            "heatmap click: screen=({},{}) slot_idx={} bucket_idx={} n_slots={} n_buckets={}",
            screen_x, screen_y, slot_idx, bucket_idx, slots.len(), hm.n_buckets,
        );
        for (i, s) in slots.iter().enumerate() {
            debug!(
                "  slot[{}] ts={} counts={:?} y_min={:?}",
                i, s.timestamp_ms, s.counts, s.y_min
            );
            for (j, e) in s.exemplars.iter().enumerate() {
                debug!(
                    "    exemplar[{}] ts={} value={} profile_id={} span_id={} labels={:?}",
                    j, e.timestamp_ms, e.value, e.profile_id, e.span_id, e.labels
                );
            }
        }

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
            slot_start_ms: slot.timestamp_ms,
            slot_end_ms: slot.timestamp_ms + hm.step_ms,
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

    /// Map a data-space `(timestamp_ms, value)` exemplar to a terminal cell
    /// `(col, row)` within the graph grid, clamped to valid bounds.
    ///
    /// `col` is relative to `graph_x`, `row` is relative to `graph_y`.
    pub fn exemplar_to_cell(&self, ts_ms: i64, value: f64, hm: &HeatmapView) -> (u16, u16) {
        let time_span = (hm.end_ms - hm.start_ms).max(1) as f64;
        let y_span = (hm.y_max - hm.y_min).max(f64::EPSILON);
        let w = self.graph_w as usize;
        let h = self.graph_h as usize;

        // Column: linear interpolation over [start_ms, end_ms).
        let col = ((ts_ms - hm.start_ms) as f64 / time_span * w as f64) as u16;

        // Row: row 0 = highest value (y_max), last row = lowest value (y_min).
        let row = ((1.0 - (value - hm.y_min) / y_span).clamp(0.0, 1.0) * h as f64) as u16;

        let col = col.min(self.graph_w.saturating_sub(1));
        let row = row.min(self.graph_h.saturating_sub(1));
        (col, row)
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
    let bot = unit.format(hm.y_min);
    let w = top.len().max(bot.len()) as u16;
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

    // ── Y-axis labels ─────────────────────────────────────────────────────────
    frame.render_widget(
        Paragraph::new(format!("{:>width$}", y_top_label, width = y_label_w as usize))
            .style(label_style),
        Rect::new(inner.x, graph_y, y_label_w, 1),
    );
    frame.render_widget(
        Paragraph::new(format!("{:>width$}", y_bot_label, width = y_label_w as usize))
            .style(label_style),
        Rect::new(inner.x, graph_y + graph_h - 1, y_label_w, 1),
    );

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
    if let Some((ts_ms, value)) = highlight {
        let (col, row) = layout.exemplar_to_cell(ts_ms, value, hm);
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
    use super::*;
    use shared::pyroscope::{HeatmapSlot, build_heatmap_view};

    /// Build a minimal HeatmapView from explicit slot data.
    fn make_heatmap(n_slots: usize, n_buckets: usize, step_ms: i64) -> HeatmapView {
        let start_ms = 1_000_000i64;
        let slots: Vec<HeatmapSlot> = (0..n_slots)
            .map(|i| HeatmapSlot {
                timestamp_ms: start_ms + i as i64 * step_ms,
                y_min: (0..n_buckets).map(|b| b as f64 * 10.0).collect(),
                counts: vec![1i32; n_buckets],
                exemplars: vec![],
            })
            .collect();
        build_heatmap_view(&slots).unwrap()
    }

    fn make_layout(graph_w: u16, graph_h: u16) -> (HeatmapLayout, Rect) {
        // Simulate inner area with a 0,0 origin.
        let inner = Rect::new(0, 0, graph_w + 12, graph_h + 2);
        let y_label_w = 10u16; // fixed for test simplicity
        let layout = HeatmapLayout::from_inner(inner, y_label_w).unwrap();
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
            timestamp_ms: 0,
            y_min: vec![0.0, 10.0, 20.0, 30.0, 40.0],
            counts: vec![1; 5],
            exemplars: vec![],
        };
        let hm = build_heatmap_view(&[slot]).unwrap();
        assert_eq!(hm.y_min, 0.0);
        assert_eq!(hm.y_max, 50.0, "y_max must be upper bound of last bucket");
    }

    #[test]
    fn heatmap_view_end_ms_includes_last_slot_duration() {
        // Two slots 5000ms apart: end_ms should be last_ts + step = start + 2*step.
        let step = 5_000i64;
        let slots = vec![
            HeatmapSlot { timestamp_ms: 0, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
            HeatmapSlot { timestamp_ms: step, y_min: vec![0.0, 1.0], counts: vec![1; 2], exemplars: vec![] },
        ];
        let hm = build_heatmap_view(&slots).unwrap();
        assert_eq!(hm.start_ms, 0);
        assert_eq!(hm.end_ms, 2 * step, "end_ms must be last_slot_ts + step_ms");
        assert_eq!(hm.step_ms, step);
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

    #[test]
    fn slot_to_column_alignment() {
        // With n_cols == graph_w each slot should map to exactly its own column.
        let n_slots = 20usize;
        let step_ms = 1000i64;
        let hm = make_heatmap(n_slots, 10, step_ms);
        let graph_w = n_slots as u16;
        let (layout, _) = make_layout(graph_w, 20);

        for (i, slot_col) in (0..n_slots).enumerate() {
            let ts = hm.start_ms + i as i64 * step_ms;
            let val = hm.y_min + (hm.y_max - hm.y_min) / 2.0;
            let (col, _) = layout.exemplar_to_cell(ts, val, &hm);
            assert_eq!(
                col, slot_col as u16,
                "slot {i} at ts {ts} should map to col {slot_col}, got {col}"
            );
        }
    }
}
