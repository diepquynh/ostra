//! How the quick answer's run that starts or ends changes the session.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::QuickAnswerSubmit;
use ostra_engine::state::*;

/// How the quick answer's run that starts or ends changes the session.
pub trait QuickRuns {
    fn quick_started(&mut self, id: &ExecutionId);

    fn quick_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: String);

    fn quick_exec_answered(&mut self, retry: bool);
}

impl QuickRuns for SessionState {
    fn quick_started(&mut self, id: &ExecutionId) {
        self.ext.os_mut().quick.exec = Some(id.clone());
        self.ext.os_mut().quick.running = true;
    }

    fn quick_finished(&mut self, status: ExecutionStatus, result: &ExecutionResult, error: String) {
        self.ext.os_mut().quick.running = false;
        match (status, parse::<QuickAnswerSubmit>(&result.submit)) {
            (ExecutionStatus::Ok, Some(a)) => self.ext.os_mut().quick.answer = Some(a),
            (ExecutionStatus::Ok, None) if !result.final_text.trim().is_empty() => {
                self.ext.os_mut().quick.answer = Some(QuickAnswerSubmit {
                    answer: result.final_text.clone(),
                    sources: vec![],
                })
            }
            (ExecutionStatus::Interrupted, _) => self.ext.os_mut().quick.exec = None,
            _ => self.ext.os_mut().quick.failed = Some(error),
        }
    }

    fn quick_exec_answered(&mut self, retry: bool) {
        if retry {
            self.ext.os_mut().quick.failed = None;
            self.ext.os_mut().quick.exec = None;
        }
    }
}
