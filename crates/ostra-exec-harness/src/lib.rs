//! Harness executors: run one leaf agent inside an installed CLI (Claude Code, Codex, Grok Build,
//! Antigravity) under a PTY, with every tool call routed through Ostra's policy engine by a hook
//! bridge and Ostra's tools served by an MCP stdio shim.

pub mod adapters;
pub mod bridge;
pub mod executor;
pub mod launch;
pub mod live;
pub mod outcome;
pub mod protocol;
pub mod pty;
pub mod services;
pub mod setup;
pub mod transcript;

pub use bridge::{HarnessBridge, HookArgs, parse_hook_args, run_hook_cli, run_mcp_stdio};
pub use executor::{HarnessExecutor, HarnessExecutorConfig};
pub use live::{LiveExecution, LiveRegistry};
pub use outcome::{AUTH_PREFIX, LAUNCH_PREFIX};
pub use protocol::{HookEvent, McpRequest, McpResponse, PolicyRequest, PolicyResponse};
pub use pty::{PtyRegistry, PtySession};
pub use services::BridgeServices;
pub use setup::{all_harness_status, ensure_agy_integration, harness_status, login_command};
