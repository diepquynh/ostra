//! The code-driven engine: sessions as event-sourced state machines, a pure planner that holds
//! every gate, judge calls where a decision needs judgment, and the runner that performs steps.
//! The built-in stages come from a [`pipeline::Pipeline`] that a plugin installs.

pub mod coord;
pub mod pipeline;
pub mod plan;
mod plugin_stage;
pub mod runner;
pub mod services;
pub mod state;
pub mod uploads;
mod work_dirs;
pub mod workflow;

pub use plan::{PlanCtx, SpawnInputs, SpawnRequest, Step, next_steps};
pub use runner::{Engine, EngineError, EngineNotice, StepHost, suggest_rule};
pub use services::{AgentMeta, BuiltSpawn, Notice, Services, SpawnEnv, SpawnFactory};
pub use state::SessionState;
