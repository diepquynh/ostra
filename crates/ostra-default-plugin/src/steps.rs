//! The built-in stages' own steps. The engine plans and dedups them as `Step::Pipeline` by their
//! key and summary, and hands them back to this crate to perform.

use ostra_core::event::CommandPurpose;
use ostra_core::ids::ExecutionId;
use ostra_core::submit::ReviewFinding;
use ostra_engine::plan::{PipelineStep, Step};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum OstraStep {
    /// Rule D8 and Step 2: run the project's format command, or stage the files a loop changed.
    Command {
        purpose: CommandPurpose,
        project: String,
        command: Option<String>,
        files: Vec<String>,
    },
    Autofix {
        project: String,
        phase: u32,
        tests: bool,
        findings: Vec<ReviewFinding>,
    },
    AnnounceBlocked {
        project: String,
        phase: u32,
        tests: bool,
        reason: String,
    },
    /// Rule O4: end a created project's init. The step checks the inventory and profile first,
    /// and records a failed step instead when either is missing or broken.
    FinishInit { project: String },
    /// Rule O5: record that a step of a created project's init left nothing usable, so the advisor
    /// looks at it.
    RecordInitProblem {
        project: String,
        execution: ExecutionId,
        error: String,
    },
    /// Rule B10: read the project's modules and named constants from disk and record them.
    ScanDocs { project: String },
}

impl OstraStep {
    pub fn key(&self) -> String {
        match self {
            OstraStep::Command {
                purpose, project, ..
            } => format!("cmd:{purpose:?}:{project}"),
            OstraStep::Autofix {
                project,
                phase,
                tests,
                ..
            } => format!("autofix:{project}:{phase}:{tests}"),
            OstraStep::AnnounceBlocked { phase, tests, .. } => format!("blocked:{phase}:{tests}"),
            OstraStep::FinishInit { project } => format!("finish-init:{project}"),
            OstraStep::RecordInitProblem { project, .. } => format!("init-problem:{project}"),
            OstraStep::ScanDocs { project } => format!("scan-docs:{project}"),
        }
    }

    pub fn summary(&self) -> String {
        match self {
            OstraStep::Command {
                purpose, project, ..
            } => format!("command {purpose:?} {project}").to_lowercase(),
            OstraStep::Autofix { phase, tests, .. } => format!(
                "autofix phase {phase}{}",
                if *tests { " tests" } else { "" }
            ),
            OstraStep::AnnounceBlocked { phase, .. } => format!("blocked phase {phase}"),
            OstraStep::FinishInit { project } => format!("finish-init {project}"),
            OstraStep::RecordInitProblem { project, .. } => format!("init-problem {project}"),
            OstraStep::ScanDocs { project } => format!("scan-docs {project}"),
        }
    }

    /// The step of a `Step::Pipeline`, when this crate planned it.
    pub fn of(step: &Step) -> Option<OstraStep> {
        match step {
            Step::Pipeline(p) => serde_json::from_value(p.data.clone()).ok(),
            _ => None,
        }
    }

    /// The project a step works in, whose init it waits for (Rule O4).
    pub fn project(&self) -> Option<&str> {
        match self {
            OstraStep::Command { project, .. } | OstraStep::Autofix { project, .. } => {
                Some(project)
            }
            _ => None,
        }
    }
}

impl From<OstraStep> for Step {
    fn from(s: OstraStep) -> Step {
        Step::Pipeline(PipelineStep {
            key: s.key(),
            summary: s.summary(),
            data: serde_json::to_value(&s).unwrap_or_default(),
        })
    }
}
