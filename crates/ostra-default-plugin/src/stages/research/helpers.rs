//! Rule SM7: a research helper that another agent asks for runs as one of the research stage's
//! explore tasks, and its document joins the session's research.

#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::ExecPurpose;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::MessageId;
use ostra_core::{AgentName, Contract};
use ostra_engine::pipeline::HelperResult;
use ostra_engine::state::{ExecRecord, SessionState};

#[allow(clippy::too_many_arguments)]
pub fn helper_task(
    s: &mut SessionState,
    ask: &MessageId,
    agent: AgentName,
    contract: Option<Contract>,
    project: &str,
    asker: &str,
    text: &str,
) -> Option<u32> {
    // Rule SM7: a research helper runs as a research task, whose document joins the
    // session's research. Logs from before contracts name only `explore`.
    if !contract.map_or(agent == AgentName::Explore, |c| c == Contract::Research) {
        return None;
    }
    let task = format!("{asker} asks for this research and waits for your findings:\n{text}");
    let t = s.push_explore(
        project.to_string(),
        task,
        ExploreOrigin::Ask { ask: ask.clone() },
    );
    if let Some(x) = s.ext.os_mut().explore.get_mut(t as usize) {
        x.agent = Some(agent);
    }
    Some(t)
}

pub fn helper_ask(s: &SessionState, purpose: &ExecPurpose) -> Option<MessageId> {
    let ExecPurpose::Explore { task } = purpose else {
        return None;
    };
    match &s.ext.os().explore.get(*task as usize)?.origin {
        ExploreOrigin::Ask { ask } => Some(ask.clone()),
        _ => None,
    }
}

pub fn helper_result(
    s: &SessionState,
    rec: &ExecRecord,
    result: &ExecutionResult,
) -> Option<HelperResult> {
    let ExecPurpose::Explore { task } = &rec.purpose else {
        return None;
    };
    let t = s.ext.os().explore.get(*task as usize);
    match result.status {
        ExecutionStatus::Ok => {
            let Some(sub) = t.and_then(|t| t.result.as_ref()) else {
                return Some(HelperResult::NotYet);
            };
            let mut text = format!(
                "{}\n\nResearch document: {}",
                sub.findings_summary, sub.research_path
            );
            if !sub.not_covered.is_empty() {
                text.push_str(&format!("\nNot covered: {}", sub.not_covered.join("; ")));
            }
            Some(HelperResult::Text(text))
        }
        // Interrupted or waiting for its own message: the helper is not done yet.
        ExecutionStatus::Interrupted | ExecutionStatus::Waiting => Some(HelperResult::NotYet),
        // An explore that errors is retried once before it counts as failed.
        _ if t.is_some_and(|t| t.failed.is_none() && !t.abandoned) => Some(HelperResult::NotYet),
        _ => None,
    }
}
