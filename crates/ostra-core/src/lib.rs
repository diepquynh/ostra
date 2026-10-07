//! Shared domain types for Ostra: ids, agents, settings, pipeline enums, events, and the contracts
//! between the engine, the executors, the policy layer, and the browser.

pub mod agent;
pub mod api;
pub mod args;
pub mod artifacts;
pub mod book;
pub mod book_search;
pub mod code;
pub mod config;
pub mod containment;
pub mod contract;
pub mod coord;
pub mod doc;
pub mod event;
pub mod exec;
pub mod executor;
pub mod git;
pub mod ids;
pub mod ignore_files;
pub mod manage;
pub mod mcp;
pub mod model;
pub mod outline;
pub mod paths;
pub mod pipeline;
pub mod plugin;
pub mod policy;
pub mod pricing;
pub mod proctree;
pub mod schema_check;
pub mod shells;
pub mod slug;
pub mod submit;
pub mod transform;
pub mod workflow;

pub use agent::{AgentName, Capability, CustomAgent, InitializerMode, WriteScope};
pub use contract::Contract;
pub use executor::{ExecutorKind, HarnessKind};
pub use ids::{DecisionId, ExecutionId, GateId, MessageId, SessionId, WorkspaceId};
pub use model::{Complexity, Effort, Tier};
