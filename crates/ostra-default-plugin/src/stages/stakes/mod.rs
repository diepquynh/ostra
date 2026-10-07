//! The Stakes judge, which can skip the plan of an `IMPLEMENT` request.

pub mod judge_input;
pub mod view;

pub use judge_input::*;

use crate::fold::*;
use crate::judge::StakesOut;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::JudgeKind;
use ostra_core::ids::DecisionId;
use ostra_core::pipeline::{Category, Stakes};
use ostra_engine::plan::*;
use ostra_engine::state::*;
use serde_json::Value;

/// How the Stakes judge's decision changes the session.
pub trait StakesFold {
    fn stakes_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool);
}

impl StakesFold for SessionState {
    fn stakes_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool) {
        if let Ok(out) = serde_json::from_value::<StakesOut>(output.clone()) {
            if overriding && !(self.ext.os().plan.runs.is_empty() && !self.any_phase_started()) {
                return;
            }
            self.ext.os_mut().stakes = Some((id.clone(), out.stakes));
            if overriding {
                self.ext.os_mut().phases.clear();
            }
            if out.stakes == Stakes::Low && self.category == Some(Category::Implement) {
                // Plan skipped for a lower-stakes request: inline phases, one per project,
                // queued in order because there is no graph to read (Rule M5).
                let scope = self.scope.clone();
                for (i, key) in scope.iter().enumerate() {
                    let l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    self.insert_phase(inline_phase(i as u32 + 1, key, "Implementation", i), l);
                }
            }
        }
    }
}

/// The Stakes stage's planning rule.
pub trait PlannerStakes {
    /// Ask the Stakes judge once; the stage is done when it decided, or on the light track.
    fn stakes_stage(&mut self, light: bool) -> bool;
}

impl<'a> PlannerStakes for Planner<'a> {
    fn stakes_stage(&mut self, light: bool) -> bool {
        let s = self.s;
        if light {
            return true;
        }
        if s.ext.os().stakes.is_none() {
            self.push(Step::Judge {
                judge: JudgeKind::Stakes,
                subject: None,
            });
        }
        s.ext.os().stakes.is_some()
    }
}
