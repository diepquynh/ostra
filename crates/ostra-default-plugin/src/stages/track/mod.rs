//! The Track judge: the light or the full track of an `IMPLEMENT` request.

pub mod judge_input;

pub use judge_input::*;

use crate::fold::*;
use crate::judge::TrackOut;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::JudgeKind;
use ostra_core::ids::DecisionId;
use ostra_engine::plan::*;
use ostra_engine::state::*;
use serde_json::Value;

/// How the Track judge's decision changes the session.
pub trait TrackFold {
    fn track_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool);
}

impl TrackFold for SessionState {
    fn track_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool) {
        let Ok(out) = serde_json::from_value::<TrackOut>(output.clone()) else {
            return;
        };
        if overriding && !self.can_override(id) {
            return;
        }
        self.ext.os_mut().track_decision = Some(id.clone());
        self.set_track(out.track);
    }
}

/// The Track stage's planning rule.
pub trait PlannerTrack {
    /// Light by default: the Track judge escalates to the full track on evidence.
    fn track_stage(&mut self) -> bool;
}

impl<'a> PlannerTrack for Planner<'a> {
    fn track_stage(&mut self) -> bool {
        let s = self.s;
        if s.ext.os().track.is_none() {
            self.push(Step::Judge {
                judge: JudgeKind::Track,
                subject: None,
            });
        }
        s.ext.os().track.is_some()
    }
}
