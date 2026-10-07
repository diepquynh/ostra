//! The built-in stages' planning entry points and the helpers several stages share.

use super::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{quick::PlannerQuick, stakes::PlannerStakes, track::PlannerTrack};
use ostra_core::event::{ExecPurpose, GatePayload, JudgeKind, SessionKind};
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::ExecutionId;
use ostra_core::pipeline::{Category, Stakes, Track};
use ostra_core::submit::Severity;
use ostra_core::workflow::{BuiltinStage, WorkflowDef};
use ostra_engine::plan::*;
use ostra_engine::state::*;

/// The phases of `project` whose build passed.
pub(crate) fn passed_phases<'a>(s: &'a SessionState, project: &str) -> Vec<&'a PhaseRun> {
    s.ext
        .os()
        .phases
        .values()
        // Rule WD2: a phase belongs to every project it works in.
        .filter(|p| p.info.projects().iter().any(|k| k == project) && p.impl_loop.is_done())
        .collect()
}

/// Rule WB4: the facts the closing and book stages settled. Seen from a project's instance, the
/// closing stage reads whether docs run for that project; from anywhere else, the projects they
/// run for.
pub fn stage_value(
    s: &SessionState,
    stage: BuiltinStage,
    scope: Option<&str>,
) -> serde_json::Value {
    let os = s.ext.os();
    match stage {
        BuiltinStage::Research => serde_json::json!({ "research_docs": s.research_docs() }),
        BuiltinStage::Track => serde_json::json!({ "track": os.track }),
        BuiltinStage::Spec => serde_json::json!({
            "spec_file": os.spec.current.as_ref().map(|c| c.spec_path.clone())
        }),
        BuiltinStage::Stakes => serde_json::json!({ "stakes": os.stakes.as_ref().map(|(_, s)| s) }),
        BuiltinStage::Plan => serde_json::json!({
            "master_plan": os.plan.current.as_ref().map(|c| c.master_plan_path.clone()),
            "phases": os.phases.len(),
        }),
        BuiltinStage::Build => serde_json::json!({ "phases": os.phases.len() }),
        BuiltinStage::Closing => match scope.and_then(|sc| sc.strip_prefix("project:")) {
            Some(p) => serde_json::json!({ "docs": docs_chosen(s, p).unwrap_or(false) }),
            None => serde_json::json!({
                "docs": s
                    .ext.os().project_tracks
                    .keys()
                    .filter(|k| docs_chosen(s, k) == Some(true))
                    .collect::<Vec<_>>()
            }),
        },
        BuiltinStage::Book => {
            serde_json::json!({ "book": s.book_written.as_ref().map(|w| w.book.clone()) })
        }
        BuiltinStage::Feedback => serde_json::json!({}),
    }
}

pub fn blocker_open(passed: &[&PhaseRun]) -> bool {
    passed
        .iter()
        .any(|p| p.impl_loop.blocker_open || p.test_loop.blocker_open)
}

pub fn revision_task(r: &Revision, request: &str) -> String {
    format!(
        "Feedback round {} from the user, after reviewing the implementation:\n{}\n\nThe original request, for context:\n{request}",
        r.round, r.instruction
    )
}

/// Rule O3: the completion report names every project an agent created and how its init ended.
pub fn created_projects_section(s: &SessionState) -> String {
    if s.created_projects.is_empty() {
        return String::new();
    }
    let rows: Vec<String> = s
        .created_projects
        .iter()
        .map(|p| {
            let init = match s.ext.os().project_inits.get(&p.key).and_then(|i| i.note.as_deref()) {
                Some(note) => format!("not initialized: {note} Initialize it from the project list before its next session."),
                None => "initialized".into(),
            };
            format!("- `{}` at `{}` ({}), {init}", p.key, p.path.display(), p.stack)
        })
        .collect();
    format!("\n\n## Projects created\n\n{}\n", rows.join("\n"))
}

pub struct ExploreRef(pub(crate) u32);

impl ExploreRef {
    pub(crate) fn purpose(&self) -> ExecPurpose {
        ExecPurpose::Explore { task: self.0 }
    }
}

pub fn last_exec_of(s: &SessionState, f: impl Fn(&ExecPurpose) -> bool) -> Option<ExecutionId> {
    s.executions
        .values()
        .filter(|r| f(&r.purpose))
        .max_by_key(|r| r.id.clone())
        .map(|r| r.id.clone())
}

/// Execution status helper for views.
pub fn is_live(status: Option<ExecutionStatus>) -> bool {
    status.is_none()
}

#[allow(dead_code)]
pub fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Blocker => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
    }
}

/// The built-in stages' planning rules over the engine's planner.
pub trait OstraPlanner<'a> {
    /// Rule WF2: a built-in stage runs Ostra's own rules for it, exactly as the fixed pipeline did.
    fn builtin_stage(&mut self, b: BuiltinStage, wf: &WorkflowDef) -> bool;

    fn nothing_running(&self) -> bool;

    fn completion(&mut self);

    fn init_flow(&mut self);
}

