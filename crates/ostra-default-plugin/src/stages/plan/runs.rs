//! How a plan run that starts or ends changes the plan stage.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::PlanSubmit;
use ostra_engine::state::*;

/// How a plan run that starts or ends changes the plan stage.
pub trait PlanRuns {
    fn plan_started(&mut self, id: &ExecutionId, resumed: bool);

    fn plan_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: &str);
}

impl PlanRuns for SessionState {
    fn plan_started(&mut self, id: &ExecutionId, resumed: bool) {
        self.ext.os_mut().plan.running = Some(id.clone());
        if !resumed {
            self.ext.os_mut().plan.runs.push(id.clone());
        }
        self.ext.os_mut().plan.needs_run = false;
        // Rule D10: the plan is revised in place against the changed spec, not replaced.
        if self.ext.os().plan.invalidated {
            self.ext.os_mut().plan.invalidated = false;
            self.ext.os_mut().plan.pending_findings = None;
            self.ext.os_mut().plan.consecutive_fails = 0;
        }
    }

    fn plan_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: &str) {
        self.ext.os_mut().plan.running = None;
        match (status, parse::<PlanSubmit>(&result.submit)) {
            (ExecutionStatus::Ok, Some(sub)) => {
                self.ext.os_mut().plan.current = Some(sub);
                self.ext.os_mut().plan.version += 1;
                self.ext.os_mut().plan.pending_findings = None;
                self.ext.os_mut().plan.revoke_approval();
                self.ext.os_mut().plan.error_retries = 0;
            }
            (ExecutionStatus::Interrupted, _) => self.ext.os_mut().plan.needs_run = true,
            _ => artifact_error(
                &mut self.ext.os_mut().plan,
                status,
                missing_submit(status, error, result),
            ),
        }
    }
}
