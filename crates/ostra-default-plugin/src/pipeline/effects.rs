//! The effects of the built-in stages' own steps: autofix, blocked phases, the init's end, the docs scan, the format and stage commands, and the files written before a spawn.

use crate::book::DocsTrack;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::event::CommandPurpose;
use ostra_core::event::SessionEvent;
use ostra_core::ids::SessionId;
use ostra_core::paths;
use ostra_engine::runner::{EngineError, StepHost, run_host, run_shell};
use ostra_engine::state::SessionState;
use std::path::Path;

/// Rule F2: the session context file is rewritten from the fold before anything reads it.
pub(crate) fn write_session_context(st: &SessionState) -> Result<(), EngineError> {
    let path = st.session_context_path();
    std::fs::write(&path, crate::context::render(st))
        .map_err(|e| EngineError::Invalid(format!("Could not write {}: {e}", path.display())))
}

pub(crate) async fn perform_own(
    host: &StepHost,
    session: &SessionId,
    step: OstraStep,
) -> Result<(), EngineError> {
    match step {
        OstraStep::Autofix {
            project,
            phase,
            tests,
            findings,
        } => {
            let root = host
                .snapshot(session)?
                .project_path(&project)
                .unwrap_or_default();
            let (applied, failed) = tokio::task::spawn_blocking(move || {
                crate::autofix::apply_findings(&root, &findings)
            })
            .await
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
            host.append(
                session,
                SessionEvent::AutofixApplied {
                    project,
                    phase,
                    tests,
                    applied,
                    failed,
                },
            )?;
            Ok(())
        }
        OstraStep::AnnounceBlocked {
            project,
            phase,
            tests,
            reason,
        } => {
            host.append(
                session,
                SessionEvent::PhaseBlocked {
                    project,
                    phase,
                    tests,
                    reason,
                },
            )?;
            Ok(())
        }
        OstraStep::FinishInit { project } => {
            let st = host.snapshot(session)?;
            let root = st.project_path(&project).unwrap_or_default();
            let inventory = st
                .ext
                .os()
                .project_inits
                .get(&project)
                .and_then(|i| i.inventory.clone());
            match (init_problem(&root), inventory) {
                (Some(p), Some(execution)) => {
                    host.append(
                        session,
                        SessionEvent::InitStepFailed {
                            project,
                            execution,
                            error: format!("The init did not finish: {p}."),
                        },
                    )?;
                }
                _ => {
                    let _ = host.db().set_project_init_status(
                        &project,
                        ostra_core::api::InitStatus::Initialized,
                    );
                    host.projects_changed();
                    host.append(session, SessionEvent::ProjectInitFinished { project })?;
                }
            }
            Ok(())
        }
        OstraStep::RecordInitProblem {
            project,
            execution,
            error,
        } => {
            host.append(
                session,
                SessionEvent::InitStepFailed {
                    project,
                    execution,
                    error,
                },
            )?;
            Ok(())
        }
        OstraStep::ScanDocs { project } => {
            let st = host.snapshot(session)?;
            let path = st.project_path(&project).ok_or_else(|| {
                EngineError::Invalid(format!("Project `{project}` is not in this workspace."))
            })?;
            let profile: ProjectProfile =
                load_toml(&paths::project_profile(&path)).unwrap_or_default();
            let map = profile.module_map;
            let (modules, refs) =
                tokio::task::spawn_blocking(move || crate::docs_scan::scan(&path, &map))
                    .await
                    .unwrap_or_default();
            host.append(
                session,
                SessionEvent::DocsScanned {
                    project,
                    modules,
                    refs,
                },
            )?;
            Ok(())
        }
        OstraStep::Command {
            purpose,
            project,
            command,
            files,
        } => perform_command(host, session, purpose, &project, command, files).await,
    }
}

/// The output of a format step skipped because its command waits for approval.
pub const FORMAT_NOT_APPROVED: &str = "The format command in project.toml changed outside Ostra, so format was skipped. Approve it in the project's settings to run it next time.";

