//! The feedback stage on the board: each feedback round and the user's review (Rule F1).

use crate::prelude::*;
use crate::view::{card, first};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    let f = &s.ext.os().feedback;
    for (i, r) in f.rounds.iter().enumerate() {
        let mut c = card(
            StageKind::ImplementationReview,
            format!("Feedback round {}", i + 1),
            StageStatus::Done,
        );
        c.detail = Some(first(&r.text));
        out.push(c);
    }
    if f.gate.is_some() || f.accepted {
        let mut c = card(
            StageKind::ImplementationReview,
            "Review the implementation".into(),
            if f.accepted {
                StageStatus::Done
            } else {
                StageStatus::Waiting
            },
        );
        c.gate = f.gate.clone();
        out.push(c);
    }
}
