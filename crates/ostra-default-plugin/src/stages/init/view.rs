//! The init flow on the board: an init session's steps, and the init of each project an agent
//! created (Rule O4).

use crate::prelude::*;
use crate::view::{card, exec_ids};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::ExecPurpose as P;
use ostra_core::ids::ExecutionId;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;

/// The steps of an init session for `project`.
pub(crate) fn session_cards(s: &SessionState, project: &str, out: &mut Vec<StageCard>) {
    let Some(i) = &s.ext.os().init else {
        return;
    };
    let st = |exec: &Option<ExecutionId>, done: bool| {
        if done {
            StageStatus::Done
        } else if exec.is_some() {
            StageStatus::Running
        } else {
            StageStatus::Pending
        }
    };
    let mut c = card(
        StageKind::Detect,
        "Detect the stack".into(),
        st(&i.detect, i.detect_result.is_some()),
    );
    c.project = Some(project.to_string());
    c.executions = exec_ids(s, |e| {
        matches!(
            e.purpose,
            P::Init {
                mode: ostra_core::InitializerMode::Detect,
                ..
            }
        )
    });
    out.push(c);
    for x in &i.scouts {
        let mut c = card(
            StageKind::Scout,
            format!("Scout {}", x.key),
            st(&x.exec, x.result.is_some()),
        );
        c.executions = x.exec.iter().cloned().collect();
        out.push(c);
    }
    let mut c = card(
        StageKind::Propose,
        "Propose skills".into(),
        st(&i.propose, i.propose_result.is_some()),
    );
    c.executions = i.propose.iter().cloned().collect();
    out.push(c);
    let mut c = card(
        StageKind::SkillApproval,
        "Approve skills".into(),
        if i.decisions.is_some() {
            StageStatus::Done
        } else if i.approval_gate.is_some() {
            StageStatus::Waiting
        } else {
            StageStatus::Pending
        },
    );
    c.gate = i.approval_gate.clone();
    out.push(c);
    for x in &i.generates {
        let mut c = card(
            StageKind::GenerateSkill,
            format!("Generate {}", x.key),
            st(&x.exec, x.result.is_some()),
        );
        c.executions = x.exec.iter().cloned().collect();
        out.push(c);
    }
    let mut c = card(
        StageKind::GenerateInventory,
        "Write the inventory".into(),
        st(&i.inventory, i.inventory_result.is_some()),
    );
    c.executions = i.inventory.iter().cloned().collect();
    out.push(c);
}

/// Rule O4: a created project's init shows in the build lane, ahead of its phases.
pub(crate) fn project_cards(s: &SessionState, out: &mut Vec<StageCard>) {
    for (key, i) in &s.ext.os().project_inits {
        let executions: Vec<ExecutionId> = s
            .executions
            .values()
            .filter(|e| {
                e.project == *key
                    && match &e.purpose {
                        P::Init { .. } => true,
                        P::Advise { execution, .. } => s
                            .executions
                            .get(execution)
                            .and_then(|r| r.loop_key)
                            .is_none(),
                        _ => false,
                    }
            })
            .map(|e| e.id.clone())
            .collect();
        let gate = i.approval_gate.clone().or_else(|| i.failed_gate.clone());
        let detail = i.note.clone().or_else(|| {
            i.advising
                .as_ref()
                .map(|_| "The advisor is looking at a failed step.".into())
        });
        let status = if i.finished && i.note.is_some() {
            StageStatus::Skipped
        } else if i.finished {
            StageStatus::Done
        } else if gate.is_some() {
            StageStatus::Waiting
        } else if executions.is_empty() {
            StageStatus::Pending
        } else {
            StageStatus::Running
        };
        let mut c = card(
            StageKind::GenerateInventory,
            format!("Initialize {key}"),
            status,
        );
        c.project = Some(key.clone());
        c.executions = executions;
        c.gate = gate;
        c.detail = detail;
        out.push(c);
    }
}
