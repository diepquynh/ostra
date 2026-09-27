//! Shared domain types for Ostra: ids, agents, settings, pipeline enums, events, and the contracts
//! between the engine, the executors, the policy layer, and the browser.

pub mod agent;
pub mod api;
pub mod args;
pub mod artifacts;
pub mod code;
pub mod config;
pub mod containment;
pub mod decoy;
pub mod doc;
pub mod egress;
pub mod event;
pub mod exec;
pub mod executor;
pub mod git;
pub mod ids;
pub mod mcp;
pub mod model;
pub mod outline;
pub mod paths;
pub mod pipeline;
pub mod policy;
pub mod pricing;
pub mod sandbox;
pub mod sandbox_init;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub mod seccomp;
pub mod slug;
pub mod submit;

pub use agent::{AgentName, Capability, InitializerMode};
pub use executor::{ExecutorKind, HarnessKind};
pub use ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
pub use model::{Complexity, Effort, Tier};
