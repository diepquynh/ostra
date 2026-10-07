//! Rules H1 and WF6: the run a new run continues, and the subagents a run works with.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{ExecPurpose, FactTarget, WorkKind};
use ostra_core::ids::ExecutionId;
use ostra_core::workflow::BuiltinStage;
use ostra_engine::state::{ExecRecord, SessionState};

pub fn previous_run(s: &SessionState, purpose: &ExecPurpose) -> Option<ExecutionId> {
    let last = |f: &dyn Fn(&ExecPurpose) -> bool| {
        s.executions
            .values()
            .filter(|r| f(&r.purpose))
            .max_by_key(|r| r.id.clone())
            .map(|r| r.id.clone())
    };
    let os = s.ext.os();
    match purpose {
        ExecPurpose::Spec { .. } if os.spec.current.is_some() => {
            last(&|p| matches!(p, ExecPurpose::Spec { .. }))
        }
        ExecPurpose::Plan { .. } if os.plan.current.is_some() => {
            last(&|p| matches!(p, ExecPurpose::Plan { .. }))
        }
        ExecPurpose::FactCheck { target, .. } => {
            let t = *target;
            last(&|p| matches!(p, ExecPurpose::FactCheck { target, .. } if *target == t))
        }
        ExecPurpose::Review { phase, tests, .. } => {
            let (ph, te) = (*phase, *tests);
            last(
                &|p| matches!(p, ExecPurpose::Review { phase, tests, .. } if *phase == ph && *tests == te),
            )
        }
        ExecPurpose::Implement { phase, work } | ExecPurpose::WriteTest { phase, work }
            if matches!(
                work,
                WorkKind::Fix | WorkKind::BlockerFix | WorkKind::Rescue | WorkKind::Resume
            ) =>
        {
            let (ph, test) = (*phase, matches!(purpose, ExecPurpose::WriteTest { .. }));
            last(&|p| match p {
                ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
                    !test && *phase == ph
                }
                ExecPurpose::WriteTest { phase, .. } => test && *phase == ph,
                _ => false,
            })
        }
        _ => None,
    }
}

pub fn partners(s: &SessionState, rec: &ExecRecord) -> Vec<(ExecutionId, String)> {
    let latest = |f: &dyn Fn(&ExecRecord) -> bool| {
        s.executions
            .values()
            .filter(|r| f(r))
            .max_by_key(|r| r.id.clone())
            .map(|r| s.subagent_of(&r.id))
    };
    let mut out: Vec<(ExecutionId, String)> = vec![];
    let checked = |target: FactTarget| -> Option<ExecutionId> {
        latest(&|r| matches!(&r.purpose, ExecPurpose::FactCheck { target: t, .. } if *t == target))
    };
    match &rec.purpose {
        ExecPurpose::Spec { .. } => {
            out.extend(checked(FactTarget::Spec).map(|c| (c, "your fact checker".to_string())))
        }
        ExecPurpose::Plan { .. } => {
            out.extend(checked(FactTarget::Plan).map(|c| (c, "your fact checker".to_string())))
        }
        ExecPurpose::FactCheck { target, .. } => {
            let author = match target {
                FactTarget::Spec => latest(&|r| matches!(r.purpose, ExecPurpose::Spec { .. })),
                FactTarget::Plan => latest(&|r| matches!(r.purpose, ExecPurpose::Plan { .. })),
            };
            out.extend(author.map(|a| (a, "the author of the document you check".to_string())));
        }
        _ if rec.loop_key.is_some() => {
            let key = rec.loop_key;
            let review = matches!(rec.purpose, ExecPurpose::Review { .. });
            let other = latest(&|r| {
                r.loop_key == key && matches!(r.purpose, ExecPurpose::Review { .. }) != review
            });
            out.extend(other.map(|o| {
                (
                    o,
                    if review {
                        "the implementer of the phase you review"
                    } else {
                        "the reviewer of your phase"
                    }
                    .to_string(),
                )
            }));
        }
        _ => {}
    }
    out
}

pub fn stage_partners(s: &SessionState, stage: BuiltinStage) -> Vec<(ExecutionId, String)> {
    let (purpose, role): (fn(&ExecPurpose) -> bool, &str) = match stage {
        BuiltinStage::Spec => (
            |p| matches!(p, ExecPurpose::Spec { .. }),
            "the author of the spec",
        ),
        BuiltinStage::Plan => (
            |p| matches!(p, ExecPurpose::Plan { .. }),
            "the author of the plan",
        ),
        _ => return vec![],
    };
    latest_subagent(s, |r| purpose(&r.purpose))
        .map(|x| (x, role.to_string()))
        .into_iter()
        .collect()
}

pub fn work_partners(s: &SessionState, scope: Option<&str>) -> Vec<(ExecutionId, String)> {
    let phase = scope
        .and_then(|s| s.strip_prefix("phase:"))
        .and_then(|p| p.parse::<u32>().ok());
    let project = scope.and_then(|s| s.strip_prefix("project:"));
    latest_subagent(s, |r| match (&r.purpose, phase, project) {
        (ExecPurpose::Implement { phase: p, .. }, Some(want), _) => *p == want,
        (ExecPurpose::Implement { .. }, None, Some(key)) => r.project == key,
        _ => false,
    })
    .map(|x| (x, "the implementer of the work you check".to_string()))
    .into_iter()
    .collect()
}

/// The subagent of the latest run that `f` picks.
pub(crate) fn latest_subagent(
    s: &SessionState,
    f: impl Fn(&ExecRecord) -> bool,
) -> Option<ExecutionId> {
    s.executions
        .values()
        .filter(|r| f(r))
        .max_by_key(|r| r.id.clone())
        .map(|r| s.subagent_of(&r.id))
}
