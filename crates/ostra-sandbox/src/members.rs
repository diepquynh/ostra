//! What one sandboxed invocation holds while it runs, and how its processes end.

use crate::cache::stable_hex;
use crate::egress;
use crate::sys::{Os, Platform};
use ostra_core::paths;
use std::sync::Arc;

/// The processes of one sandboxed invocation. Seatbelt has no pid namespace and nothing like
/// `--die-with-parent`, so a child that calls `setsid` or double-forks outlives its shell. Each
/// invocation's policy allows looking up two mach service names that no daemon registers: its
/// own, and one shared by every sandbox of this data dir, and the OS finds every process whose
/// policy allows a name (`Os::marked`). Under bubblewrap the pid namespace does this instead, so
/// no process is ever marked.
#[derive(Debug)]
pub struct Members {
    pub(crate) own: String,
    pub(crate) group: String,
    /// The egress proxy and forwarded sockets, held so they live as long as the invocation.
    /// Under Seatbelt their ports are the loopback ports its policy allows.
    pub(crate) listeners: Vec<Arc<egress::Listener>>,
    /// Held so the decoys stay watched while a process of the invocation runs.
    #[allow(dead_code)]
    pub(crate) decoys: Option<Arc<crate::decoy::Decoys>>,
}

impl Members {
    pub(crate) fn new(
        listeners: Vec<Arc<egress::Listener>>,
        decoys: Option<Arc<crate::decoy::Decoys>>,
    ) -> Members {
        Members {
            own: format!("dev.ostra.sandbox.{}", uuid::Uuid::new_v4().simple()),
            group: group_marker(),
            listeners,
            decoys,
        }
    }

    /// Kills every process still running in this invocation's sandbox.
    pub fn kill(&self) {
        kill_marked(&self.own);
    }
}

impl Drop for Members {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The marker every sandbox of this data dir carries, so a restarted server finds the processes
/// an earlier one left behind.
fn group_marker() -> String {
    format!("dev.ostra.sandbox.d{}", stable_hex("", &paths::data_dir()))
}

/// Kills processes that sandboxes of this data dir left running. Call once at server start,
/// before any execution starts.
pub fn kill_leftovers() -> usize {
    kill_marked(&group_marker())
}

/// SIGKILLs every process of this user in a sandbox marked `marker`, until none is left. Returns
/// how many it killed.
fn kill_marked(marker: &str) -> usize {
    let mut total = 0;
    for _ in 0..20 {
        let found = Platform::marked(marker);
        if found.is_empty() {
            break;
        }
        total += found.into_iter().filter(|&pid| Platform::kill(pid)).count();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    total
}
