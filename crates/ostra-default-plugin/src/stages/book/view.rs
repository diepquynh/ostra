//! The docs stage on the board: each project's documentation, the book write, and the book.

use crate::book::DocsTrack;
use crate::data::{DocsState, ProjectTrack};
use crate::view::{Artifacts, card, exec_ids, run_status};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::ExecPurpose as P;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;

/// The documentation card of project `key`, once its docs started.
pub(crate) fn docs_card(s: &SessionState, key: &str, t: &ProjectTrack, out: &mut Vec<StageCard>) {
    let docs = t.docs_aggregate();
    let Some(st) = run_status(&docs) else {
        return;
    };
    let mut c = card(
        StageKind::Documentation,
        format!("Documentation for {key}"),
        st,
    );
    c.project = Some(key.to_string());
    c.executions = exec_ids(
        s,
        |e| matches!(&e.purpose, P::Docs { project, .. } | P::DocsSurvey { project } | P::DocsCheck { project, .. } | P::DocsSynthesis { project, .. } if project == key),
    );
    let pages = t.planned_pages().len();
    let rounds = t.docs_rounds.len();
    c.detail = match &docs {
        DocsState::Done(d) => Some(format!(
            "{} pages after {rounds} synthesis rounds",
            d.sections.len()
        )),
        _ if rounds > 0 => Some(format!("{pages} pages, synthesis round {rounds}")),
        _ if pages > 0 => Some(format!(
            "{} of {pages} first drafts written",
            t.drafts().len()
        )),
        _ => None,
    };
    out.push(c);
}

/// Rule B5: the book write, once it ran.
pub(crate) fn book_card(s: &SessionState, out: &mut Vec<StageCard>) {
    if let Some(w) = &s.book_written {
        let mut c = card(
            StageKind::BookWrite,
            format!("Book {}", w.book),
            if w.error.is_some() {
                StageStatus::Failed
            } else {
                StageStatus::Done
            },
        );
        c.detail = Some(w.error.clone().unwrap_or_else(|| w.projects.join(", ")));
        out.push(c);
    }
}

pub(crate) fn artifacts(s: &SessionState, a: &mut Artifacts) {
    if let Some(w) = s.book_written.as_ref().filter(|w| w.error.is_none()) {
        a.add(
            ostra_core::book::book_dir(&s.workspace_root, &w.book).join("index.md"),
            "book",
            "Documentation book".into(),
            None,
        );
    }
}
