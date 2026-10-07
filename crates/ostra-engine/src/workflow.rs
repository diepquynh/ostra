//! Workflows (HANDOVER 10.9): the pipeline as a graph of stages, built-in and custom. The fold
//! keeps each custom stage's runs and outcome; the planner walks the graph and runs every stage
//! whose stages before it are done.

use crate::plan::{SpawnInputs, SpawnRequest, Step};
use crate::state::{ExecRecord, SessionState};
use ostra_agents::spawn::{Common, CustomParams};
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::event::{ExecPurpose, GateAnswer, GatePayload};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::pipeline::Category;
use ostra_core::submit::{CustomSubmit, StageVerdict};
use ostra_core::transform::{SCOPE_REF, SESSION_REF, WhenMode, lookup, parse_ref};
use ostra_core::workflow::{
    BuiltinStage, OnFail, StageDef, StageRun, StageScope, WorkflowChoice, WorkflowDef,
};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// What became of one custom stage instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    Passed,
    /// It failed and the workflow went on (`on_fail = "continue"`, or the user's answer).
    FailedOn,
    /// The session stops (`on_fail = "fail"`, or the user's answer).
    Stopped(String),
    /// Rule WB5: its conditions did not hold, so it did not run. Counts as done.
    Skipped,
}

/// What a custom stage instance waits for before its next run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageNext {
    /// The next round, with what it must take into account.
    Retry(Option<String>),
    /// A user's decision (Rule WF5).
    Gate,
}

/// One custom stage, once for the session or once per phase or project (its `scope`).
#[derive(Debug, Clone, Default)]
pub struct StageTrack {
    pub runs: Vec<ExecutionId>,
    pub round: u32,
    pub outcome: Option<StageOutcome>,
    pub next: Option<StageNext>,
    pub gate: Option<GateId>,
    /// The ExecutionFailed gate of its failed run.
    pub failed_gate: Option<GateId>,
    /// The last run's submit.
    pub last: Option<CustomSubmit>,
    /// What the user said at its gates, oldest first.
    pub notes: Vec<String>,
    /// Rule PL3: a plugin stage's decisions, oldest first.
    pub decisions: Vec<ostra_core::plugin::StageDecision>,
    /// How many runs the stage had when its plugin last decided.
    pub decided_runs: usize,
    /// A run the plugin decided on that has not started yet.
    pub pending_run: Option<(String, Option<String>)>,
    /// The question a plugin stage asks the user.
    pub question: Option<(String, Vec<String>)>,
    /// Rules WB2 and WB3: a transform or prompt node's output.
    pub output: Option<Value>,
}

/// The fold key of a stage instance.
pub fn stage_key(node: &str, scope: Option<&str>) -> String {
    format!("{node}|{}", scope.unwrap_or_default())
}

/// Rule WF4: the spawn of a custom agent's stage.
pub fn custom_params(
    req: &SpawnRequest,
    s: &SessionState,
    common: Common,
) -> Result<CustomParams, String> {
    let i = &req.inputs;
    Ok(CustomParams {
        common,
        agent: req.agent,
        stage: i.stage_id.clone().ok_or("missing stage")?,
        task: i.task.clone().unwrap_or_else(|| s.full_request()),
        instructions: i.instructions.clone(),
        round: i.stage_round.unwrap_or(1),
        phase: i.phase.as_ref().map(|p| p.id.to_string()),
        phase_file: i.phase.as_ref().and_then(|p| p.file.clone()),
        implementer_report: i.implementer_report.clone(),
        inputs: i.stage_inputs.clone(),
        spec_file: i.spec_file.clone(),
        master_plan: i.target.clone(),
        research_docs: i.research_docs.clone(),
        earlier_stages: i.earlier_stages.clone(),
        prior_findings: i.prior_findings.clone(),
        report_file: i.report_file.clone(),
        user_notes: i.user_notes.clone(),
    })
}

