//! Rules PL6 and PL7: build workflows and transform functions in Rust code. A plugin lists them
//! in its manifest; Ostra shows them in the console, read only, and runs them like the
//! workspace's own. A workflow is named `<plugin>:<name>` there, and so is a transform function.
//!
//! ```
//! use ostra_sdk::workflow::{Node, TransformFn, Workflow};
//! use ostra_sdk::{CondOp, ValueKind};
//!
//! let flow = Workflow::extending("release-check", "ostra:implement")
//!     .description("The implement pipeline with a release check after the build.")
//!     .node(Node::agent("audit", "security-auditor").after(["build"]).before(["closing"]))
//!     .node(
//!         Node::transform("high", "release:high-risk")
//!             .after(["audit"])
//!             .before(["closing"])
//!             .input("findings", "audit.findings"),
//!     )
//!     .node(
//!         Node::plugin_stage("gate", "release:gate")
//!             .after(["high"])
//!             .before(["closing"])
//!             .when("high.output", CondOp::NotEmpty, None),
//!     )
//!     .build();
//! let high_risk = TransformFn::new("high-risk", "Findings whose file is under src/auth.")
//!     .input("findings", ValueKind::Array, "Findings with a file.")
//!     .output(ValueKind::Array)
//!     .build();
//! assert_eq!(flow.name, "release-check");
//! assert_eq!(high_risk.inputs.len(), 1);
//! ```

use ostra_core::plugin::PluginWorkflow;
use ostra_core::transform::{CondOp, Condition, TransformInfo, TransformParam, ValueKind};
use ostra_core::workflow::{OnFail, StageFile, StageScope, WorkflowFile};
use serde_json::Value;

/// A workflow under construction.
pub struct Workflow {
    name: String,
    file: WorkflowFile,
}

impl Workflow {
    /// A workflow on base pipeline `base` (`implement`, `research`, ...), with every node listed.
    pub fn new(name: &str, base: &str) -> Self {
        Workflow {
            name: name.into(),
            file: WorkflowFile {
                base: Some(base.into()),
                ..Default::default()
            },
        }
    }

    /// A workflow that starts from another: `ostra:<base>` for Ostra's default, or another
    /// plugin workflow as `<plugin>:<name>`.
    pub fn extending(name: &str, parent: &str) -> Self {
        Workflow {
            name: name.into(),
            file: WorkflowFile {
                extends: Some(parent.into()),
                ..Default::default()
            },
        }
    }

    pub fn description(mut self, text: &str) -> Self {
        self.file.description = text.into();
        self
    }

    /// Leave out a node of the workflow it extends.
    pub fn remove(mut self, id: &str) -> Self {
        self.file.remove.push(id.into());
        self
    }

    /// Fix the implement pipeline's track (`light` or `full`) in place of the Track judge.
    pub fn track(mut self, track: ostra_core::pipeline::Track) -> Self {
        self.file.track = Some(track);
        self
    }

    /// Bind an agent to a contract in every built-in stage that reads it.
    pub fn bind(mut self, contract: &str, agent: &str) -> Self {
        self.file.agents.insert(contract.into(), agent.into());
        self
    }

    pub fn node(mut self, node: Node) -> Self {
        self.file.stages.push(node.0);
        self
    }

    pub fn build(self) -> PluginWorkflow {
        PluginWorkflow {
            name: self.name,
            workflow: self.file,
        }
    }
}

/// One node of a workflow.
pub struct Node(StageFile);

impl Node {
    fn of(id: &str, f: impl FnOnce(&mut StageFile)) -> Self {
        let mut s = StageFile {
            id: id.into(),
            max_rounds: 3,
            ..Default::default()
        };
        f(&mut s);
        Node(s)
    }

    /// One of Ostra's stages: `research`, `track`, `spec`, `stakes`, `plan`, `build`,
    /// `feedback`, or `closing`.
    pub fn stage(id: &str, stage: &str) -> Self {
        Node::of(id, |s| s.uses = Some(format!("ostra:{stage}")))
    }

