//! Rule PL4: Ostra's own agents as the standard plugin `ostra`, written with `ostra-sdk` like any
//! other plugin. Each agent is its embedded `assets/agents/<name>/agent.toml` and `prompt.md`,
//! read with the SDK's definition parser and nothing else: its contract, write scope, brief, and
//! grants are all in its files, in the same fields any agent declares (Rules CA5 and CA6).
//!
//! The catalog takes the built-in agents from this plugin, so a built-in agent, a workspace's
//! markdown agent, and a plugin's agent are the same `PluginAgent` until they become definitions,
//! and the engine binds each built-in stage to an agent by contract, defaulting to these.

use crate::{AgentsError, asset_text};
use ostra_core::AgentName;
use ostra_sdk::{Plugin, PluginAgent, PluginManifest, STANDARD_PLUGIN};
use std::sync::OnceLock;

/// The standard plugin.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standard;

impl Standard {
    /// One built-in agent, from its files.
    pub fn agent(agent: AgentName) -> Result<PluginAgent, AgentsError> {
        let dir = format!("agents/{}", agent.as_str());
        let toml_path = format!("{dir}/agent.toml");
        let prompt = asset_text(&format!("{dir}/prompt.md"))?;
        ostra_sdk::definition::parse_toml(agent.as_str(), &asset_text(&toml_path)?, &prompt)
            .map_err(|message| AgentsError::Parse {
                path: toml_path,
                message,
            })
    }

    /// Rule PL4: the standard agent that returns `contract`, which a built-in stage runs when its
    /// workflow binds no other.
    pub fn default_for(contract: ostra_core::Contract) -> Option<AgentName> {
        Standard::manifest_ref()
            .agents
            .iter()
            .find(|a| a.returns.as_deref() == Some(contract.as_str()))
            .and_then(|a| AgentName::builtin(&a.name))
    }

    fn build() -> Result<PluginManifest, AgentsError> {
        Ok(PluginManifest {
            workflows: vec![],
            transforms: vec![],
            name: STANDARD_PLUGIN.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Ostra's own agents: research, spec, fact-check, plan, build, review, test, docs, setup, and rescue.".into(),
            agents: AgentName::ALL
                .into_iter()
                .map(Standard::agent)
                .collect::<Result<_, _>>()?,
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
            Standard::build()
                .unwrap_or_else(|e| panic!("embedded agent definitions are invalid: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl Plugin for Standard {
    fn manifest(&self) -> PluginManifest {
        Standard::manifest_ref().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for c in ostra_core::Contract::BUILTIN {
            if c != ostra_core::Contract::Stage {
                assert!(Standard::default_for(c).is_some(), "{c}");
            }
        }
        // It is a plain manifest, so it crosses the stdio protocol unchanged.
        let wire: PluginManifest =
            serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(wire, m);
    }
}
