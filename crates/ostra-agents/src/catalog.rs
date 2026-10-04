//! Rule CA1: the agents one workspace can run. The built-in agents come from the standard plugin
//! (Rule PL4); custom agents come from markdown files in the workspace's `.ostra/agents/` and from
//! plugin manifests. All three arrive as an SDK `PluginAgent` and become definitions in
//! [`from_plugin_agent`]. A catalog is
//! read fresh per execution, like other settings (pattern 7), and problems surface when the
//! workspace is validated, not when a stage spawns.

use crate::brief::Section;
use crate::{AgentDef, AgentOrigin, builtin_def};
use ostra_core::{AgentName, Capability, Contract, CustomAgent, ExecutorKind, Tier, WriteScope};
use ostra_sdk::PluginAgent;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default timeout of a custom agent without one.
pub const DEFAULT_TIMEOUT_SECS: u64 = 1200;
/// The longest a custom agent's timeout may be, as for the built-in agents that write code.
pub const MAX_TIMEOUT_SECS: u64 = 7200;
/// The largest custom agent prompt file, because the whole body goes into every run's system prompt.
pub const MAX_PROMPT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Default)]
pub struct AgentCatalog {
    custom: BTreeMap<AgentName, Arc<AgentDef>>,
}

/// One problem in a custom agent definition, with the file or plugin it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogIssue {
    pub source: String,
    pub message: String,
}

/// Capabilities a custom agent gets when its frontmatter lists none: it reads and messages.
pub const DEFAULT_CAPABILITIES: [Capability; 5] = [
    Capability::Read,
    Capability::SearchText,
    Capability::Glob,
    Capability::Report,
    Capability::Coordinate,
];

impl AgentCatalog {
    /// The built-in agents only.
    pub fn builtin() -> AgentCatalog {
        AgentCatalog::default()
    }

    /// The built-in agents, every valid markdown agent in `<workspace>/.ostra/agents/`, and the
    /// plugin agents given. Invalid definitions are left out and returned as issues.
    /// Without `workspace`, only the built-in and plugin agents load: the workspace's files wait
    /// for approval (Rule A1).
    /// `contracts` are the plugin contracts the workspace's plugins define, with their schemas
    /// (Rule PL5); an agent that returns one gets its schema, and one that returns a contract no
    /// plugin defines is an issue.
    pub fn load(
        workspace: Option<&Path>,
        plugin_agents: &[AgentDef],
        contracts: &[(Contract, serde_json::Value)],
    ) -> (AgentCatalog, Vec<CatalogIssue>) {
        let mut cat = AgentCatalog::default();
        let mut issues = vec![];
        let dir = workspace.map(ostra_core::paths::workspace_agents_dir);
        let mut files: Vec<PathBuf> = dir
            .iter()
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            let source = path.display().to_string();
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| format!("Make the file readable: {e}."))
                .and_then(|text| parse_markdown(&path, &text));
            match parsed {
                Ok(def) => {
                    if let Err(message) = cat.insert(def) {
                        issues.push(CatalogIssue { source, message });
                    }
                }
                Err(message) => issues.push(CatalogIssue { source, message }),
            }
        }
        for def in plugin_agents {
            let source = match &def.origin {
                AgentOrigin::Plugin(p) => format!("plugin {p}"),
                _ => "plugin".into(),
            };
            if let Err(message) = check_def(def).and_then(|_| cat.insert(def.clone())) {
                issues.push(CatalogIssue { source, message });
            }
        }
        let mut unknown = vec![];
        for (name, def) in cat.custom.iter_mut() {
            let Contract::Plugin(c) = def.returns else {
                continue;
            };
            match contracts.iter().find(|(k, _)| *k == def.returns) {
                Some((_, schema)) => Arc::make_mut(def).contract_schema = Some(schema.clone()),
                None => {
                    unknown.push(*name);
                    issues.push(CatalogIssue {
                        source: origin_label(&def.origin),
                        message: format!(
                            "Agent `{name}` returns `{c}`, which no plugin of the workspace defines. Add the plugin, or change `returns`.",
                            c = c.as_str()
                        ),
                    });
                }
            }
        }
        for name in unknown {
            cat.custom.remove(&name);
        }
        (cat, issues)
    }

    fn insert(&mut self, def: AgentDef) -> Result<(), String> {
        if let Some(prev) = self.custom.get(&def.name) {
            return Err(format!(
                "Rename one of the two agents named `{}`: {} defines it too.",
                def.name,
                origin_label(&prev.origin)
            ));
        }
        self.custom.insert(def.name, Arc::new(def));
        Ok(())
    }

    /// Rule AG2: this catalog with workspace agent `name` replaced by `def`, or removed when `def`
    /// is `None`, so a save can be checked before the file is written. An agent of another source
    /// with the same name is refused like a second file would be.
    pub fn with_workspace_def(
        &self,
        name: AgentName,
        def: Option<AgentDef>,
    ) -> Result<AgentCatalog, String> {
        let mut cat = self.clone();
        if cat
            .custom
            .get(&name)
            .is_some_and(|d| matches!(d.origin, AgentOrigin::Workspace(_)))
        {
            cat.custom.remove(&name);
        }
        if let Some(def) = def {
            cat.insert(def)?;
        }
        Ok(cat)
    }

    /// The definition of a built-in or custom agent of this workspace.
    pub fn def(&self, agent: AgentName) -> Option<&AgentDef> {
        match agent {
            AgentName::Custom(_) => self.custom.get(&agent).map(|d| d.as_ref()),
            a => builtin_def(a),
        }
    }

    pub fn contains(&self, agent: AgentName) -> bool {
        self.def(agent).is_some()
    }

    /// The custom agents, sorted by name.
    pub fn custom(&self) -> impl Iterator<Item = &AgentDef> {
        self.custom.values().map(|d| d.as_ref())
    }

    /// Every agent: the built-in ones first, then the custom ones.
    pub fn names(&self) -> Vec<AgentName> {
        let mut v: Vec<AgentName> = AgentName::ALL.to_vec();
        v.extend(self.custom.keys().copied());
        v
    }

    /// Rule SM6: agents `SendMessage` may start as helpers.
    pub fn helpers(&self) -> Vec<(AgentName, Contract)> {
        self.names()
            .into_iter()
            .filter_map(|a| self.def(a).filter(|d| d.helper).map(|d| (a, d.returns)))
            .collect()
    }

    pub fn render_prompt(
        &self,
        agent: AgentName,
        executor: ExecutorKind,
    ) -> Result<String, crate::AgentsError> {
        let def = self
            .def(agent)
            .ok_or_else(|| crate::AgentsError::Missing(format!("agent {agent}")))?;
        crate::render_def(def, executor)
    }
}

