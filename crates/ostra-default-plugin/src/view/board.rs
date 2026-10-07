//! The board: the stage cards of each built-in stage, in pipeline order, and the helpers the
//! stages' view modules share.

use super::*;
use crate::data::{LoopNext, StageRun, WorkLoop};
use crate::prelude::*;
use crate::stages::{
    book, build, closing, feedback, init, plan, quick, research, spec, stakes, track,
};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::{GatePayload, SessionKind};
use ostra_core::ids::ExecutionId;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;

pub(crate) fn exec_ids(
    s: &SessionState,
    f: impl Fn(&ostra_engine::state::ExecRecord) -> bool,
) -> Vec<ExecutionId> {
    s.executions
        .values()
        .filter(|e| f(e))
        .map(|e| e.id.clone())
        .collect()
}

pub(crate) fn gate_for(
    s: &SessionState,
    f: impl Fn(&GatePayload) -> bool,
) -> Option<&ostra_engine::state::GateRecord> {
    s.gates.values().rev().find(|g| f(&g.payload))
}

pub(crate) fn run_status<T>(r: &StageRun<T>) -> Option<StageStatus> {
    match r {
        StageRun::NotStarted => None,
        StageRun::Running(_) => Some(StageStatus::Running),
        StageRun::Done(_) => Some(StageStatus::Done),
        StageRun::Failed { .. } => Some(StageStatus::Failed),
        StageRun::Abandoned => Some(StageStatus::Skipped),
    }
}

pub(crate) fn card(stage: StageKind, label: String, status: StageStatus) -> StageCard {
    StageCard {
        stage,
        lane: stage.lane(),
        label,
        status,
        project: None,
        phase: None,
        executions: vec![],
        gate: None,
        detail: None,
    }
}

/// The work loop whose stuck run the advisor execution `exec` looks at (Rule O7).
pub(crate) fn advised_loop(s: &SessionState, exec: &ExecutionId) -> Option<(u32, bool)> {
    match s.executions.get(exec).map(|e| &e.purpose) {
        Some(
            ostra_core::event::ExecPurpose::Advise { execution, .. }
            | ostra_core::event::ExecPurpose::Unblock { execution, .. },
        ) => s.executions.get(execution).and_then(|r| r.loop_key),
        _ => None,
    }
}

pub(crate) fn advising_detail(l: &WorkLoop) -> Option<String> {
    match l.next {
        LoopNext::RescueAdvise { .. } => Some("The advisor is looking at the stuck run.".into()),
        LoopNext::RescueFix { .. } => {
            Some("An implementer is fixing what stopped the stuck run.".into())
        }
        _ => None,
    }
}

pub(crate) fn loop_status(l: &WorkLoop) -> StageStatus {
    if l.running.is_some() {
        return StageStatus::Running;
    }
    match &l.next {
        LoopNext::Idle => StageStatus::Pending,
        LoopNext::Done => StageStatus::Done,
        LoopNext::Blocked { .. } => StageStatus::Blocked,
        LoopNext::CapReached { .. } | LoopNext::RescueGate { .. } | LoopNext::Failed { .. } => {
            StageStatus::Waiting
        }
        _ => StageStatus::Running,
    }
}

pub fn stages(s: &SessionState) -> Vec<StageCard> {
    let mut out = vec![];
    if let SessionKind::Init { project } = &s.kind {
        init::view::session_cards(s, project, &mut out);
        out.push(completion_card(s));
        return out;
    }
    research::view::cards(s, &mut out);
    track::view::cards(s, &mut out);
    spec::view::cards(s, &mut out);
    stakes::view::cards(s, &mut out);
    plan::view::cards(s, &mut out);
    init::view::project_cards(s, &mut out);
    build::view::cards(s, &mut out);
    feedback::view::cards(s, &mut out);
    for (key, t) in &s.ext.os().project_tracks {
        closing::view::cards(key, t, &mut out);
        book::view::docs_card(s, key, t, &mut out);
    }
    book::view::book_card(s, &mut out);
    quick::view::cards(s, &mut out);
    out.push(completion_card(s));
    out
}
