//! The sandbox as the setup check and the console show it.

use crate::backend::{Decision, backend, decide, known_gaps};
use ostra_core::HarnessKind;
use ostra_core::api::SandboxStatus;
use ostra_core::config::{SandboxConfig, SandboxMode};

/// What the sandbox does on this machine under `cfg`.
pub fn status(cfg: &SandboxConfig) -> SandboxStatus {
    let (active, message) = match decide(cfg) {
        Ok(Decision::Sandboxed(_)) => (true, None),
        Ok(Decision::Unsandboxed(w)) => (false, w),
        Err(e) => (false, Some(e)),
    };
    SandboxStatus {
        mode: cfg.mode,
        default_mode: SandboxMode::default(),
        available: backend().is_some(),
        backend: backend().map(|b| b.name().to_string()),
        active,
        message,
        gaps: backend()
            .filter(|_| active)
            .and_then(|b| known_gaps(b, cfg.network)),
        decoys: backend().is_some_and(crate::decoy::supported),
        builtin_decoys: crate::decoy::HOME_DECOYS
            .iter()
            .map(|(rel, _)| format!("~/{rel}"))
            .collect(),
        builtin_hosts: crate::egress::DEFAULT_ALLOWED_HOSTS
            .iter()
            .chain(
                HarnessKind::ALL
                    .iter()
                    .flat_map(|h| crate::egress::model_hosts(*h)),
            )
            .map(|h| h.to_string())
            .collect(),
    }
}