fn render_findings(c: &CustomSubmit) -> String {
    let mut out = c.summary.clone();
    for f in &c.findings {
        out.push_str(&format!(
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
    out
}

impl SessionState {
    /// Rule WF8: the agent that fills `contract` in built-in stage `stage`: the workflow's
    /// binding, else the standard plugin's agent for that contract (Rule PL4).
    pub fn agent_for(&self, stage: BuiltinStage, contract: Contract) -> AgentName {
        self.workflow
            .as_ref()
            .and_then(|w| w.stages.iter().find(|d| d.builtin() == Some(stage)))
            .and_then(|d| d.agents.get(&contract).copied())
            .unwrap_or_else(|| self.pipeline.default_agent(contract))
    }

    /// Rule PL4: the standard plugin's agent for a contract no built-in stage binds, such as a
    /// quick answer or a project's setup.
    pub fn default_agent(&self, contract: Contract) -> AgentName {
        self.pipeline.default_agent(contract)
    }

    /// Rule PL5: record what a plugin's handler made of a run's result, and apply it where the
    /// run's verdict would apply.
    pub fn on_result_handled(&mut self, execution: &ExecutionId, outcome: &CustomSubmit) {
        let Some(rec) = self.executions.get_mut(execution) else {
            return;
        };
        rec.handled = Some(outcome.clone());
        let rec = rec.clone();
        if let Some(result) = rec.result.clone() {
            self.stage_outcome(&rec, &result, Some(outcome.clone()));
            self.coord_finished(&rec, &result);
        }
    }

    /// Rule PL5: ended runs of a plugin contract whose result its plugin has not handled yet.
    pub fn results_due(&self) -> Vec<ExecutionId> {
        self.executions
            .values()
            .filter(|r| matches!(r.contract, Contract::Plugin(_)) && r.handled.is_none())
            .filter(|r| {
                r.result.as_ref().is_some_and(|x| {
                    x.submit.is_some()
                        && matches!(
                            x.status,
                            ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
                        )
                })
            })
            .filter(|r| !matches!(r.purpose, ExecPurpose::Message { .. }))
            .map(|r| r.id.clone())
            .collect()
    }

    /// Rule PL5: what the plugin's handler is shown for run `exec`.
    pub fn result_view(
        &self,
        exec: &ExecutionId,
    ) -> Option<(String, ostra_core::plugin::ResultView)> {
        let rec = self.executions.get(exec)?;
        let Contract::Plugin(c) = rec.contract else {
            return None;
        };
        let (stage, scope) = match &rec.purpose {
            ExecPurpose::Stage { node, scope, .. } => (Some(node.clone()), scope.clone()),
            _ => (None, None),
        };
        Some((
            c.plugin().to_string(),
            ostra_core::plugin::ResultView {
                session: self.id.clone(),
                execution: exec.clone(),
                agent: rec.agent.to_string(),
                contract: c.name().to_string(),
                stage,
                scope,
                submit: rec.result.as_ref()?.submit.clone()?,
            },
        ))
    }

    /// The workflow this session runs: the one it recorded, else the built-in pipeline of its
    /// category (logs written before workflows).
    pub fn active_workflow(&self) -> Option<WorkflowDef> {
        if let Some(w) = &self.workflow {
            return Some(w.clone());
        }
        let c = self.category?;
        (c != Category::QuickAnswer).then(|| self.pipeline.default_workflow(c))
    }

    /// Rule WF1: the session waits for its workflow to be resolved and recorded.
    pub fn workflow_due(&self) -> Option<Step> {
        if self.workflow.is_some() {
            return None;
        }
        match self.workflow_choice.as_ref()? {
            WorkflowChoice::Named { name } => Some(Step::ResolveWorkflow {
                name: Some(name.clone()),
                category: None,
            }),
            WorkflowChoice::ByCategory => match self.category? {
                Category::QuickAnswer => None,
                c => Some(Step::ResolveWorkflow {
                    name: None,
                    category: Some(c),
                }),
            },
        }
    }

    pub fn on_workflow_resolved(&mut self, wf: &WorkflowDef) {
        self.workflow = Some(wf.clone());
        self.pipeline.clone().workflow_resolved(self, wf);
        // Rule WF1: a named workflow's base is the session's category, whatever Classify picks.
        if matches!(self.workflow_choice, Some(WorkflowChoice::Named { .. }))
            && self.category.is_some()
        {
            self.category = Some(wf.base);
        }
    }

    /// Rule WF1: the category a named workflow forces on Classify's pick.
    pub fn forced_category(&self) -> Option<Category> {
        match self.workflow_choice {
            Some(WorkflowChoice::Named { .. }) => self.workflow.as_ref().map(|w| w.base),
            _ => None,
        }
    }

    pub fn stage_track(&self, node: &str, scope: Option<&str>) -> Option<&StageTrack> {
        self.stages.get(&stage_key(node, scope))
    }

    fn stage_def(&self, node: &str) -> Option<StageDef> {
        self.workflow.as_ref()?.stage(node).cloned()
    }

    pub fn stage_started(&mut self, id: &ExecutionId, purpose: &ExecPurpose) {
        if let ExecPurpose::Stage { node, scope, round } = purpose {
            let t = self
                .stages
                .entry(stage_key(node, scope.as_deref()))
                .or_default();
            t.runs.push(id.clone());
            t.round = *round;
            t.next = None;
            t.pending_run = None;
        }
    }

    pub fn stage_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        // Rule PL5: a plugin contract's result waits for its plugin's handler.
        if matches!(rec.contract, Contract::Plugin(_)) && rec.handled.is_none() {
            return;
        }
        self.stage_outcome(rec, result, rec.handled.clone());
    }

    /// Rule WF5: apply one stage run's outcome: its handled result, else its own submit.
    fn stage_outcome(
        &mut self,
        rec: &ExecRecord,
        result: &ExecutionResult,
        handled: Option<CustomSubmit>,
    ) {
        let ExecPurpose::Stage { node, scope, round } = &rec.purpose else {
            return;
        };
        if !matches!(
            result.status,
            ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
        ) {
            return;
        }
        let def = self.stage_def(node);
        let key = stage_key(node, scope.as_deref());
        let submit: Option<CustomSubmit> = handled.or_else(|| {
            result
                .submit
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        });
        // Rule PL3: a plugin stage's runs inform its plugin, which decides what follows.
        if def
            .as_ref()
            .is_some_and(|d| matches!(d.run, StageRun::Plugin { .. }))
        {
            self.stages.entry(key).or_default().last = submit;
            return;
        }
        let t = self.stages.entry(key).or_default();
        let Some(c) = submit else {
            t.next = Some(StageNext::Gate);
            return;
        };
        t.last = Some(c.clone());
        let (on_fail, max) = def
            .map(|d| (d.on_fail, d.max_rounds))
            .unwrap_or((OnFail::Gate, 1));
        match c.verdict {
            StageVerdict::Pass => t.outcome = Some(StageOutcome::Passed),
            StageVerdict::NeedsUser => t.next = Some(StageNext::Gate),
            StageVerdict::Fail => match on_fail {
                OnFail::Continue => t.outcome = Some(StageOutcome::FailedOn),
                OnFail::Fail => {
                    t.outcome = Some(StageOutcome::Stopped(format!(
                        "Stage `{node}` failed: {}",
                        c.summary
                    )))
                }
                OnFail::Retry if *round < max => {
                    t.next = Some(StageNext::Retry(Some(render_findings(&c))))
                }
                OnFail::Retry | OnFail::Gate => t.next = Some(StageNext::Gate),
            },
        }
    }

    /// Rule WB5: the node's conditions did not hold.
    pub fn on_stage_skipped(&mut self, node: &str, scope: Option<&str>) {
        let t = self.stages.entry(stage_key(node, scope)).or_default();
        t.outcome = Some(StageOutcome::Skipped);
        t.next = None;
    }

    /// Rules WB2 and WB3: a transform or prompt node ran. A failure follows the node's `on_fail`
    /// like an agent's `fail` verdict.
    pub fn on_node_ran(
        &mut self,
        node: &str,
        scope: Option<&str>,
        round: u32,
        output: Option<&Value>,
        error: Option<&str>,
        cost_usd: f64,
    ) {
        self.node_cost += cost_usd;
        let (on_fail, max) = self
            .stage_def(node)
            .map(|d| (d.on_fail, d.max_rounds))
            .unwrap_or((OnFail::Gate, 1));
        let t = self.stages.entry(stage_key(node, scope)).or_default();
        t.round = round;
        t.next = None;
        match (output, error) {
            (Some(v), None) => {
                t.output = Some(v.clone());
                t.last = Some(CustomSubmit {
                    verdict: StageVerdict::Pass,
                    summary: format!("Node `{node}` gave its output."),
                    findings: vec![],
                    question: None,
                    options: vec![],
                    report_path: None,
                    data: Some(v.clone()),
                });
                t.outcome = Some(StageOutcome::Passed);
            }
            (_, e) => {
                let summary = e.unwrap_or("The node gave no output.").to_string();
                t.last = Some(CustomSubmit {
                    verdict: StageVerdict::Fail,
                    summary: summary.clone(),
                    findings: vec![],
                    question: None,
                    options: vec![],
                    report_path: None,
                    data: None,
                });
                match on_fail {
                    OnFail::Continue => t.outcome = Some(StageOutcome::FailedOn),
                    OnFail::Fail => {
                        t.outcome = Some(StageOutcome::Stopped(format!(
                            "Node `{node}` failed: {summary}"
                        )))
                    }
                    OnFail::Retry if round < max => t.next = Some(StageNext::Retry(None)),
                    OnFail::Retry | OnFail::Gate => t.next = Some(StageNext::Gate),
                }
            }
        }
    }

    /// Rule WB4: what a reference to node `node` reads, seen from an instance in `scope`. A node
    /// that runs once per project or phase reads as its instance in the same project or phase,
    /// and as the list of all its instances from anywhere else.
    pub fn node_value(&self, node: &str, scope: Option<&str>) -> Value {
        match node {
            SESSION_REF => return self.session_value(),
            SCOPE_REF => return scope_value(self, scope),
            _ => {}
        }
        let Some(d) = self.workflow.as_ref().and_then(|w| w.stage(node)) else {
            return Value::Null;
        };
        if let StageRun::Builtin { stage } = d.run {
            return self.builtin_value(stage, scope);
        }
        let same_kind = match (d.scope, scope) {
            (StageScope::Project, Some(sc)) => sc.starts_with("project:"),
            (StageScope::Phase, Some(sc)) => sc.starts_with("phase:"),
            _ => false,
        };
        match d.scope {
            StageScope::Session => self.instance_value(node, None),
            _ if same_kind => self.instance_value(node, scope),
            _ => {
                let prefix = format!("{node}|");
                Value::Array(
                    self.stages
                        .iter()
                        .filter(|(k, _)| k.starts_with(&prefix) && k.len() > prefix.len())
                        .map(|(k, _)| self.instance_value(node, Some(&k[prefix.len()..])))
                        .collect(),
                )
            }
        }
    }

    fn instance_value(&self, node: &str, scope: Option<&str>) -> Value {
        let Some(t) = self.stage_track(node, scope) else {
            return Value::Null;
        };
        if t.outcome == Some(StageOutcome::Skipped) {
            return Value::Null;
        }
        let mut v = match t.last.as_ref() {
            Some(c) => serde_json::to_value(c).unwrap_or(Value::Null),
            None => json!({}),
        };
        if let Value::Object(m) = &mut v {
            let out = t
                .output
                .clone()
                .or_else(|| t.last.as_ref().and_then(|c| c.data.clone()))
                .unwrap_or(Value::Null);
            m.insert("output".into(), out);
            if let Some(sc) = scope {
                m.insert("scope".into(), Value::String(sc.into()));
            }
        }
        v
    }

    fn session_value(&self) -> Value {
        let mut v = json!({
            "request": self.full_request(),
            "category": self.category,
            "projects": self.scope,
            "title": self.title,
        });
        if let Value::Object(m) = &mut v {
            m.extend(self.pipeline.session_facts(self));
        }
        v
    }

    /// What a reference to a built-in stage reads: the facts it settled.
    fn builtin_value(&self, stage: BuiltinStage, scope: Option<&str>) -> Value {
        self.pipeline.stage_value(self, stage, scope)
    }

    /// Rule WB4: the value of one reference, such as `audit.data.risk`.
    pub fn ref_value(&self, reference: &str, scope: Option<&str>) -> Value {
        match parse_ref(reference) {
            Ok(r) => lookup(&self.node_value(&r.node, scope), &r.path),
            Err(_) => Value::Null,
        }
    }

    /// Rule WB4: a node's inputs, each resolved from the node it names.
    pub fn node_inputs(&self, d: &StageDef, scope: Option<&str>) -> Map<String, Value> {
        d.inputs
            .iter()
            .map(|(k, r)| (k.clone(), self.ref_value(r, scope)))
            .collect()
    }

    /// Rule WB5: whether a node's conditions hold.
    pub fn when_holds(&self, d: &StageDef, scope: Option<&str>) -> bool {
        if d.when.is_empty() {
            return true;
        }
        let test = |c: &ostra_core::transform::Condition| {
            c.op.eval(
                &self.ref_value(&c.reference, scope),
                c.value.as_ref().unwrap_or(&Value::Null),
            )
        };
        match d.when_mode {
            WhenMode::All => d.when.iter().all(test),
            WhenMode::Any => d.when.iter().any(test),
        }
    }

    /// Rule PL3: fold a plugin's decision for one stage instance.
    pub fn on_stage_decided(
        &mut self,
        node: &str,
        scope: Option<&str>,
        decision: &ostra_core::plugin::StageDecision,
    ) {
        use ostra_core::plugin::StageDecision;
        let (on_fail, max) = self
            .stage_def(node)
            .map(|d| (d.on_fail, d.max_rounds))
            .unwrap_or((OnFail::Gate, 1));
        let t = self.stages.entry(stage_key(node, scope)).or_default();
        t.decisions.push(decision.clone());
        t.decided_runs = t.runs.len();
        t.next = None;
        let failed = |t: &mut StageTrack, summary: String| match on_fail {
            OnFail::Continue => t.outcome = Some(StageOutcome::FailedOn),
            OnFail::Fail => {
                t.outcome = Some(StageOutcome::Stopped(format!(
                    "Stage `{node}` failed: {summary}"
                )))
            }
            OnFail::Gate | OnFail::Retry => {
                t.last = Some(CustomSubmit {
                    verdict: StageVerdict::Fail,
                    summary,
                    findings: vec![],
                    question: None,
                    options: vec![],
                    report_path: None,
                    data: None,
                });
                t.question = None;
                t.next = Some(StageNext::Gate);
            }
        };
        match decision {
            StageDecision::Run {
                agent,
                instructions,
            } => {
                if t.runs.len() as u32 >= max {
                    failed(
                        t,
                        format!(
                            "The plugin asked for another run after {max} runs, the stage's `max_rounds`."
                        ),
                    );
                } else {
                    t.pending_run = Some((agent.clone(), instructions.clone()));
                }
            }
            StageDecision::Pass { .. } => t.outcome = Some(StageOutcome::Passed),
            StageDecision::Fail { summary } => failed(t, summary.clone()),
            StageDecision::Ask { question, options } => {
                t.question = Some((question.clone(), options.clone()));
                t.next = Some(StageNext::Gate);
            }
        }
    }

    pub fn stage_gate_opened(&mut self, id: &GateId, payload: &GatePayload) {
        if let GatePayload::StageReview { stage, scope, .. } = payload {
            let t = self
                .stages
                .entry(stage_key(stage, scope.as_deref()))
                .or_default();
            t.gate = Some(id.clone());
            t.next = None;
        }
    }

    /// Rule WF5: the user's answer at a stage's gate.
    pub fn stage_gate_answered(&mut self, id: &GateId, payload: &GatePayload, answer: &GateAnswer) {
        let GatePayload::StageReview {
            stage,
            scope,
            verdict,
            options,
            ..
        } = payload
        else {
            return;
        };
        let t = self
            .stages
            .entry(stage_key(stage, scope.as_deref()))
            .or_default();
        if t.gate.as_ref() != Some(id) {
            return;
        }
        t.gate = None;
        let (option, text) = match answer {
            GateAnswer::Choice { option, text } => (
                option.as_str(),
                text.clone().filter(|t| !t.trim().is_empty()),
            ),
            _ => ("stop", None),
        };
        match (verdict, option) {
            (_, "stop") => {
                t.outcome = Some(StageOutcome::Stopped(format!(
                    "The user stopped the session at stage `{stage}`."
                )))
            }
            (StageVerdict::Fail, "continue") => t.outcome = Some(StageOutcome::FailedOn),
            (StageVerdict::Fail, _) => {
                if let Some(g) = &text {
                    t.notes.push(g.clone());
                }
                t.next = Some(StageNext::Retry(t.last.as_ref().map(render_findings)));
            }
            (_, choice) => {
                let said = match (choice, text) {
                    ("other", Some(t)) => t,
                    (c, Some(t)) => format!("{c}: {t}"),
                    (c, None) if options.iter().any(|o| o == c) => c.to_string(),
                    (c, None) => c.to_string(),
                };
                t.notes.push(said);
                t.next = Some(StageNext::Retry(None));
            }
        }
    }

    pub fn stage_exec_gate(&mut self, rec: &ExecRecord, gate: Option<&GateId>, retry: bool) {
        let ExecPurpose::Stage { node, scope, .. } = &rec.purpose else {
            return;
        };
        let t = self
            .stages
            .entry(stage_key(node, scope.as_deref()))
            .or_default();
        match gate {
            Some(g) => t.failed_gate = Some(g.clone()),
            None => {
                t.failed_gate = None;
                if retry {
                    t.next = Some(StageNext::Retry(None));
                } else {
                    t.outcome = Some(StageOutcome::FailedOn);
                }
            }
        }
    }

    /// Rule WF6: the subagents of the stages a stage run reads, with their role toward it.
    pub fn stage_partners(&self, rec: &ExecRecord) -> Vec<(ExecutionId, String)> {
        let ExecPurpose::Stage { node, scope, .. } = &rec.purpose else {
            return vec![];
        };
        let Some(wf) = &self.workflow else {
            return vec![];
        };
        let mut out = vec![];
        let latest = |f: &dyn Fn(&ExecRecord) -> bool| {
            self.executions
                .values()
                .filter(|r| f(r))
                .max_by_key(|r| r.id.clone())
                .map(|r| self.subagent_of(&r.id))
        };
        for a in wf.ancestors(node) {
            let Some(d) = wf.stage(&a) else { continue };
            match &d.run {
                StageRun::Builtin { stage } => {
                    out.extend(self.pipeline.stage_partners(self, *stage))
                }
                _ => {
                    let other = a.clone();
                    out.extend(
                        latest(&|r| {
                            matches!(&r.purpose, ExecPurpose::Stage { node, .. } if *node == other)
                        })
                        .map(|x| (x, format!("the agent of stage `{a}`"))),
                    );
                }
            }
        }
        out.extend(self.pipeline.work_partners(self, scope.as_deref()));
        out
    }

    /// Rule WF4: phase stages of phase `phase` are all done, so phases that depend on it may go on.
    pub fn phase_stages_done(&self, phase: u32) -> bool {
        let Some(wf) = &self.workflow else {
            return true;
        };
        let scope = format!("phase:{phase}");
        wf.stages
            .iter()
            .filter(|d| d.scope == StageScope::Phase && d.builtin().is_none())
            .all(|d| {
                self.stage_track(&d.id, Some(&scope))
                    .and_then(|t| t.outcome.as_ref())
                    .is_some_and(|o| !matches!(o, StageOutcome::Stopped(_)))
            })
    }

    /// One line per earlier custom stage, for a stage's spawn (Rule WF4).
    fn earlier_stage_lines(
        &self,
        wf: &WorkflowDef,
        node: &str,
        scope: Option<&str>,
    ) -> Vec<String> {
        let mut out = vec![];
        for a in wf.ancestors(node) {
            let Some(d) = wf.stage(&a) else { continue };
            if d.builtin().is_some() {
                continue;
            }
            let scopes: Vec<Option<String>> = match d.scope {
                StageScope::Session => vec![None],
                _ => vec![scope.map(String::from)],
            };
            for sc in scopes {
                let Some(t) = self.stage_track(&a, sc.as_deref()) else {
                    continue;
                };
                let Some(c) = &t.last else { continue };
                let verdict = serde_json::to_value(c.verdict)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                let mut line = format!("{a}: {verdict}. {}", c.summary.replace('\n', " "));
                if let Some(p) = &c.report_path {
                    line.push_str(&format!(" Report: {p}"));
                }
                out.push(line);
            }
        }
        out
    }
}

fn scope_value(s: &SessionState, scope: Option<&str>) -> Value {
    match scope {
        Some(sc) if sc.starts_with("project:") => {
            json!({ "kind": "project", "project": &sc["project:".len()..] })
        }
        Some(sc) if sc.starts_with("phase:") => {
            let id = sc["phase:".len()..].parse::<u32>().ok();
            json!({
                "kind": "phase",
                "phase": id,
                "project": id.and_then(|i| s.pipeline.phase(s, i)).map(|p| p.info.project),
            })
        }
        _ => json!({ "kind": "session" }),
    }
}

/// Rule WB5: a node that has not started yet and whose conditions do not hold is skipped. Its
/// conditions read only nodes it waits for, which are done, so the answer cannot change later.
pub fn skip_step(s: &SessionState, d: &StageDef, scope: Option<&str>) -> Option<Step> {
    if d.when.is_empty() {
        return None;
    }
    let t = s.stage_track(&d.id, scope);
    let fresh = t.is_none_or(|t| {
        t.outcome.is_none() && t.runs.is_empty() && t.round == 0 && t.decisions.is_empty()
    });
    (fresh && !s.when_holds(d, scope)).then(|| Step::SkipStage {
        node: d.id.clone(),
        scope: scope.map(String::from),
    })
}

/// Rules WB2, WB3, and WF5: the next action for one instance of a transform or prompt node.
pub fn data_stage_action(s: &SessionState, d: &StageDef, scope: Option<&str>) -> Action {
    let t = s.stage_track(&d.id, scope).cloned().unwrap_or_default();
    match &t.outcome {
        Some(StageOutcome::Passed | StageOutcome::FailedOn | StageOutcome::Skipped) => {
            return Action::Done;
        }
        Some(StageOutcome::Stopped(why)) => return Action::Fail(why.clone()),
        None => {}
    }
    if let Some(step) = skip_step(s, d, scope) {
        return Action::Skip(step);
    }
    if t.gate.is_some() {
        return Action::Wait;
    }
    if t.next == Some(StageNext::Gate) {
        let summary = t
            .last
            .as_ref()
            .map(|c| c.summary.clone())
            .unwrap_or_else(|| "The node gave no output.".into());
        return Action::Gate(Step::OpenGate {
            title: format!("Node {} failed", d.id),
            explanation: format!(
                "Node {} failed in round {} of {}. Run it again, continue the workflow without its output, or stop the session. {summary}",
                d.id, t.round, d.max_rounds
            ),
            payload: GatePayload::StageReview {
                stage: d.id.clone(),
                scope: scope.map(String::from),
                agent: None,
                execution: None,
                verdict: StageVerdict::Fail,
                summary,
                findings: vec![],
                question: None,
                options: vec![],
                round: t.round,
                max_rounds: d.max_rounds,
            },
        });
    }
    if t.round == 0 || matches!(t.next, Some(StageNext::Retry(_))) {
        return Action::Skip(Step::RunNode {
            node: d.id.clone(),
            scope: scope.map(String::from),
            round: t.round + 1,
            model: matches!(d.run, StageRun::Prompt { .. }),
        });
    }
    Action::Wait
}

/// Rules WF2 and PL3: every custom stage of `wf` names an agent the workspace defines, and every
/// plugin stage one a plugin serves.
pub fn check_runnable(
    wf: &WorkflowDef,
    agents: &ostra_agents::AgentCatalog,
    plugin_stages: &[(String, String)],
) -> Result<(), String> {
    let mut issues = vec![];
    for d in &wf.stages {
        match &d.run {
            StageRun::Agent { agent } => match agents.def(*agent) {
                None => issues.push(format!(
                    "Stage `{}` runs agent `{agent}`, which the workspace does not define. Add it to .ostra/agents.",
                    d.id
                )),
                // Rule CA5: a custom stage reads a verdict, from the stage contract or a plugin's.
                Some(def) if !def.returns.custom_stage() => issues.push(format!(
                    "Stage `{}` runs `{agent}`, which returns `{}`: a custom stage needs an agent that returns `stage` or a plugin contract. Bind it to the built-in stage that reads `{}` instead.",
                    d.id, def.returns, def.returns
                )),
                Some(_) => {}
            },
            StageRun::Plugin { plugin, stage } => {
                if !plugin_stages.iter().any(|(p, s)| p == plugin && s == stage) {
                    issues.push(format!(
                        "Stage `{}` runs `{plugin}:{stage}`, which no plugin of the workspace serves.",
                        d.id
                    ));
                }
            }
            // Rules WB2 and WB3: checked with the workflow's structure.
            StageRun::Transform { .. } | StageRun::Prompt { .. } => {}
            // Rule WF8: a bound agent exists and returns the contract it fills.
            StageRun::Builtin { .. } => {
                for (contract, agent) in &d.agents {
                    match agents.def(*agent) {
                        None => issues.push(format!(
                            "Stage `{}` binds `{agent}` to `{contract}`, but the workspace defines no agent `{agent}`.",
                            d.id
                        )),
                        Some(def) if def.returns != *contract => issues.push(format!(
                            "Stage `{}` binds `{agent}` to `{contract}`, but `{agent}` returns `{}`. Bind an agent whose `returns` is `{contract}`.",
                            d.id, def.returns
                        )),
                        Some(_) => {}
                    }
                }
            }
        }
    }
    // Rule WB6: every reference reads a field the node it names gives, of the kind it needs.
    issues.extend(check_types(wf, agents));
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join(" "))
    }
}

