//! The Workflow builder and the agent screen (HANDOVER 10.11 and 9.5): reading, saving, and
//! deleting a workspace's workflow and agent files. A save is checked the way a load checks the
//! file, against every workflow it could break, before anything is written, and keeps an approved
//! workspace approved (Rule A1).

use crate::WorkspaceRt;
use ostra_agents::{AgentCatalog, AgentDef, AgentOrigin};
use ostra_core::AgentName;
use ostra_core::api::{
    AgentDetail, AgentDoc, BuilderPalette, BuiltinStageInfo, PluginStageInfo, WorkflowDoc,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::model::{Effort, Tier};
use ostra_core::paths;
use ostra_core::transform::CondOp;
use ostra_core::workflow::{
    BUILTIN_BASES, BuiltinStage, StageRun, WorkflowDef, WorkflowFile, WorkflowSet, category_name,
    valid_name,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug)]
pub enum BuilderError {
    NotFound(String),
    /// The save would leave the workspace with these problems, so nothing was written.
    Invalid(Vec<String>),
    /// Something else depends on what the request removes.
    Conflict(String),
    Io(String),
}

fn builtin_base(name: &str) -> Option<ostra_core::pipeline::Category> {
    BUILTIN_BASES
        .iter()
        .copied()
        .find(|c| category_name(*c) == name)
}

impl WorkspaceRt {
    fn workflow_path(&self, name: &str) -> PathBuf {
        paths::workspace_workflows_dir(&self.root).join(format!("{name}.toml"))
    }

    fn agent_path(&self, name: &str) -> PathBuf {
        paths::workspace_agents_dir(&self.root).join(format!("{name}.md"))
    }

    fn runnable(
        &self,
        set: &WorkflowSet,
        name: &str,
        agents: &AgentCatalog,
    ) -> Result<WorkflowDef, String> {
        set.resolve(name).and_then(|wf| {
            ostra_engine::workflow::check_runnable(
                &wf,
                agents,
                &self.host.plugin_stages(&self.root),
            )
            .map(|_| wf)
        })
    }

    /// Every workspace workflow that does not resolve or cannot run with `agents`, by name.
    fn broken_workflows(
        &self,
        set: &WorkflowSet,
        agents: &AgentCatalog,
    ) -> BTreeMap<String, String> {
        set.files
            .keys()
            .filter_map(|n| self.runnable(set, n, agents).err().map(|e| (n.clone(), e)))
            .collect()
    }

    /// Rule WB1: one workflow as the builder edits it, every node written out.
    pub fn workflow_doc(&self, name: &str) -> Result<WorkflowDoc, BuilderError> {
        let (set, unreadable) = self.workflow_set();
        if let Some((_, e)) = unreadable.iter().find(|(n, _)| n == name) {
            return Err(BuilderError::Invalid(vec![e.clone()]));
        }
        // Rule PL6: a plugin's workflow is shown whole and read only.
        if let Some(raw) = set.plugin_files.get(name) {
            let agents = self.host.agents(&self.root).0;
            let resolved = set.resolve(name).ok();
            return Ok(WorkflowDoc {
                name: name.into(),
                builtin: false,
                file: match &resolved {
                    Some(def) => WorkflowFile::from_def(def, vec![], raw.layout.clone()),
                    None => raw.clone(),
                },
                issues: self
                    .runnable(&set, name, &agents)
                    .err()
                    .into_iter()
                    .collect(),
                resolved,
                flattened: raw.extends.is_some() || !raw.remove.is_empty(),
                plugin: name.split_once(':').map(|(p, _)| p.to_string()),
            });
        }
        let Some(raw) = set.files.get(name) else {
            return match builtin_base(name) {
                // Rule WF9: the workspace has no copy, so it runs Ostra's default.
                Some(c) => {
                    let def = ostra_default_plugin::workflow(c);
                    Ok(WorkflowDoc {
                        name: name.into(),
                        builtin: true,
                        file: WorkflowFile::from_def(&def, vec![], BTreeMap::new()),
                        resolved: Some(def),
                        issues: vec![],
                        flattened: false,
                        plugin: None,
                    })
                }
                None => Err(BuilderError::NotFound(format!(
                    "The workspace has no workflow `{name}`."
                ))),
            };
        };
        let agents = self.host.agents(&self.root).0;
        let flattened = raw.extends.is_some() || !raw.remove.is_empty();
        Ok(match self.runnable(&set, name, &agents) {
            Ok(def) => WorkflowDoc {
                name: name.into(),
                builtin: false,
                file: WorkflowFile::from_def(&def, raw.default_for.clone(), raw.layout.clone()),
                resolved: Some(def),
                issues: vec![],
                flattened,
                plugin: None,
            },
            Err(e) => {
                let resolved = set.resolve(name).ok();
                WorkflowDoc {
                    name: name.into(),
                    builtin: false,
                    file: match &resolved {
                        Some(def) => {
                            WorkflowFile::from_def(def, raw.default_for.clone(), raw.layout.clone())
                        }
                        None => raw.clone(),
                    },
                    resolved,
                    issues: vec![e],
                    flattened,
                    plugin: None,
                }
            }
        })
    }

