//! How a run that starts or ends changes the built-in stages: a dispatcher to the stage that owns
//! the run, plus the work loops that several stages share.

use super::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{
    book::BookRuns, closing::ClosingRuns, plan::PlanRuns, quick::QuickRuns, research::ResearchRuns,
    spec::SpecRuns,
};
use ostra_core::event::{ExecPurpose, FactTarget};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::ExecutionId;
use ostra_core::submit::FactCheckSubmit;
use ostra_engine::state::*;

/// How a run that starts or ends changes the built-in stages.
pub trait FoldExecutions {
    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    );

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult);
}

impl FoldExecutions for SessionState {
    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    ) {
        // A resumed run is the same run: it adds no run, pass, or work count, and keeps the
        // inputs its conversation already saw.
        match purpose {
            ExecPurpose::Explore { task } => self.explore_started(*task, id),
            ExecPurpose::Spec { .. } => self.spec_started(id, resumed),
            ExecPurpose::Plan { .. } => self.plan_started(id, resumed),
            ExecPurpose::FactCheck { target, .. } => match target {
                FactTarget::Spec => self.ext.os_mut().spec.start_check(id),
                FactTarget::Plan => self.ext.os_mut().plan.start_check(id),
            },
            ExecPurpose::Epa { phase } => self.epa_started(*phase, id),
            ExecPurpose::Docs { .. }
            | ExecPurpose::DocsSurvey { .. }
            | ExecPurpose::DocsCheck { .. }
            | ExecPurpose::DocsSynthesis { .. } => self.docs_started(id, purpose),
            ExecPurpose::QuickAnswer => self.quick_started(id),
            ExecPurpose::Init { mode, item } => self.init_started(id, *mode, item.clone()),
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if let Some(l) = self.stuck_loop_mut(execution)
                    && let LoopNext::RescueAdvise { advisor, .. } = &mut l.next
                {
                    *advisor = Some(id.clone());
                } else if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    i.advising = Some(id.clone());
                }
            }
            ExecPurpose::Unblock { execution, .. } => {
                if let Some(l) = self.fixing_loop_mut(execution)
                    && let LoopNext::RescueFix { fixer, .. } = &mut l.next
                {
                    *fixer = Some(id.clone());
                }
            }
            ExecPurpose::PromptGen { handoff_for: None } if !resumed => {
                self.ext.os_mut().prompt_gens += 1
            }
            _ => {}
        }
        if let Some(key) = loop_key {
            let is_handoff = matches!(
                purpose,
                ExecPurpose::PromptGen {
                    handoff_for: Some(_)
                }
            );
            if matches!(purpose, ExecPurpose::PromptGen { .. }) && !resumed {
                self.ext.os_mut().prompt_gens += u32::from(is_handoff);
            }
            if let Some(l) = self.loop_mut(key) {
                l.running = Some(id.clone());
                l.in_flight = Some(l.next.clone());
                if !resumed
                    && matches!(
                        purpose,
                        ExecPurpose::Implement { .. }
                            | ExecPurpose::WriteTest { .. }
                            | ExecPurpose::Verify { .. }
                    )
                    || matches!(purpose, ExecPurpose::PromptGen { handoff_for: None })
                {
                    l.work_count += 1;
                }
            }
        }
    }

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        let status = result.status;
        let error = exec_error(result);
        match &rec.purpose {
            ExecPurpose::Explore { task } => self.explore_finished(*task, status, result, &error),
            ExecPurpose::Spec { .. } => self.spec_finished(status, result, &error),
            ExecPurpose::Plan { .. } => self.plan_finished(status, result, &error),
            ExecPurpose::FactCheck { target, .. } => {
                let parsed: Option<FactCheckSubmit> = parse(&result.submit);
                let (checks, failed_msg) = (parsed, missing_submit(status, &error, result));
                match target {
                    FactTarget::Spec => fact_finished(
                        &mut self.ext.os_mut().spec,
                        &rec.id,
                        status,
                        checks,
                        failed_msg,
                    ),
                    FactTarget::Plan => fact_finished(
                        &mut self.ext.os_mut().plan,
                        &rec.id,
                        status,
                        checks,
                        failed_msg,
                    ),
                }
            }
            ExecPurpose::Epa { phase } => self.epa_finished(*phase, rec, status, result, &error),
            ExecPurpose::Docs { .. }
            | ExecPurpose::DocsSurvey { .. }
            | ExecPurpose::DocsCheck { .. }
            | ExecPurpose::DocsSynthesis { .. } => self.docs_finished(rec, result, error),
            ExecPurpose::QuickAnswer => self.quick_finished(status, result, error),
            ExecPurpose::Init { mode, item } => {
                self.init_finished(&rec.id, *mode, item.clone(), status, result, error)
            }
            ExecPurpose::Unblock { execution, .. } => {
                self.loop_fix_finished(execution, &rec.id, status, result, error)
            }
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if self.stuck_loop_mut(execution).is_some() {
                    self.loop_advice_finished(execution, status, result, error);
                } else {
                    self.advice_finished(project, execution, status, result, error);
                }
            }
            _ => {}
        }
        if let Some(key) = rec.loop_key {
            self.loop_finished(key, rec, result);
        }
    }
}