/// Rule WB6: the shape of an agent node's value: its submit, with `output` set to its `data`.
fn submit_shape(data: Value) -> Value {
    let text = json!({"type": "string"});
    json!({"type": "object", "properties": {
        "verdict": text, "summary": text, "question": text, "report_path": text, "scope": text,
        "options": {"type": "array", "items": text},
        "findings": {"type": "array", "items": {"type": "object", "properties": {
            "file": text, "description": text, "fix": text
        }}},
        "data": data, "output": data,
    }})
}

fn data_shape(output: Value) -> Value {
    json!({"type": "object", "properties": {
        "verdict": {"type": "string"}, "summary": {"type": "string"}, "scope": {"type": "string"},
        "output": output,
    }})
}

fn kind_shape(kind: ostra_core::transform::ValueKind) -> Value {
    match kind {
        ostra_core::transform::ValueKind::Any => json!({}),
        k => json!({"type": k.as_str().replace("bool", "boolean")}),
    }
}

/// Rule WB6: what a reference to node `node` reads, as a JSON Schema, seen from `reader`. `None`
/// when the workflow has no such node.
pub fn node_shape(
    wf: &WorkflowDef,
    node: &str,
    reader: &StageDef,
    agents: &ostra_agents::AgentCatalog,
) -> Option<Value> {
    let text = json!({"type": "string"});
    if node == SESSION_REF {
        return Some(json!({"type": "object", "properties": {
            "request": text, "category": text, "track": text, "stakes": text, "title": text,
            "projects": {"type": "array", "items": text},
        }}));
    }
    if node == SCOPE_REF {
        return Some(json!({"type": "object", "properties": {
            "kind": text, "project": text, "phase": {"type": "number"},
        }}));
    }
    let d = wf.stage(node)?;
    let value = match &d.run {
        StageRun::Builtin { stage } => return Some(stage.value_shape()),
        StageRun::Agent { agent } => submit_shape(
            agents
                .def(*agent)
                .filter(|def| def.returns == Contract::Stage)
                .and_then(|def| def.submit_data.clone())
                .unwrap_or(json!({})),
        ),
        StageRun::Plugin { .. } => submit_shape(json!({})),
        StageRun::Prompt { .. } => data_shape(d.output_schema.clone().unwrap_or(json!({}))),
        StageRun::Transform { function } => {
            let out = match function.as_str() {
                // These keep the items they are given, so the list keeps its item shape.
                "filter" | "sort" | "unique" => d
                    .inputs
                    .get("items")
                    .and_then(|r| ref_shape(wf, r, d, agents).ok())
                    .unwrap_or(json!({"type": "array"})),
                _ => ostra_core::transform::function_info_with(
                    function,
                    &wf.functions,
                    &wf.plugin_transforms,
                )
                .map(|i| kind_shape(i.output))
                .unwrap_or(json!({})),
            };
            data_shape(out)
        }
    };
    let same_kind = matches!(
        (d.scope, reader.scope),
        (StageScope::Project, StageScope::Project) | (StageScope::Phase, StageScope::Phase)
    );
    Some(if d.scope == StageScope::Session || same_kind {
        value
    } else {
        json!({"type": "array", "items": value})
    })
}

