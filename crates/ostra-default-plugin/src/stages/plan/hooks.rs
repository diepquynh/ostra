//! The plan stage's answers to the engine's `Pipeline` hooks.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::state::SessionState;
use std::path::PathBuf;

pub(crate) fn master_plan(s: &SessionState) -> Option<PathBuf> {
    s.ext
        .os()
        .plan
        .current
        .as_ref()
        .map(|c| PathBuf::from(&c.master_plan_path))
}
