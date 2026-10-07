//! Rules D1, D2, and M1: research tasks and the sufficiency check.

use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::JudgeKind;
use ostra_core::pipeline::Category;
use ostra_core::workflow::BuiltinStage;
use ostra_engine::plan::*;

/// Rules D1, D2, and M1: research tasks and the sufficiency check.
pub trait PlannerResearch<'a> {
    fn explore_tasks(&mut self);

    /// Rule D2: no explore running and no needed `Not covered` item left unjudged.
    fn explore_complete(&mut self) -> bool;
}

impl<'a> PlannerResearch<'a> for Planner<'a> {
    fn explore_tasks(&mut self) {
        let s = self.s;
        for t in &s.ext.os().explore {
            if t.exec.is_none() && !t.abandoned && t.failed.is_none() {
                // Rule M1: read-only stages fan out; every ready explore spawns at once.
                let inputs = SpawnInputs {
                    task: Some(format!("{}{}", t.task, s.added_context())),
                    ..Default::default()
                };
                let agent = t
                    .agent
                    .unwrap_or_else(|| s.agent_for(BuiltinStage::Research, Contract::Research));
                self.spawn(
                    agent,
                    ExploreRef(t.idx).purpose(),
                    &t.project,
                    s.project_session_dir(&t.project),
                    inputs,
                );
            } else if let (Some(err), None, Some(exec), false, false) = (
                &t.failed,
                &t.gate,
                &t.exec,
                t.abandoned,
                // Rule H3: a helper's failure is its asker's answer, not a gate.
                matches!(t.origin, ExploreOrigin::Ask { .. }),
            ) {
                let agent = s
                    .executions
                    .get(exec)
                    .map(|r| r.agent)
                    .unwrap_or_else(|| s.agent_for(BuiltinStage::Research, Contract::Research));
                self.exec_failed_gate(exec, agent, &t.project, err);
            }
        }
    }

    fn explore_complete(&mut self) -> bool {
        let s = self.s;
        let tasks: Vec<&ExploreTask> = s
            .ext
            .os()
            .explore
            .iter()
            .filter(|t| !t.origin.loop_bound())
            .collect();
        if tasks.iter().any(|t| !t.finished()) {
            return false;
        }
        let unjudged: Vec<&&ExploreTask> = tasks
            .iter()
            .filter(|t| !t.judged && t.result.as_ref().is_some_and(|r| !r.not_covered.is_empty()))
            .collect();
        if !unjudged.is_empty() && s.ext.os().sufficiency_rounds < SUFFICIENCY_ROUNDS {
            let subject = unjudged
                .iter()
                .map(|t| t.idx.to_string())
                .collect::<Vec<_>>()
                .join(",");
            self.push(Step::Judge {
                judge: JudgeKind::Sufficiency,
                subject: Some(subject),
            });
            return false;
        }
        if s.research_docs().is_empty() {
            // Rule D1: with no research document, the spec is not entered.
            if matches!(
                s.category,
                Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
            ) {
                self.push(Step::Fail {
                    error: "Every research task failed or was abandoned, so there is no research document to write a spec from.".into(),
                });
            }
            return false;
        }
        true
    }
}
