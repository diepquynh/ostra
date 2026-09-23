/// Largest byte index `<= i` on a char boundary.
pub fn floor_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest byte index `>= i` on a char boundary.
pub fn ceil_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Keep the first and last `max / 2` bytes, with a note of how much was cut from the middle.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head = floor_boundary(s, half);
    let tail = ceil_boundary(s, s.len() - half);
    format!("{}\n\n... [{} characters truncated] ...\n\n{}", &s[..head], s.len() - head - (s.len() - tail), &s[tail..])
}

/// Keep the first `max` bytes, with a note.
pub fn truncate_end(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = floor_boundary(s, max);
    format!("{}\n\n... [truncated: {} more characters]", &s[..cut], s.len() - cut)
}

/// Unified diff for the Activity view, capped at 50000 bytes.
pub fn unified_diff(path: &str, old: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }
    let diff = similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string();
    Some(truncate_end(&diff, 50_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries() {
        let s = "é".repeat(100);
        let t = truncate_middle(&s, 51);
        assert!(t.contains("truncated"));
        assert!(t.starts_with('é'));
        assert_eq!(truncate_middle("short", 10), "short");
        assert!(truncate_end(&s, 3).starts_with('é'));
    }
}