    /// Rule WB1: save a workflow from the builder. It must resolve and run, give no category two
    /// default workflows, and break no workflow that extends it.
    pub fn save_workflow(
        &self,
        name: &str,
        file: WorkflowFile,
    ) -> Result<WorkflowDoc, BuilderError> {
        let issues = self.check_workflow(name, &file);
        if !issues.is_empty() {
            return Err(BuilderError::Invalid(issues));
        }
        let text = file.to_toml().map_err(BuilderError::Io)?;
        crate::trust::save_definition(
            self.host.registry(),
            &self.root,
            &self.workflow_path(name),
            Some(&text),
        )
        .map_err(|e| BuilderError::Io(e.to_string()))?;
        self.workflow_doc(name)
    }

    /// Rules WB1 and WB6: what a save of `file` as workflow `name` would be refused for, so the
    /// builder can show it while the user edits.
    pub fn check_workflow(&self, name: &str, file: &WorkflowFile) -> Vec<String> {
        if let Some((plugin, _)) = name.split_once(':') {
            return vec![format!(
                "Save it under a name of your own: `{name}` is built in code by plugin `{plugin}`, so Ostra does not change it."
            )];
        }
        if !valid_name(name) {
            return vec![format!(
                "Name the workflow in lowercase kebab-case, starting with a letter: `{name}` is not."
            )];
        }
        let (mut set, _) = self.workflow_set();
        let agents = self.host.agents(&self.root).0;
        let before = self.broken_workflows(&set, &agents);
        set.files.insert(name.to_string(), file.clone());
        let mut issues: Vec<String> = vec![];
        if let Err(e) = self.runnable(&set, name, &agents) {
            issues.push(e);
        }
        for (other, e) in self.broken_workflows(&set, &agents) {
            if other != name && !before.contains_key(&other) {
                issues.push(format!("Saving it breaks workflow `{other}`: {e}"));
            }
        }
        for c in BUILTIN_BASES {
            if let Err(e) = set.default_for(c)
                && file
                    .default_for
                    .iter()
                    .any(|d| ostra_core::workflow::parse_category(d) == Some(c))
            {
                issues.push(e);
            }
        }
        issues
    }

    /// Rule WF9: write Ostra's default workflows the workspace has no copy of.
    pub fn restore_default_workflows(&self) -> Result<Vec<String>, BuilderError> {
        let (set, _) = self.workflow_set();
        let missing = set.missing_defaults();
        let files = crate::trust::default_workflow_files(&self.root, &missing);
        crate::trust::save_definitions(self.host.registry(), &self.root, &files)
            .map_err(|e| BuilderError::Io(e.to_string()))?;
        Ok(missing)
    }

    pub fn delete_workflow(&self, name: &str) -> Result<(), BuilderError> {
        let (set, _) = self.workflow_set();
        if !set.files.contains_key(name) {
            return Err(BuilderError::NotFound(format!(
                "The workspace has no workflow file `{name}`."
            )));
        }
        if let Some((other, _)) = set
            .files
            .iter()
            .find(|(_, f)| f.extends.as_deref() == Some(name))
        {
            return Err(BuilderError::Conflict(format!(
                "Workflow `{other}` extends `{name}`. Change or delete `{other}` first."
            )));
        }
        crate::trust::save_definition(
            self.host.registry(),
            &self.root,
            &self.workflow_path(name),
            None,
        )
        .map_err(|e| BuilderError::Io(e.to_string()))
    }

    /// Rule AG3: the workspace's plugins and what each is doing.
    pub fn plugins(&self) -> Vec<ostra_core::api::PluginInfo> {
        self.host
            .plugin_infos(&self.root, &self.effective_settings())
    }

    fn function_path(&self, name: &str) -> PathBuf {
        paths::workspace_transforms_dir(&self.root).join(format!("{name}.toml"))
    }

