//! How a path analysis run that starts or ends changes the closing stage.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::ReportSubmit;
use ostra_engine::state::*;
use std::path::PathBuf;

/// How a path analysis run that starts or ends changes the closing stage.
pub trait ClosingRuns {
    fn epa_started(&mut self, phase: u32, id: &ExecutionId);

    fn epa_finished(
        &mut self,
        phase: u32,
        rec: &ExecRecord,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: &str,
    );
}

impl ClosingRuns for SessionState {
    fn epa_started(&mut self, phase: u32, id: &ExecutionId) {
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase) {
            p.epa = EpaState::Running(id.clone());
        }
    }

    fn epa_finished(
        &mut self,
        phase: u32,
        rec: &ExecRecord,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: &str,
    ) {
        let report = parse::<ReportSubmit>(&result.submit)
            .map(|r| PathBuf::from(r.report_path))
            .or_else(|| rec.report_path.clone());
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase) {
            let retries = match &p.epa {
                EpaState::Failed { retries, .. } => *retries,
                _ => 0,
            };
            p.epa = match status {
                ExecutionStatus::Ok => EpaState::Done(report.unwrap_or_default()),
                ExecutionStatus::Interrupted => EpaState::NotStarted,
                ExecutionStatus::Error if retries < ERROR_RETRIES => EpaState::NotStarted,
                _ => EpaState::Failed {
                    exec: rec.id.clone(),
                    error: error.to_string(),
                    gate: None,
                    retries: retries + 1,
                },
            };
        }
    }
}
