//! Shared domain types for Ostra: ids, agents, settings, pipeline enums, events, and the contracts
//! between the engine, the executors, the policy layer, and the browser.

pub mod agent;
pub mod api;
pub mod args;
pub mod code;
pub mod config;
pub mod doc;
pub mod event;
pub mod exec;
pub mod executor;
pub mod ids;
pub mod mcp;
pub mod model;
pub mod outline;
pub mod paths;
pub mod pipeline;
pub mod policy;
pub mod pricing;
pub mod slug;
pub mod submit;

pub use agent::{AgentName, Capability, InitializerMode};
pub use executor::{ExecutorKind, HarnessKind};
pub use ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
pub use model::{Complexity, Effort, Tier};