    pub fn agent(id: &str, agent: &str) -> Self {
        Node::of(id, |s| s.agent = Some(agent.into()))
    }

    /// A stage driven by a plugin's stage logic, `<plugin>:<stage>`.
    pub fn plugin_stage(id: &str, stage: &str) -> Self {
        Node::of(id, |s| s.plugin = Some(stage.into()))
    }

    /// A transform function: Ostra's, the workspace's, or a plugin's as `<plugin>:<name>`.
    pub fn transform(id: &str, function: &str) -> Self {
        Node::of(id, |s| s.transform = Some(function.into()))
    }

    /// One model call whose answer matches `output_schema`, an object schema.
    pub fn prompt(id: &str, prompt: &str, output_schema: Value) -> Self {
        Node::of(id, |s| {
            s.prompt = Some(prompt.into());
            s.output_schema = Some(output_schema);
        })
    }

    pub fn after<const N: usize>(mut self, ids: [&str; N]) -> Self {
        self.0.after = Some(ids.iter().map(|s| s.to_string()).collect());
        self
    }

    pub fn before<const N: usize>(mut self, ids: [&str; N]) -> Self {
        self.0.before = ids.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Wire input `name` from a reference such as `audit.findings`.
    pub fn input(mut self, name: &str, reference: &str) -> Self {
        self.0.inputs.insert(name.into(), reference.into());
        self
    }

    pub fn arg(mut self, name: &str, value: Value) -> Self {
        self.0.args.insert(name.into(), value);
        self
    }

    /// Run only when `reference op value` holds; several conditions must all hold.
    pub fn when(mut self, reference: &str, op: CondOp, value: Option<Value>) -> Self {
        self.0.when.push(Condition {
            reference: reference.into(),
            op,
            value,
        });
        self
    }

    pub fn instructions(mut self, text: &str) -> Self {
        self.0.instructions = Some(text.into());
        self
    }

    pub fn scope(mut self, scope: StageScope) -> Self {
        self.0.scope = scope;
        self
    }

    pub fn on_fail(mut self, on_fail: OnFail, max_rounds: u32) -> Self {
        self.0.on_fail = on_fail;
        self.0.max_rounds = max_rounds;
        self
    }

    /// Bind an agent to a contract this built-in stage reads.
    pub fn bind(mut self, contract: &str, agent: &str) -> Self {
        self.0.agents.insert(contract.into(), agent.into());
        self
    }
}

/// What a plugin transform function takes and gives, for the manifest. [`crate::Plugin::transform`]
/// runs it.
pub struct TransformFn(TransformInfo);

impl TransformFn {
    pub fn new(name: &str, description: &str) -> Self {
        TransformFn(TransformInfo {
            name: name.into(),
            description: description.into(),
            inputs: vec![],
            variadic: false,
            args: vec![],
            output: ValueKind::Any,
            custom: false,
            plugin: None,
        })
    }

    pub fn input(mut self, name: &str, kind: ValueKind, description: &str) -> Self {
        self.0.inputs.push(param(name, kind, true, description));
        self
    }

    pub fn optional_input(mut self, name: &str, kind: ValueKind, description: &str) -> Self {
        self.0.inputs.push(param(name, kind, false, description));
        self
    }

    pub fn arg(mut self, name: &str, kind: ValueKind, description: &str) -> Self {
        self.0.args.push(param(name, kind, true, description));
        self
    }

    pub fn optional_arg(mut self, name: &str, kind: ValueKind, description: &str) -> Self {
        self.0.args.push(param(name, kind, false, description));
        self
    }

    pub fn output(mut self, kind: ValueKind) -> Self {
        self.0.output = kind;
        self
    }

    pub fn build(self) -> TransformInfo {
        self.0
    }
}

fn param(name: &str, kind: ValueKind, required: bool, description: &str) -> TransformParam {
    TransformParam {
        name: name.into(),
        kind,
        required,
        description: description.into(),
    }
}
