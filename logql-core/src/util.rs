/// Convert a char-indexed cursor position to a byte offset in `s`.
pub fn char_to_byte(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Return the byte offset of the start of the last "word" in `s`.
/// Word characters: ASCII alphanumeric, `_`, `:` (PromQL/LogQL recording-rule names).
pub fn word_boundary_byte(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 0
        && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b':')
    {
        i -= 1;
    }
    i
}
