//! The docs stage's answers to the engine's `Pipeline` hooks: the docs scan, the request file of
//! a `DOCS` session, and the drafts each docs run reads.

use super::DocsTrack;
use crate::inputs::OstraInputs;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::event::{ExecPurpose, SessionEvent};
use ostra_core::ids::SessionId;
use ostra_core::paths;
use ostra_core::pipeline::Category;
use ostra_engine::plan::SpawnRequest;
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::SessionState;

/// Rule WD1: a docs run works in its own project first, then in the other projects whose
/// parts this session writes, so a page can describe how they work together.
pub(crate) fn work_projects(s: &SessionState, purpose: &ExecPurpose) -> Option<Vec<String>> {
    match purpose {
        ExecPurpose::Docs { project, .. }
        | ExecPurpose::DocsSurvey { project }
        | ExecPurpose::DocsCheck { project, .. }
        | ExecPurpose::DocsSynthesis { project, .. } => {
            // A `DOCS` session has no phases, so its documented projects are the ones with docs on.
            let mut others = s.docs_projects();
            if others.is_empty() {
                others = s
                    .ext
                    .os()
                    .project_tracks
                    .keys()
                    .filter(|k| s.project_docs_on(k))
                    .cloned()
                    .collect();
            }
            let mut all = vec![project.clone()];
            all.extend(others.into_iter().filter(|k| k != project));
            Some(all)
        }
        _ => None,
    }
}

/// Rule B10: read the project's modules and named constants and record them, because the fold
/// cannot read the file system.
pub(crate) async fn scan_docs(
    host: &StepHost,
    session: &SessionId,
    project: String,
) -> Result<(), EngineError> {
    let st = host.snapshot(session)?;
    let path = st.project_path(&project).ok_or_else(|| {
        EngineError::Invalid(format!("Project `{project}` is not in this workspace."))
    })?;
    let profile: ProjectProfile = load_toml(&paths::project_profile(&path)).unwrap_or_default();
    let map = profile.module_map;
    let (modules, refs) = tokio::task::spawn_blocking(move || super::scan::scan(&path, &map))
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

/// Rule DOCS: the request stands in for each writer's implementer report. Rule B10: every docs
/// run after the survey reads the current drafts and the inventory.
pub(crate) fn spawn_files(st: &SessionState, req: &SpawnRequest) {
    if st.category == Some(Category::Docs) {
        for p in OstraInputs::of(&req.inputs)
            .implementer_reports
            .iter()
            .filter(|p| !p.exists())
        {
            let _ = std::fs::write(
                p,
                format!(
                    "# Documentation request\n\nNo implementer ran in this session. The user asked for documentation directly.\n\n## Request\n\n{}\n\n## Changed files\n\nNone, because no implementer ran. Take the code to document from the request: the flows, areas, files, or symbols it names. When it names none, document the whole project, starting from its entry points.\n",
                    st.full_request()
                ),
            );
        }
    }
    if let ExecPurpose::Docs { project, .. }
    | ExecPurpose::DocsSurvey { project }
    | ExecPurpose::DocsCheck { project, .. }
    | ExecPurpose::DocsSynthesis { project, .. } = &req.purpose
    {
        write_docs_drafts(st, project);
    }
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