    /// Who calls function `name`: workflow nodes as `workflow/node`, and composites.
    fn function_callers(&self, set: &WorkflowSet, name: &str) -> Vec<String> {
        let mut out = vec![];
        for (wf, file) in &set.files {
            for s in &file.stages {
                if s.transform.as_deref() == Some(name) {
                    out.push(format!("{wf}/{}", s.id));
                }
            }
        }
        for (f, file) in &set.functions {
            if f != name && file.steps.iter().any(|s| s.transform == name) {
                out.push(format!("transform {f}"));
            }
        }
        out
    }

    /// Rule WB7: one composite function as its editor reads it.
    pub fn function_doc(&self, name: &str) -> Result<ostra_core::api::FunctionDoc, BuilderError> {
        let (set, _) = self.workflow_set();
        let (_, unreadable) = WorkflowSet::load_functions(&self.root);
        if let Some((_, e)) = unreadable.iter().find(|(n, _)| n == name) {
            return Err(BuilderError::Invalid(vec![e.clone()]));
        }
        let file = set.functions.get(name).cloned().ok_or_else(|| {
            BuilderError::NotFound(format!("The workspace has no transform function `{name}`."))
        })?;
        Ok(ostra_core::api::FunctionDoc {
            name: name.into(),
            issues: ostra_core::transform::check_function(name, &file, &set.functions),
            info: ostra_core::transform::function_info(name, &set.functions),
            used_by: self.function_callers(&set, name),
            file,
        })
    }

    /// Rule WB7: save a composite function. It must check, and every workflow and function that
    /// worked before must still work with it.
    pub fn save_function(
        &self,
        name: &str,
        file: ostra_core::transform::FunctionFile,
    ) -> Result<ostra_core::api::FunctionDoc, BuilderError> {
        if !valid_name(name) {
            return Err(BuilderError::Invalid(vec![format!(
                "Name the function in lowercase kebab-case, starting with a letter: `{name}` is not."
            )]));
        }
        let (mut set, _) = self.workflow_set();
        let agents = self.host.agents(&self.root).0;
        let before = self.broken_workflows(&set, &agents);
        let before_fns: Vec<String> = set
            .functions
            .iter()
            .filter(|(n, f)| {
                !ostra_core::transform::check_function(n, f, &set.functions).is_empty()
            })
            .map(|(n, _)| n.clone())
            .collect();
        set.functions.insert(name.to_string(), file.clone());
        let mut issues = ostra_core::transform::check_function(name, &file, &set.functions);
        for (n, f) in &set.functions {
            if n != name
                && !before_fns.contains(n)
                && !ostra_core::transform::check_function(n, f, &set.functions).is_empty()
            {
                issues.push(format!("Saving it breaks transform function `{n}`."));
            }
        }
        for (wf, e) in self.broken_workflows(&set, &agents) {
            if !before.contains_key(&wf) {
                issues.push(format!("Saving it breaks workflow `{wf}`: {e}"));
            }
        }
        if !issues.is_empty() {
            return Err(BuilderError::Invalid(issues));
        }
        let text = toml::to_string(&file).map_err(|e| BuilderError::Io(e.to_string()))?;
        crate::trust::save_definition(
            self.host.registry(),
            &self.root,
            &self.function_path(name),
            Some(&text),
        )
        .map_err(|e| BuilderError::Io(e.to_string()))?;
        self.function_doc(name)
    }

    pub fn delete_function(&self, name: &str) -> Result<(), BuilderError> {
        let (set, _) = self.workflow_set();
        if !set.functions.contains_key(name) {
            return Err(BuilderError::NotFound(format!(
                "The workspace has no transform function `{name}`."
            )));
        }
        let callers = self.function_callers(&set, name);
        if !callers.is_empty() {
            return Err(BuilderError::Conflict(format!(
                "{} use `{name}`. Change them first.",
                callers.join(", ")
            )));
        }
        crate::trust::save_definition(
            self.host.registry(),
            &self.root,
            &self.function_path(name),
            None,
        )
        .map_err(|e| BuilderError::Io(e.to_string()))
    }