impl<'a> OstraPlanner<'a> for Planner<'a> {
    fn builtin_stage(&mut self, b: BuiltinStage, wf: &WorkflowDef) -> bool {
        let s = self.s;
        let implement = wf.base == Category::Implement;
        let light = implement && s.ext.os().track == Some(Track::Light);
        match b {
            BuiltinStage::Research => self.explore_complete(),
            BuiltinStage::Track => self.track_stage(),
            BuiltinStage::Spec => light || self.spec_flow(wf.base != Category::Spec),
            BuiltinStage::Stakes => self.stakes_stage(light),
            BuiltinStage::Plan if implement => {
                if light {
                    return true;
                }
                let low = matches!(s.ext.os().stakes, Some((_, Stakes::Low)));
                (low || self.plan_flow())
                    && !s.ext.os().plan.invalidated
                    && !s.ext.os().spec.needs_run
            }
            BuiltinStage::Plan => self.plan_flow(),
            BuiltinStage::Build => {
                if implement {
                    // Rule O4: a project a phase created is initialized before its phases go on.
                    crate::init::plan_created(self.s, &mut |step| self.out.push(step));
                }
                self.phases();
                self.phase_stages(wf);
                self.build_done(wf)
            }
            BuiltinStage::Feedback => self.implementation_review(),
            BuiltinStage::Closing => {
                self.closing_stages();
                self.all_implement_done()
            }
            BuiltinStage::Book => self.book_flow(),
        }
    }

    fn nothing_running(&self) -> bool {
        self.s.running_executions().next().is_none()
            && self.s.open_gates().all(|g| matches!(g.payload, GatePayload::Permission { .. }))
            // A blocked phase is announced before the completion report is written.
            && self.s.ext.os().phases.values().all(|p| {
                [&p.impl_loop, &p.test_loop].iter().all(|l| !l.is_blocked() || l.announced_block)
            })
    }

    fn completion(&mut self) {
        let s = self.s;
        // Rule H9: nothing completes while a subagent waits for an answer.
        if !self.nothing_running()
            || s.ext.os().project_inits.values().any(|i| !i.finished)
            || s.coordination_open()
        {
            return;
        }
        match &s.completion_decision {
            None => self.push(Step::Judge {
                judge: JudgeKind::Completion,
                subject: None,
            }),
            Some(id) => {
                let md = s
                    .decisions
                    .get(id)
                    .and_then(|d| {
                        d.output
                            .get("report_markdown")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .map(|md| md + &created_projects_section(s));
                self.push(Step::Complete {
                    report_markdown: md,
                });
            }
        }
    }

    fn init_flow(&mut self) {
        crate::init::plan_init(self.s, &mut |step| self.out.push(step));
    }
}

pub trait OstraFlows<'a> {
    /// Everything the session plans after the YOLO answers: the init flow, Classify, the Route
    /// answer judge, the workflow, research, and the stages.
    fn session_flow(&mut self);
}

impl<'a> OstraFlows<'a> for Planner<'a> {
    fn session_flow(&mut self) {
        let s = self.s;
        if let SessionKind::Init { .. } = s.kind {
            self.init_flow();
            return;
        }
        // Rule C2: queued context waits for the running executions, and nothing starts before it.
        if s.held_amendments().next().is_some() {
            return;
        }
        // Rule WF1: a named workflow is resolved first, because its base is the category.
        if let Some(step @ Step::ResolveWorkflow { name: Some(_), .. }) = s.workflow_due() {
            self.push(step);
            return;
        }
        let Some(category) = s.category else {
            if s.classify.is_none() {
                self.push(Step::Judge {
                    judge: JudgeKind::Classify,
                    subject: None,
                });
            }
            return;
        };
        // Rule J1: a spec or plan answer waits for the judge before any agent sees it.
        for gate in s.ext.os().held_answers.keys() {
            self.push(Step::Judge {
                judge: JudgeKind::RouteAnswer,
                subject: Some(gate.to_string()),
            });
        }
        // Rule C2: added context waits for the judge, and nothing starts or re-runs before it.
        let mut pending = s.pending_amendments().peekable();
        if pending.peek().is_some() {
            for i in pending {
                self.push(Step::Judge {
                    judge: JudgeKind::RouteAnswer,
                    subject: Some(amendment_subject(i)),
                });
            }
            return;
        }
        // Rule WF1: a new session records its workflow before any stage runs.
        if let Some(step) = s.workflow_due() {
            self.push(step);
            return;
        }
        // Explore tasks spawn whatever the stage: rescue explores can arrive mid-build.
        self.explore_tasks();
        match category {
            Category::QuickAnswer => self.quick_answer(),
            _ => self.workflow_flow(),
        }
    }
}