fn origin_label(o: &AgentOrigin) -> String {
    match o {
        AgentOrigin::Standard => "Ostra".into(),
        AgentOrigin::Workspace(p) => format!("\"{}\"", p.display()),
        AgentOrigin::Plugin(p) => format!("plugin {p}"),
    }
}

/// Rule CA1: parse one custom agent's markdown file.
pub fn parse_markdown(path: &Path, text: &str) -> Result<AgentDef, String> {
    if text.len() > MAX_PROMPT_BYTES {
        return Err(format!(
            "Shorten the agent file to at most {} KiB: it goes into every run's system prompt.",
            MAX_PROMPT_BYTES / 1024
        ));
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let agent = ostra_sdk::definition::parse_markdown(&stem, text)?;
    let def = from_plugin_agent(agent, AgentOrigin::Workspace(path.to_path_buf()))?;
    check_def(&def)?;
    Ok(def)
}

/// One agent definition from an SDK `PluginAgent`, whatever wrote it. The only difference by
/// origin is the name: the standard plugin's agents are the built-in names (Rule PL4), and every
/// other source names a custom agent.
pub fn from_plugin_agent(a: PluginAgent, origin: AgentOrigin) -> Result<AgentDef, String> {
    let name = if origin == AgentOrigin::Standard {
        AgentName::builtin(&a.name)
            .ok_or_else(|| format!("`{}` is not a built-in agent.", a.name))?
    } else {
        AgentName::Custom(CustomAgent::new(&a.name)?)
    };
    let returns: Contract = match &a.returns {
        Some(r) => r.parse()?,
        None => Contract::Stage,
    };
    let brief = match &a.brief {
        Some(list) => list
            .iter()
            .map(|s| Section::parse(s))
            .collect::<Result<_, _>>()?,
        None => Section::DEFAULT.to_vec(),
    };
    let capabilities = a
        .capabilities
        .unwrap_or_else(|| DEFAULT_CAPABILITIES.to_vec());
    let writes = capabilities.iter().any(|c| c.writes());
    Ok(AgentDef {
        name,
        description: a.description.trim().to_string(),
        default_tier: a.default_tier.unwrap_or(Tier::Balanced),
        effort: a.effort,
        capabilities,
        timeout_secs: a.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS),
        write_scope: a.write_scope.unwrap_or(if writes {
            WriteScope::Project
        } else {
            WriteScope::Session
        }),
        origin,
        programmatic: a.prompt.is_none(),
        prompt: a.prompt,
        submit_data: a.data_schema,
        helper: a.helper,
        returns,
        contract_schema: None,
        brief,
    })
}

