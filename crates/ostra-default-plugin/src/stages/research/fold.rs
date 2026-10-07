//! Rules D1 and D2: Classify, the research tasks, and the Track decision.

use crate::fold::*;
use crate::judge::{ClassifyOut, ExploreTaskSpec, MAX_ANSWER_RESEARCH, clean_title};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::paths;
use ostra_core::pipeline::{Category, Track};
use ostra_engine::state::*;

/// Rules D1 and D2: Classify, the research tasks, and the Track decision.
pub trait FoldResearch {
    /// Rule J1: queue the research the judge asked for, capped, in projects the session has.
    fn queue_research(&mut self, tasks: &[ExploreTaskSpec], origin: ExploreOrigin) -> Vec<u32>;

    /// Rule U1: research tasks the Route answer judge may skip, numbered from 1 as it sees them.
    fn skippable_research(&self) -> impl Iterator<Item = &ExploreTask> + '_;

    /// Rule U1: skip the research tasks the user told the judge to drop. A running one stops.
    fn skip_research(&mut self, numbers: &[u32]);

    fn apply_classify(&mut self, out: &ClassifyOut);

    /// Light track: one inline phase per project, queued in order (Rule M5), built from the
    /// request and the research. Full track: the spec and plan stages create the phases.
    fn set_track(&mut self, track: Track);

    fn any_explore_started(&self) -> bool;

    fn can_override_classify_now(&self) -> bool;
}

impl FoldResearch for SessionState {
    fn queue_research(&mut self, tasks: &[ExploreTaskSpec], origin: ExploreOrigin) -> Vec<u32> {
        let why = if origin == ExploreOrigin::Amendment {
            "The user asked for this research when they added context to the request"
        } else {
            "The user asked for this research while answering a question"
        };
        let primary = self.primary();
        tasks
            .iter()
            .filter(|t| !t.task.trim().is_empty())
            .take(MAX_ANSWER_RESEARCH)
            .map(|t| {
                let project = if self.valid_project(&t.project) {
                    t.project.clone()
                } else {
                    primary.clone()
                };
                let task = format!(
                    "{}\n\n{why}. The whole request, for context: {}",
                    t.task, self.request
                );
                self.push_explore(project, task, origin.clone())
            })
            .collect()
    }

    fn skippable_research(&self) -> impl Iterator<Item = &ExploreTask> + '_ {
        self.ext.os().explore.iter().filter(|t| {
            !t.finished()
                && t.failed.is_none()
                && !matches!(
                    t.origin,
                    ExploreOrigin::Ask { .. } | ExploreOrigin::Rescue { .. }
                )
        })
    }

    fn skip_research(&mut self, numbers: &[u32]) {
        let idx: Vec<u32> = self
            .skippable_research()
            .filter(|t| numbers.contains(&(t.idx + 1)))
            .map(|t| t.idx)
            .collect();
        for i in idx {
            let running = self.ext.os().explore[i as usize]
                .exec
                .clone()
                .filter(|e| self.executions.get(e).is_some_and(|r| r.result.is_none()));
            match running {
                Some(e) => {
                    self.interrupting.insert(e.clone(), Interrupt::Skipped);
                    self.skip_task(&e);
                }
                None => {
                    let t = &mut self.ext.os_mut().explore[i as usize];
                    t.abandoned = true;
                    if let ExploreOrigin::LoopAnswer { phase, tests } = t.origin {
                        self.release_answer_research((phase, tests));
                    }
                }
            }
        }
    }

    fn apply_classify(&mut self, out: &ClassifyOut) {
        // Rule WF1: a named workflow's base is the category, whatever the judge picked.
        self.category = Some(self.forced_category().unwrap_or(out.category));
        if let Some(t) = clean_title(&out.title) {
            self.title = Some(t);
        }
        // Rule O6: pinned projects are the whole scope, whatever the judge picked.
        let picked = if self.pinned.is_empty() {
            &out.projects
        } else {
            &self.pinned
        };
        let mut scope: Vec<String> = picked
            .iter()
            .filter(|p| self.valid_project(p))
            .cloned()
            .collect();
        scope.dedup();
        if scope.is_empty()
            && let Some(p) = self.projects.first()
        {
            scope.push(p.key.clone());
        }
        // Rule O3: a created project stays in scope when the request is classified again.
        for c in &self.created_projects {
            if !scope.contains(&c.key) {
                scope.push(c.key.clone());
            }
        }
        self.scope = scope;
        self.ext.os_mut().opts_in = out.opts_in;
        self.ext
            .os_mut()
            .explore
            .retain(|t| t.origin != ExploreOrigin::Classify);
        self.ext.os_mut().phases.clear();
        let explores = matches!(
            out.category,
            Category::Research | Category::Spec | Category::Plan | Category::Implement
        );
        if explores {
            let mut tasks: Vec<(String, String)> = out
                .explore_tasks
                .iter()
                .filter(|t| self.valid_project(&t.project) && !t.task.trim().is_empty())
                .map(|t| (t.project.clone(), t.task.clone()))
                .collect();
            if tasks.is_empty() {
                // Rule D1: the spec derives its criteria from research, so there is always one.
                tasks = self
                    .scope
                    .iter()
                    .map(|p| (p.clone(), self.full_request()))
                    .collect();
            }
            for (project, task) in tasks {
                self.push_explore(project, task, ExploreOrigin::Classify);
            }
        }
        let scope = self.scope.clone();
        match out.category {
            Category::Verify => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.review = ReviewMode::Never;
                    l.stage = false;
                    self.insert_phase(inline_phase(id, key, "Verification", i), l);
                }
            }
            Category::Docs => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::docs_request());
                    self.insert_phase(
                        inline_phase(id, key, "Documentation requested by the user", i),
                        l,
                    );
                    if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .closing = Some((false, true));
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .format = Some(None);
                }
            }
            Category::Test => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::test_request());
                    self.insert_phase(inline_phase(id, key, "Tests requested by the user", i), l);
                    if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .closing = Some((true, false));
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .format = Some(None);
                }
            }
            // Quick change (HANDOVER 8.2): one pass per project, no review, staged when it changed files.
            Category::QuickChange => {
                for (i, key) in scope.iter().enumerate() {
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.review = ReviewMode::Never;
                    self.insert_phase(inline_phase(i as u32 + 1, key, "Quick change", i), l);
                }
            }
            Category::Prompt => {
                if let Some(key) = scope.first() {
                    let mut l = WorkLoop::new(false, Contract::Prompt, Contract::Implementation);
                    l.review = ReviewMode::IfCodeChanged;
                    self.insert_phase(inline_phase(1, key, "Prompt change", 0), l);
                }
            }
            _ => {}
        }
        if let Some(track) = self.ext.os().track {
            self.set_track(track);
        }
    }

    fn set_track(&mut self, track: Track) {
        self.ext.os_mut().track = Some(track);
        if self.category != Some(Category::Implement) {
            return;
        }
        self.ext.os_mut().phases.clear();
        if track == Track::Light {
            let scope = self.scope.clone();
            for (i, key) in scope.iter().enumerate() {
                let l = WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                self.insert_phase(inline_phase(i as u32 + 1, key, "Implementation", i), l);
            }
        }
    }

    fn any_explore_started(&self) -> bool {
        self.ext.os().explore.iter().any(|t| t.exec.is_some())
    }

    fn can_override_classify_now(&self) -> bool {
        !self.any_explore_started()
            && self
                .ext
                .os()
                .phases
                .values()
                .all(|p| p.impl_loop.work_count == 0)
    }
}
