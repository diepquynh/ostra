//! Workflows (HANDOVER 10.9): the pipeline as a graph of stages. Built-in stages run Ostra's own
//! rules; custom stages run a custom agent or a plugin's stage logic. A workspace keeps its
//! workflows as TOML files in `.ostra/workflows/`, and each session records the one it runs, so the
//! fold never reads a file (pattern 1).

use crate::agent::AgentName;
use crate::contract::Contract;
use crate::model::{Effort, Tier};
use crate::pipeline::{Category, Lane, Track};
use crate::transform::{Condition, WhenMode, parse_ref};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use ts_rs::TS;

/// Rule WF2: the built-in stages, in the order the pipeline runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum BuiltinStage {
    /// Research tasks and the sufficiency check (Rules D1, D2).
    Research,
    /// The Track judge: light or full.
    Track,
    /// The spec, its fact-check, and its approval.
    Spec,
    /// The Stakes judge, which can skip the plan.
    Stakes,
    /// The plan, its fact-check, and its approval.
    Plan,
    /// Every phase's implement and review loop.
    Build,
    /// The user's review of the implementation (Rule F1).
    Feedback,
    /// Per project: format, the closing gate, tests, docs, and the book.
    Closing,
}

impl BuiltinStage {
    pub const ALL: [BuiltinStage; 8] = [
        BuiltinStage::Research,
        BuiltinStage::Track,
        BuiltinStage::Spec,
        BuiltinStage::Stakes,
        BuiltinStage::Plan,
        BuiltinStage::Build,
        BuiltinStage::Feedback,
        BuiltinStage::Closing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinStage::Research => "research",
            BuiltinStage::Track => "track",
            BuiltinStage::Spec => "spec",
            BuiltinStage::Stakes => "stakes",
            BuiltinStage::Plan => "plan",
            BuiltinStage::Build => "build",
            BuiltinStage::Feedback => "feedback",
            BuiltinStage::Closing => "closing",
        }
    }

    /// The `uses` value that names it: `ostra:<stage>`.
    pub fn uses(self) -> String {
        format!("ostra:{}", self.as_str())
    }

    pub fn parse_uses(s: &str) -> Option<BuiltinStage> {
        let bare = s.trim().strip_prefix("ostra:")?;
        BuiltinStage::ALL.into_iter().find(|b| b.as_str() == bare)
    }

    pub fn lane(self) -> Lane {
        match self {
            BuiltinStage::Research | BuiltinStage::Track => Lane::Research,
            BuiltinStage::Spec => Lane::Requirements,
            BuiltinStage::Stakes | BuiltinStage::Plan => Lane::Design,
            BuiltinStage::Build => Lane::Build,
            BuiltinStage::Feedback => Lane::Review,
            BuiltinStage::Closing => Lane::Test,
        }
    }

    /// Rule WF8: the result contracts this stage reads, each from the agent its workflow binds to
    /// it, or from the standard plugin's agent for that contract.
    pub fn contracts(self) -> &'static [Contract] {
        use Contract::*;
        match self {
            BuiltinStage::Research => &[Research],
            BuiltinStage::Spec => &[Spec, FactCheck],
            BuiltinStage::Plan => &[Plan, FactCheck],
            BuiltinStage::Build => &[Implementation, Review, Prompt, Advice],
            BuiltinStage::Closing => &[
                PathAnalysis,
                Tests,
                Review,
                Documentation,
                Architecture,
                Implementation,
                Advice,
            ],
            BuiltinStage::Track | BuiltinStage::Stakes | BuiltinStage::Feedback => &[],
        }
    }

    /// What the stage does, for the Workflow builder's palette.
    pub fn description(self) -> &'static str {
        match self {
            BuiltinStage::Research => {
                "Research tasks over the code and the Sufficiency judge, which asks for more until the research covers the request."
            }
            BuiltinStage::Track => {
                "The Track judge picks the light track (build and review) or the full track (spec and plan first)."
            }
            BuiltinStage::Spec => {
                "The spec, its open questions, its fact-check, and your approval. Nothing on the light track."
            }
            BuiltinStage::Stakes => {
                "The Stakes judge, which can skip the plan for a low-stakes change. Nothing on the light track."
            }
            BuiltinStage::Plan => {
                "The plan and its phases, its fact-check, and your approval. Nothing on the light track or with low stakes."
            }
            BuiltinStage::Build => {
                "Every phase's implement and review loop. Phase-scoped nodes run inside it, after each phase's review."
            }
            BuiltinStage::Feedback => {
                "Your review of the implementation, and the feedback rounds it starts."
            }
            BuiltinStage::Closing => {
                "Per project: the format command, the closing gate, tests, docs, and the book."
            }
        }
    }

    /// Rule WF2: a workflow may leave this stage out, because the engine has a defined path
    /// without it.
    pub fn removable(self) -> bool {
        matches!(
            self,
            BuiltinStage::Track | BuiltinStage::Feedback | BuiltinStage::Closing
        )
    }
}

/// Where a custom stage runs once per.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StageScope {
    /// Once for the session.
    #[default]
    Session,
    /// Once per project in the session's scope.
    Project,
    /// Once per plan phase, after its review passes. Phases that depend on it wait for it.
    Phase,
}

/// Rule WF5: what a custom stage's `fail` verdict does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OnFail {
    /// Ask the user: run the stage again, continue, or stop the session.
    #[default]
    Gate,
    /// Run the stage again with its findings, up to `max_rounds`, then ask the user.
    Retry,
    /// Record the failure and go on.
    Continue,
    /// Stop the session.
    Fail,
}

/// What a stage runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum StageRun {
    Builtin {
        stage: BuiltinStage,
    },
    /// A custom agent from the workspace's catalog.
    Agent {
        agent: AgentName,
    },
    /// Rule PL3: a plugin's stage logic decides what runs.
    Plugin {
        plugin: String,
        stage: String,
    },
    /// Rule WB2: one of Ostra's transform functions over earlier nodes' results.
    Transform {
        function: String,
    },
    /// Rule WB3: one model call whose answer matches the node's `output_schema`. The node's
    /// instructions are the prompt.
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<Tier>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<Effort>,
    },
}

/// One node of a resolved workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageDef {
    pub id: String,
    pub run: StageRun,
    /// The stages that must be done before this one starts.
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub scope: StageScope,
    /// The node's own instructions, given to its agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub on_fail: OnFail,
    /// Runs of the stage before a `retry` asks the user instead.
    #[serde(default = "default_rounds")]
    pub max_rounds: u32,
    /// The board lane its runs show in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    /// Rule WF8: on a built-in stage, the agent that fills each contract it reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<Contract, AgentName>,
    /// Rule WB4: values from earlier nodes, by name: `{ risk = "audit.data.risk" }`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Rule WB2: a transform node's fixed arguments.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub args: Map<String, Value>,
    /// Rule WB5: the node runs only when these hold, and is skipped otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
    #[serde(default, skip_serializing_if = "WhenMode::is_all")]
    pub when_mode: WhenMode,
    /// Rule WB3: the JSON Schema a prompt node's answer matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub output_schema: Option<Value>,
}

