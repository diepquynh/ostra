//! Subagent coordination in the fold (HANDOVER 10.8): questions between subagents, who answers
//! them, and which run waits for which message. Everything here is a pure function of the log.

use crate::state::{ExecRecord, ExploreOrigin, SessionState, purpose_key};
use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::coord::{
    AskInput, AskTarget, CoordReply, DeliveryKind, MAX_CONVERSATION_RUNS, MAX_HELPERS_PER_RUN,
    MAX_SESSION_ASKS, ReplyInput, RunEnd, SUBAGENT_ASK, SUBAGENT_LIST, SUBAGENT_REPLY,
};
use ostra_core::event::{ExecPurpose, FactTarget, SessionEvent};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, MessageId};
use std::path::PathBuf;

/// One question from a run to a helper or another subagent.
#[derive(Debug, Clone)]
pub struct Ask {
    pub id: MessageId,
    pub from: ExecutionId,
    pub target: AskTarget,
    pub message: String,
    pub at: DateTime<Utc>,
    /// The helper research task a question to `explore` started.
    pub explore_task: Option<u32>,
    /// The run that got the question: the helper, the consult run, or the subagent woken in place.
    pub answerer: Option<ExecutionId>,
    pub answer: Option<String>,
    pub answer_delivered: bool,
}

/// Where a question to a subagent goes now (Rule H3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The subagent waits on the asker: wake it in place with the question.
    WakeInPlace(ExecutionId),
    /// The subagent ended its run: a consult run continues this head.
    Consult(ExecutionId),
    /// The subagent is busy: the question waits.
    Pending,
    Failed(String),
}

/// A message ready for a waiting run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub ask: MessageId,
    pub kind: DeliveryKind,
    pub note: String,
}

/// The header of a run that continues its conversation for a pair loop (Rule H5).
pub fn continuation_note(block: &str) -> String {
    format!(
        "Ostra continues your conversation for the next round of the same work, because you already know it. \
         Everything you read and wrote earlier still holds unless the lines below change it. Work from this spawn \
         block, which lists what the round needs, and end with your submit call as before.\n\n{block}"
    )
}

/// The header of a consult run (Rule H3).
pub fn consult_note(block: &str, reply_tool: &str) -> String {
    format!(
        "Another subagent asks you a question about your work. Answer it from your conversation and what you \
         read, then call {reply_tool} with the complete answer and end your turn. Change no file in this run: \
         name any change in your answer instead.\n\n{block}"
    )
}

pub use ostra_core::coord::reply_tool;

fn ended_ok(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
    )
}

fn failed(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Error | ExecutionStatus::Denied | ExecutionStatus::Cancelled
    )
}

impl SessionState {
    // -----------------------------------------------------------------------------------------
    // Subagents (Rule H1)
    // -----------------------------------------------------------------------------------------

    /// The subagent ID of an execution: its conversation's first execution.
    pub fn subagent_of(&self, exec: &ExecutionId) -> ExecutionId {
        self.subagents
            .get(exec)
            .cloned()
            .unwrap_or_else(|| exec.clone())
    }

    fn runs_of(&self, subagent: &ExecutionId) -> impl Iterator<Item = &ExecRecord> {
        self.executions
            .values()
            .filter(move |r| &self.subagent_of(&r.id) == subagent)
    }

    /// The subagent's latest run.
    pub fn head(&self, subagent: &ExecutionId) -> Option<&ExecRecord> {
        self.runs_of(subagent).max_by_key(|r| r.id.clone())
    }

    fn subagent_live(&self, subagent: &ExecutionId) -> bool {
        self.runs_of(subagent).any(|r| r.result.is_none())
    }

    /// The question a run asked and has not had answered yet.
    pub fn open_ask_of(&self, exec: &ExecutionId) -> Option<&Ask> {
        self.asks
            .values()
            .rev()
            .find(|a| &a.from == exec)
            .filter(|a| !a.answer_delivered)
    }

    /// Rule H2: the run asked and waits, ended (native) or alive (harness).
    pub fn is_waiting(&self, exec: &ExecutionId) -> bool {
        let Some(rec) = self.executions.get(exec) else {
            return false;
        };
        match &rec.result {
            Some(r) => r.status == ExecutionStatus::Waiting,
            None => self.open_ask_of(exec).is_some(),
        }
    }