    /// Rule WB1: every kind of node the builder can place, besides the agents.
    pub fn builder_palette(&self) -> BuilderPalette {
        let plugins = self
            .host
            .plugin_infos(&self.root, &self.effective_settings());
        BuilderPalette {
            builtin_stages: BuiltinStage::ALL
                .into_iter()
                .map(|b| BuiltinStageInfo {
                    stage: b,
                    uses: b.uses(),
                    description: b.description().into(),
                    contracts: b.contracts().to_vec(),
                    removable: b.removable(),
                })
                .collect(),
            plugin_stages: plugins
                .iter()
                .filter_map(|p| p.manifest.as_ref().map(|m| (p.name.clone(), m)))
                .flat_map(|(plugin, m)| {
                    m.stages.iter().map(move |s| PluginStageInfo {
                        plugin: plugin.clone(),
                        stage: s.name.clone(),
                        description: s.description.clone(),
                    })
                })
                .collect(),
            transforms: {
                // Rule WB7: the workspace's composites come after Ostra's functions.
                let (set, _) = self.workflow_set();
                let mut all = ostra_core::transform::transforms();
                all.extend(
                    set.functions
                        .keys()
                        .filter_map(|n| ostra_core::transform::function_info(n, &set.functions)),
                );
                // Rule PL7: plugin functions, by their full names.
                all.extend(set.plugin_transforms.iter().map(|(n, t)| {
                    ostra_core::transform::TransformInfo {
                        name: n.clone(),
                        ..t.clone()
                    }
                }));
                all
            },
            cond_ops: CondOp::ALL.to_vec(),
            tiers: Tier::ALL.to_vec(),
        }
    }

    /// Rule AG1: one agent, with its definition, rendered prompt, submit schema, and the workflow
    /// nodes that use it.
    pub fn agent_detail(&self, name: &str) -> Result<AgentDetail, BuilderError> {
        let missing = || BuilderError::NotFound(format!("The workspace has no agent `{name}`."));
        let agent: AgentName = name.parse().map_err(|_| missing())?;
        let mut catalog = self.host.agents(&self.root).0;
        // Rule A1: a file that waits for approval is out of the catalog, but its editor still
        // shows it.
        let waiting = catalog.def(agent).is_none() && self.agent_path(name).is_file();
        if waiting {
            let path = self.agent_path(name);
            let text =
                std::fs::read_to_string(&path).map_err(|e| BuilderError::Io(e.to_string()))?;
            let def = ostra_agents::catalog::parse_markdown(&path, &text)
                .map_err(|e| BuilderError::Invalid(vec![e]))?;
            catalog = catalog
                .with_workspace_def(agent, Some(def))
                .map_err(|e| BuilderError::Invalid(vec![e]))?;
        }
        let def = catalog.def(agent).ok_or_else(missing)?;
        let info = crate::settings::agent_infos(&self.host.global(), &self.settings(), &catalog)
            .into_iter()
            .find(|i| i.name == agent)
            .ok_or_else(missing)?;
        let (set, _) = self.workflow_set();
        let mut used_by = vec![];
        for wf in set.files.keys() {
            let Ok(def) = set.resolve(wf) else { continue };
            for d in &def.stages {
                let runs = matches!(d.run, StageRun::Agent { agent: a } if a == agent);
                if runs || d.agents.values().any(|a| *a == agent) {
                    used_by.push(format!("{wf}/{}", d.id));
                }
            }
        }
        Ok(AgentDetail {
            info,
            doc: doc_of(def),
            editable: matches!(def.origin, AgentOrigin::Workspace(_)),
            waiting_approval: waiting,
            prompt_preview: ostra_agents::render_def(def, ExecutorKind::Native).ok(),
            submit_schema: def.submit_schema(),
            used_by,
        })
    }

    /// Rule AG2: save a workspace agent's file from the agent editor. It must parse like a loaded
    /// file, take no name another source holds, and leave every workflow that ran able to run.
    pub fn save_agent(&self, name: &str, doc: AgentDoc) -> Result<AgentDetail, BuilderError> {
        let path = self.agent_path(name);
        let text = agent_markdown(&doc).map_err(|e| BuilderError::Invalid(vec![e]))?;
        let def = ostra_agents::catalog::parse_markdown(&path, &text)
            .map_err(|e| BuilderError::Invalid(vec![e]))?;
        if let ostra_core::Contract::Plugin(c) = def.returns {
            let known = self
                .host
                .plugin_infos(&self.root, &self.effective_settings())
                .iter()
                .filter_map(|p| p.manifest.as_ref())
                .any(|m| m.name == c.plugin() && m.contracts.iter().any(|k| k.name == c.name()));
            if !known {
                return Err(BuilderError::Invalid(vec![format!(
                    "Set `returns` to a contract a running plugin defines: no plugin of the workspace defines `{}`.",
                    c.as_str()
                )]));
            }
        }
        let catalog = self.host.agents(&self.root).0;
        let next = catalog
            .with_workspace_def(def.name, Some(def))
            .map_err(|e| BuilderError::Invalid(vec![e]))?;
        let (set, _) = self.workflow_set();
        let before = self.broken_workflows(&set, &catalog);
        let issues: Vec<String> = self
            .broken_workflows(&set, &next)
            .into_iter()
            .filter(|(n, _)| !before.contains_key(n))
            .map(|(n, e)| format!("Saving it breaks workflow `{n}`: {e}"))
            .collect();
        if !issues.is_empty() {
            return Err(BuilderError::Invalid(issues));
        }
        crate::trust::save_definition(self.host.registry(), &self.root, &path, Some(&text))
            .map_err(|e| BuilderError::Io(e.to_string()))?;
        self.agent_detail(name)
    }

