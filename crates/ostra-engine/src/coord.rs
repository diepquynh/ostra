//! Messaging in the fold (HANDOVER 10.8): the messages subagents send each other, which run waits,
//! and what each run is handed at its next turn boundary. Everything here is a pure function of the
//! log.

use crate::state::{ExecRecord, ExploreOrigin, SessionState, purpose_key};
use chrono::{DateTime, Utc};
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::coord::{
    LIST_AGENTS, MAX_CONVERSATION_RUNS, MAX_HELPERS_PER_RUN, MAX_SESSION_MESSAGES, MessageKind,
    MessageTarget, SEND_MESSAGE, SendInput,
};
use ostra_core::event::{ExecPurpose, FactTarget, SessionEvent};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, MessageId};
use std::path::PathBuf;

/// One message between subagents.
#[derive(Debug, Clone)]
pub struct Message {
    pub id: MessageId,
    /// The run that sent it, or the helper run whose result it carries.
    pub from: ExecutionId,
    pub target: MessageTarget,
    /// The receiving subagent. A message to a new helper has none until the helper starts.
    pub to: Option<ExecutionId>,
    pub text: String,
    pub kind: MessageKind,
    /// The sender paused after sending it.
    pub wait: bool,
    pub at: DateTime<Utc>,
    /// The helper research task a message to `explore` started.
    pub explore_task: Option<u32>,
    /// The run it was handed to.
    pub delivered_to: Option<ExecutionId>,
}

/// What a paused run waits for (Rule SM3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOn {
    /// A message from this subagent, or any message.
    Subagent(ExecutionId),
    /// The helper the message started, once it starts.
    Helper(MessageId),
    /// Any message.
    Any,
}

/// A message hand-over ready for a run (Rule SM2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub ids: Vec<MessageId>,
    /// Rule SM3: why no message will come to a waiting run.
    pub notice: Option<String>,
    /// The text the run reads.
    pub note: String,
    /// Rule SM6: the senders that wait for this run's reply.
    pub owes: Vec<ExecutionId>,
}

/// The header of a run that continues its conversation for a pair loop (Rule H5).
pub fn continuation_note(block: &str) -> String {
    format!(
        "Ostra continues your conversation for the next round of the same work, because you already know it. \
         Everything you read and wrote earlier still holds unless the lines below change it. Work from this spawn \
         block, which lists what the round needs, and end with your submit call as before.\n\n{block}"
    )
}

/// The header of a run that continues an ended conversation for its messages (Rule SM4).
pub fn message_continuation_note(messages: &str) -> String {
    format!(
        "Ostra continues your conversation because other subagents sent you messages after your run ended. \
         Your earlier work and your tools are as before. Do what the messages ask, reply where a sender waits, \
         and end with your submit call, which records your work again.\n\n{messages}"
    )
}

fn ended_ok(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
    )
}

impl SessionState {
    // -----------------------------------------------------------------------------------------
    // Subagents (Rule SM1)
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

    fn message(&self, id: &MessageId) -> Option<&Message> {
        self.messages.iter().find(|m| &m.id == id)
    }

    fn message_mut(&mut self, id: &MessageId) -> Option<&mut Message> {
        self.messages.iter_mut().find(|m| &m.id == id)
    }

    /// Rule SM3: the run paused itself and no message has woken it yet, ended (native) or alive
    /// (harness).
    pub fn is_waiting(&self, exec: &ExecutionId) -> bool {
        self.waits.contains_key(exec)
            && self.executions.get(exec).is_some_and(|r| {
                r.result
                    .as_ref()
                    .is_none_or(|r| r.status == ExecutionStatus::Waiting)
            })
    }

