//! The Track judge on the board.

use crate::prelude::*;
use crate::view::card;
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::pipeline::{Category, StageKind, Track};
use ostra_engine::state::SessionState;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    if let Some(track) = s.ext.os().track
        && s.category == Some(Category::Implement)
    {
        let mut c = card(
            StageKind::Track,
            "Choose the track".into(),
            StageStatus::Done,
        );
        c.detail = Some(match track {
            Track::Light => "Light: spec and plan skipped".into(),
            Track::Full => "Full: spec, plan, and approvals".into(),
        });
        out.push(c);
    }
}
