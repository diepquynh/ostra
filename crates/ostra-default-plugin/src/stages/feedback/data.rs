//! The feedback stage's state: the user's feedback rounds and revisions.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::ids::GateId;

#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    /// 1-based feedback round.
    pub round: u32,
    pub instruction: String,
}

/// The review after every phase finished: feedback rounds until the user accepts.
#[derive(Debug, Clone, Default)]
pub struct FeedbackTrack {
    pub rounds: Vec<FeedbackRound>,
    pub gate: Option<GateId>,
    pub accepted: bool,
}

#[derive(Debug, Clone)]
pub struct FeedbackRound {
    pub text: String,
    /// `None` until the round is routed, by the Feedback judge or directly.
    pub route: Option<AnswerRoute>,
    pub reason: Option<String>,
    pub targets: Vec<FeedbackTarget>,
    /// A requirement change waits for the spec to be approved again before it is built.
    pub awaiting_spec: bool,
    pub phases: Vec<u32>,
}