/// Rule WB6: the shape at a reference, or why the reference reads nothing.
pub fn ref_shape(
    wf: &WorkflowDef,
    reference: &str,
    reader: &StageDef,
    agents: &ostra_agents::AgentCatalog,
) -> Result<Value, String> {
    let r = parse_ref(reference)?;
    let mut cur = node_shape(wf, &r.node, reader, agents)
        .ok_or_else(|| format!("there is no node `{}`", r.node))?;
    let mut at = r.node.clone();
    for p in &r.path {
        match cur.get("type").and_then(Value::as_str) {
            Some("object") => match cur.get("properties").and_then(Value::as_object) {
                Some(props) => match props.get(p) {
                    Some(next) => cur = next.clone(),
                    None => {
                        let mut have: Vec<&String> = props.keys().collect();
                        have.sort();
                        return Err(format!(
                            "`{at}` has no field `{p}`{}",
                            if have.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    "; it has {}",
                                    have.iter()
                                        .map(|k| format!("`{k}`"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            }
                        ));
                    }
                },
                None => cur = json!({}),
            },
            Some("array") if p.parse::<usize>().is_ok() => {
                cur = cur.get("items").cloned().unwrap_or(json!({}))
            }
            Some("array") => {
                return Err(format!(
                    "`{at}` is a list, so read an item by its index, such as `{at}.0`, or wire the list into a transform"
                ));
            }
            Some(other) => return Err(format!("`{at}` is a {other}, which has no field `{p}`")),
            None => return Ok(json!({})),
        }
        at = format!("{at}.{p}");
    }
    Ok(cur)
}

/// A JSON type in the words an error message uses.
fn words(kind: &str) -> &'static str {
    match kind {
        "array" => "a list",
        "object" => "an object",
        "string" => "a text",
        "number" | "integer" => "a number",
        "bool" | "boolean" => "true or false",
        "null" => "null",
        _ => "a value",
    }
}

