//! The code-driven engine: sessions as event-sourced state machines, a pure planner that holds
//! every gate, judge calls where a decision needs judgment, and the runner that performs steps.

pub mod autofix;
pub mod context;
pub mod coord;
pub mod docs_areas;
pub mod factory;
pub mod init;
pub mod judge;
pub mod judge_input;
pub mod plan;
mod plugin_stage;
pub mod runner;
pub mod services;
pub mod state;
pub mod uploads;
pub mod view;
pub mod workflow;

pub use plan::{PlanCtx, SpawnInputs, SpawnRequest, Step, next_steps};
pub use runner::{Engine, EngineError, EngineNotice, suggest_rule};
pub use services::{AgentMeta, BuiltSpawn, Notice, Services, SpawnEnv, SpawnFactory};
pub use state::SessionState;
