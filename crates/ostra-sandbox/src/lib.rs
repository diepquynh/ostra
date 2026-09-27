//! The sandbox under agent commands and the programs Ostra starts for a project. The policy layer
//! judges each tool call by the paths it names, which a shell can hide; the sandbox is what the
//! kernel enforces underneath it: the host is read-only, only the execution's own roots are
//! writable, Ostra's data and the user's credentials are not reachable, and the network leads
//! out only through the egress proxy.
//!
//! The crate has three layers, so an OS difference lives in one place:
//!
//! - [`Profile`] describes one execution's boundary without naming a backend: mounts, the
//!   environment, the egress policy, decoys.
//! - A [`Backend`] renders a profile into a command. Each backend (`bwrap.rs`, `seatbelt.rs`) is
//!   plain code compiled on every OS, so its rendering is testable anywhere.
//! - `sys` is the only code that calls the kernel. One implementation of its `Os` trait per OS
//!   is chosen once, in `sys/mod.rs`.

mod backend;
mod bwrap;
mod cache;
pub mod decoy;
pub mod egress;
mod git;
mod host;
pub mod init;
mod members;
mod profile;
pub mod report;
mod seatbelt;
mod status;
mod sys;
pub mod validate;

#[cfg(all(test, unix))]
mod tests;

pub use backend::{
    Backend, Decision, backend, decide, first_warning, known_gaps, unavailable_message,
    unavailable_reason,
};
pub use bwrap::{Bubblewrap, helper, set_helper};
pub use cache::{remove_session_cache, session_cache};
pub use git::{git_repair_note, git_repos, repair_git_dirs};
pub use host::{HostCommand, host_command};
pub use members::{Members, kill_leftovers};
pub use profile::{Profile, SandboxedCommand, new_scratch};
pub use seatbelt::{SANDBOX_EXEC, TTY_PARAM, set_server_port};
pub use status::status;

/// Moves this process's startup environment where other processes of the user cannot read it,
/// where the OS lets them (macOS). Call first thing in `main`, before any thread starts.
pub fn scrub_startup_env() {
    use sys::Os;
    sys::Platform::scrub_startup_env();
}