/// Rule WB6: references that read nothing, and inputs wired from a value of the wrong kind.
pub fn check_types(wf: &WorkflowDef, agents: &ostra_agents::AgentCatalog) -> Vec<String> {
    use ostra_core::transform::ValueKind;
    let mut out = vec![];
    for d in &wf.stages {
        let info = match &d.run {
            StageRun::Transform { function } => ostra_core::transform::function_info_with(
                function,
                &wf.functions,
                &wf.plugin_transforms,
            ),
            _ => None,
        };
        for (name, r) in &d.inputs {
            match ref_shape(wf, r, d, agents) {
                Err(e) => out.push(format!("Node `{}` reads `{r}`, but {e}.", d.id)),
                Ok(shape) => {
                    let want = info
                        .as_ref()
                        .and_then(|i| i.inputs.iter().find(|p| &p.name == name))
                        .map(|p| p.kind)
                        .unwrap_or(ValueKind::Any);
                    let have = shape.get("type").and_then(Value::as_str);
                    let fits = match (want, have) {
                        (ValueKind::Any, _) | (_, None) => true,
                        (ValueKind::Bool, Some(h)) => h == "boolean",
                        (ValueKind::Number, Some(h)) => h == "number" || h == "integer",
                        (k, Some(h)) => k.as_str() == h,
                    };
                    if !fits {
                        out.push(format!(
                            "Wire {} into input `{name}` of node `{}`: `{r}` is {}.",
                            words(want.as_str()),
                            d.id,
                            words(have.unwrap_or("value"))
                        ));
                    }
                }
            }
        }
        for c in &d.when {
            if let Err(e) = ref_shape(wf, &c.reference, d, agents) {
                out.push(format!(
                    "A condition of node `{}` reads `{}`, but {e}.",
                    d.id, c.reference
                ));
            }
        }
    }
    out
}