fn default_rounds() -> u32 {
    3
}

/// The largest `max_rounds` a stage may set, because every round is a model run.
pub const MAX_STAGE_ROUNDS: u32 = 10;
/// The most custom stages one workflow may hold (Rule WF7).
pub const MAX_CUSTOM_STAGES: usize = 24;

impl StageDef {
    pub fn builtin(&self) -> Option<BuiltinStage> {
        match self.run {
            StageRun::Builtin { stage } => Some(stage),
            _ => None,
        }
    }

    pub fn lane(&self) -> Lane {
        self.lane.unwrap_or(match self.run {
            StageRun::Builtin { stage } => stage.lane(),
            _ => Lane::Review,
        })
    }

    /// A node that runs in the engine without an agent: a transform or a prompt.
    pub fn is_data_node(&self) -> bool {
        matches!(
            self.run,
            StageRun::Transform { .. } | StageRun::Prompt { .. }
        )
    }

    /// Rule WB4: every reference the node reads, from its inputs and its conditions.
    pub fn references(&self) -> Vec<&str> {
        self.inputs
            .values()
            .map(String::as_str)
            .chain(self.when.iter().map(|c| c.reference.as_str()))
            .collect()
    }
}

/// A resolved workflow: every stage, with `extends` and `remove` already applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The built-in pipeline the workflow runs on, which sets the session's category.
    pub base: Category,
    /// Rule WF3: a fixed track in place of the Track judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Track>,
    pub stages: Vec<StageDef>,
    /// Rule WB7: the workspace's composite functions the workflow calls, recorded with it so a
    /// later edit of a function file never changes a running session.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: crate::transform::Functions,
    /// Rule PL7: what each plugin transform function the workflow calls takes and gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_transforms: crate::transform::PluginTransforms,
}

/// What a new session asked for (Rule WF1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum WorkflowChoice {
    /// The workspace's workflow for the category Classify picks.
    ByCategory,
    /// A workflow by name, whose base sets the category.
    Named { name: String },
}

/// Rule WF9: Ostra's default workflows, one TOML file per base pipeline, shipped in
/// `assets/workflows/`. A workspace runs its own copy in `.ostra/workflows/` when it has one.
pub const DEFAULT_WORKFLOWS: [(Category, &str); 9] = [
    (
        Category::Research,
        include_str!("../../../assets/workflows/research.toml"),
    ),
    (
        Category::Spec,
        include_str!("../../../assets/workflows/spec.toml"),
    ),
    (
        Category::Plan,
        include_str!("../../../assets/workflows/plan.toml"),
    ),
    (
        Category::Implement,
        include_str!("../../../assets/workflows/implement.toml"),
    ),
    (
        Category::Verify,
        include_str!("../../../assets/workflows/verify.toml"),
    ),
    (
        Category::Test,
        include_str!("../../../assets/workflows/test.toml"),
    ),
    (
        Category::Docs,
        include_str!("../../../assets/workflows/docs.toml"),
    ),
    (
        Category::Prompt,
        include_str!("../../../assets/workflows/prompt.toml"),
    ),
    (
        Category::QuickChange,
        include_str!("../../../assets/workflows/quick-change.toml"),
    ),
];

/// The prefix that names Ostra's shipped default over a workspace copy: `extends = "ostra:implement"`.
pub const DEFAULT_PREFIX: &str = "ostra:";

fn defaults() -> &'static [(Category, WorkflowFile)] {
    static PARSED: std::sync::OnceLock<Vec<(Category, WorkflowFile)>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        DEFAULT_WORKFLOWS
            .iter()
            .map(|(c, text)| {
                let file: WorkflowFile = toml::from_str(text)
                    .unwrap_or_else(|e| panic!("assets/workflows/{}.toml: {e}", category_name(*c)));
                (*c, file)
            })
            .collect()
    })
}

/// The shipped default workflow file named `name`.
pub fn default_file(name: &str) -> Option<&'static WorkflowFile> {
    defaults()
        .iter()
        .find(|(c, _)| category_name(*c) == name)
        .map(|(_, f)| f)
}

/// The shipped default workflow file's text, as a migration writes it into a workspace.
pub fn default_text(name: &str) -> Option<&'static str> {
    DEFAULT_WORKFLOWS
        .iter()
        .find(|(c, _)| category_name(*c) == name)
        .map(|(_, t)| *t)
}

/// Rule WF2: the built-in stages each base pipeline runs, in order, as its default workflow lists
/// them.
pub fn builtin_chain(base: Category) -> Vec<BuiltinStage> {
    defaults()
        .iter()
        .find(|(c, _)| *c == base)
        .map(|(_, f)| {
            f.stages
                .iter()
                .filter_map(|s| s.uses.as_deref().and_then(BuiltinStage::parse_uses))
                .collect()
        })
        .unwrap_or_default()
}

/// Category names as a workflow file writes them: `implement`, `quick-change`, or the log's own
/// `IMPLEMENT`.
pub fn parse_category(s: &str) -> Option<Category> {
    let up = s.trim().replace('-', "_").to_ascii_uppercase();
    serde_json::from_value(serde_json::Value::String(up)).ok()
}

pub fn category_name(c: Category) -> String {
    c.as_str().to_ascii_lowercase().replace('_', "-")
}

impl WorkflowDef {
    /// Rule WF9: Ostra's shipped default workflow for a base pipeline.
    pub fn builtin(base: Category) -> WorkflowDef {
        WorkflowSet::default()
            .resolve(&format!("{DEFAULT_PREFIX}{}", category_name(base)))
            .unwrap_or_else(|e| panic!("the default `{}` workflow: {e}", category_name(base)))
    }

    pub fn stage(&self, id: &str) -> Option<&StageDef> {
        self.stages.iter().find(|s| s.id == id)
    }

    pub fn has_builtin(&self, b: BuiltinStage) -> bool {
        self.stages.iter().any(|s| s.builtin() == Some(b))
    }

    /// The stages in an order where every stage comes after the stages it waits for.
    pub fn ordered(&self) -> Vec<&StageDef> {
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut out = vec![];
        while out.len() < self.stages.len() {
            let before = out.len();
            for s in &self.stages {
                if !done.contains(s.id.as_str())
                    && s.after.iter().all(|a| done.contains(a.as_str()))
                {
                    done.insert(&s.id);
                    out.push(s);
                }
            }
            if out.len() == before {
                break;
            }
        }
        out
    }

