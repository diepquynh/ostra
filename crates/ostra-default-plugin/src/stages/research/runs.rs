//! How an explore run that starts or ends changes the research stage.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::WorkKind;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::ExploreSubmit;
use ostra_engine::state::*;

/// How an explore run that starts or ends changes the research stage.
pub trait ResearchRuns {
    fn explore_started(&mut self, task: u32, id: &ExecutionId);

    fn explore_finished(
        &mut self,
        task: u32,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: &str,
    );
}

impl ResearchRuns for SessionState {
    fn explore_started(&mut self, task: u32, id: &ExecutionId) {
        if let Some(t) = self.ext.os_mut().explore.get_mut(task as usize) {
            t.exec = Some(id.clone());
            t.running = true;
            t.failed = None;
        }
    }

    fn explore_finished(
        &mut self,
        task: u32,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: &str,
    ) {
        let idx = task as usize;
        let parsed: Option<ExploreSubmit> = parse(&result.submit);
        let Some(t) = self.ext.os_mut().explore.get_mut(idx) else {
            return;
        };
        t.running = false;
        match (status, parsed) {
            (ExecutionStatus::Ok, Some(sub)) => t.result = Some(sub),
            (ExecutionStatus::Interrupted, _) => t.exec = None,
            (_, _) => {
                if t.retries < ERROR_RETRIES && status == ExecutionStatus::Error {
                    t.retries += 1;
                    t.exec = None;
                } else {
                    t.failed = Some(missing_submit(status, error, result));
                }
            }
        }
        let origin = t.origin.clone();
        let summary = t.result.clone();
        if let ExploreOrigin::LoopAnswer { phase, tests } = origin {
            self.release_answer_research((phase, tests));
        }
        if let (ExploreOrigin::Rescue { phase, tests }, Some(sub)) = (origin, summary) {
            let task_idx = task;
            if let Some(l) = self.loop_mut((phase, tests))
                && let LoopNext::RescueExplore {
                    task: waiting,
                    stuck,
                } = l.next.clone()
                && waiting == task_idx
            {
                let fact = format!(
                    "A targeted explore found: {}\nResearch document: {}",
                    sub.findings_summary, sub.research_path
                );
                l.next = LoopNext::Work {
                    kind: WorkKind::Rescue,
                    instructions: Some(rescue_context(&stuck, &fact)),
                };
            }
        }
    }
}
