//! Containment signals (Rule P3): what an execution did that looks like an attempt to leave its
//! sandbox or read what it must not. The runner records each one as an event, and the third for
//! one execution pauses the session.

use crate::exec::ExecutionDelta;
use crate::policy::PolicyDecision;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Signals for one execution that pause its session.
pub const PAUSE_AFTER: usize = 3;

pub const SECRET_READ: &str = "secret-read";
pub const SELF_PROTECTION: &str = "self-protection";
pub const GIT_METADATA: &str = "git-metadata";

/// The guards whose denials are signals. Other guards refuse ordinary mistakes, such as a write
/// outside the phase's scope.
const GUARDS: [&str; 3] = [SECRET_READ, SELF_PROTECTION, GIT_METADATA];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ContainmentSignal {
    /// A Layer 1 guard refused a tool call.
    Guard { rule: String, reason: String },
    /// The egress proxy refused a loopback, private, or link-local destination.
    Egress { host: String, port: u16 },
    /// A process opened a decoy credential file.
    Decoy { path: String },
}

impl ContainmentSignal {
    /// One line for the session's notes and the Activity view.
    pub fn describe(&self) -> String {
        match self {
            ContainmentSignal::Guard { rule, .. } => {
                format!("the {rule} guard refused a tool call")
            }
            ContainmentSignal::Egress { host, port } => {
                format!("the sandbox refused a connection to the local address {host}:{port}")
            }
            ContainmentSignal::Decoy { path } => {
                format!("a command opened the decoy credential file {path}")
            }
        }
    }
}

/// The signal `delta` carries, if any. A refused public host is not one, because builds call
/// telemetry hosts; it shows in the Activity view only.
pub fn classify(delta: &ExecutionDelta) -> Option<ContainmentSignal> {
    match delta {
        ExecutionDelta::Policy {
            decision: PolicyDecision::Deny { reason, rule },
            ..
        } if rule.layer == "guard" && GUARDS.contains(&rule.rule.as_str()) => {
            Some(ContainmentSignal::Guard {
                rule: rule.rule.clone(),
                reason: reason.clone(),
            })
        }
        ExecutionDelta::Egress {
            host,
            port,
            allowed: false,
            local: true,
            ..
        } => Some(ContainmentSignal::Egress {
            host: host.clone(),
            port: *port,
        }),
        ExecutionDelta::Decoy { path } => Some(ContainmentSignal::Decoy { path: path.clone() }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::RuleRef;

    fn deny(rule: RuleRef) -> ExecutionDelta {
        ExecutionDelta::Policy {
            call_id: "c".into(),
            decision: PolicyDecision::deny(rule, "no"),
        }
    }

    fn egress(allowed: bool, local: bool) -> ExecutionDelta {
        ExecutionDelta::Egress {
            host: "10.0.0.1".into(),
            port: 80,
            allowed,
            reason: None,
            local,
        }
    }

    #[test]
    fn only_escape_guards_and_local_refusals_are_signals() {
        for g in GUARDS {
            assert!(classify(&deny(RuleRef::guard(g))).is_some(), "{g}");
        }
        assert!(classify(&deny(RuleRef::guard("write-scope"))).is_none());
        assert!(classify(&deny(RuleRef::permission(SECRET_READ))).is_none());
        assert_eq!(
            classify(&egress(false, true)),
            Some(ContainmentSignal::Egress {
                host: "10.0.0.1".into(),
                port: 80
            })
        );
        assert!(classify(&egress(false, false)).is_none());
        assert!(classify(&egress(true, false)).is_none());
        assert_eq!(
            classify(&ExecutionDelta::Decoy {
                path: "/home/u/.vault-token".into()
            }),
            Some(ContainmentSignal::Decoy {
                path: "/home/u/.vault-token".into()
            })
        );
    }
}
