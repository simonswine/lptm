use crux_core::{render::render, Command};

use crate::app::{Effect, Event, Model, Screen};
use crate::time_range::{
    encode_absolute, format_unix_s_utc, parse_datetime_utc_ns,
    preset_index, resolve_range_s, PRESETS,
};

pub const FOCUS_PRESETS: u8 = 0;
pub const FOCUS_ABS_FROM: u8 = 1;
pub const FOCUS_ABS_TO: u8 = 2;

/// Template used to initialise a blank datetime field.
/// Positions of separator characters (not overwritable with digits).
pub const DATETIME_TEMPLATE: &str = "YYYY-MM-DD HH:MM:SS";
const DATETIME_LEN: usize = 19;

/// Characters in `DATETIME_TEMPLATE` that are separators — skipped on cursor movement
/// and never overwritten.
fn is_separator(pos: usize) -> bool {
    matches!(pos, 4 | 7 | 10 | 13 | 16)
}

/// Advance `pos` forward, skipping separator positions, clamped to `DATETIME_LEN - 1`.
fn next_digit_pos(pos: usize) -> usize {
    let mut p = pos + 1;
    while p < DATETIME_LEN && is_separator(p) {
        p += 1;
    }
    p.min(DATETIME_LEN - 1)
}

/// Move `pos` backward, skipping separator positions, clamped to 0.
fn prev_digit_pos(pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = pos - 1;
    while p > 0 && is_separator(p) {
        p -= 1;
    }
    p
}

/// Return the first non-separator position after `pos` (for post-overwrite advance).
fn advance_after_overwrite(pos: usize) -> usize {
    let next = pos + 1;
    if next >= DATETIME_LEN {
        return DATETIME_LEN - 1;
    }
    if is_separator(next) {
        next + 1
    } else {
        next
    }
    .min(DATETIME_LEN - 1)
}

/// Active cursor for the currently-focused field.
fn active_cursor(model: &Model) -> usize {
    match model.time_range_picker_focus {
        FOCUS_ABS_FROM => model.time_range_picker_from_cursor,
        FOCUS_ABS_TO => model.time_range_picker_to_cursor,
        _ => 0,
    }
}

/// Set cursor for the currently-focused field.
fn set_cursor(model: &mut Model, pos: usize) {
    match model.time_range_picker_focus {
        FOCUS_ABS_FROM => model.time_range_picker_from_cursor = pos,
        FOCUS_ABS_TO => model.time_range_picker_to_cursor = pos,
        _ => {}
    }
}

/// Active field string (mutable).
fn active_field_mut(model: &mut Model) -> Option<&mut String> {
    match model.time_range_picker_focus {
        FOCUS_ABS_FROM => Some(&mut model.time_range_picker_abs_from),
        FOCUS_ABS_TO => Some(&mut model.time_range_picker_abs_to),
        _ => None,
    }
}

// ── Public handlers ───────────────────────────────────────────────────────────

pub fn handle_open(model: &mut Model, now_unix_ms: i64) -> Command<Effect, Event> {
    model.time_range_picker_open = true;
    model.time_range_picker_focus = FOCUS_PRESETS;
    model.time_range_picker_index = preset_index(&model.time_range).unwrap_or(3);

    let now_s = (now_unix_ms / 1000).max(0) as u64;
    let (start_s, end_s) = resolve_range_s(&model.time_range, now_s);
    model.time_range_picker_abs_from = format_unix_s_utc(start_s);
    model.time_range_picker_abs_to = format_unix_s_utc(end_s);
    // Place cursor at position 0 for both fields.
    model.time_range_picker_from_cursor = 0;
    model.time_range_picker_to_cursor = 0;

    render()
}

pub fn handle_close(model: &mut Model) -> Command<Effect, Event> {
    model.time_range_picker_open = false;
    render()
}

pub fn handle_next(model: &mut Model) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        let len = PRESETS.len();
        model.time_range_picker_index = (model.time_range_picker_index + 1) % len;
    }
    render()
}