    /// The subagent a waiting run waits on, when it waits on one.
    fn waits_on(&self, exec: &ExecutionId) -> Option<ExecutionId> {
        match self.waits.get(exec)? {
            WaitOn::Subagent(s) => Some(s.clone()),
            WaitOn::Helper(m) => self.message(m).and_then(|m| m.to.clone()),
            WaitOn::Any => None,
        }
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

    /// Rule SM4: an ended subagent a message continues with its own tools.
    pub fn can_continue(&self, subagent: &ExecutionId) -> bool {
        let Some(h) = self.head(subagent) else {
            return false;
        };
        let Some(r) = &h.result else {
            return false;
        };
        ended_ok(r.status)
            && !matches!(h.purpose, ExecPurpose::Inspect { .. })
            && !(matches!(h.executor, ExecutorKind::Harness(_)) && r.native_session_id.is_none())
            && self.runs_of(subagent).count() < MAX_CONVERSATION_RUNS
    }

    /// Messages to `subagent` not handed over yet. A helper's result to a subagent that ended is
    /// dropped, because nothing waits for it.
    fn pending_for(&self, subagent: &ExecutionId) -> Vec<&Message> {
        let ended = self
            .head(subagent)
            .is_some_and(|h| h.result.is_some() && !self.is_waiting(&h.id));
        self.messages
            .iter()
            .filter(|m| m.to.as_ref() == Some(subagent) && m.delivered_to.is_none())
            .filter(|m| !(ended && m.kind == MessageKind::Result))
            .collect()
    }

    /// Rule SM3: why no message will come to waiting run `exec`, when none can.
    fn wait_notice(&self, exec: &ExecutionId) -> Option<String> {
        let me = self.subagent_of(exec);
        match self.waits.get(exec)? {
            WaitOn::Any => {
                let others_busy = self.executions.values().any(|r| {
                    self.subagent_of(&r.id) != me && (r.result.is_none() || self.is_waiting(&r.id))
                });
                let queued = self.messages.iter().any(|m| {
                    m.delivered_to.is_none()
                        && m.kind == MessageKind::Sent
                        && m.to
                            .as_ref()
                            .is_some_and(|t| *t != me && self.can_continue(t))
                });
                (!others_busy && !queued).then(|| {
                    "No other subagent of this session is running or waiting, so no message can come. Continue your task without one.".to_string()
                })
            }
            _ => {
                let on = self.waits_on(exec)?;
                let h = self.head(&on)?;
                let r = h.result.as_ref()?;
                if self.is_waiting(&h.id) {
                    return None;
                }
                let revived = self
                    .pending_for(&on)
                    .iter()
                    .any(|m| m.kind == MessageKind::Sent)
                    && self.can_continue(&on);
                if revived {
                    return None;
                }
                Some(format!(
                    "{} ended its run with status {:?} without sending you a message. Continue without its answer, or message another subagent.",
                    self.describe(&on),
                    r.status
                ))
            }
        }
    }

    /// Rule SM2: what run `exec` is handed at its next turn boundary, or to wake it.
    pub fn next_delivery(&self, exec: &ExecutionId) -> Option<Delivery> {
        let rec = self.executions.get(exec)?;
        let me = self.subagent_of(exec);
        if self.head(&me).map(|h| &h.id) != Some(exec) {
            return None;
        }
        let waiting = self.is_waiting(exec);
        if rec.result.is_some() && !waiting {
            return None;
        }
        let msgs = self.pending_for(&me);
        let notice = if msgs.is_empty() && waiting {
            self.wait_notice(exec)
        } else {
            None
        };
        if msgs.is_empty() && notice.is_none() {
            return None;
        }
        let owes = self.owers_after(&me, &msgs);
        Some(Delivery {
            ids: msgs.iter().map(|m| m.id.clone()).collect(),
            note: self.render(rec.executor, &msgs, notice.as_deref(), &owes),
            notice,
            owes,
        })
    }

    /// Rule SM4: the messages a run that continues ended subagent `subagent` starts with.
    pub fn continuation_delivery(
        &self,
        subagent: &ExecutionId,
        executor: ExecutorKind,
    ) -> Option<Delivery> {
        let msgs = self.pending_for(subagent);
        if !msgs.iter().any(|m| m.kind == MessageKind::Sent) {
            return None;
        }
        let owes = self.owers_after(subagent, &msgs);
        Some(Delivery {
            ids: msgs.iter().map(|m| m.id.clone()).collect(),
            note: self.render(executor, &msgs, None, &owes),
            notice: None,
            owes,
        })
    }

    /// The senders of `msgs` that wait for `me` to answer.
    fn owers_after(&self, me: &ExecutionId, msgs: &[&Message]) -> Vec<ExecutionId> {
        let mut out: Vec<ExecutionId> = vec![];
        for m in msgs
            .iter()
            .filter(|m| m.wait && m.kind == MessageKind::Sent)
        {
            let sender = self.subagent_of(&m.from);
            if !out.contains(&sender)
                && self
                    .head(&sender)
                    .is_some_and(|h| self.waits_on(&h.id).as_ref() == Some(me))
            {
                out.push(sender);
            }
        }
        out
    }

    fn render(
        &self,
        executor: ExecutorKind,
        msgs: &[&Message],
        notice: Option<&str>,
        owes: &[ExecutionId],
    ) -> String {
        let send = ostra_core::coord::send_tool(executor);
        let mut out = String::new();
        if !msgs.is_empty() {
            out.push_str(&format!(
                "Ostra hands you {} that arrived for you. Act on {}, then continue your task.\n",
                if msgs.len() == 1 {
                    "a message".to_string()
                } else {
                    format!("{} messages", msgs.len())
                },
                if msgs.len() == 1 { "it" } else { "them" }
            ));
        }
        for m in msgs {
            let sender = self.subagent_of(&m.from);
            let head = match m.kind {
                MessageKind::Result => format!("The result of {}", self.describe(&sender)),
                MessageKind::Sent => format!("Message from {}", self.describe(&sender)),
            };
            let reply = if owes.contains(&sender) {
                format!(
                    ". It waits for your reply: answer with {send} and `to: \"{sender}\"` before you submit"
                )
            } else {
                String::new()
            };
            out.push_str(&format!("\n{head}{reply}:\n{}\n", m.text.trim_end()));
        }
        if let Some(n) = notice {
            out.push_str(n);
            out.push('\n');
        }
        if let Some(first) = msgs.iter().map(|m| m.at).min() {
            out.push_str(&self.amended_since(first));
        }
        out
    }

    fn amended_since(&self, at: DateTime<Utc>) -> String {
        let added: Vec<&str> = self
            .amendments
            .iter()
            .filter(|a| a.at > at && a.delivered)
            .map(|a| a.text.as_str())
            .collect();
        if added.is_empty() {
            return String::new();
        }
        format!(
            "\nThe user added to the request while you waited. Take it into account:\n{}\n",
            added.join("\n")
        )
    }

    /// Rule SM6: the subagents that wait for run `exec`'s reply and have not had one since.
    pub fn owed_by(&self, exec: &ExecutionId) -> Vec<ExecutionId> {
        let me = self.subagent_of(exec);
        let mut out: Vec<ExecutionId> = vec![];
        for (i, m) in self.messages.iter().enumerate() {
            if m.delivered_to.as_ref() != Some(exec) || !m.wait || m.kind != MessageKind::Sent {
                continue;
            }
            let sender = self.subagent_of(&m.from);
            let still_waits = self
                .head(&sender)
                .is_some_and(|h| self.waits_on(&h.id).as_ref() == Some(&me));
            let answered = self.messages[i + 1..].iter().any(|r| {
                r.kind == MessageKind::Sent
                    && self.subagent_of(&r.from) == me
                    && r.to.as_ref() == Some(&sender)
            });
            if still_waits && !answered && !out.contains(&sender) {
                out.push(sender);
            }
        }
        out
    }

    /// Rule SM6: why run `exec` may not submit yet.
    pub fn submit_blocked(&self, exec: &ExecutionId) -> Option<String> {
        let owed = self.owed_by(exec);
        if owed.is_empty() {
            return None;
        }
        let executor = self.executions.get(exec)?.executor;
        Some(ostra_core::coord::reply_instruction(
            &ostra_core::coord::send_tool(executor),
            &owed.iter().map(|o| o.to_string()).collect::<Vec<_>>(),
        ))
    }

    /// Rule SM9: a message is still to be handed over, or a subagent waits.
    pub fn coordination_open(&self) -> bool {
        self.waits.keys().any(|e| self.is_waiting(e))
            || self.messages.iter().any(|m| {
                m.delivered_to.is_none()
                    && m.kind == MessageKind::Sent
                    && m.to.as_ref().is_some_and(|t| {
                        self.subagent_live(t)
                            || self.head(t).is_some_and(|h| self.is_waiting(&h.id))
                            || self.can_continue(t)
                    })
            })
            || self.helpers_due().next().is_some()
    }

    /// Rule SM4: ended subagents with messages to act on, and the first of those messages.
    pub fn continuations_due(&self) -> Vec<(ExecutionId, MessageId)> {
        let mut roots: Vec<ExecutionId> = self
            .executions
            .keys()
            .filter(|id| self.subagent_of(id) == **id)
            .cloned()
            .collect();
        roots.sort();
        roots
            .into_iter()
            .filter(|s| !self.subagent_live(s) && self.can_continue(s))
            .filter_map(|s| {
                let first = self
                    .pending_for(&s)
                    .into_iter()
                    .find(|m| m.kind == MessageKind::Sent)?
                    .id
                    .clone();
                Some((self.head(&s)?.id.clone(), first))
            })
            .collect()
    }

    /// Rule SM7: messages to a custom helper agent whose helper has not started.
    pub fn helpers_due(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter().filter(|m| {
            m.to.is_none()
                && m.explore_task.is_none()
                && matches!(m.target, MessageTarget::Agent { .. })
        })
    }

    /// Waiting native runs a message wakes now (Rule SM3).
    pub fn wakes_due(&self) -> Vec<ExecutionId> {
        self.executions
            .values()
            .filter(|r| {
                r.result
                    .as_ref()
                    .is_some_and(|x| x.status == ExecutionStatus::Waiting)
                    && self.next_delivery(&r.id).is_some()
            })
            .map(|r| r.id.clone())
            .collect()
    }

    /// The message a helper run serves.
    pub fn helper_message(&self, rec: &ExecRecord) -> Option<&Message> {
        match &rec.purpose {
            ExecPurpose::Helper { message } => self.message(message),
            ExecPurpose::Explore { task } => match &self.explore.get(*task as usize)?.origin {
                ExploreOrigin::Ask { ask } => self.message(ask),
                _ => None,
            },
            _ => None,
        }
    }

    // -----------------------------------------------------------------------------------------
    // Fold
    // -----------------------------------------------------------------------------------------

    fn add_message(
        &mut self,
        id: &MessageId,
        from: &ExecutionId,
        target: &MessageTarget,
        text: &str,
        wait: bool,
        at: DateTime<Utc>,
    ) {
        let (to, explore_task) = match target {
            MessageTarget::Subagent { id: s } => (Some(self.subagent_of(s)), None),
            // Rule SM7: a research helper runs as a research task, whose document joins the
            // session's research. Logs from before contracts name only `explore`.
            MessageTarget::Agent {
                agent,
                project,
                contract,
            } if contract.map_or(*agent == AgentName::Explore, |c| c == Contract::Research) => {
                let task = format!(
                    "{} asks for this research and waits for your findings:\n{text}",
                    self.describe(&self.subagent_of(from))
                );
                let t = self.push_explore(
                    project.clone(),
                    task,
                    ExploreOrigin::Ask { ask: id.clone() },
                );
                if let Some(x) = self.explore.get_mut(t as usize) {
                    x.agent = Some(*agent);
                }
                (None, Some(t))
            }
            MessageTarget::Agent { .. } => (None, None),
        };
        if wait {
            let on = match target {
                MessageTarget::Subagent { .. } => WaitOn::Subagent(to.clone().unwrap_or_default()),
                MessageTarget::Agent { .. } => WaitOn::Helper(id.clone()),
            };
            self.waits.insert(from.clone(), on);
        }
        self.messages.push(Message {
            id: id.clone(),
            from: from.clone(),
            target: target.clone(),
            to,
            text: text.to_string(),
            kind: MessageKind::Sent,
            wait,
            at,
            explore_task,
            delivered_to: None,
        });
    }

    fn deliver(&mut self, id: &MessageId, to: &ExecutionId) {
        if let Some(m) = self.message_mut(id)
            && m.delivered_to.is_none()
        {
            m.delivered_to = Some(to.clone());
        }
    }

    pub(crate) fn on_coord_event(&mut self, event: &SessionEvent, at: DateTime<Utc>) {
        match event {
            SessionEvent::MessageSent {
                id,
                from,
                to,
                text,
                wait,
            } => self.add_message(id, from, to, text, *wait, at),
            SessionEvent::AgentWaiting { id } => {
                self.waits.insert(id.clone(), WaitOn::Any);
            }
            SessionEvent::MessagesDelivered { to, ids, .. } => {
                for id in ids {
                    self.deliver(id, to);
                }
                self.waits.remove(to);
            }
            // Logs written before messaging: a question waited, and an answer was a message back.
            SessionEvent::AgentAsked {
                id,
                from,
                target,
                message,
            } => self.add_message(id, from, target, message, true, at),
            SessionEvent::AgentReplied { ask, from, message } => {
                let Some(asker) = self.message(ask).map(|m| self.subagent_of(&m.from)) else {
                    return;
                };
                self.messages.push(Message {
                    id: reply_id(ask),
                    from: from.clone(),
                    target: MessageTarget::Subagent { id: asker.clone() },
                    to: Some(asker),
                    text: message.clone(),
                    kind: MessageKind::Sent,
                    wait: false,
                    at,
                    explore_task: None,
                    delivered_to: None,
                });
            }
            SessionEvent::MessageDelivered { ask, to, kind } => match kind {
                ostra_core::coord::DeliveryKind::Question => self.deliver(ask, to),
                ostra_core::coord::DeliveryKind::Answer => {
                    for id in [reply_id(ask), result_id(ask)] {
                        self.deliver(&id, to);
                    }
                    self.waits.remove(to);
                }
            },
            _ => {}
        }
    }

    /// A helper or a pre-messaging consult run got its message as its spawn.
    pub(crate) fn coord_started(&mut self, id: &ExecutionId, purpose: &ExecPurpose) {
        let root = self.subagent_of(id);
        match purpose {
            ExecPurpose::Consult { ask, .. } => self.deliver(ask, id),
            ExecPurpose::Helper { message } => {
                let message = message.clone();
                if let Some(m) = self.message_mut(&message) {
                    m.to = Some(root);
                    m.delivered_to = Some(id.clone());
                }
            }
            ExecPurpose::Explore { task } => {
                if let Some(ExploreOrigin::Ask { ask }) =
                    self.explore.get(*task as usize).map(|t| t.origin.clone())
                    && let Some(m) = self.message_mut(&ask)
                    && m.to.is_none()
                {
                    m.to = Some(root);
                    m.delivered_to = Some(id.clone());
                }
            }
            _ => {}
        }
    }

    /// A run ended: it no longer waits, and a helper's result goes to the run that started it.
    pub(crate) fn coord_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        if result.status != ExecutionStatus::Waiting {
            self.waits.remove(&rec.id);
        }
        let Some(asked) = self.helper_message(rec).cloned() else {
            return;
        };
        let text = match (&rec.purpose, result.status) {
            (ExecPurpose::Explore { task }, ExecutionStatus::Ok) => {
                let Some(sub) = self
                    .explore
                    .get(*task as usize)
                    .and_then(|t| t.result.clone())
                else {
                    return;
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
            // Interrupted or waiting for its own message: the helper is not done yet.
            (_, ExecutionStatus::Interrupted | ExecutionStatus::Waiting) => return,
            // An explore that errors is retried once before it counts as failed.
            (ExecPurpose::Explore { task }, _)
                if self
                    .explore
                    .get(*task as usize)
                    .is_some_and(|t| t.failed.is_none() && !t.abandoned) =>
            {
                return;
            }
            // Rule PL5: a plugin contract's result reaches the starter as its plugin handled it.
            (ExecPurpose::Helper { .. }, s)
                if ended_ok(s) && matches!(rec.contract, Contract::Plugin(_)) =>
            {
                match &rec.handled {
                    Some(h) => render_submit(&serde_json::to_value(h).unwrap_or_default()),
                    None => return,
                }
            }
            (ExecPurpose::Helper { .. }, s) if ended_ok(s) => match &result.submit {
                Some(v) => render_submit(v),
                None => "The helper ended without a result.".into(),
            },
            (_, status) => format!(
                "The helper ended its run with status {status:?} without a result. Continue without it, or ask again."
            ),
        };
        let id = result_id(&asked.id);
        if self.message(&id).is_some() {
            return;
        }
        let to = self.subagent_of(&asked.from);
        self.messages.push(Message {
            id,
            from: rec.id.clone(),
            target: MessageTarget::Subagent { id: to.clone() },
            to: Some(to),
            text,
            kind: MessageKind::Result,
            wait: false,
            at: rec.ended_at.unwrap_or(asked.at),
            explore_task: None,
            delivered_to: None,
        });
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
            // Rule WF5: the next round of a workflow stage continues its agent's conversation.
            ExecPurpose::Stage { node, scope, round } if *round > 1 => {
                let (n, sc) = (node.clone(), scope.clone());
                last(
                    &|p| matches!(p, ExecPurpose::Stage { node, scope, .. } if *node == n && *scope == sc),
                )
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
            || self
                .amendments
                .iter()
                .any(|a| a.delivered && a.at > prev_rec.started_at)
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

    /// The purpose key of a waiting native run: `Some(None)` holds its stage's spawn, and
    /// `Some(Some(run))` wakes the run with its messages.
    pub fn wake_for_key(&self, key: &str) -> Option<Option<ExecutionId>> {
        let exec = self.waiting_keys.get(key)?;
        Some(self.next_delivery(exec).map(|_| exec.clone()))
    }

    // -----------------------------------------------------------------------------------------
    // Tool calls (Rule SM5)
    // -----------------------------------------------------------------------------------------

    /// Check a `SendMessage` from `exec` and build its event. `helpers` are the agents the
    /// workspace lets `SendMessage` start, with the contract each returns.
    pub fn send_event(
        &self,
        exec: &ExecutionId,
        input: &serde_json::Value,
        helpers: &[(AgentName, Contract)],
    ) -> Result<(SessionEvent, String), String> {
        let a = SendInput::parse(input)?;
        let rec = self.running_rec(exec)?;
        let sent = self
            .messages
            .iter()
            .filter(|m| m.kind == MessageKind::Sent)
            .count();
        if sent >= MAX_SESSION_MESSAGES {
            return Err(format!(
                "Continue without sending: this session reached its limit of {MAX_SESSION_MESSAGES} messages between subagents."
            ));
        }
        let me = self.subagent_of(exec);
        let target = match (&a.to, &a.agent) {
            (_, Some(name)) => {
                let (agent, contract) = name
                    .parse::<AgentName>()
                    .ok()
                    .and_then(|a| helpers.iter().find(|(h, _)| *h == a).copied())
                    .ok_or_else(|| {
                        format!(
                            "Start a helper of an agent Ostra can start for you: {}. `{name}` is not one; message an existing subagent with `to` instead.",
                            helpers
                                .iter()
                                .map(|(a, _)| format!("`{a}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?;
                if self.helper_message(rec).is_some() {
                    return Err(
                        "Do the work yourself: a helper may not start helpers of its own.".into(),
                    );
                }
                let started = self
                    .messages
                    .iter()
                    .filter(|x| &x.from == exec && matches!(x.target, MessageTarget::Agent { .. }))
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
                MessageTarget::Agent {
                    agent,
                    project,
                    contract: Some(contract),
                }
            }
            (Some(raw), None) => {
                let raw: ExecutionId = raw.clone().into();
                if !self.executions.contains_key(&raw) {
                    return Err(format!(
                        "Message a subagent {LIST_AGENTS} shows: `{raw}` is not a subagent of this session."
                    ));
                }
                let to = self.subagent_of(&raw);
                if to == me {
                    return Err("Message another subagent: this ID is your own.".into());
                }
                let h = self.head(&to).ok_or("That subagent has no run.")?;
                if let Some(r) = &h.result
                    && !self.is_waiting(&h.id)
                    && !self.can_continue(&to)
                {
                    return Err(format!(
                        "{} ended with status {:?} and cannot take messages any more, so nothing reads this one. Continue without it, or message another subagent.",
                        self.describe(&to),
                        r.status
                    ));
                }
                MessageTarget::Subagent { id: to }
            }
            (None, None) => unreachable!("SendInput::parse requires a target"),
        };
        let who = match &target {
            MessageTarget::Agent { agent, .. } => format!("a new {agent} helper"),
            MessageTarget::Subagent { id } => self.describe(id),
        };
        Ok((
            SessionEvent::MessageSent {
                id: MessageId::new(),
                from: exec.clone(),
                to: target,
                text: a.message,
                wait: a.wait,
            },
            who,
        ))
    }

    /// Check a `WaitForMessage` from `exec` and build its event.
    pub fn wait_event(&self, exec: &ExecutionId) -> Result<SessionEvent, String> {
        self.running_rec(exec)?;
        if let Some(why) = self.submit_blocked(exec) {
            return Err(why);
        }
        Ok(SessionEvent::AgentWaiting { id: exec.clone() })
    }

    fn running_rec(&self, exec: &ExecutionId) -> Result<&ExecRecord, String> {
        self.executions
            .get(exec)
            .filter(|r| r.result.is_none())
            .ok_or_else(|| "This run is not running in its session any more.".to_string())
    }

    /// The `ListAgents` text for `exec`.
    pub fn list_agents(&self, exec: &ExecutionId, helpers: &[(AgentName, Contract)]) -> String {
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
                    None => "waiting for a message".into(),
                }
            } else {
                match &head.result {
                    None => "running".into(),
                    Some(r) if self.can_continue(root) => {
                        format!(
                            "{} (a message continues it)",
                            format!("{:?}", r.status).to_lowercase()
                        )
                    }
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
            "\nMessage a subagent with {SEND_MESSAGE} and `to`, or start a helper with `agent`: {}.\n",
            helpers
                .iter()
                .map(|(a, _)| a.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        out
    }

    /// The subagents a run works with, and their role toward it.
    fn partners(&self, exec: &ExecutionId) -> Vec<(ExecutionId, String)> {
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
        let mut out: Vec<(ExecutionId, String)> = vec![];
        let checked = |target: FactTarget| -> Option<ExecutionId> {
            latest(
                &|r| matches!(&r.purpose, ExecPurpose::FactCheck { target: t, .. } if *t == target),
            )
        };
        match &rec.purpose {
            ExecPurpose::Spec { .. } => {
                out.extend(checked(FactTarget::Spec).map(|c| (c, "your fact checker".to_string())))
            }
            ExecPurpose::Plan { .. } => {
                out.extend(checked(FactTarget::Plan).map(|c| (c, "your fact checker".to_string())))
            }
            ExecPurpose::FactCheck { target, .. } => {
                let author = match target {
                    FactTarget::Spec => latest(&|r| matches!(r.purpose, ExecPurpose::Spec { .. })),
                    FactTarget::Plan => latest(&|r| matches!(r.purpose, ExecPurpose::Plan { .. })),
                };
                out.extend(author.map(|a| (a, "the author of the document you check".to_string())));
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
                        }
                        .to_string(),
                    )
                }));
            }
            _ => {}
        }
        // Rule WF6: a workflow stage names the subagents of the stages it reads.
        out.extend(self.stage_partners(rec));
        for sender in self.owed_by(exec) {
            if !out.iter().any(|(id, _)| *id == sender) {
                out.push((sender, "waits for your reply".to_string()));
            }
        }
        out
    }

    /// A purpose key a waiting run blocks.
    pub(crate) fn waiting_key(rec: &ExecRecord) -> String {
        purpose_key(&rec.purpose)
    }
}

/// The id of the reply to question `ask` in a log written before messaging.
fn reply_id(ask: &MessageId) -> MessageId {
    MessageId::from(format!("{ask}-reply"))
}

/// The id of the result a helper started by message `id` returns.
fn result_id(id: &MessageId) -> MessageId {
    MessageId::from(format!("{id}-result"))
}

/// A helper's submit as the message its starter reads.
fn render_submit(v: &serde_json::Value) -> String {
    if let Ok(c) = serde_json::from_value::<ostra_core::submit::CustomSubmit>(v.clone()) {
        let verdict = serde_json::to_value(c.verdict)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let mut s = format!("Verdict: {verdict}\n{}", c.summary);
        for f in &c.findings {
            s.push_str(&format!(
                "\n- {}{}{}",
                f.file
                    .as_deref()
                    .map(|p| format!("{p}: "))
                    .unwrap_or_default(),
                f.description,
                f.fix
                    .as_deref()
                    .map(|x| format!(" Fix: {x}"))
                    .unwrap_or_default()
            ));
        }
        if let Some(p) = &c.report_path {
            s.push_str(&format!("\nReport: {p}"));
        }
        if let Some(d) = &c.data {
            s.push_str(&format!("\nData: {d}"));
        }
        return s;
    }
    serde_json::to_string_pretty(v).unwrap_or_default()
}
