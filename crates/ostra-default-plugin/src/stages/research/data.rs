//! The research stage's state: explore tasks and where each came from.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::agent::AgentName;
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::submit::ExploreSubmit;

#[derive(Debug, Clone, PartialEq)]
pub enum ExploreOrigin {
    Classify,
    Sufficiency,
    Amendment,
    Rescue {
        phase: u32,
        tests: bool,
    },
    /// Rule J1: research the Route answer or Feedback judge queued for an answer before the spec.
    Answer,
    /// Rule J1: research for an answer at a phase's gate; only that loop waits for it.
    LoopAnswer {
        phase: u32,
        tests: bool,
    },
    /// Rule H3: a helper a subagent started with `SubagentAsk`; only the asker waits for it.
    Ask {
        ask: ostra_core::ids::MessageId,
    },
}

impl ExploreOrigin {
    /// Research one work loop waits for, which never holds the rest of the session.
    pub fn loop_bound(&self) -> bool {
        matches!(
            self,
            ExploreOrigin::Rescue { .. }
                | ExploreOrigin::LoopAnswer { .. }
                | ExploreOrigin::Ask { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct ExploreTask {
    pub idx: u32,
    pub project: String,
    pub task: String,
    pub origin: ExploreOrigin,
    pub exec: Option<ExecutionId>,
    pub running: bool,
    pub result: Option<ExploreSubmit>,
    /// An error that exhausted retries, waiting for a gate answer.
    pub failed: Option<String>,
    pub abandoned: bool,
    pub retries: u32,
    pub judged: bool,
    pub gate: Option<GateId>,
    /// Rule SM7: the research helper a message started; else the research stage's bound agent.
    pub agent: Option<AgentName>,
}

impl ExploreTask {
    pub fn finished(&self) -> bool {
        self.result.is_some() || self.abandoned
    }
}
