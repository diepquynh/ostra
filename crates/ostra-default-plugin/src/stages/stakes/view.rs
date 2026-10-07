//! The Stakes judge on the board, and the plan it skips.

use crate::prelude::*;
use crate::view::card;
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::pipeline::{StageKind, Stakes};
use ostra_engine::state::SessionState;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    if let Some((_, stakes)) = s.ext.os().stakes {
        let mut c = card(
            StageKind::Stakes,
            "Judge the stakes".into(),
            StageStatus::Done,
        );
        c.detail = Some(format!("{stakes:?}"));
        out.push(c);
        if stakes == Stakes::Low {
            out.push(card(
                StageKind::Plan,
                "Plan skipped: low stakes".into(),
                StageStatus::Skipped,
            ));
        }
    }
}
