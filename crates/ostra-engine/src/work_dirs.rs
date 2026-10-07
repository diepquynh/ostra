//! Rule WD1: the projects each run works in. The planner names them on every spawn, so a run can
//! work in several projects of the workspace, wherever their folders are.

use crate::plan::SpawnRequest;
use crate::state::SessionState;
use ostra_core::event::ExecPurpose;

impl SessionState {
    /// Every project in the session's scope, `first` first. Before the scope is known, every
    /// project of the session.
    pub fn scope_from(&self, first: &str) -> Vec<String> {
        let scope: Vec<String> = if self.scope.is_empty() {
            self.projects.iter().map(|p| p.key.clone()).collect()
        } else {
            self.scope.clone()
        };
        lead_with(first, scope)
    }

    /// Rule WD1: the projects a spawn works in, `req.project` first.
    pub fn work_projects(&self, req: &SpawnRequest) -> Vec<String> {
        let first = req.project.as_str();
        // A resumed run and a run that continues a conversation keep the folders they had.
        if let Some(rec) = req
            .resumes
            .as_ref()
            .or(req.continues.as_ref())
            .and_then(|id| self.executions.get(id))
        {
            return lead_with(first, rec.projects.clone());
        }
        let list = match &req.purpose {
            ExecPurpose::Init { .. } | ExecPurpose::Advise { .. } | ExecPurpose::Inspect { .. } => {
                vec![]
            }
            ExecPurpose::Message { subagent, .. } | ExecPurpose::Consult { subagent, .. } => self
                .executions
                .get(subagent)
                .map(|r| r.projects.clone())
                .unwrap_or_default(),
            ExecPurpose::Stage { scope, .. } => match scope.as_deref() {
                None => self.scope_from(first),
                Some(s) => match s.strip_prefix("phase:").and_then(|p| p.parse().ok()) {
                    Some(phase) => self.phase_projects(phase),
                    None => vec![],
                },
            },
            p => match self.pipeline.work_projects(self, p) {
                Some(list) => list,
                None => match phase_of(p) {
                    Some(phase) => self.phase_projects(phase),
                    None => self.scope_from(first),
                },
            },
        };
        lead_with(first, list)
    }

    /// Rule WD2: every project of plan phase `phase`, its main project first.
    pub fn phase_projects(&self, phase: u32) -> Vec<String> {
        self.pipeline
            .phase(self, phase)
            .map(|p| p.info.projects())
            .unwrap_or_default()
    }
}

fn phase_of(p: &ExecPurpose) -> Option<u32> {
    match p {
        ExecPurpose::Implement { phase, .. }
        | ExecPurpose::Review { phase, .. }
        | ExecPurpose::Epa { phase }
        | ExecPurpose::WriteTest { phase, .. }
        | ExecPurpose::Verify { phase }
        | ExecPurpose::Unblock { phase, .. } => Some(*phase),
        _ => None,
    }
}

fn lead_with(first: &str, rest: Vec<String>) -> Vec<String> {
    let mut all = vec![first.to_string()];
    for p in rest {
        if !p.is_empty() && !all.contains(&p) {
            all.push(p);
        }
    }
    all
}