/// The instances of a custom stage: its scope values.
pub fn stage_scopes(s: &SessionState, d: &StageDef) -> Vec<Option<String>> {
    match d.scope {
        StageScope::Session => vec![None],
        StageScope::Project => s
            .scope
            .iter()
            .map(|p| Some(format!("project:{p}")))
            .collect(),
        StageScope::Phase => s
            .pipeline
            .passed_phases(s)
            .into_iter()
            .map(|p| Some(format!("phase:{p}")))
            .collect(),
    }
}

/// What the planner does for one custom stage instance, or `true` when it is done.
pub enum Action {
    Done,
    Wait,
    Fail(String),
    Gate(Step),
    /// A step that is neither a gate nor a spawn: skip the node, or run it in the engine.
    Skip(Step),
    ExecFailed {
        exec: ExecutionId,
        agent: AgentName,
        project: String,
        error: String,
    },
    Spawn(Box<SpawnRequest>),
}

/// Rules WF4 and WF5: the next action for one instance of a custom agent stage.
pub fn agent_stage_action(
    s: &SessionState,
    wf: &WorkflowDef,
    d: &StageDef,
    agent: AgentName,
    scope: Option<&str>,
) -> Action {
    let t = s.stage_track(&d.id, scope).cloned().unwrap_or_default();
    match &t.outcome {
        Some(StageOutcome::Passed | StageOutcome::FailedOn | StageOutcome::Skipped) => {
            return Action::Done;
        }
        Some(StageOutcome::Stopped(why)) => return Action::Fail(why.clone()),
        None => {}
    }
    if let Some(step) = skip_step(s, d, scope) {
        return Action::Skip(step);
    }
    if t.gate.is_some() || t.failed_gate.is_some() {
        return Action::Wait;
    }
    let last = t.runs.last().and_then(|id| s.executions.get(id));
    if let Some(rec) = last {
        match &rec.result {
            None => return Action::Wait,
            Some(r) if r.status == ExecutionStatus::Waiting => return Action::Wait,
            Some(r)
                if matches!(
                    r.status,
                    ExecutionStatus::Error | ExecutionStatus::Denied | ExecutionStatus::Cancelled
                ) && t.next.is_none() =>
            {
                return Action::ExecFailed {
                    exec: rec.id.clone(),
                    agent: rec.agent,
                    project: rec.project.clone(),
                    error: r.error.clone().unwrap_or_else(|| format!("{:?}", r.status)),
                };
            }
            _ => {}
        }
    }
    if t.next == Some(StageNext::Gate) {
        let (rec, c) = match (last, &t.last) {
            (Some(rec), Some(c)) => (rec, c.clone()),
            (Some(rec), None) => (
                rec,
                CustomSubmit {
                    verdict: StageVerdict::Fail,
                    summary: "The stage's agent ended without its result.".into(),
                    findings: vec![],
                    question: None,
                    options: vec![],
                    report_path: None,
                    data: None,
                },
            ),
            _ => return Action::Wait,
        };
        let title = match c.verdict {
            StageVerdict::NeedsUser => format!("Stage {} asks you", d.id),
            _ => format!("Stage {} failed", d.id),
        };
        let explanation = match c.verdict {
            StageVerdict::NeedsUser => format!(
                "{} needs a decision before the workflow goes on: {}",
                agent,
                c.question.clone().unwrap_or_default()
            ),
            _ => format!(
                "{agent} found a problem in round {} of {}. Run the stage again, continue the workflow without it, or stop the session. {}",
                t.round, d.max_rounds, c.summary
            ),
        };
        return Action::Gate(Step::OpenGate {
            title,
            explanation,
            payload: GatePayload::StageReview {
                stage: d.id.clone(),
                scope: scope.map(String::from),
                agent: Some(agent),
                execution: Some(rec.id.clone()),
                verdict: c.verdict,
                summary: c.summary.clone(),
                findings: c.findings.clone(),
                question: c.question.clone(),
                options: c.options.clone(),
                round: t.round,
                max_rounds: d.max_rounds,
            },
        });
    }
    if last.is_some() && t.next.is_none() {
        // Interrupted runs are resumed by their spawn (Rule P2), with the same round.
        if last.is_some_and(|r| {
            r.result
                .as_ref()
                .is_some_and(|x| x.status == ExecutionStatus::Interrupted)
        }) {
            return Action::Spawn(Box::new(stage_request(
                s,
                wf,
                d,
                agent,
                scope,
                t.round.max(1),
                None,
                &t.notes,
            )));
        }
        return Action::Wait;
    }
    let prior = match &t.next {
        Some(StageNext::Retry(p)) => p.clone(),
        _ => None,
    };
    Action::Spawn(Box::new(stage_request(
        s,
        wf,
        d,
        agent,
        scope,
        t.round + 1,
        prior,
        &t.notes,
    )))
}