/// Rule CA2: what every custom agent definition must hold.
pub fn check_def(def: &AgentDef) -> Result<(), String> {
    if def.name.is_builtin() {
        return Err(format!(
            "Give the agent another name: `{}` is a built-in agent.",
            def.name
        ));
    }
    if def.description.is_empty() {
        return Err("Give `description`: what the agent does and when it runs.".into());
    }
    if !def.programmatic && def.prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Err("Write the agent's instructions below the frontmatter.".into());
    }
    if def.timeout_secs == 0 || def.timeout_secs > MAX_TIMEOUT_SECS {
        return Err(format!(
            "Set `timeout_seconds` between 1 and {MAX_TIMEOUT_SECS}."
        ));
    }
    if def.submit_data.is_some() && def.returns != Contract::Stage {
        return Err(format!(
            "Remove `data_schema`, or set `returns` to `stage`: `{}` results have their own shape.",
            def.returns
        ));
    }
    let writes = def.capabilities.iter().any(|c| c.writes());
    if writes && def.write_scope == WriteScope::ReadOnly {
        return Err(
            "Drop `write` and `edit`, or set `write_scope` to `session` or `project`: a read-only agent writes no file."
                .into(),
        );
    }
    for k in def.effort.keys() {
        if !ExecutorKind::all().iter().any(|e| e.tier_table() == k) && k != "plugin" {
            return Err(format!(
                "Key `[effort]` by executor (native, claude, codex, grok, agy): `{k}` is not one."
            ));
        }
    }
    if let Some(s) = &def.submit_data {
        ostra_core::schema_check::validate_schema(s)
            .map_err(|e| format!("Fix `data_schema`: {e}"))?;
    }
    if let Some(p) = &def.prompt {
        // A broken template is caught at load, not in the middle of a session.
        crate::render_def(def, ExecutorKind::Native)
            .map(|_| ())
            .map_err(|e| {
                format!("Fix the instructions: {e}. Prompt tokens look like {{{{ tool_read }}}}.")
            })?;
        let _ = p;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "+++\ndescription = \"Audits auth code\"\ncapabilities = [\"read\", \"search_text\"]\n[data_schema]\ntype = \"object\"\nrequired = [\"risk\"]\n[data_schema.properties.risk]\ntype = \"string\"\nenum = [\"low\", \"high\"]\n+++\nRead the auth module and rate it with {{ tool_read }}.\n";

    #[test]
    fn loads_a_markdown_agent() {
        let dir = tempfile::tempdir().unwrap();
        let agents = ostra_core::paths::workspace_agents_dir(dir.path());
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("auth-auditor.md"), FILE).unwrap();
        std::fs::write(agents.join("explore.md"), FILE).unwrap();
        std::fs::write(agents.join("broken.md"), "no frontmatter").unwrap();
        let (cat, issues) = AgentCatalog::load(Some(dir.path()), &[], &[]);
        assert_eq!(issues.len(), 2, "{issues:?}");
        let a: AgentName = "auth-auditor".parse().unwrap();
        let def = cat.def(a).unwrap();
        assert_eq!(def.write_scope, WriteScope::Session);
        assert_eq!(def.default_tier, Tier::Balanced);
        assert_eq!(
            def.submit_schema()["properties"]["data"]["required"][0],
            "risk"
        );
        let prompt = cat.render_prompt(a, ExecutorKind::Native).unwrap();
        assert!(prompt.contains("Read the auth module and rate it with Read."));
        assert!(prompt.contains("submit_auth_auditor"));
        assert!(cat.def(AgentName::Plan).is_some());
        assert!(cat.names().contains(&a));
    }

    #[test]
    fn refuses_unsafe_definitions() {
        let p = Path::new("x.md");
        let documents = "+++\ndescription = \"d\"\nreturns = \"spec\"\ncapabilities = [\"document_spec\", \"manage_projects\"]\n+++\nbody\n";
        let d = parse_markdown(p, documents).unwrap();
        assert_eq!(
            d.returns,
            Contract::Spec,
            "CA5: any agent may return a built-in contract"
        );
        assert!(
            d.capabilities.contains(&Capability::DocumentSpec),
            "CA6: and hold its grants"
        );
        let data_off_stage = "+++\ndescription = \"d\"\nreturns = \"spec\"\n[data_schema]\ntype = \"object\"\n+++\nbody\n";
        assert!(parse_markdown(p, data_off_stage).is_err());
        let ro = "+++\ndescription = \"d\"\ncapabilities = [\"write\"]\nwrite_scope = \"read_only\"\n+++\nbody\n";
        assert!(parse_markdown(p, ro).is_err());
        let w = "+++\ndescription = \"d\"\ncapabilities = [\"read\", \"edit\"]\n+++\nbody\n";
        assert_eq!(
            parse_markdown(p, w).unwrap().write_scope,
            WriteScope::Project
        );
        let bad_tpl = "+++\ndescription = \"d\"\n+++\nUse {{ tool_nope }}.\n";
        assert!(parse_markdown(p, bad_tpl).is_err());
    }
}
