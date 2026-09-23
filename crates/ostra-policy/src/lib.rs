//! Ostra's policy engine. Every tool call from every executor passes two layers in order:
//! guards (layer 1), which nothing overrides, then permissions (layer 2) in Claude Code's model of
//! modes plus allow, ask, and deny rules.

pub mod bash;
pub mod build;
pub mod guards;
pub mod perms;
mod policy;

pub use perms::validate_rule;
pub use policy::{ExecutionPolicy, Observation, PolicyInputs};

/// Every path a Codex `*** Begin Patch` block adds, updates, deletes, or moves to, in order.
pub fn apply_patch_paths(patch: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in patch.lines() {
        let line = line.trim();
        let path = [
            "*** Add File:",
            "*** Update File:",
            "*** Delete File:",
            "*** Move to:",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(str::trim);
        if let Some(p) = path
            && !p.is_empty()
            && !out.iter().any(|o| o == p)
        {
            out.push(p.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_paths() {
        let patch = "*** Begin Patch\n*** Update File: src/a.ts\n@@\n-x\n+y\n*** Add File: src/b.ts\n+new\n*** Update File: src/c.ts\n*** Move to: src/d.ts\n*** Delete File: src/e.ts\n*** Update File: src/a.ts\n*** End Patch";
        assert_eq!(
            apply_patch_paths(patch),
            ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts", "src/e.ts"]
        );
    }
}