    /// Every stage `id` waits for, directly or through others.
    pub fn ancestors(&self, id: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut todo: Vec<&str> = self
            .stage(id)
            .map(|s| s.after.iter().map(String::as_str).collect())
            .unwrap_or_default();
        while let Some(a) = todo.pop() {
            if out.insert(a.to_string())
                && let Some(s) = self.stage(a)
            {
                todo.extend(s.after.iter().map(String::as_str));
            }
        }
        out
    }

    /// Rules WF2 and WF7: the structure every workflow must have. Agent and plugin names are
    /// checked against the workspace by the caller.
    pub fn validate(&self) -> Vec<String> {
        let mut out = vec![];
        if self.base == Category::QuickAnswer {
            out.push("Pick another `base`: quick answers run outside workflows.".into());
            return out;
        }
        let mut ids = BTreeSet::new();
        for s in &self.stages {
            if !is_stage_id(&s.id) {
                out.push(format!(
                    "Name stage `{}` in lowercase kebab-case, starting with a letter.",
                    s.id
                ));
            }
            if !ids.insert(s.id.as_str()) {
                out.push(format!(
                    "Give each stage its own id: `{}` is used twice.",
                    s.id
                ));
            }
        }
        for s in &self.stages {
            for a in &s.after {
                if a == &s.id {
                    out.push(format!("Stage `{}` cannot wait for itself.", s.id));
                } else if !ids.contains(a.as_str()) {
                    out.push(format!(
                        "Stage `{}` waits for `{a}`, which is not a stage of this workflow.",
                        s.id
                    ));
                }
            }
            if s.max_rounds == 0 || s.max_rounds > MAX_STAGE_ROUNDS {
                out.push(format!(
                    "Set `max_rounds` of stage `{}` between 1 and {MAX_STAGE_ROUNDS}.",
                    s.id
                ));
            }
            if s.builtin().is_some() && s.scope != StageScope::Session {
                out.push(format!(
                    "Remove `scope` from built-in stage `{}`: built-in stages set their own.",
                    s.id
                ));
            }
            out.extend(s.node_issues(&self.functions, &self.plugin_transforms));
        }
        if self.ordered().len() < self.stages.len() {
            out.push(
                "Break the loop in `after`: the stages wait for each other in a circle.".into(),
            );
            return out;
        }
        // Rule WB4: a node reads only nodes it waits for, so their results exist when it runs.
        for s in &self.stages {
            let anc = self.ancestors(&s.id);
            for r in s.references() {
                if let Ok(r) = parse_ref(r)
                    && r.node != crate::transform::SESSION_REF
                    && r.node != crate::transform::SCOPE_REF
                    && !anc.contains(&r.node)
                {
                    out.push(format!(
                        "Make node `{}` wait for `{}`, directly or through other nodes, because it reads `{}`.",
                        s.id,
                        r.node,
                        r.node
                    ));
                }
            }
        }
        let custom = self.stages.iter().filter(|s| s.builtin().is_none()).count();
        if custom > MAX_CUSTOM_STAGES {
            out.push(format!(
                "Keep at most {MAX_CUSTOM_STAGES} custom stages in one workflow."
            ));
        }
        // Rule WF2: built-in stages keep their order, and only some may be left out.
        let chain = builtin_chain(self.base);
        for s in &self.stages {
            if let Some(b) = s.builtin()
                && !chain.contains(&b)
            {
                out.push(format!(
                    "Remove stage `{}`: the `{}` pipeline has no `{}` stage.",
                    s.id,
                    category_name(self.base),
                    b.as_str()
                ));
            }
        }
        let present: Vec<(BuiltinStage, &StageDef)> = chain
            .iter()
            .filter_map(|b| {
                self.stages
                    .iter()
                    .find(|s| s.builtin() == Some(*b))
                    .map(|s| (*b, s))
            })
            .collect();
        for b in &chain {
            if !present.iter().any(|(p, _)| p == b) {
                let ok = b.removable() && !(*b == BuiltinStage::Track && self.track.is_none());
                if !ok {
                    out.push(match b {
                        BuiltinStage::Track => {
                            "Keep stage `ostra:track`, or set `track` to `light` or `full`.".into()
                        }
                        _ => format!(
                            "Keep stage `{}`: the `{}` pipeline needs it.",
                            b.uses(),
                            category_name(self.base)
                        ),
                    });
                }
            }
        }
        if self.track.is_some() && self.base != Category::Implement {
            out.push("Remove `track`: only the `implement` pipeline has tracks.".into());
        }
        for w in present.windows(2) {
            let (prev, later) = (w[0].1, w[1].1);
            if !self.ancestors(&later.id).contains(&prev.id) {
                out.push(format!(
                    "Make stage `{}` wait for `{}`, directly or through other stages, because built-in stages run in order.",
                    later.id, prev.id
                ));
            }
        }
        for s in &self.stages {
            for a in &s.after {
                if self.stage(a).is_some_and(|x| x.scope == StageScope::Phase) {
                    out.push(format!(
                        "Make stage `{}` wait for the build instead of `{a}`: a phase stage runs once per phase inside the build, so nothing waits for it directly.",
                        s.id
                    ));
                }
            }
            if s.scope == StageScope::Phase {
                let build = present
                    .iter()
                    .find(|(b, _)| *b == BuiltinStage::Build)
                    .map(|(_, s)| s.id.clone());
                match build {
                    None => out.push(format!(
                        "Set another `scope` for stage `{}`: this workflow has no build stage, so it has no phases.",
                        s.id
                    )),
                    Some(build) => {
                        let anc = self.ancestors(&s.id);
                        let desc = self
                            .stages
                            .iter()
                            .any(|d| d.id == build && self.ancestors(&d.id).contains(&s.id));
                        if anc.contains(&build) || desc {
                            out.push(format!(
                                "Leave stage `{}` unordered against `{build}`: a phase stage runs inside the build, after each phase's review.",
                                s.id
                            ));
                        }
                    }
                }
            }
        }
        out
    }
}

