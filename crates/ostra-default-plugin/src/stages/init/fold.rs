//! HANDOVER 8.4 and Rules O4 and O5: the init flow's state changes.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::ExecPurpose;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::{InitializerSubmit, SubmitStatus};
use ostra_engine::state::*;
use serde_json::Value;

/// Drop the result of the step `exec` ran, so the step runs again.
pub fn clear_init_result(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect_result = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose_result = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory_result = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.result = None;
            }
        }
    }
}

pub fn reset_init_item(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect = None;
    } else if i.adopt.as_ref() == Some(exec) {
        i.adopt = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.exec = None;
            }
        }
    }
}

/// HANDOVER 8.4 and Rules O4 and O5: the init flow's state changes.
pub trait FoldInit {
    /// The init track an initializer execution belongs to: the init session's own, or a created
    /// project's (Rule O4).
    fn init_track_mut(&mut self, project: &str) -> Option<&mut InitTrack>;

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    );

    fn init_finished(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    );

    /// Rule O5: the step key an initializer execution's advice is counted under.
    fn init_step_key(&self, exec: &ExecutionId) -> String;
}

impl FoldInit for SessionState {
    fn init_track_mut(&mut self, project: &str) -> Option<&mut InitTrack> {
        if self
            .ext
            .os()
            .init
            .as_ref()
            .is_some_and(|i| i.project == project)
        {
            return self.ext.os_mut().init.as_mut();
        }
        self.ext.os_mut().project_inits.get_mut(project)
    }

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    ) {
        use ostra_core::InitializerMode as M;
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        match mode {
            M::Detect => i.detect = Some(id.clone()),
            M::Adopt => i.adopt = Some(id.clone()),
            M::Propose => i.propose = Some(id.clone()),
            M::GenerateInventory => i.inventory = Some(id.clone()),
            M::Scout | M::GenerateSkill => {
                let key = item.unwrap_or_default();
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                match list.iter_mut().find(|x| x.key == key) {
                    Some(x) => x.exec = Some(id.clone()),
                    None => list.push(InitItem {
                        key,
                        exec: Some(id.clone()),
                        result: None,
                    }),
                }
            }
        }
    }

    fn init_finished(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        use ostra_core::InitializerMode as M;
        let parsed: Option<InitializerSubmit> = parse(&result.submit);
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        let ok = status == ExecutionStatus::Ok
            && parsed
                .as_ref()
                .is_some_and(|p| p.status == SubmitStatus::Ok);
        if !ok {
            let retry_key = format!("{mode}:{}", item.clone().unwrap_or_default());
            let retries = i.retries.entry(retry_key).or_insert(0);
            if (status == ExecutionStatus::Interrupted)
                || (status == ExecutionStatus::Error && *retries < ERROR_RETRIES)
            {
                if status == ExecutionStatus::Error {
                    *retries += 1;
                }
                reset_init_item(i, id);
            } else {
                let msg = match parsed {
                    Some(p) if p.status == SubmitStatus::Stuck => {
                        stuck_problem(&p.summary, &p.stuck.map(|s| s.need).unwrap_or_default())
                    }
                    _ => missing_submit(status, &error, result),
                };
                i.failed = Some((id.clone(), msg));
            }
            return;
        }
        // Models sometimes send the result object JSON-encoded as a string.
        let value = match parsed.map(|p| p.result).unwrap_or(Value::Null) {
            Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
            v => v,
        };
        match mode {
            M::Detect => i.detect_result = Some(value),
            M::Adopt => i.adopt_result = Some(value),
            M::Propose => i.propose_result = Some(value),
            M::GenerateInventory => i.inventory_result = Some(value),
            M::Scout | M::GenerateSkill => {
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                if let Some(x) = list.iter_mut().find(|x| x.exec.as_ref() == Some(id)) {
                    x.result = Some(value);
                }
            }
        }
    }

    fn init_step_key(&self, exec: &ExecutionId) -> String {
        match self.executions.get(exec).map(|r| &r.purpose) {
            Some(ExecPurpose::Init { mode, item }) => {
                format!("{mode}:{}", item.clone().unwrap_or_default())
            }
            _ => String::new(),
        }
    }
}