pub fn handle_prev(model: &mut Model) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        let len = PRESETS.len();
        model.time_range_picker_index = (model.time_range_picker_index + len - 1) % len;
    }
    render()
}

/// Tab cycles: Presets → From → To → Presets.
/// On entering a field, cursor jumps to position 0.
pub fn handle_toggle_focus(model: &mut Model) -> Command<Effect, Event> {
    model.time_range_picker_focus = (model.time_range_picker_focus + 1) % 3;
    // Reset cursor to start when entering a field.
    match model.time_range_picker_focus {
        FOCUS_ABS_FROM => model.time_range_picker_from_cursor = 0,
        FOCUS_ABS_TO => model.time_range_picker_to_cursor = 0,
        _ => {}
    }
    render()
}

/// Overwrite the character at the cursor position and advance past the next separator.
/// Does nothing when focus is on the preset list or when the character is not a digit.
pub fn handle_custom_input(model: &mut Model, c: char) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        return render();
    }
    // Only digits are accepted in overwrite mode.
    if !c.is_ascii_digit() {
        return render();
    }
    let cursor = active_cursor(model);
    if cursor >= DATETIME_LEN || is_separator(cursor) {
        return render();
    }
    if let Some(field) = active_field_mut(model) {
        // Ensure the field is fully expanded to DATETIME_LEN first.
        ensure_datetime_len(field);
        // Overwrite the byte at cursor position.
        let bytes = unsafe { field.as_bytes_mut() };
        bytes[cursor] = c as u8;
    }
    // Advance cursor, skipping over separator characters.
    let new_cursor = advance_after_overwrite(cursor);
    set_cursor(model, new_cursor);
    render()
}

/// Move the cursor one digit-position to the left, skipping separators.
pub fn handle_cursor_left(model: &mut Model) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        return render();
    }
    let cursor = active_cursor(model);
    let new_cursor = prev_digit_pos(cursor);
    set_cursor(model, new_cursor);
    render()
}

/// Move the cursor one digit-position to the right, skipping separators.
pub fn handle_cursor_right(model: &mut Model) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        return render();
    }
    let cursor = active_cursor(model);
    let new_cursor = next_digit_pos(cursor);
    set_cursor(model, new_cursor);
    render()
}

/// Backspace: move cursor one digit position left (the field is fixed-length; we don't shrink it).
pub fn handle_custom_backspace(model: &mut Model) -> Command<Effect, Event> {
    if model.time_range_picker_focus == FOCUS_PRESETS {
        return render();
    }
    let cursor = active_cursor(model);
    let new_cursor = prev_digit_pos(cursor);
    set_cursor(model, new_cursor);
    render()
}

pub fn handle_commit(model: &mut Model, _now_unix_ms: i64) -> Command<Effect, Event> {
    let new_range = if model.time_range_picker_focus == FOCUS_PRESETS {
        PRESETS[model.time_range_picker_index.min(PRESETS.len() - 1)].to_string()
    } else {
        let from = model.time_range_picker_abs_from.trim();
        let to = model.time_range_picker_abs_to.trim();
        if parse_datetime_utc_ns(from).is_some() && parse_datetime_utc_ns(to).is_some() {
            encode_absolute(from, to)
        } else {
            PRESETS[model.time_range_picker_index.min(PRESETS.len() - 1)].to_string()
        }
    };

    model.time_range = new_range;
    model.time_range_picker_open = false;
    model.time_range_picker_abs_from.clear();
    model.time_range_picker_abs_to.clear();
    model.time_range_picker_from_cursor = 0;
    model.time_range_picker_to_cursor = 0;
    model.time_range_picker_focus = FOCUS_PRESETS;

    match model.screen {
        Screen::PyroscopeMode => {
            model.pyroscope_series_loading = true;
            model.pyroscope_series_error = None;
        }
        Screen::LokiMode => {
            model.loki_loading = true;
            model.loki_error = None;
        }
        Screen::TempoMode => {
            model.tempo_loading = true;
            model.tempo_error = None;
        }
        _ => {}
    }

    render()
}