impl StageDef {
    /// Rules WB2 to WB5: the problems with one node on its own.
    fn node_issues(
        &self,
        functions: &crate::transform::Functions,
        plugin: &crate::transform::PluginTransforms,
    ) -> Vec<String> {
        let id = &self.id;
        let mut out = vec![];
        let data = !self.inputs.is_empty()
            || !self.args.is_empty()
            || !self.when.is_empty()
            || self.output_schema.is_some();
        if self.builtin().is_some() && data {
            out.push(format!(
                "Remove `inputs`, `args`, `when`, and `output_schema` from built-in stage `{id}`: built-in stages always run, on their own rules."
            ));
            return out;
        }
        for (name, r) in &self.inputs {
            if !is_input_name(name) {
                out.push(format!(
                    "Name input `{name}` of node `{id}` with letters, digits, and `_`, starting with a letter."
                ));
            }
            if let Err(e) = parse_ref(r) {
                out.push(format!("Input `{name}` of node `{id}`: {e}"));
            }
        }
        for c in &self.when {
            if let Err(e) = parse_ref(&c.reference) {
                out.push(format!("A condition of node `{id}`: {e}"));
            }
            match (c.op.takes_value(), &c.value) {
                (true, None) => out.push(format!(
                    "Give the `{}` condition on `{}` of node `{id}` a `value` to compare against.",
                    c.op.as_str(),
                    c.reference
                )),
                (false, Some(_)) => out.push(format!(
                    "Remove `value` from the `{}` condition on `{}` of node `{id}`: that operator takes none.",
                    c.op.as_str(),
                    c.reference
                )),
                _ => {}
            }
        }
        if !matches!(self.run, StageRun::Transform { .. }) && !self.args.is_empty() {
            out.push(format!(
                "Remove `args` from node `{id}`: only a transform node takes arguments."
            ));
        }
        if !matches!(self.run, StageRun::Prompt { .. }) && self.output_schema.is_some() {
            out.push(format!(
                "Remove `output_schema` from node `{id}`: only a prompt node declares one. An agent declares its `data_schema` in its own file."
            ));
        }
        match &self.run {
            StageRun::Transform { function } => {
                let inputs: Vec<String> = self.inputs.keys().cloned().collect();
                out.extend(crate::transform::check_transform(
                    id, function, &inputs, &self.args, functions, plugin,
                ));
                if self.instructions.is_some() {
                    out.push(format!(
                        "Remove `instructions` from transform node `{id}`: a transform runs code, not a model."
                    ));
                }
            }
            StageRun::Prompt { .. } => {
                if self
                    .instructions
                    .as_deref()
                    .is_none_or(|i| i.trim().is_empty())
                {
                    out.push(format!("Write the prompt of node `{id}`."));
                }
                match &self.output_schema {
                    None => out.push(format!(
                        "Give prompt node `{id}` an `output_schema`, because later nodes read its answer by field."
                    )),
                    Some(schema) => {
                        if schema.get("type").and_then(Value::as_str) != Some("object") {
                            out.push(format!(
                                "Make the `output_schema` of node `{id}` an object schema (`type = \"object\"`): the model answers with one JSON object."
                            ));
                        } else if let Err(e) = crate::schema_check::validate_schema(schema) {
                            out.push(format!("Fix the `output_schema` of node `{id}`: {e}"));
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }
}

fn is_input_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.len() <= 48
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A workflow or node name: lowercase kebab-case, starting with a letter, at most 48 characters.
pub fn valid_name(s: &str) -> bool {
    is_stage_id(s)
}

fn is_stage_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ---------------------------------------------------------------------------------------------
// Workflow files
// ---------------------------------------------------------------------------------------------

/// One `.ostra/workflows/<name>.toml` as written. The Workflow builder edits this shape (Rule WB1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkflowFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The built-in pipeline it runs on. Taken from `extends` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// A built-in pipeline (`implement`) or another workflow of the workspace to start from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Categories this workflow runs for when the user picks none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_for: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Track>,
    /// Stage ids of the extended workflow to leave out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
    /// Rule WF8: the agent for a contract in every built-in stage that reads it, such as
    /// `[agents] review = "strict-reviewer"`. A stage's own `agents` wins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, String>,
    /// Rule WB1: where the builder draws each node, by id. The engine never reads it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: BTreeMap<String, [f64; 2]>,
    #[serde(default, rename = "stage", skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StageFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct StageFile {
    pub id: String,
    /// `ostra:<stage>` for a built-in stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uses: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `<plugin>:<stage>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Rule WB2: a transform function's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
    /// Rule WB3: a prompt node's prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Vec<String>>,
    /// Stages that wait for this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "is_session")]
    pub scope: StageScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "is_gate")]
    pub on_fail: OnFail,
    #[serde(default = "default_rounds", skip_serializing_if = "is_default_rounds")]
    pub max_rounds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    /// Rule WF8: on a built-in stage (`uses`), the agent for each contract it reads, such as
    /// `agents = { spec = "my-spec-writer" }`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub args: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
    #[serde(default, skip_serializing_if = "WhenMode::is_all")]
    pub when_mode: WhenMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub output_schema: Option<Value>,
}

fn is_session(s: &StageScope) -> bool {
    *s == StageScope::Session
}

fn is_gate(o: &OnFail) -> bool {
    *o == OnFail::Gate
}

fn is_default_rounds(n: &u32) -> bool {
    *n == default_rounds()
}

impl WorkflowFile {
    /// Rule WB1: a resolved workflow written out whole, every stage with its `after`, so the file
    /// resolves to the same workflow without `extends`.
    pub fn from_def(
        def: &WorkflowDef,
        default_for: Vec<String>,
        layout: BTreeMap<String, [f64; 2]>,
    ) -> WorkflowFile {
        let stages = def
            .stages
            .iter()
            .map(|d| {
                let mut f = StageFile {
                    id: d.id.clone(),
                    after: Some(d.after.clone()),
                    scope: d.scope,
                    instructions: d.instructions.clone(),
                    on_fail: d.on_fail,
                    max_rounds: d.max_rounds,
                    lane: d.lane,
                    agents: d
                        .agents
                        .iter()
                        .map(|(c, a)| (c.as_str().to_string(), a.to_string()))
                        .collect(),
                    inputs: d.inputs.clone(),
                    args: d.args.clone(),
                    when: d.when.clone(),
                    when_mode: d.when_mode,
                    output_schema: d.output_schema.clone(),
                    ..Default::default()
                };
                match &d.run {
                    StageRun::Builtin { stage } => f.uses = Some(stage.uses()),
                    StageRun::Agent { agent } => f.agent = Some(agent.to_string()),
                    StageRun::Plugin { plugin, stage } => {
                        f.plugin = Some(format!("{plugin}:{stage}"))
                    }
                    StageRun::Transform { function } => f.transform = Some(function.clone()),
                    StageRun::Prompt { tier, effort } => {
                        f.prompt = f.instructions.take();
                        f.tier = *tier;
                        f.effort = *effort;
                    }
                }
                f
            })
            .collect();
        WorkflowFile {
            description: def.description.clone(),
            base: Some(category_name(def.base)),
            default_for,
            track: def.track,
            layout,
            stages,
            ..Default::default()
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }
}

/// Every workflow a workspace can run: the built-in ones, then its own files.
#[derive(Debug, Clone, Default)]
pub struct WorkflowSet {
    pub files: BTreeMap<String, WorkflowFile>,
    /// Rule WB7: the composite functions in `.ostra/transforms/`.
    pub functions: crate::transform::Functions,
    /// Rule PL6: the workflows the workspace's plugins build, by `<plugin>:<name>`.
    pub plugin_files: BTreeMap<String, WorkflowFile>,
    /// Rule PL7: the transform functions the workspace's plugins run, by `<plugin>:<name>`.
    pub plugin_transforms: crate::transform::PluginTransforms,
}

impl WorkflowSet {
    /// Read `<workspace>/.ostra/workflows/*.toml`. Unreadable files come back as issues.
    pub fn load(workspace: &std::path::Path) -> (WorkflowSet, Vec<(String, String)>) {
        let dir = crate::paths::workspace_workflows_dir(workspace);
        let mut set = WorkflowSet::default();
        let mut issues = vec![];
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    toml::from_str::<WorkflowFile>(&t).map_err(|e| e.message().to_string())
                });
            match parsed {
                Ok(f) if is_stage_id(&name) => {
                    set.files.insert(name, f);
                }
                Ok(_) => issues.push((
                    name.clone(),
                    format!("Name the file in lowercase kebab-case: `{name}.toml` is not."),
                )),
                Err(e) => issues.push((name, format!("Fix the file: {e}"))),
            }
        }
        set.functions = Self::load_functions(workspace).0;
        (set, issues)
    }

