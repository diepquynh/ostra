/// Project keys: lowercase slug `[a-z0-9][a-z0-9-]*`.
pub fn is_project_key(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Longest stack a project setting may hold.
pub const MAX_STACK_LEN: usize = 64;

/// A project stack is free text (`rust`, `elixir-phoenix`, `Kotlin Ktor`), because the set of real
/// stacks has no end. It must be trimmed, non-empty, short, and free of control characters.
pub fn is_stack_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= MAX_STACK_LEN
        && !value.chars().any(char::is_control)
}

/// Why a stack value is refused.
pub fn stack_issue(value: &str) -> String {
    if value.chars().any(char::is_control) {
        "Remove the control character from the stack, because the stack is one line of plain text."
            .into()
    } else if value.chars().count() > MAX_STACK_LEN {
        format!(
            "Shorten the stack to {MAX_STACK_LEN} characters or fewer, for example `rust` or `python-django`."
        )
    } else {
        "Name the stack in plain text, for example `rust` or `python-django`, or leave it empty so the initializer detects it from the code.".into()
    }
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
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
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

    #[test]
    fn stacks_are_free_text() {
        for ok in ["elixir-phoenix", "rust", "Kotlin Ktor", "c++/qt"] {
            assert!(is_stack_name(ok), "{ok}");
        }
        for bad in [
            "",
            " rust",
            "rust\tx",
            "go\nlang",
            &"x".repeat(MAX_STACK_LEN + 1),
        ] {
            assert!(!is_stack_name(bad), "{bad:?}");
        }
        assert!(stack_issue("a\u{7}b").starts_with("Remove the control character"));
        assert!(stack_issue(&"x".repeat(65)).starts_with("Shorten the stack"));
    }
}
