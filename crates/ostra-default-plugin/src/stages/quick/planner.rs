//! The quick answer: one run of the standard plugin's `answer` agent, whose answer completes the
//! session.

use crate::inputs::OstraInputs;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::{ExecPurpose, GatePayload};
use ostra_core::ids::ExecutionId;
use ostra_engine::plan::*;

/// The quick answer's planning rule.
pub trait PlannerQuick {
    fn quick_answer(&mut self);
}

impl<'a> PlannerQuick for Planner<'a> {
    fn quick_answer(&mut self) {
        let s = self.s;
        let q = &s.ext.os().quick;
        if let Some(a) = &q.answer {
            self.push(Step::Complete {
                report_markdown: Some(a.answer.clone()),
            });
            return;
        }
        if q.running {
            return;
        }
        if let Some(err) = &q.failed {
            // Either failure gate counts: a harness that cannot run opens HarnessFailure.
            let failure_gate = |g: &ostra_engine::state::GateRecord, exec: &ExecutionId| {
                matches!(&g.payload,
                    GatePayload::ExecutionFailed { execution, .. }
                    | GatePayload::HarnessFailure { execution, .. } if execution == exec)
            };
            if let Some(exec) = &q.exec
                && !s
                    .gates
                    .values()
                    .any(|g| failure_gate(g, exec) && g.answer.is_none())
            {
                if s.gates.values().any(|g| failure_gate(g, exec)) {
                    self.push(Step::Fail { error: err.clone() });
                } else {
                    self.exec_failed_gate(
                        exec,
                        s.default_agent(Contract::Answer),
                        &s.primary(),
                        err,
                    );
                }
            }
            return;
        }
        if q.exec.is_none() {
            let primary = s.primary();
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    question: Some(s.full_request()),
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            self.spawn(
                s.default_agent(Contract::Answer),
                ExecPurpose::QuickAnswer,
                &primary,
                s.session_root.clone(),
                inputs,
            );
        }
    }
}