    /// Rule WB7: read `<workspace>/.ostra/transforms/*.toml`. Unreadable files come back as issues.
    pub fn load_functions(
        workspace: &std::path::Path,
    ) -> (crate::transform::Functions, Vec<(String, String)>) {
        let dir = crate::paths::workspace_transforms_dir(workspace);
        let mut out = crate::transform::Functions::new();
        let mut issues = vec![];
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    toml::from_str::<crate::transform::FunctionFile>(&t)
                        .map_err(|e| e.message().to_string())
                });
            match parsed {
                Ok(f) if is_stage_id(&name) => {
                    out.insert(name, f);
                }
                Ok(_) => issues.push((
                    name.clone(),
                    format!("Name the file in lowercase kebab-case: `{name}.toml` is not."),
                )),
                Err(e) => issues.push((name, format!("Fix the file: {e}"))),
            }
        }
        (out, issues)
    }

    /// Rule PL6: add the workflows and transform functions of plugin `plugin`, each named
    /// `<plugin>:<name>`.
    pub fn add_plugin(&mut self, plugin: &str, manifest: &crate::plugin::PluginManifest) {
        for w in &manifest.workflows {
            self.plugin_files
                .insert(format!("{plugin}:{}", w.name), w.workflow.clone());
        }
        for t in &manifest.transforms {
            self.plugin_transforms.insert(
                format!("{plugin}:{}", t.name),
                crate::transform::TransformInfo {
                    plugin: Some(plugin.to_string()),
                    custom: false,
                    ..t.clone()
                },
            );
        }
    }

    /// The names a user can pick: the default workflows and the workspace's own.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = BUILTIN_BASES.iter().map(|c| category_name(*c)).collect();
        v.extend(
            self.files
                .keys()
                .filter(|k| !v.contains(k))
                .cloned()
                .collect::<Vec<_>>(),
        );
        v
    }

    /// Rule WF9: the default workflows the workspace has no copy of.
    pub fn missing_defaults(&self) -> Vec<String> {
        BUILTIN_BASES
            .iter()
            .map(|c| category_name(*c))
            .filter(|n| !self.files.contains_key(n))
            .collect()
    }

    /// The workflow a session of `category` runs when the user named none: the workspace file
    /// whose `default_for` lists it, else the one named after the category: the workspace's copy,
    /// or Ostra's default (Rule WF9).
    pub fn default_for(&self, category: Category) -> Result<WorkflowDef, String> {
        let mut own = self.files.iter().filter(|(_, f)| {
            f.default_for
                .iter()
                .any(|c| parse_category(c) == Some(category))
        });
        match (own.next(), own.next()) {
            (Some((name, _)), None) => self.resolve(name),
            (Some((a, _)), Some((b, _))) => Err(format!(
                "Keep `{}` in the `default_for` of one workflow only: `{a}` and `{b}` both list it.",
                category_name(category)
            )),
            (None, _) => self.resolve(&category_name(category)),
        }
    }

    /// Rule WF1: the workflow named `name`, with every `extends` and `remove` applied.
    pub fn resolve(&self, name: &str) -> Result<WorkflowDef, String> {
        self.resolve_depth(name, 0)
    }

    fn resolve_depth(&self, name: &str, depth: usize) -> Result<WorkflowDef, String> {
        if depth > 8 {
            return Err(format!(
                "Shorten the `extends` chain of `{name}`: it goes deeper than 8 workflows or loops."
            ));
        }
        // Rule WF9: a workspace file wins over Ostra's default of the same name, and `ostra:<name>`
        // always names the default.
        let shipped = name.strip_prefix(DEFAULT_PREFIX);
        let file = match shipped {
            Some(n) => default_file(n),
            // Rule PL6: `<plugin>:<name>` is a workflow a plugin builds.
            None if name.contains(':') => self.plugin_files.get(name),
            None => self.files.get(name).or_else(|| default_file(name)),
        }
        .ok_or_else(|| {
            format!(
                "Name a workflow of this workspace, of its plugins, or one of Ostra's defaults: `{name}` is none of them."
            )
        })?;
        let name = shipped.unwrap_or(name);
        let parent = match &file.extends {
            Some(p) if p == name => {
                return Err(format!(
                    "Workflow `{name}` cannot extend itself. To start from Ostra's default, write `extends = \"{DEFAULT_PREFIX}{name}\"`."
                ));
            }
            Some(p) => Some(self.resolve_depth(p, depth + 1)?),
            None => None,
        };
        let base = match (&file.base, &parent) {
            (Some(b), _) => parse_category(b).ok_or_else(|| {
                format!(
                    "Set `base` to a pipeline such as `implement`, `research`, `spec`, `plan`, `verify`, `test`, `docs`, `prompt`, or `quick-change`: `{b}` is not one."
                )
            })?,
            (None, Some(p)) => p.base,
            (None, None) => {
                return Err(format!(
                    "Give workflow `{name}` a `base` pipeline or a workflow to `extends`."
                ));
            }
        };
        // Rule WF9: sessions of a category run the workflow named after it, so it keeps that base.
        if let Some(c) = BUILTIN_BASES.iter().find(|c| category_name(**c) == name)
            && *c != base
        {
            return Err(format!(
                "Keep `base = \"{name}\"` in workflow `{name}`, because sessions of that category run it. Save a workflow with another base under another name."
            ));
        }
        if let Some(p) = &parent
            && p.base != base
        {
            return Err(format!(
                "Keep the `base` of `{name}` equal to that of `{}`, which it extends.",
                p.name
            ));
        }
        let mut stages: Vec<StageDef> = parent
            .as_ref()
            .map(|p| p.stages.clone())
            .unwrap_or_default();
        for r in &file.remove {
            let before = stages.len();
            stages.retain(|s| &s.id != r);
            if stages.len() == before {
                return Err(format!(
                    "Remove only stages the extended workflow has: `{r}` is not one."
                ));
            }
            // Whatever waited for the removed stage now waits for what it waited for.
            let inherited: Vec<String> = parent
                .as_ref()
                .and_then(|p| p.stage(r))
                .map(|s| s.after.clone())
                .unwrap_or_default();
            for s in &mut stages {
                if s.after.contains(r) {
                    s.after.retain(|a| a != r);
                    for a in &inherited {
                        if !s.after.contains(a) {
                            s.after.push(a.clone());
                        }
                    }
                }
            }
        }
        let tail: Option<String> = stages.last().map(|s| s.id.clone());
        let mut prev: Option<String> = if parent.is_some() { tail } else { None };
        let mut befores: Vec<(String, Vec<String>)> = vec![];
        for f in &file.stages {
            let picked = [
                f.uses.is_some(),
                f.agent.is_some(),
                f.plugin.is_some(),
                f.transform.is_some(),
                f.prompt.is_some(),
            ]
            .iter()
            .filter(|x| **x)
            .count();
            if picked != 1 {
                return Err(format!(
                    "Give stage `{}` exactly one of `uses`, `agent`, `plugin`, `transform`, or `prompt`.",
                    f.id
                ));
            }
            if (f.tier.is_some() || f.effort.is_some()) && f.prompt.is_none() {
                return Err(format!(
                    "Remove `tier` and `effort` from stage `{}`: only a prompt node sets them; an agent sets its own.",
                    f.id
                ));
            }
            if f.prompt.is_some() && f.instructions.is_some() {
                return Err(format!(
                    "Put the whole prompt of node `{}` in `prompt` and remove `instructions`.",
                    f.id
                ));
            }
            let run = match (&f.uses, &f.agent, &f.plugin) {
                _ if f.transform.is_some() => StageRun::Transform {
                    function: f.transform.clone().unwrap_or_default().trim().to_string(),
                },
                _ if f.prompt.is_some() => StageRun::Prompt {
                    tier: f.tier,
                    effort: f.effort,
                },
                (Some(u), None, None) => StageRun::Builtin {
                    stage: BuiltinStage::parse_uses(u).ok_or_else(|| {
                        format!(
                            "Set `uses` of stage `{}` to a built-in stage such as `ostra:build`: `{u}` is not one.",
                            f.id
                        )
                    })?,
                },
                (None, Some(a), None) => StageRun::Agent {
                    agent: a.parse().map_err(|_| {
                        format!("Name an agent in stage `{}`: `{a}` is not a valid agent name.", f.id)
                    })?,
                },
                (None, None, Some(p)) => {
                    let (plugin, stage) = p.split_once(':').ok_or_else(|| {
                        format!(
                            "Write `plugin` of stage `{}` as `<plugin>:<stage>`: `{p}` is not.",
                            f.id
                        )
                    })?;
                    StageRun::Plugin {
                        plugin: plugin.trim().into(),
                        stage: stage.trim().into(),
                    }
                }
                _ => unreachable!("exactly one kind was checked above"),
            };
            let after = match (&f.after, f.scope) {
                (Some(a), _) => a.clone(),
                // A phase stage runs inside the build, so it waits for what the build waits for.
                (None, StageScope::Phase) => stages
                    .iter()
                    .find(|s| s.builtin() == Some(BuiltinStage::Build))
                    .map(|s| s.after.clone())
                    .unwrap_or_default(),
                (None, _) => prev.iter().cloned().collect(),
            };
            if stages.iter().any(|s| s.id == f.id) {
                return Err(format!(
                    "Give stage `{}` another id: the extended workflow already has one.",
                    f.id
                ));
            }
            let mut agents = BTreeMap::new();
            for (c, a) in &f.agents {
                let StageRun::Builtin { stage } = run else {
                    return Err(format!(
                        "Remove `agents` from stage `{}`: only a built-in stage (`uses`) binds agents; a custom stage names one `agent`.",
                        f.id
                    ));
                };
                let contract: Contract = c.parse()?;
                if !stage.contracts().contains(&contract) {
                    return Err(format!(
                        "Bind only contracts stage `{}` reads ({}): `{c}` is not one.",
                        f.id,
                        stage
                            .contracts()
                            .iter()
                            .map(|c| c.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                let agent: AgentName = a.parse().map_err(|_| {
                    format!(
                        "Name an agent for `{c}` in stage `{}`: `{a}` is not a valid agent name.",
                        f.id
                    )
                })?;
                agents.insert(contract, agent);
            }
            stages.push(StageDef {
                id: f.id.clone(),
                run,
                after,
                scope: f.scope,
                instructions: f
                    .prompt
                    .clone()
                    .or_else(|| f.instructions.clone())
                    .filter(|i| !i.trim().is_empty()),
                on_fail: f.on_fail,
                max_rounds: f.max_rounds,
                lane: f.lane,
                agents,
                inputs: f.inputs.clone(),
                args: f.args.clone(),
                when: f.when.clone(),
                when_mode: f.when_mode,
                output_schema: f.output_schema.clone(),
            });
            if !f.before.is_empty() {
                befores.push((f.id.clone(), f.before.clone()));
            }
            // A stage that only runs before others, or per phase, does not make the next one wait
            // for it.
            if f.before.is_empty() && f.scope != StageScope::Phase {
                prev = Some(f.id.clone());
            }
        }
        for (id, later) in befores {
            for l in later {
                let Some(s) = stages.iter_mut().find(|s| s.id == l) else {
                    return Err(format!(
                        "Stage `{id}` runs before `{l}`, which is not a stage of this workflow."
                    ));
                };
                if !s.after.contains(&id) {
                    s.after.push(id.clone());
                }
            }
        }
        for (c, a) in &file.agents {
            let contract: Contract = c.parse()?;
            let agent: AgentName = a.parse().map_err(|_| {
                format!(
                    "Name an agent for `{c}` under `[agents]`: `{a}` is not a valid agent name."
                )
            })?;
            let mut bound = false;
            for st in stages.iter_mut() {
                if let StageRun::Builtin { stage } = st.run
                    && stage.contracts().contains(&contract)
                {
                    st.agents.entry(contract).or_insert(agent);
                    bound = true;
                }
            }
            if !bound {
                return Err(format!(
                    "Remove `{c}` from `[agents]`: no built-in stage of this workflow reads it."
                ));
            }
        }
        let wf = WorkflowDef {
            name: name.to_string(),
            description: if file.description.is_empty() {
                parent.map(|p| p.description).unwrap_or_default()
            } else {
                file.description.clone()
            },
            base,
            track: file.track.or(parent_track(self, file)),
            plugin_transforms: stages
                .iter()
                .filter_map(|d| match &d.run {
                    StageRun::Transform { function } => self
                        .plugin_transforms
                        .get(function)
                        .map(|t| (function.clone(), t.clone())),
                    _ => None,
                })
                .collect(),
            functions: crate::transform::functions_used(
                stages.iter().filter_map(|d| match &d.run {
                    StageRun::Transform { function } => Some(function.as_str()),
                    _ => None,
                }),
                &self.functions,
            ),
            stages,
        };
        let issues = wf.validate();
        if !issues.is_empty() {
            return Err(format!("Workflow `{name}`: {}", issues.join(" ")));
        }
        Ok(wf)
    }
}

fn parent_track(set: &WorkflowSet, file: &WorkflowFile) -> Option<Track> {
    file.extends
        .as_deref()
        .and_then(|p| set.files.get(p))
        .and_then(|p| p.track)
}

/// The base pipelines a workflow can name, as built-in workflows.
pub const BUILTIN_BASES: [Category; 9] = [
    Category::Research,
    Category::Spec,
    Category::Plan,
    Category::Implement,
    Category::Verify,
    Category::Test,
    Category::Docs,
    Category::Prompt,
    Category::QuickChange,
];

/// A workflow as the browser lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowInfo {
    pub name: String,
    pub description: String,
    pub base: Category,
    /// The categories it runs for by default.
    pub default_for: Vec<Category>,
    pub builtin: bool,
    /// Rule PL6: the plugin that builds it in code, which makes it read only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plugin: Option<String>,
    pub stages: Vec<StageDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(files: &[(&str, &str)]) -> WorkflowSet {
        WorkflowSet {
            files: files
                .iter()
                .map(|(n, t)| (n.to_string(), toml::from_str(t).unwrap()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn builtins_are_valid_chains() {
        for c in BUILTIN_BASES {
            let w = WorkflowDef::builtin(c);
            assert!(w.validate().is_empty(), "{c}: {:?}", w.validate());
            assert_eq!(w.ordered().len(), w.stages.len());
        }
        assert_eq!(parse_category("quick-change"), Some(Category::QuickChange));
        assert_eq!(parse_category("IMPLEMENT"), Some(Category::Implement));
    }

    /// Rule WF9: the defaults are TOML files, the chains come from them, and a workspace copy wins.
    #[test]
    fn wf9_defaults_come_from_toml_and_a_workspace_copy_wins() {
        assert_eq!(
            builtin_chain(Category::Implement),
            BuiltinStage::ALL.to_vec(),
            "the implement default lists every built-in stage in order"
        );
        assert_eq!(builtin_chain(Category::Test), vec![BuiltinStage::Closing]);
        assert!(builtin_chain(Category::QuickAnswer).is_empty());
        let empty = WorkflowSet::default();
        assert_eq!(empty.missing_defaults().len(), 9);
        let own = set(&[(
            "research",
            "base = \"research\"\ndescription = \"Ours.\"\n[[stage]]\nid = \"research\"\nuses = \"ostra:research\"\n[[stage]]\nid = \"notes\"\nagent = \"x\"\n",
        )]);
        let w = own.default_for(Category::Research).unwrap();
        assert_eq!(w.description, "Ours.");
        assert!(w.stage("notes").is_some());
        assert!(!own.missing_defaults().contains(&"research".to_string()));
        assert!(
            own.resolve("ostra:research")
                .unwrap()
                .stage("notes")
                .is_none()
        );
        let extended = set(&[(
            "research",
            "extends = \"ostra:research\"\n[[stage]]\nid = \"notes\"\nagent = \"x\"\n",
        )]);
        assert_eq!(extended.resolve("research").unwrap().stages.len(), 2);
        let e = set(&[("research", "extends = \"research\"")])
            .resolve("research")
            .unwrap_err();
        assert!(e.contains("ostra:research"), "{e}");
        let e = set(&[(
            "implement",
            "base = \"research\"\n[[stage]]\nid = \"research\"\nuses = \"ostra:research\"",
        )])
        .resolve("implement")
        .unwrap_err();
        assert!(e.contains("Keep `base = \"implement\"`"), "{e}");
    }

    #[test]
    fn extends_inserts_removes_and_reorders() {
        let s = set(&[(
            "secure",
            r#"
extends = "implement"
default_for = ["implement"]
remove = ["feedback"]

[[stage]]
id = "audit"
agent = "security-auditor"
after = ["build"]
before = ["closing"]
scope = "project"
on_fail = "retry"

[[stage]]
id = "notes"
agent = "release-notes"
after = ["build"]
"#,
        )]);
        let w = s.default_for(Category::Implement).unwrap();
        assert_eq!(w.name, "secure");
        assert!(w.stage("feedback").is_none());
        assert_eq!(w.stage("closing").unwrap().after, vec!["build", "audit"]);
        assert_eq!(w.stage("notes").unwrap().after, vec!["build"]);
        assert_eq!(w.stage("audit").unwrap().scope, StageScope::Project);
        let order: Vec<&str> = w.ordered().iter().map(|s| s.id.as_str()).collect();
        let pos = |id| order.iter().position(|x| *x == id).unwrap();
        assert!(pos("audit") < pos("closing") && pos("build") < pos("audit"));
        assert!(
            s.default_for(Category::Research)
                .unwrap()
                .stage("research")
                .is_some()
        );
    }

    #[test]
    fn broken_workflows_are_refused_with_the_fix() {
        let bad = |t: &str| set(&[("w", t)]).resolve("w").unwrap_err();
        assert!(bad("extends = \"implement\"\nremove = [\"plan\"]").contains("needs it"));
        assert!(bad("extends = \"implement\"\nremove = [\"track\"]").contains("set `track`"));
        assert!(
            set(&[(
                "w",
                "extends = \"implement\"\nremove = [\"track\"]\ntrack = \"full\""
            )])
            .resolve("w")
            .is_ok()
        );
        assert!(
            bad("base = \"implement\"\n[[stage]]\nid = \"a\"\nagent = \"x-y\"\nafter = [\"a\"]")
                .contains("itself")
        );
        assert!(bad("extends = \"implement\"\n[[stage]]\nid = \"a\"\nagent = \"x\"\nscope = \"phase\"\nafter = [\"build\"]").contains("unordered"));
        assert!(
            bad("extends = \"research\"\n[[stage]]\nid = \"a\"\nagent = \"x\"\nscope = \"phase\"")
                .contains("no build stage")
        );
        assert!(bad("extends = \"w\"").contains("itself"));
        assert!(bad("extends = \"nope\"").contains("none of them"));
        let two = set(&[
            ("a", "extends = \"research\"\ndefault_for = [\"research\"]"),
            ("b", "extends = \"research\"\ndefault_for = [\"research\"]"),
        ]);
        assert!(two.default_for(Category::Research).is_err());
    }

    #[test]
    fn wf8_built_in_stages_bind_agents_by_contract() {
        let s = set(&[(
            "w",
            "extends = \"implement\"\n[agents]\nreview = \"strict-reviewer\"\nspec = \"my-writer\"\n",
        )]);
        let w = s.resolve("w").unwrap();
        let reviewer: AgentName = "strict-reviewer".parse().unwrap();
        assert_eq!(
            w.stage("build").unwrap().agents[&Contract::Review],
            reviewer
        );
        assert_eq!(
            w.stage("closing").unwrap().agents[&Contract::Review],
            reviewer
        );
        assert!(
            w.stage("spec")
                .unwrap()
                .agents
                .contains_key(&Contract::Spec)
        );
        let bad = set(&[("w", "extends = \"research\"\n[agents]\nreview = \"x\"\n")]);
        assert!(bad.resolve("w").unwrap_err().contains("no built-in stage"));
        let wrong = set(&[(
            "w",
            "base = \"research\"\n[[stage]]\nid = \"research\"\nuses = \"ostra:research\"\nagents = { spec = \"x\" }\n",
        )]);
        assert!(wrong.resolve("w").unwrap_err().contains("reads"));
    }

    #[test]
    fn a_phase_stage_defaults_beside_the_build() {
        let s = set(&[(
            "w",
            "extends = \"implement\"\n[[stage]]\nid = \"per-phase\"\nagent = \"x\"\nscope = \"phase\"\n[[stage]]\nid = \"last\"\nagent = \"y\"",
        )]);
        let w = s.resolve("w").unwrap();
        assert_eq!(w.stage("per-phase").unwrap().after, vec!["plan"]);
        assert_eq!(w.stage("last").unwrap().after, vec!["closing"]);
    }

    const BUILDER: &str = r#"
extends = "implement"

[[stage]]
id = "audit"
agent = "security-auditor"
after = ["build"]
before = ["closing"]

[[stage]]
id = "high"
transform = "filter"
after = ["audit"]
before = ["closing"]
inputs = { items = "audit.findings" }
args = { field = "data.risk", op = "eq", to = "high" }

[[stage]]
id = "triage"
prompt = "Group the findings by the fix they need."
tier = "fast"
after = ["high"]
before = ["closing"]
inputs = { findings = "high.output" }
when = [{ ref = "high.output", op = "not_empty" }]
output_schema = { type = "object", required = ["groups"], properties = { groups = { type = "array" } } }

[layout]
audit = [10.0, 20.0]
"#;

    /// Rules WB1 to WB5: transforms, prompts, inputs, and conditions resolve, and a resolved
    /// workflow written out whole resolves to the same workflow.
    #[test]
    fn wb_data_nodes_resolve_and_round_trip() {
        let s = set(&[("w", BUILDER)]);
        let w = s.resolve("w").unwrap();
        let high = w.stage("high").unwrap();
        assert_eq!(
            high.run,
            StageRun::Transform {
                function: "filter".into()
            }
        );
        let triage = w.stage("triage").unwrap();
        assert!(matches!(
            triage.run,
            StageRun::Prompt {
                tier: Some(Tier::Fast),
                ..
            }
        ));
        assert_eq!(
            triage.instructions.as_deref(),
            Some("Group the findings by the fix they need.")
        );
        assert_eq!(triage.when.len(), 1);
        assert_eq!(s.files["w"].layout["audit"], [10.0, 20.0]);

        let file = WorkflowFile::from_def(&w, vec![], s.files["w"].layout.clone());
        let text = file.to_toml().unwrap();
        let back = set(&[("w", text.as_str())]).resolve("w").unwrap();
        assert_eq!(back, w, "{text}");
    }

    #[test]
    fn wb_broken_data_nodes_are_refused_with_the_fix() {
        let bad = |t: &str| set(&[("w", t)]).resolve("w").unwrap_err();
        let base = "extends = \"research\"\n";
        let e = bad(&format!(
            "{base}[[stage]]\nid = \"n\"\ntransform = \"count\"\ninputs = {{ items = \"later.output\" }}\n[[stage]]\nid = \"later\"\ntransform = \"constant\"\nargs = {{ value = 1 }}\nafter = [\"n\"]"
        ));
        assert!(e.contains("wait for `later`"), "{e}");
        let e = bad(&format!(
            "{base}[[stage]]\nid = \"p\"\nprompt = \"x\"\noutput_schema = {{ type = \"string\" }}"
        ));
        assert!(e.contains("object schema"), "{e}");
        let e = bad(&format!("{base}[[stage]]\nid = \"p\"\nprompt = \"x\""));
        assert!(e.contains("output_schema"), "{e}");
        let e = bad(&format!(
            "{base}[[stage]]\nid = \"a\"\nagent = \"x\"\nwhen = [{{ ref = \"research.x\", op = \"eq\" }}]"
        ));
        assert!(e.contains("a `value`"), "{e}");
        let e = bad(&format!(
            "{base}[[stage]]\nid = \"a\"\nagent = \"x\"\nargs = {{ k = 1 }}"
        ));
        assert!(e.contains("only a transform"), "{e}");
        let e = bad(&format!(
            "{base}[[stage]]\nid = \"a\"\nagent = \"x\"\ntransform = \"count\""
        ));
        assert!(e.contains("exactly one"), "{e}");
        let e = bad(
            "base = \"research\"\n[[stage]]\nid = \"research\"\nuses = \"ostra:research\"\nwhen = [{ ref = \"session.track\", op = \"exists\" }]",
        );
        assert!(e.contains("built-in stages always run"), "{e}");
        // Session facts need no stage to wait for.
        assert!(
            set(&[(
                "w",
                "extends = \"research\"\n[[stage]]\nid = \"a\"\nagent = \"x\"\nwhen = [{ ref = \"session.category\", op = \"eq\", value = \"RESEARCH\" }]"
            )])
            .resolve("w")
            .is_ok()
        );
    }

    #[test]
    fn a_phase_stage_sits_beside_the_build() {
        let s = set(&[(
            "w",
            "extends = \"implement\"\n[[stage]]\nid = \"per-phase\"\nagent = \"x\"\nscope = \"phase\"\nafter = [\"plan\"]",
        )]);
        let w = s.resolve("w").unwrap();
        assert_eq!(w.stage("per-phase").unwrap().after, vec!["plan"]);
    }
}
