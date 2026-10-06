//! Rule PL4: the definitions of Ostra's own plugin `ostra`, written with `ostra-sdk` like any other
//! plugin. The crate `ostra-default-plugin` adds the pipeline that runs the built-in stages. This
//! crate offers Ostra's built-in agents and its default workflows, read from files and nothing else:
//!
//! - Each agent is its embedded `assets/agents/<name>/agent.toml` and `prompt.md`, read with the
//!   SDK's definition parser. Its contract, write scope, brief, and grants are all in its files, in
//!   the same fields any agent declares (Rules CA5 and CA6).
//! - Each default workflow is its embedded `assets/workflows/<base>.toml` (Rule WF9), offered as
//!   one of the plugin's workflows, so a workflow names it `ostra:<base>`.
//!
//! The catalog takes the built-in agents from this plugin, so a built-in agent, a workspace's
//! markdown agent, and a plugin's agent are the same `PluginAgent` until they become definitions,
//! and the engine binds each built-in stage to an agent by contract, defaulting to these.

use ostra_core::pipeline::Category;
use ostra_core::workflow::{WorkflowDef, WorkflowFile, WorkflowSet, category_name};
use ostra_core::{AgentName, Contract};
use ostra_sdk::{Plugin, PluginAgent, PluginManifest, PluginWorkflow, STANDARD_PLUGIN};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "../../assets/agents"]
struct AgentFiles;