#[allow(clippy::too_many_arguments)]
pub fn stage_request(
    s: &SessionState,
    wf: &WorkflowDef,
    d: &StageDef,
    agent: AgentName,
    scope: Option<&str>,
    round: u32,
    prior: Option<String>,
    notes: &[String],
) -> SpawnRequest {
    let phase = scope
        .and_then(|x| x.strip_prefix("phase:"))
        .and_then(|p| p.parse::<u32>().ok())
        .and_then(|p| s.pipeline.phase(s, p));
    let project = match (scope.and_then(|x| x.strip_prefix("project:")), &phase) {
        (Some(p), _) => p.to_string(),
        (None, Some(p)) => p.info.project.clone(),
        _ => s.primary(),
    };
    let session_dir: PathBuf = if scope.is_none() {
        s.session_root.clone()
    } else {
        s.project_session_dir(&project)
    };
    let inputs = SpawnInputs {
        task: Some(s.full_request()),
        stage_id: Some(d.id.clone()),
        stage_round: Some(round),
        instructions: d.instructions.clone(),
        prior_findings: prior,
        spec_file: s.pipeline.spec_file(s),
        target: s.pipeline.master_plan(s),
        research_docs: s.pipeline.research_docs(s),
        phase: phase.as_ref().map(|p| p.info.clone()),
        earlier_stages: s.earlier_stage_lines(wf, &d.id, scope),
        stage_inputs: s
            .node_inputs(d, scope)
            .into_iter()
            .map(|(k, v)| format!("{k} = {v}"))
            .collect(),
        user_notes: notes.to_vec(),
        implementer_report: phase.and_then(|p| p.implementer_report),
        ..Default::default()
    };
    SpawnRequest {
        also: Vec::new(),
        agent,
        purpose: ExecPurpose::Stage {
            node: d.id.clone(),
            scope: scope.map(String::from),
            round,
        },
        stage: ostra_core::pipeline::StageKind::Custom,
        project,
        session_dir,
        inputs,
        resumes: None,
        continues: None,
    }
}
