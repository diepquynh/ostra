//! The quick answer on the board.

use crate::prelude::*;
use crate::view::card;
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    let q = &s.ext.os().quick;
    if q.exec.is_some() {
        let mut c = card(
            StageKind::QuickAnswer,
            "Answer".into(),
            if q.answer.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Running
            },
        );
        c.executions = q.exec.iter().cloned().collect();
        out.push(c);
    }
}