/// Rule D8 and Step 2: run the project's format command, or stage the files a loop changed, and
/// record what ran.
pub(crate) async fn perform_command(
    host: &StepHost,
    session: &SessionId,
    purpose: CommandPurpose,
    project: &str,
    command: Option<String>,
    files: Vec<String>,
) -> Result<(), EngineError> {
    let root = host
        .snapshot(session)?
        .project_path(project)
        .unwrap_or_default();
    let started = |command: &str| {
        host.append(
            session,
            SessionEvent::CommandStarted {
                purpose,
                project: project.into(),
                command: command.into(),
            },
        )
        .map(|_| ())
    };
    let (cmd_text, exit, tail) = match purpose {
        CommandPurpose::Format => match command {
            None => (
                String::new(),
                None,
                "No format command in project.toml, so format was skipped.".to_string(),
            ),
            // Rule A1: a format command runs only once the user approved it.
            Some(cmd) if !host.services().command_approved(&root, &cmd) => {
                (cmd, None, FORMAT_NOT_APPROVED.to_string())
            }
            Some(cmd) => {
                started(&cmd)?;
                // The project's own program, so it runs under the agent sandbox.
                let (code, out) = match ostra_sandbox::host_command(
                    "bash",
                    &["-c".into(), cmd.clone()],
                    &root,
                    &[&root],
                    &host.services().workspace().sandbox(),
                ) {
                    Ok(hc) => run_host(&root, &hc, 600).await,
                    Err(e) => (None, e),
                };
                (cmd, code, out)
            }
        },
        CommandPurpose::Stage => {
            if files.is_empty() {
                (String::new(), Some(0), "No files to stage.".to_string())
            } else {
                // Staging keeps each review focused on the unstaged diff (Step 2).
                for p in ostra_sandbox::repair_git_dirs(&ostra_sandbox::git_repos(&[&root])) {
                    tracing::warn!("removed a planted {} before staging", p.display());
                }
                let mut args: Vec<String> = ostra_core::git::AUTOMATIC
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                args.extend(ostra_core::git::filter_overrides(&root).await);
                args.extend([
                    "-C".into(),
                    root.display().to_string(),
                    "add".into(),
                    "-A".into(),
                    "--".into(),
                ]);
                args.extend(files.iter().cloned());
                let text = format!("git {}", args.join(" "));
                started(&text)?;
                let (code, out) = run_shell(&root, "git", &args, 60).await;
                (text, code, out)
            }
        }
        CommandPurpose::Autofix => (String::new(), Some(0), String::new()),
    };
    host.append(
        session,
        SessionEvent::CommandRan {
            purpose,
            project: project.into(),
            command: cmd_text,
            exit_code: exit,
            output_tail: tail,
        },
    )?;
    Ok(())
}

/// Why an init left the project without a usable inventory or profile.
pub(crate) fn init_problem(root: &Path) -> Option<String> {
    if !paths::project_inventory(root).exists() {
        return Some("the initializer did not write .ostra/INVENTORY.md".into());
    }
    ostra_core::config::load_toml_required::<ProjectProfile>(&paths::project_profile(root))
        .err()
        .map(|e| format!("the generated .ostra/project.toml is not valid: {e}"))
}

/// Rule B10: the drafts of a project as Markdown, the inventory, and an index of the page plan.
pub(crate) fn write_docs_drafts(st: &SessionState, project: &str) {
    let Some(track) = st.ext.os().project_tracks.get(project) else {
        return;
    };
    let dir = st.docs_drafts_dir(project);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Some(scan) = &track.docs_scan {
        let mut text = format!(
            "# Reference sheet for `{project}`\n\nThe modules and the named constants that the engine found in the source. The book must cover each module, and a reader looks up each constant.\n\n## Modules\n\n"
        );
        for m in &scan.modules {
            text.push_str(&format!("- `{}`: {}\n", m.name, m.globs.join(", ")));
        }
        text.push_str("\n## Named constants, by file\n\n");
        let mut file = "";
        for r in &scan.refs {
            if r.file != file {
                file = &r.file;
                text.push_str(&format!("\n### `{file}`\n\n"));
            }
            text.push_str(&format!("- `{}`\n", r.name));
        }
        let _ = std::fs::write(dir.join("reference.md"), text);
    }
    let Some(survey) = track.survey_plan() else {
        return;
    };
    let drafts = track.placed_drafts();
    for d in &drafts {
        let _ = std::fs::write(
            st.docs_draft_path(project, &d.id),
            ostra_core::book::render_section(project, d),
        );
        // Rule B10: the draft before the last revision, so a fact-check re-pass diffs the two.
        if let Some(prev) = track.page_docs.get(&d.id).and_then(|p| p.previous.as_ref()) {
            let mut prev = prev.clone();
            prev.id = d.id.clone();
            let _ = std::fs::write(
                dir.join(format!("{}.prev.md", d.id)),
                ostra_core::book::render_section(project, &prev),
            );
        }
    }
    let mut index = format!(
        "# Page plan for `{project}`\n\n{}\n\n",
        survey.overview.trim()
    );
    for p in &survey.pages {
        let state = if drafts.iter().any(|d| d.id == p.id) {
            format!("draft: `{}.md`", p.id)
        } else if p.rewrite {
            "no draft".into()
        } else {
            "kept from the book".into()
        };
        index.push_str(&format!(
            "- `{}` {} (group: {}, {state}): {}\n",
            p.id, p.title, p.group, p.covers
        ));
    }
    let _ = std::fs::write(dir.join("index.md"), index);
    let mut inv = format!(
        "# Inventory for `{project}`\n\nEvery item the book must cover, with its owning page.\n\n| Item | Name | Owner | Sources | Settings | Names |\n| --- | --- | --- | --- | --- | --- |\n"
    );
    for i in &track.current_inventory() {
        let owner = match &i.out_of_scope {
            Some(why) => format!("out of scope: {why}"),
            None => format!("`{}`", i.owner),
        };
        let code = |v: &[String]| {
            v.iter()
                .map(|n| format!("`{}`", n.replace('|', "\\|")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        inv.push_str(&format!(
            "| `{}` | {} | {owner} | {} | {} | {} |\n",
            i.id,
            i.name.replace('|', "\\|"),
            i.sources.join(", ").replace('|', "\\|"),
            code(&i.settings),
            code(&i.names)
        ));
    }
    let _ = std::fs::write(st.docs_inventory_path(project), inv);
}
