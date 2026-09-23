/// Project keys: lowercase slug `[a-z0-9][a-z0-9-]*`.
pub fn is_project_key(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Suggest a project key from a folder name, the way init-kit Step 0 does.
pub fn suggest_project_key(folder: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in folder.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "project".to_string() } else { out }
}

/// Topic slug for artifact names: lowercase words joined by dashes, at most `max` chars.
pub fn topic_slug(text: &str, max: usize) -> String {
    let mut slug = suggest_project_key(text);
    if slug.len() > max {
        slug.truncate(max);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert!(is_project_key("backend"));
        assert!(is_project_key("web-2"));
        assert!(!is_project_key("-web"));
        assert!(!is_project_key("Web"));
        assert!(!is_project_key(""));
        assert_eq!(suggest_project_key("Shop Backend_v2"), "shop-backend-v2");
        assert_eq!(suggest_project_key("___"), "project");
    }
}
