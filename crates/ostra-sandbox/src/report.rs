//! What the egress proxy and the decoys report, turned into execution deltas for the engine.

use ostra_core::exec::{ExecutionDelta, ExecutionHost};
use std::sync::Arc;

/// Sends the egress proxy's decisions to `host` as [`ExecutionDelta::Egress`], from a thread of
/// its own, because the proxy's runtime must not block on the host. The thread ends when the
/// proxy drops the callback.
pub fn egress(host: Arc<dyn ExecutionHost>) -> crate::egress::OnDecision {
    let (tx, rx) = std::sync::mpsc::channel::<crate::egress::Decision>();
    let _ = std::thread::Builder::new()
        .name("ostra-egress-report".into())
        .spawn(move || {
            for d in rx {
                host.emit(ExecutionDelta::Egress {
                    host: d.host,
                    port: d.port,
                    allowed: d.allowed,
                    reason: d.reason,
                    local: d.local,
                });
            }
        });
    Arc::new(move |d| {
        let _ = tx.send(d);
    })
}

/// Sends each decoy open to `host` as [`ExecutionDelta::Decoy`].
pub fn decoys(host: Arc<dyn ExecutionHost>) -> crate::decoy::OnOpen {
    Arc::new(move |p| {
        host.emit(ExecutionDelta::Decoy {
            path: p.display().to_string(),
        })
    })
}