    /// The question this run was given and has not answered.
    pub fn owed_by(&self, exec: &ExecutionId) -> Option<&Ask> {
        self.asks
            .values()
            .find(|a| a.answerer.as_ref() == Some(exec) && a.answer.is_none())
            .filter(|a| matches!(a.target, AskTarget::Subagent { .. }))
    }

    /// The subagent a waiting run waits on.
    fn waits_on(&self, exec: &ExecutionId) -> Option<ExecutionId> {
        let ask = self.open_ask_of(exec)?;
        match &ask.target {
            AskTarget::Subagent { id } => Some(self.subagent_of(id)),
            AskTarget::Agent { .. } => ask.answerer.as_ref().map(|a| self.subagent_of(a)),
        }
    }

    /// The answer of a question, including the failures the fold derives (Rule H3).
    pub fn effective_answer(&self, ask: &Ask) -> Option<String> {
        if let Some(a) = &ask.answer {
            return Some(a.clone());
        }
        if ask.answerer.is_none()
            && let Route::Failed(why) = self.route(ask)
        {
            return Some(why);
        }
        None
    }

    fn describe(&self, subagent: &ExecutionId) -> String {
        match self.executions.get(subagent) {
            Some(r) => format!(
                "subagent {subagent} ({}, {})",
                r.agent,
                r.purpose.run_label()
            ),
            None => format!("subagent {subagent}"),
        }
    }

    /// Rule H3: where a question to a subagent goes now.
    pub fn route(&self, ask: &Ask) -> Route {
        let AskTarget::Subagent { id } = &ask.target else {
            return Route::Pending;
        };
        let subagent = self.subagent_of(id);
        let asker = self.subagent_of(&ask.from);
        let Some(h) = self.head(&subagent) else {
            return Route::Failed(format!("No subagent {id} exists in this session."));
        };
        let waits_on_asker = self.waits_on(&h.id).as_ref() == Some(&asker);
        match &h.result {
            None if self.is_waiting(&h.id) && waits_on_asker => Route::WakeInPlace(h.id.clone()),
            None => Route::Pending,
            Some(r) if r.status == ExecutionStatus::Waiting => {
                if waits_on_asker {
                    Route::WakeInPlace(h.id.clone())
                } else {
                    Route::Pending
                }
            }
            Some(_) if self.subagent_live(&subagent) => Route::Pending,
            Some(r) if ended_ok(r.status) => Route::Consult(h.id.clone()),
            Some(r) if failed(r.status) => Route::Failed(format!(
                "{} ended with status {:?}, so it cannot answer. Continue without the answer, or ask another subagent.",
                self.describe(&subagent),
                r.status
            )),
            Some(_) => Route::Pending,
        }
    }

    /// The message that wakes a waiting run: the answer to its own question first, then a
    /// question from the subagent it waits on.
    pub fn next_delivery(&self, exec: &ExecutionId) -> Option<Delivery> {
        if !self.is_waiting(exec) {
            return None;
        }
        let rec = self.executions.get(exec)?;
        if let Some(ask) = self.open_ask_of(exec)
            && let Some(answer) = self.effective_answer(ask)
        {
            let who = match &ask.target {
                AskTarget::Subagent { id } => self.describe(&self.subagent_of(id)),
                AskTarget::Agent { agent, .. } => match &ask.answerer {
                    Some(a) => format!("the {agent} helper {}", self.subagent_of(a)),
                    None => format!("the {agent} helper"),
                },
            };
            let mut note = format!(
                "The answer from {who} to your question arrived. Continue your task from here.\n\nYour question: {}\n\nAnswer:\n{answer}",
                ask.message
            );
            note.push_str(&self.amended_since(ask.at));
            return Some(Delivery {
                ask: ask.id.clone(),
                kind: DeliveryKind::Answer,
                note,
            });
        }
        let q = self.asks.values().find(|a| {
            a.answerer.is_none()
                && a.answer.is_none()
                && self.route(a) == Route::WakeInPlace(exec.clone())
        })?;
        Some(Delivery {
            ask: q.id.clone(),
            kind: DeliveryKind::Question,
            note: format!(
                "{} asks you a question while you wait for it. Answer it with {} and the complete answer, then end your turn: you go back to waiting for your own answer.\n\nQuestion:\n{}",
                self.describe(&self.subagent_of(&q.from)),
                reply_tool(rec.executor),
                q.message
            ),
        })
    }