    pub fn delete_agent(&self, name: &str) -> Result<(), BuilderError> {
        let missing =
            || BuilderError::NotFound(format!("The workspace has no agent file `{name}`."));
        let agent: AgentName = name.parse().map_err(|_| missing())?;
        let catalog = self.host.agents(&self.root).0;
        let path = self.agent_path(name);
        if !path.is_file() {
            return Err(missing());
        }
        let next = catalog
            .with_workspace_def(agent, None)
            .map_err(BuilderError::Conflict)?;
        let (set, _) = self.workflow_set();
        let before = self.broken_workflows(&set, &catalog);
        let broken: Vec<String> = self
            .broken_workflows(&set, &next)
            .into_keys()
            .filter(|n| !before.contains_key(n))
            .collect();
        if !broken.is_empty() {
            return Err(BuilderError::Conflict(format!(
                "Workflows {} use agent `{name}`. Change them first.",
                broken
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        crate::trust::save_definition(self.host.registry(), &self.root, &path, None)
            .map_err(|e| BuilderError::Io(e.to_string()))
    }
}

fn doc_of(def: &AgentDef) -> AgentDoc {
    AgentDoc {
        name: def.name.to_string(),
        description: def.description.clone(),
        returns: def.returns.to_string(),
        default_tier: Some(def.default_tier),
        capabilities: def.capabilities.clone(),
        write_scope: Some(def.write_scope),
        brief: def.brief.iter().map(|s| s.as_str().to_string()).collect(),
        timeout_seconds: Some(def.timeout_secs),
        effort: def.effort.clone(),
        data_schema: def.submit_data.clone(),
        helper: def.helper,
        prompt: def.prompt.clone().unwrap_or_default(),
    }
}

/// Rule AG2: a workspace agent's markdown file: TOML frontmatter between `+++` lines, then the
/// prompt. Tables come last, because TOML puts plain keys before tables.
pub fn agent_markdown(doc: &AgentDoc) -> Result<String, String> {
    #[derive(Serialize)]
    struct Front<'a> {
        description: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        returns: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default_tier: Option<Tier>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
        #[serde(skip_serializing_if = "<[_]>::is_empty")]
        capabilities: &'a [ostra_core::Capability],
        #[serde(skip_serializing_if = "Option::is_none")]
        write_scope: Option<ostra_core::WriteScope>,
        #[serde(skip_serializing_if = "<[_]>::is_empty")]
        brief: &'a [String],
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        helper: bool,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        effort: &'a BTreeMap<String, Effort>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_schema: Option<toml::Value>,
    }
    if doc.description.trim().is_empty() {
        return Err(
            "Describe the agent in a sentence: other agents and the builder show it.".into(),
        );
    }
    let data_schema = doc
        .data_schema
        .as_ref()
        .filter(|v| !v.is_null())
        .map(|v| toml::Value::try_from(v).map_err(|e| format!("Fix `data_schema`: {e}.")))
        .transpose()?;
    let returns = doc.returns.trim();
    let front = Front {
        description: doc.description.trim(),
        returns: (!returns.is_empty() && returns != "stage").then_some(returns),
        default_tier: doc.default_tier,
        timeout_seconds: doc.timeout_seconds,
        capabilities: &doc.capabilities,
        write_scope: doc.write_scope,
        brief: &doc.brief,
        helper: doc.helper,
        effort: &doc.effort,
        data_schema,
    };
    let toml = toml::to_string(&front).map_err(|e| e.to_string())?;
    Ok(format!("+++\n{toml}+++\n{}", doc.prompt))
}
