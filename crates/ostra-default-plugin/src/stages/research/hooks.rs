//! The research stage's answers to the engine's `Pipeline` hooks: research documents, skipped
//! research, and amendments that reach the research judge.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{ExecPurpose, JudgeKind};
use ostra_core::pipeline::Category;
use ostra_engine::plan::SpawnRequest;
use ostra_engine::state::SessionState;

/// Rule U1: research skipped while this spawn waited for a slot does not start.
pub(crate) fn spawn_dropped(st: &SessionState, req: &SpawnRequest) -> bool {
    matches!(&req.purpose, ExecPurpose::Explore { task }
        if st.ext.os().explore.get(*task as usize).is_some_and(|t| t.abandoned))
}

/// Rule U1: an amendment goes to the Route answer judge once Classify picked a category that
/// researches.
pub(crate) fn routes_amendments(s: &SessionState) -> bool {
    s.classify.is_some()
        && matches!(
            s.category,
            Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
        )
}

/// Rule U1: research the decision skipped stops now.
pub(crate) fn judge_skips_work(kind: JudgeKind) -> bool {
    kind == JudgeKind::RouteAnswer
}