/// Rule WF9: the default workflows, one TOML file per base pipeline.
const WORKFLOWS: [(Category, &str); 9] = [
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

/// The standard plugin.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standard;

impl Standard {
    /// One built-in agent, from its files.
    pub fn agent(agent: AgentName) -> Result<PluginAgent, String> {
        let toml_path = format!("{agent}/agent.toml");
        let prompt = agent_file(&format!("{agent}/prompt.md"))?;
        ostra_sdk::definition::parse_toml(agent.as_str(), &agent_file(&toml_path)?, &prompt)
            .map_err(|e| format!("assets/agents/{toml_path}: {e}"))
    }

    /// Rule PL4: the standard agent that returns `contract`, which a built-in stage runs when its
    /// workflow binds no other.
    pub fn default_for(contract: Contract) -> Option<AgentName> {
        Standard::manifest_ref()
            .agents
            .iter()
            .find(|a| a.returns.as_deref() == Some(contract.as_str()))
            .and_then(|a| AgentName::builtin(&a.name))
    }

    fn build() -> Result<PluginManifest, String> {
        Ok(PluginManifest {
            name: STANDARD_PLUGIN.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Ostra's own agents and default workflows: research, spec, fact-check, plan, build, review, test, docs, setup, and rescue.".into(),
            agents: AgentName::ALL
                .into_iter()
                .map(Standard::agent)
                .collect::<Result<_, _>>()?,
            workflows: WORKFLOWS
                .iter()
                .map(|(c, text)| {
                    let name = category_name(*c);
                    toml::from_str::<WorkflowFile>(text)
                        .map(|workflow| PluginWorkflow {
                            name: name.clone(),
                            workflow,
                        })
                        .map_err(|e| format!("assets/workflows/{name}.toml: {e}"))
                })
                .collect::<Result<_, _>>()?,
            transforms: vec![],
            // The built-in stages are the planner's own rules (`ostra:<stage>` in a workflow).
            stages: vec![],
            contracts: vec![],
        })
    }

    /// The manifest, read once. The embedded assets are checked by this crate's tests, so a broken
    /// one is a build defect, not a runtime condition.
    pub fn manifest_ref() -> &'static PluginManifest {
        static MANIFEST: OnceLock<PluginManifest> = OnceLock::new();
        MANIFEST.get_or_init(|| {
            Standard::build().unwrap_or_else(|e| panic!("embedded plugin `ostra` is invalid: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl Plugin for Standard {
    fn manifest(&self) -> PluginManifest {
        Standard::manifest_ref().clone()
    }
}

fn agent_file(path: &str) -> Result<String, String> {
    let file = AgentFiles::get(path).ok_or_else(|| format!("assets/agents/{path} is missing"))?;
    String::from_utf8(file.data.into_owned()).map_err(|e| format!("assets/agents/{path}: {e}"))
}

/// Rule WF9: add the default workflows to `set`, where `ostra:<base>` and a bare `<base>` the
/// workspace has no copy of resolve to them.
pub fn add_workflows(set: &mut WorkflowSet) {
    set.add_plugin(STANDARD_PLUGIN, Standard::manifest_ref());
}

/// A workflow set that holds only the default workflows.
pub fn workflow_set() -> WorkflowSet {
    let mut set = WorkflowSet::default();
    add_workflows(&mut set);
    set
}

/// Rule WF9: the default workflow of a base pipeline, resolved.
pub fn workflow(base: Category) -> WorkflowDef {
    static RESOLVED: OnceLock<Vec<(Category, WorkflowDef)>> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            let set = workflow_set();
            WORKFLOWS
                .iter()
                .map(|(c, _)| {
                    let name = category_name(*c);
                    let def = set
                        .resolve(&format!("{STANDARD_PLUGIN}:{name}"))
                        .unwrap_or_else(|e| panic!("the default `{name}` workflow: {e}"));
                    (*c, def)
                })
                .collect()
        })
        .iter()
        .find(|(c, _)| *c == base)
        .map(|(_, d)| d.clone())
        .unwrap_or_else(|| panic!("`{}` has no default workflow", category_name(base)))
}

/// The default workflow file's text, as a new workspace's copy is written.
pub fn workflow_text(name: &str) -> Option<&'static str> {
    WORKFLOWS
        .iter()
        .find(|(c, _)| category_name(*c) == name)
        .map(|(_, t)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::workflow::{BUILTIN_BASES, BuiltinStage};

    #[test]
    fn pl4_the_standard_plugin_defines_every_built_in_agent() {
        let m = Standard.manifest();
        assert_eq!(m.name, "ostra");
        let names: Vec<&str> = m.agents.iter().map(|a| a.name.as_str()).collect();
        let all: Vec<&str> = AgentName::ALL.iter().map(|a| a.as_str()).collect();
        assert_eq!(names, all);
        for a in &m.agents {
            assert!(
                a.prompt.as_deref().is_some_and(|p| !p.trim().is_empty()),
                "{}",
                a.name
            );
            assert!(a.returns.is_some(), "{}", a.name);
        }
        for c in Contract::BUILTIN {
            if c != Contract::Stage {
                assert!(Standard::default_for(c).is_some(), "{c}");
            }
        }
        // It is a plain manifest, so it crosses the stdio protocol unchanged.
        let wire: PluginManifest =
            serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(wire, m);
    }

    /// Rule WF9: the defaults are the plugin's workflows, one per base pipeline, and each is a
    /// valid chain.
    #[test]
    fn wf9_the_standard_plugin_offers_a_valid_default_per_base() {
        let names: Vec<String> = Standard::manifest_ref()
            .workflows
            .iter()
            .map(|w| w.name.clone())
            .collect();
        let bases: Vec<String> = BUILTIN_BASES.iter().map(|c| category_name(*c)).collect();
        assert_eq!(names, bases);
        let set = workflow_set();
        for c in BUILTIN_BASES {
            let w = workflow(c);
            assert_eq!(w.base, c);
            assert_eq!(w.ordered().len(), w.stages.len());
            assert!(workflow_text(&category_name(c)).is_some());
            assert_eq!(set.resolve(&category_name(c)).unwrap(), w);
        }
        assert_eq!(
            set.builtin_chain(Category::Implement),
            BuiltinStage::ALL.to_vec(),
            "the implement default lists every built-in stage in order"
        );
        assert_eq!(
            set.builtin_chain(Category::Test),
            vec![BuiltinStage::Closing]
        );
        assert!(set.builtin_chain(Category::QuickAnswer).is_empty());
        assert!(
            set.plugin_files.is_empty(),
            "the defaults are not listed as another plugin's workflows"
        );
    }
}