    fn amended_since(&self, at: DateTime<Utc>) -> String {
        let added: Vec<&str> = self
            .amendments
            .iter()
            .filter(|a| a.at > at)
            .map(|a| a.text.as_str())
            .collect();
        if added.is_empty() {
            return String::new();
        }
        format!(
            "\n\nThe user added to the request while you waited. Take it into account:\n{}",
            added.join("\n")
        )
    }

    /// Rule H9: a question is open or a subagent waits.
    pub fn coordination_open(&self) -> bool {
        self.asks
            .values()
            .any(|a| !a.answer_delivered && self.is_waiting(&a.from))
    }

    // -----------------------------------------------------------------------------------------
    // Fold
    // -----------------------------------------------------------------------------------------

    pub(crate) fn on_coord_event(&mut self, event: &SessionEvent, at: DateTime<Utc>) {
        match event {
            SessionEvent::AgentAsked {
                id,
                from,
                target,
                message,
            } => {
                let explore_task = match target {
                    AskTarget::Agent { project, .. } => {
                        let task = format!(
                            "{} asks for this research and waits for your findings:\n{message}",
                            self.describe(&self.subagent_of(from))
                        );
                        Some(self.push_explore(
                            project.clone(),
                            task,
                            ExploreOrigin::Ask { ask: id.clone() },
                        ))
                    }
                    AskTarget::Subagent { .. } => None,
                };
                self.asks.insert(
                    id.clone(),
                    Ask {
                        id: id.clone(),
                        from: from.clone(),
                        target: target.clone(),
                        message: message.clone(),
                        at,
                        explore_task,
                        answerer: None,
                        answer: None,
                        answer_delivered: false,
                    },
                );
            }
            SessionEvent::AgentReplied { ask, message, .. } => {
                if let Some(a) = self.asks.get_mut(ask)
                    && a.answer.is_none()
                {
                    a.answer = Some(message.clone());
                }
            }
            SessionEvent::MessageDelivered { ask, to, kind } => {
                let answer = self.effective_answer_of(ask);
                if let Some(a) = self.asks.get_mut(ask) {
                    match kind {
                        DeliveryKind::Question => a.answerer = Some(to.clone()),
                        DeliveryKind::Answer => {
                            a.answer = a.answer.take().or(answer);
                            a.answer_delivered = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn effective_answer_of(&self, ask: &MessageId) -> Option<String> {
        self.asks.get(ask).and_then(|a| self.effective_answer(a))
    }

    /// A helper or consult run got its question as its spawn.
    pub(crate) fn coord_started(&mut self, id: &ExecutionId, purpose: &ExecPurpose) {
        let ask = match purpose {
            ExecPurpose::Consult { ask, .. } => Some(ask.clone()),
            ExecPurpose::Explore { task } => match self.explore.get(*task as usize) {
                Some(t) => match &t.origin {
                    ExploreOrigin::Ask { ask } => Some(ask.clone()),
                    _ => None,
                },
                None => None,
            },
            _ => None,
        };
        if let Some(a) = ask.and_then(|a| self.asks.get_mut(&a)) {
            a.answerer = Some(id.clone());
        }
    }

    /// A run that ended without answering the question it was given answers with that.
    pub(crate) fn coord_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        let owed: Vec<MessageId> = self
            .asks
            .values()
            .filter(|a| a.answerer.as_ref() == Some(&rec.id) && a.answer.is_none())
            .map(|a| a.id.clone())
            .collect();
        for id in owed {
            let answer = match (&rec.purpose, result.status) {
                (ExecPurpose::Explore { task }, ExecutionStatus::Ok) => {
                    let Some(sub) = self
                        .explore
                        .get(*task as usize)
                        .and_then(|t| t.result.clone())
                    else {
                        continue;
                    };
                    let mut s = format!(
                        "{}\n\nResearch document: {}",
                        sub.findings_summary, sub.research_path
                    );
                    if !sub.not_covered.is_empty() {
                        s.push_str(&format!("\nNot covered: {}", sub.not_covered.join("; ")));
                    }
                    s
                }
                // Interrupted or waiting for its own answer: the question stays with this run.
                (
                    ExecPurpose::Explore { .. },
                    ExecutionStatus::Interrupted | ExecutionStatus::Waiting,
                ) => {
                    continue;
                }
                // An explore that errors is retried once before it counts as failed.
                (ExecPurpose::Explore { task }, _)
                    if self
                        .explore
                        .get(*task as usize)
                        .is_some_and(|t| t.failed.is_none() && !t.abandoned) =>
                {
                    continue;
                }
                (_, ExecutionStatus::Interrupted)
                    if !matches!(rec.purpose, ExecPurpose::Consult { .. }) =>
                {
                    continue;
                }
                (_, status) => format!(
                    "{} ended its run with status {status:?} without answering. Continue without the answer, or ask again.",
                    self.describe(&self.subagent_of(&rec.id))
                ),
            };
            if let Some(a) = self.asks.get_mut(&id) {
                a.answer = Some(answer);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Pair loops (Rules H5 to H7)
    // -----------------------------------------------------------------------------------------

    /// The run a pair-loop spawn continues, or `None` for a fresh run.
    pub fn continuation(&self, agent: AgentName, purpose: &ExecPurpose) -> Option<ExecutionId> {
        let last = |f: &dyn Fn(&ExecPurpose) -> bool| {
            self.executions
                .values()
                .filter(|r| f(&r.purpose))
                .max_by_key(|r| r.id.clone())
                .map(|r| r.id.clone())
        };
        let prev = match purpose {
            ExecPurpose::Spec { .. } if self.spec.current.is_some() => {
                last(&|p| matches!(p, ExecPurpose::Spec { .. }))
            }
            ExecPurpose::Plan { .. } if self.plan.current.is_some() => {
                last(&|p| matches!(p, ExecPurpose::Plan { .. }))
            }
            ExecPurpose::FactCheck { target, .. } => {
                let t = *target;
                last(&|p| matches!(p, ExecPurpose::FactCheck { target, .. } if *target == t))
            }
            ExecPurpose::Review { phase, tests, .. } => {
                let (ph, te) = (*phase, *tests);
                last(
                    &|p| matches!(p, ExecPurpose::Review { phase, tests, .. } if *phase == ph && *tests == te),
                )
            }
            ExecPurpose::Implement { phase, work } | ExecPurpose::WriteTest { phase, work }
                if matches!(
                    work,
                    ostra_core::event::WorkKind::Fix
                        | ostra_core::event::WorkKind::BlockerFix
                        | ostra_core::event::WorkKind::Rescue
                        | ostra_core::event::WorkKind::Resume
                ) =>
            {
                let (ph, test) = (*phase, matches!(purpose, ExecPurpose::WriteTest { .. }));
                last(&|p| match p {
                    ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
                        !test && *phase == ph
                    }
                    ExecPurpose::WriteTest { phase, .. } => test && *phase == ph,
                    _ => false,
                })
            }
            _ => None,
        }?;
        let prev_rec = self.executions.get(&prev)?;
        let prev_ok = prev_rec
            .result
            .as_ref()
            .is_some_and(|r| ended_ok(r.status) && r.submit.is_some());
        let subagent = self.subagent_of(&prev);
        let head = self.head(&subagent)?;
        let result = head.result.as_ref()?;
        // Rule H6: fall back to a fresh run.
        let fresh = !prev_ok
            || head.agent != agent
            || !ended_ok(result.status)
            || result.submit.is_none()
            || self
                .forced_executor(agent)
                .is_some_and(|e| e != head.executor)
            || (matches!(head.executor, ExecutorKind::Harness(_))
                && result.native_session_id.is_none())
            || self.amendments.iter().any(|a| a.at > prev_rec.started_at)
            || self.runs_of(&subagent).count() >= MAX_CONVERSATION_RUNS;
        (!fresh).then(|| head.id.clone())
    }

    /// Rule H7: the subagent of `head` has a live run.
    pub fn conversation_busy(&self, head: &ExecutionId) -> bool {
        self.subagent_live(&self.subagent_of(head))
    }

    /// The session dir a subagent's runs work in.
    pub fn session_dir_of(&self, rec: &ExecRecord) -> PathBuf {
        match rec.purpose {
            ExecPurpose::Spec { .. }
            | ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec | FactTarget::Plan,
                ..
            } => self.session_root.clone(),
            _ => self.project_session_dir(&rec.project),
        }
    }

    /// The purpose key of a waiting native run that has a message to wake it.
    pub fn wake_for_key(&self, key: &str) -> Option<Option<ExecutionId>> {
        let exec = self.waiting_keys.get(key)?;
        Some(self.next_delivery(exec).map(|_| exec.clone()))
    }

    // -----------------------------------------------------------------------------------------
    // Tool calls (Rule H4)
    // -----------------------------------------------------------------------------------------

    /// Check a `SubagentAsk` from `exec` and build its event.
    pub fn ask_event(
        &self,
        exec: &ExecutionId,
        input: &serde_json::Value,
    ) -> Result<(SessionEvent, String), String> {
        let a = AskInput::parse(input)?;
        let rec = self.running_rec(exec)?;
        if matches!(rec.purpose, ExecPurpose::Consult { .. }) {
            return Err(format!(
                "Answer with {SUBAGENT_REPLY} instead: this run answers a question and may not ask one."
            ));
        }
        if self.owed_by(exec).is_some() {
            return Err(format!(
                "Answer the question you were given with {SUBAGENT_REPLY} first, because its asker waits for you."
            ));
        }
        if self.asks.len() >= MAX_SESSION_ASKS {
            return Err(format!(
                "Continue without asking: this session reached its limit of {MAX_SESSION_ASKS} questions between subagents."
            ));
        }
        let target = match a.helper()? {
            Some(agent) => {
                let is_helper = matches!(rec.purpose, ExecPurpose::Explore { task }
                    if self.explore.get(task as usize).is_some_and(|t| matches!(t.origin, ExploreOrigin::Ask { .. })));
                if is_helper {
                    return Err(
                        "Do the research yourself: a helper may not start helpers of its own."
                            .into(),
                    );
                }
                let started = self
                    .asks
                    .values()
                    .filter(|x| &x.from == exec && matches!(x.target, AskTarget::Agent { .. }))
                    .count();
                if started >= MAX_HELPERS_PER_RUN {
                    return Err(format!(
                        "Continue with what you have: this run already started {MAX_HELPERS_PER_RUN} helpers, the limit."
                    ));
                }
                let project = a.project.clone().unwrap_or_else(|| rec.project.clone());
                if self.project_path(&project).is_none() {
                    return Err(format!(
                        "Give `project` as a key in this session's scope ({}): `{project}` is not one.",
                        self.scope.join(", ")
                    ));
                }
                AskTarget::Agent { agent, project }
            }
            None => {
                let raw: ExecutionId = a.subagent_id.clone().unwrap_or_default().into();
                if !self.executions.contains_key(&raw) {
                    return Err(format!(
                        "Ask a subagent {SUBAGENT_LIST} shows: `{raw}` is not a subagent of this session."
                    ));
                }
                let subagent = self.subagent_of(&raw);
                if subagent == self.subagent_of(exec) {
                    return Err("Ask another subagent: this ID is your own.".into());
                }
                AskTarget::Subagent { id: subagent }
            }
        };
        let who = match &target {
            AskTarget::Agent { agent, .. } => format!("the {agent} helper"),
            AskTarget::Subagent { id } => self.describe(id),
        };
        Ok((
            SessionEvent::AgentAsked {
                id: MessageId::new(),
                from: exec.clone(),
                target,
                message: a.message,
            },
            who,
        ))
    }

    /// Check a `SubagentReply` from `exec` and build its event and what it does to the run.
    pub fn reply_event(
        &self,
        exec: &ExecutionId,
        input: &serde_json::Value,
    ) -> Result<(SessionEvent, RunEnd), String> {
        let r = ReplyInput::parse(input)?;
        let rec = self.running_rec(exec)?;
        let Some(ask) = self.owed_by(exec) else {
            return Err(format!(
                "No question waits for your answer, so there is nothing to reply to. Use {SUBAGENT_ASK} to ask, or go on with your task."
            ));
        };
        let end = if matches!(rec.purpose, ExecPurpose::Consult { .. }) {
            RunEnd::Finish
        } else {
            RunEnd::Wait
        };
        Ok((
            SessionEvent::AgentReplied {
                ask: ask.id.clone(),
                from: exec.clone(),
                message: r.message,
            },
            end,
        ))
    }

    fn running_rec(&self, exec: &ExecutionId) -> Result<&ExecRecord, String> {
        self.executions
            .get(exec)
            .filter(|r| r.result.is_none())
            .ok_or_else(|| "This run is not running in its session any more.".to_string())
    }

    /// The `SubagentList` text for `exec`.
    pub fn subagent_list(&self, exec: &ExecutionId) -> String {
        let me = self.subagent_of(exec);
        let partners = self.partners(exec);
        let mut out =
            format!("Your subagent ID: {me}\n\nSubagents of this session, oldest first:\n");
        let mut roots: Vec<&ExecutionId> = self
            .executions
            .keys()
            .filter(|id| self.subagent_of(id) == **id)
            .collect();
        roots.sort();
        for root in roots {
            let (Some(first), Some(head)) = (self.executions.get(root), self.head(root)) else {
                continue;
            };
            let status = if self.is_waiting(&head.id) {
                match self.waits_on(&head.id) {
                    Some(on) => format!("waiting on {on}"),
                    None => "waiting on a helper".into(),
                }
            } else {
                match &head.result {
                    None => "running".into(),
                    Some(r) => format!("{:?}", r.status).to_lowercase(),
                }
            };
            let mut line = format!(
                "- {root}: {}, {}, project {}, {status}",
                first.agent,
                first.purpose.run_label(),
                first.project
            );
            if let Some(p) = &head.report_path {
                line.push_str(&format!(", report {}", p.display()));
            }
            if *root == me {
                line.push_str(" (you)");
            } else if let Some(role) = partners.iter().find(|(id, _)| id == root).map(|(_, r)| r) {
                line.push_str(&format!(" ({role})"));
            }
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str(&format!(
            "\nAsk a subagent with {SUBAGENT_ASK} and `subagent_id`, or start a helper with `agent`: {}.\n",
            ostra_core::coord::HELPER_AGENTS
                .iter()
                .map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out
    }

    /// The subagents a run works with, and their role toward it.
    fn partners(&self, exec: &ExecutionId) -> Vec<(ExecutionId, &'static str)> {
        let Some(rec) = self.executions.get(exec) else {
            return vec![];
        };
        let latest = |f: &dyn Fn(&ExecRecord) -> bool| {
            self.executions
                .values()
                .filter(|r| f(r))
                .max_by_key(|r| r.id.clone())
                .map(|r| self.subagent_of(&r.id))
        };
        let mut out = vec![];
        let checked = |target: FactTarget| -> Option<ExecutionId> {
            latest(
                &|r| matches!(&r.purpose, ExecPurpose::FactCheck { target: t, .. } if *t == target),
            )
        };
        match &rec.purpose {
            ExecPurpose::Spec { .. } => {
                out.extend(checked(FactTarget::Spec).map(|c| (c, "your fact checker")))
            }
            ExecPurpose::Plan { .. } => {
                out.extend(checked(FactTarget::Plan).map(|c| (c, "your fact checker")))
            }
            ExecPurpose::FactCheck { target, .. } => {
                let author = match target {
                    FactTarget::Spec => latest(&|r| matches!(r.purpose, ExecPurpose::Spec { .. })),
                    FactTarget::Plan => latest(&|r| matches!(r.purpose, ExecPurpose::Plan { .. })),
                };
                out.extend(author.map(|a| (a, "the author of the document you check")));
            }
            _ if rec.loop_key.is_some() => {
                let key = rec.loop_key;
                let review = matches!(rec.purpose, ExecPurpose::Review { .. });
                let other = latest(&|r| {
                    r.loop_key == key && matches!(r.purpose, ExecPurpose::Review { .. }) != review
                });
                out.extend(other.map(|o| {
                    (
                        o,
                        if review {
                            "the implementer of the phase you review"
                        } else {
                            "the reviewer of your phase"
                        },
                    )
                }));
            }
            _ => {}
        }
        if let Some(ask) = self.owed_by(exec) {
            out.push((
                self.subagent_of(&ask.from),
                "asked you the question you answer",
            ));
        }
        out
    }

    /// A purpose key a waiting run blocks.
    pub(crate) fn waiting_key(rec: &ExecRecord) -> String {
        purpose_key(&rec.purpose)
    }
}

/// What a coordination call returns to the model and does to its run.
pub fn reply(text: String, end: RunEnd) -> CoordReply {
    CoordReply { text, end }
}
