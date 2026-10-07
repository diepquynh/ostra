//! How a spec run that starts or ends changes the spec stage.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::GenerateSpecSubmit;
use ostra_engine::state::*;

/// How a spec run that starts or ends changes the spec stage.
pub trait SpecRuns {
    fn spec_started(&mut self, id: &ExecutionId, resumed: bool);

    fn spec_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: &str);
}

impl SpecRuns for SessionState {
    fn spec_started(&mut self, id: &ExecutionId, resumed: bool) {
        let docs = self.research_docs();
        let t = &mut self.ext.os_mut().spec;
        t.running = Some(id.clone());
        t.needs_run = false;
        if !resumed {
            t.runs.push(id.clone());
            t.sent = InputMark {
                answers: t.answers.len(),
                changes: t.changes.len(),
                docs,
            };
        }
    }

    fn spec_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: &str) {
        self.ext.os_mut().spec.running = None;
        match (status, parse::<GenerateSpecSubmit>(&result.submit)) {
            (ExecutionStatus::Ok, Some(sub)) => {
                self.ext.os_mut().spec.current = Some(sub);
                self.ext.os_mut().spec.applied = self.ext.os_mut().spec.sent.clone();
                self.ext.os_mut().spec.version += 1;
                self.ext.os_mut().spec.pending_findings = None;
                self.ext.os_mut().spec.revoke_approval();
                self.ext.os_mut().spec.error_retries = 0;
            }
            (ExecutionStatus::Interrupted, _) => self.ext.os_mut().spec.needs_run = true,
            _ => artifact_error(
                &mut self.ext.os_mut().spec,
                status,
                missing_submit(status, error, result),
            ),
        }
    }
}