/// Ensure `field` is exactly `DATETIME_LEN` bytes long, padding with `_` if shorter.
fn ensure_datetime_len(field: &mut String) {
    while field.len() < DATETIME_LEN {
        field.push('_');
    }
    field.truncate(DATETIME_LEN);
}

/// Return the active cursor position for the given focus, for rendering.
pub fn cursor_for_focus(focus: u8, from_cursor: usize, to_cursor: usize) -> Option<usize> {
    match focus {
        FOCUS_ABS_FROM => Some(from_cursor),
        FOCUS_ABS_TO => Some(to_cursor),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_model() -> Model {
        let mut m = Model::default();
        m.time_range = "1h".to_string();
        m
    }

    const NOW_MS: i64 = 7_200_000; // 1970-01-01 02:00:00 UTC

    #[test]
    fn open_pre_fills_from_to() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        assert_eq!(m.time_range_picker_abs_from, "1970-01-01 01:00:00");
        assert_eq!(m.time_range_picker_abs_to, "1970-01-01 02:00:00");
        assert_eq!(m.time_range_picker_from_cursor, 0);
    }

    #[test]
    fn open_seeds_preset_index_for_relative() {
        let mut m = fresh_model();
        m.time_range = "6h".to_string();
        let _ = handle_open(&mut m, NOW_MS);
        assert_eq!(PRESETS[m.time_range_picker_index], "6h");
    }

    #[test]
    fn open_pre_fills_from_to_for_absolute() {
        let mut m = fresh_model();
        m.time_range = "2024-01-15 10:00:00/2024-01-15 14:00:00".to_string();
        let _ = handle_open(&mut m, NOW_MS);
        assert_eq!(m.time_range_picker_abs_from, "2024-01-15 10:00:00");
        assert_eq!(m.time_range_picker_abs_to, "2024-01-15 14:00:00");
    }

    #[test]
    fn close_does_not_change_range() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        let _ = handle_close(&mut m);
        assert_eq!(m.time_range, "1h");
        assert!(!m.time_range_picker_open);
    }

    #[test]
    fn next_wraps_preset_list() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_index = PRESETS.len() - 1;
        let _ = handle_next(&mut m);
        assert_eq!(m.time_range_picker_index, 0);
    }

    #[test]
    fn prev_wraps_preset_list() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_index = 0;
        let _ = handle_prev(&mut m);
        assert_eq!(m.time_range_picker_index, PRESETS.len() - 1);
    }

    #[test]
    fn next_ignored_when_on_from_field() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_index = 2;
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        let _ = handle_next(&mut m);
        assert_eq!(m.time_range_picker_index, 2);
    }

    #[test]
    fn toggle_cycles_three_states() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        assert_eq!(m.time_range_picker_focus, FOCUS_PRESETS);
        let _ = handle_toggle_focus(&mut m);
        assert_eq!(m.time_range_picker_focus, FOCUS_ABS_FROM);
        let _ = handle_toggle_focus(&mut m);
        assert_eq!(m.time_range_picker_focus, FOCUS_ABS_TO);
        let _ = handle_toggle_focus(&mut m);
        assert_eq!(m.time_range_picker_focus, FOCUS_PRESETS);
    }

    #[test]
    fn toggle_resets_cursor_to_zero() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_from_cursor = 5;
        let _ = handle_toggle_focus(&mut m); // → FROM
        assert_eq!(m.time_range_picker_from_cursor, 0);
    }

    #[test]
    fn overwrite_digit_in_from_field() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS); // from = "1970-01-01 01:00:00", cursor=0
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        let _ = handle_custom_input(&mut m, '2'); // overwrites '1' at pos 0
        assert!(m.time_range_picker_abs_from.starts_with('2'));
    }

    #[test]
    fn cursor_advances_past_separator() {
        // Separator at position 4 ('-'): after overwriting pos 3, cursor should jump to 5.
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 3;
        let _ = handle_custom_input(&mut m, '0');
        // pos 3 overwritten, cursor should be at 5 (skipping separator at 4)
        assert_eq!(m.time_range_picker_from_cursor, 5);
    }

    #[test]
    fn cursor_left_skips_separator() {
        // Moving left from pos 5 should land at pos 3 (skipping separator at 4)
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 5;
        let _ = handle_cursor_left(&mut m);
        assert_eq!(m.time_range_picker_from_cursor, 3);
    }

    #[test]
    fn cursor_right_skips_separator() {
        // Moving right from pos 3 should land at pos 5 (skipping separator at 4)
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 3;
        let _ = handle_cursor_right(&mut m);
        assert_eq!(m.time_range_picker_from_cursor, 5);
    }

    #[test]
    fn cursor_left_clamped_at_zero() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 0;
        let _ = handle_cursor_left(&mut m);
        assert_eq!(m.time_range_picker_from_cursor, 0);
    }

    #[test]
    fn cursor_right_clamped_at_end() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = DATETIME_LEN - 1;
        let _ = handle_cursor_right(&mut m);
        assert_eq!(m.time_range_picker_from_cursor, DATETIME_LEN - 1);
    }

    #[test]
    fn backspace_moves_cursor_left() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 5;
        let _ = handle_custom_backspace(&mut m);
        assert_eq!(m.time_range_picker_from_cursor, 3);
    }

    #[test]
    fn non_digit_input_rejected() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        let before = m.time_range_picker_abs_from.clone();
        let _ = handle_custom_input(&mut m, 'x');
        assert_eq!(m.time_range_picker_abs_from, before);
    }

    #[test]
    fn commit_preset_stores_relative() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_index = 5; // "6h"
        m.time_range_picker_focus = FOCUS_PRESETS;
        let _ = handle_commit(&mut m, NOW_MS);
        assert_eq!(m.time_range, "6h");
    }

    #[test]
    fn commit_from_field_focus_stores_absolute() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_abs_from = "2024-01-15 10:00:00".to_string();
        m.time_range_picker_abs_to = "2024-01-15 14:00:00".to_string();
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        let _ = handle_commit(&mut m, NOW_MS);
        assert!(m.time_range.contains('/'));
    }

    #[test]
    fn commit_invalid_fields_falls_back_to_preset() {
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_index = 3; // "1h"
        m.time_range_picker_abs_from = "____-__-__ __:__:__".to_string();
        m.time_range_picker_abs_to = "2024-01-15 14:00:00".to_string();
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        let _ = handle_commit(&mut m, NOW_MS);
        assert_eq!(m.time_range, "1h");
    }

    #[test]
    fn commit_loki_sets_loading() {
        let mut m = fresh_model();
        m.screen = Screen::LokiMode;
        let _ = handle_open(&mut m, NOW_MS);
        let _ = handle_commit(&mut m, NOW_MS);
        assert!(m.loki_loading);
    }

    #[test]
    fn commit_tempo_sets_loading() {
        let mut m = fresh_model();
        m.screen = Screen::TempoMode;
        let _ = handle_open(&mut m, NOW_MS);
        let _ = handle_commit(&mut m, NOW_MS);
        assert!(m.tempo_loading);
    }

    #[test]
    fn is_separator_at_correct_positions() {
        // YYYY-MM-DD HH:MM:SS
        // 0123456789012345678
        //     ^  ^  ^  ^  ^
        assert!(is_separator(4));   // '-'
        assert!(is_separator(7));   // '-'
        assert!(is_separator(10));  // ' '
        assert!(is_separator(13));  // ':'
        assert!(is_separator(16));  // ':'
        assert!(!is_separator(0));
        assert!(!is_separator(5));
        assert!(!is_separator(18));
    }

    #[test]
    fn separator_not_overwritten_by_input() {
        // Cursor at separator position 4 — input should be a no-op.
        let mut m = fresh_model();
        let _ = handle_open(&mut m, NOW_MS);
        m.time_range_picker_focus = FOCUS_ABS_FROM;
        m.time_range_picker_from_cursor = 4;
        let before = m.time_range_picker_abs_from.clone();
        let _ = handle_custom_input(&mut m, '9');
        assert_eq!(m.time_range_picker_abs_from, before);
    }
}
