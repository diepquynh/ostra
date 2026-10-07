//! The Track judge's answers to the engine's `Pipeline` hooks: the forced track and a workflow's
//! fixed track.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::workflow::WorkflowDef;
use ostra_engine::state::SessionState;

/// The New task form can force the track.
pub(crate) fn created(s: &mut SessionState) {
    s.ext.os_mut().track = s.options.track;
}

/// Rule WF3: a workflow's `track` fixes the track unless the New task form forced one.
pub(crate) fn workflow_resolved(s: &mut SessionState, wf: &WorkflowDef) {
    let os = s.ext.os_mut();
    if os.track.is_none() {
        os.track = wf.track;
    }
}
