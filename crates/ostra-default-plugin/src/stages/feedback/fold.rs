//! Rule F1: the user's review of the implementation, and the plan's phases it adds to.

use crate::fold::*;
use crate::judge::{AnswerRoute, FeedbackTarget};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::model::Complexity;
use ostra_core::pipeline::{PhaseInfo, TestPolicy};
use ostra_engine::state::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Rule F1: the user's review of the implementation, and the plan's phases it adds to.
pub trait FoldFeedback {
    fn add_feedback(&mut self, text: String, routed: bool);

    fn route_feedback(
        &mut self,
        i: usize,
        route: AnswerRoute,
        targets: Vec<FeedbackTarget>,
        reason: Option<String>,
    );

    fn add_revision_phases(&mut self, i: usize);

    fn insert_phase(&mut self, info: PhaseInfo, impl_loop: WorkLoop);

    fn any_phase_started(&self) -> bool;

    /// Replace the phase set with the approved plan's phases (Rules D6, D7).
    fn adopt_plan_phases(&mut self);
}

impl FoldFeedback for SessionState {
    fn add_feedback(&mut self, text: String, routed: bool) {
        self.ext.os_mut().feedback.rounds.push(FeedbackRound {
            text: text.clone(),
            route: None,
            reason: None,
            targets: vec![],
            awaiting_spec: false,
            phases: vec![],
        });
        // Before Rule J1, a round with no spec and one project was built without the judge.
        if !routed && self.ext.os().spec.current.is_none() && self.scope.len() == 1 {
            let i = self.ext.os().feedback.rounds.len() - 1;
            let targets = vec![FeedbackTarget {
                project: self.primary(),
                instruction: text,
            }];
            self.route_feedback(i, AnswerRoute::ImplementationDetail, targets, None);
        }
    }

    fn route_feedback(
        &mut self,
        i: usize,
        route: AnswerRoute,
        targets: Vec<FeedbackTarget>,
        reason: Option<String>,
    ) {
        let text = self.ext.os().feedback.rounds[i].text.clone();
        let mut targets: Vec<FeedbackTarget> = targets
            .into_iter()
            .filter(|t| self.valid_project(&t.project) && !t.instruction.trim().is_empty())
            .collect();
        if targets.is_empty() {
            targets.push(FeedbackTarget {
                project: self.primary(),
                instruction: text.clone(),
            });
        }
        // Rule D10: a requirement change goes into the spec before anything is built from it.
        let spec_first =
            route == AnswerRoute::RequirementChange && self.ext.os().spec.current.is_some();
        let r = &mut self.ext.os_mut().feedback.rounds[i];
        r.route = Some(route);
        r.reason = reason;
        r.targets = targets;
        if spec_first {
            r.awaiting_spec = true;
            self.ext.os_mut().spec.changes.push(format!(
                "The user reviewed the implementation and asked for: {text}"
            ));
            self.ext.os_mut().spec.needs_run = true;
            self.ext.os_mut().spec.revoke_approval();
        } else {
            self.add_revision_phases(i);
        }
    }

    fn add_revision_phases(&mut self, i: usize) {
        let round = i as u32 + 1;
        let targets = self.ext.os().feedback.rounds[i].targets.clone();
        for t in targets {
            let id = self
                .ext
                .os()
                .phases
                .keys()
                .chain(self.ext.os().superseded_phases.iter().map(|p| &p.info.id))
                .max()
                .copied()
                .unwrap_or(0)
                + 1;
            let info = PhaseInfo {
                also: Vec::new(),
                id,
                deliverable: None,
                project: t.project.clone(),
                title: format!("Revision {round}"),
                complexity: Complexity::Low,
                test_policy: TestPolicy::Required,
                depends_on: Some(vec![]),
                file: None,
                test_rationale: None,
            };
            self.insert_phase(
                info,
                WorkLoop::new(false, Contract::Implementation, Contract::Implementation),
            );
            if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                p.revision = Some(Revision {
                    round,
                    instruction: t.instruction,
                });
            }
            self.ext.os_mut().feedback.rounds[i].phases.push(id);
        }
    }

    fn insert_phase(&mut self, info: PhaseInfo, impl_loop: WorkLoop) {
        self.ext
            .os_mut()
            .project_tracks
            .entry(info.project.clone())
            .or_default();
        self.ext.os_mut().phases.insert(
            info.id,
            PhaseRun {
                info,
                impl_loop,
                test_loop: WorkLoop::new(true, Contract::Tests, Contract::Tests),
                epa: EpaState::NotStarted,
                implementer_report: None,
                blocked_gate: None,
                revision: None,
            },
        );
    }

    fn any_phase_started(&self) -> bool {
        self.ext
            .os()
            .phases
            .values()
            .any(|p| !p.impl_loop.is_idle() && p.impl_loop.work_count > 0)
    }

    fn adopt_plan_phases(&mut self) {
        let Some(plan) = self.ext.os().plan.current.clone() else {
            return;
        };
        let old = std::mem::take(&mut self.ext.os_mut().phases);
        self.ext
            .os_mut()
            .superseded_phases
            .extend(old.into_values().filter(|p| p.impl_loop.work_count > 0));
        for p in &plan.phases {
            let complexity = p
                .complexity
                .parse::<Complexity>()
                .unwrap_or(Complexity::Medium);
            let test_policy = if p.test_policy.trim().eq_ignore_ascii_case("skip") {
                TestPolicy::Skip
            } else {
                TestPolicy::Required
            };
            // Rule M5: a dependency on a phase that does not exist is unreadable, so it depends
            // on every earlier phase.
            let ids: BTreeSet<u32> = plan.phases.iter().map(|x| x.id).collect();
            let depends_on = if p.depends_on.iter().all(|d| ids.contains(d) && *d != p.id) {
                Some(p.depends_on.clone())
            } else {
                None
            };
            let info = PhaseInfo {
                also: Vec::new(),
                id: p.id,
                deliverable: Some(p.deliverable.clone()).filter(|d| !d.is_empty()),
                project: p.project.clone(),
                title: p.title.clone(),
                complexity,
                test_policy,
                depends_on,
                file: Some(PathBuf::from(&p.file)),
                test_rationale: p.test_rationale.clone(),
            };
            let valid = self.valid_project(&info.project)
                || self.project_to_create(&info.project).is_some();
            self.insert_phase(
                info,
                WorkLoop::new(false, Contract::Implementation, Contract::Implementation),
            );
            if !valid && let Some(ph) = self.ext.os_mut().phases.get_mut(&p.id) {
                ph.impl_loop.next = LoopNext::Blocked {
                    reason: format!(
                        "The plan names project `{}`, which is not in this workspace.",
                        ph.info.project
                    ),
                };
            }
        }
        let os = self.ext.os_mut();
        let phases = &os.phases;
        os.project_tracks
            .retain(|k, _| phases.values().any(|p| &p.info.project == k));
    }
}
