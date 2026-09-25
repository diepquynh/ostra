//! The project's commands in `.ostra/project.toml`, edited from the project screen. The engine and
//! the policy read the profile fresh per execution, so a saved command applies to the next run.

use crate::api::ApiErr;
use crate::files;
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use ostra_core::config::{Commands, ProjectProfile, ValidationIssue, load_toml, save_toml};
use ostra_core::paths;

pub const MAX_COMMAND_LEN: usize = 2_000;

/// A command as the profile stores it: trimmed, and `None` when blank. `Err` is the problem.
fn clean(v: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(v) = v else { return Ok(None) };
    let v = v.trim();
    if v.is_empty() || v == "—" {
        return Ok(None);
    }
    if v.contains(['\n', '\r']) {
        return Err("Write the command on one line; chain steps with `&&`.");
    }
    if v.chars().any(char::is_control) {
        return Err("Remove the control characters from the command.");
    }
    if v.len() > MAX_COMMAND_LEN {
        return Err(
            "Shorten the command to 2000 characters, or move it into a script and call that.",
        );
    }
    Ok(Some(v.to_string()))
}

pub fn save(w: &WorkspaceRt, key: &str, body: Commands) -> Result<Commands, ApiErr> {
    let root = files::project_root(w, key)?;
    let path = paths::project_profile(&root);
    if !path.is_file() {
        return Err(ApiErr::new(
            StatusCode::CONFLICT,
            format!(
                "Initialize {key} first: its commands live in `.ostra/project.toml`, which initializing writes."
            ),
        ));
    }
    let mut profile: ProjectProfile = load_toml(&path).map_err(|e| {
        ApiErr::new(
            StatusCode::CONFLICT,
            format!("Fix `.ostra/project.toml` first: {e}"),
        )
    })?;
    let mut issues = vec![];
    let mut take = |field: &str, v: Option<String>| match clean(v) {
        Ok(v) => v,
        Err(message) => {
            issues.push(ValidationIssue {
                path: format!("commands.{field}"),
                message: message.into(),
            });
            None
        }
    };
    let commands = Commands {
        build: take("build", body.build),
        test: take("test", body.test),
        test_one: take("test_one", body.test_one),
        format: take("format", body.format),
        lint: take("lint", body.lint),
        typecheck: take("typecheck", body.typecheck),
        run: take("run", body.run),
    };
    if !issues.is_empty() {
        return Err(ApiErr::invalid_with(
            "Fix the commands and save again.",
            issues,
        ));
    }
    profile.commands = commands;
    save_toml(&path, &profile)
        .map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(profile.commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_trimmed_blanks_dropped_and_multiline_refused() {
        assert_eq!(
            clean(Some("  cargo test  ".into())),
            Ok(Some("cargo test".into()))
        );
        assert_eq!(clean(Some("   ".into())), Ok(None));
        assert_eq!(clean(Some("—".into())), Ok(None));
        assert_eq!(clean(None), Ok(None));
        assert!(clean(Some("make\nmake install".into())).is_err());
        assert!(clean(Some("a\u{7}b".into())).is_err());
        assert!(clean(Some("x".repeat(MAX_COMMAND_LEN + 1))).is_err());
    }
}
