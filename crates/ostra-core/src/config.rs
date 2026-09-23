//! Settings files: `~/.config/ostra/config.toml` (global), `<workspace>/.ostra/workspace.toml`, and
//! `<project>/.ostra/project.toml`. Route resolution and save-time validation live here, because a
//! route that does not resolve is a settings error shown at save time, not at spawn time.

use crate::agent::{AgentName, JUDGE_ROUTE, route_keys};
use crate::executor::{ExecutorKind, HarnessKind};
use crate::model::{Complexity, Tier};
use crate::slug::is_project_key;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;

// ---------------------------------------------------------------------------------------------
// Global config
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct GlobalConfig {
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Tier to model, per executor table (`native`, `claude`, `codex`, `grok`, `agy`).
    pub tiers: BTreeMap<String, TierTable>,
    pub harness: BTreeMap<String, HarnessConfig>,
    pub permissions: PermissionRules,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ProviderConfig {
    /// Environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// Environment variable holding a bearer token, sent as `Authorization: Bearer` (the
    /// `ANTHROPIC_AUTH_TOKEN` convention of gateways and proxies). Used when no API key is set.
    pub auth_token_env: Option<String>,
    /// OS keychain service name to read the key from when the variables are unset.
    pub keychain_service: Option<String>,
    /// Override for the API base URL (tests, proxies).
    pub base_url: Option<String>,
    /// Environment variable holding the base URL, used when `base_url` is unset.
    pub base_url_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct TierTable {
    pub fast: Option<String>,
    pub balanced: Option<String>,
    pub advanced: Option<String>,
    pub frontier: Option<String>,
}

impl TierTable {
    pub fn get(&self, tier: Tier) -> Option<&str> {
        match tier {
            Tier::Fast => self.fast.as_deref(),
            Tier::Balanced => self.balanced.as_deref(),
            Tier::Advanced => self.advanced.as_deref(),
            Tier::Frontier => self.frontier.as_deref(),
        }
    }

    fn of(fast: &str, balanced: &str, advanced: &str, frontier: &str) -> Self {
        TierTable {
            fast: Some(fast.into()),
            balanced: Some(balanced.into()),
            advanced: Some(advanced.into()),
            frontier: Some(frontier.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct HarnessConfig {
    pub command: String,
    /// Extra arguments added to every launch.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ServerConfig {
    /// Fixed port. Absent means pick a free port.
    pub port: Option<u16>,
    /// Address to listen on. Absent means `127.0.0.1`. `0.0.0.0` or `::` listens on every
    /// interface, for remote access.
    pub bind: Option<String>,
    /// Extra host names the browser may use, for example a reverse proxy's domain. Every local
    /// interface address and the machine's host name are allowed automatically when listening on
    /// all interfaces.
    pub allowed_hosts: Vec<String>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "anthropic".into(),
            ProviderConfig {
                api_key_env: Some("ANTHROPIC_API_KEY".into()),
                auth_token_env: Some("ANTHROPIC_AUTH_TOKEN".into()),
                base_url_env: Some("ANTHROPIC_BASE_URL".into()),
                ..Default::default()
            },
        );
        providers.insert(
            "openai".into(),
            ProviderConfig {
                api_key_env: Some("OPENAI_API_KEY".into()),
                base_url_env: Some("OPENAI_BASE_URL".into()),
                ..Default::default()
            },
        );
        let mut tiers = BTreeMap::new();
        tiers.insert(
            "native".into(),
            TierTable::of(
                "anthropic:claude-haiku-4-5-20251001",
                "anthropic:claude-sonnet-5",
                "anthropic:claude-opus-5-5",
                "anthropic:claude-fable-5-1",
            ),
        );
        tiers.insert("claude".into(), TierTable::of("haiku", "sonnet", "opus", "fable"));
        tiers.insert(
            "codex".into(),
            TierTable::of("gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol", "gpt-5.6-sol"),
        );
        tiers.insert("grok".into(), TierTable::of("grok-4.5", "grok-4.5", "grok-4.5", "grok-4.5"));
        tiers.insert("agy".into(), TierTable::of("flash", "flash", "flash", "flash"));
        let mut harness = BTreeMap::new();
        for h in HarnessKind::ALL {
            harness.insert(h.as_str().to_string(), HarnessConfig { command: h.as_str().into(), args: vec![] });
        }
        GlobalConfig {
            providers,
            tiers,
            harness,
            permissions: PermissionRules { deny: vec!["Bash(rm -rf /*)".into()], ..Default::default() },
            server: ServerConfig::default(),
        }
    }
}

impl GlobalConfig {
    pub fn harness_command(&self, harness: HarnessKind) -> String {
        self.harness
            .get(harness.as_str())
            .map(|h| h.command.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| harness.as_str().to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PermissionMode {
    /// Ask for edits and unlisted commands.
    #[default]
    Default,
    /// Edits inside the project are allowed.
    AcceptEdits,
    /// Read-only.
    Plan,
    /// No asks.
    Bypass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct PermissionRules {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
}

impl PermissionRules {
    /// Scopes merge in order: global, workspace, session. Lists concatenate.
    pub fn merged(scopes: &[&PermissionRules]) -> PermissionRules {
        let mut out = PermissionRules::default();
        for s in scopes {
            out.allow.extend(s.allow.iter().cloned());
            out.ask.extend(s.ask.iter().cloned());
            out.deny.extend(s.deny.iter().cloned());
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct WorkspacePermissions {
    pub mode: PermissionMode,
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
}

impl WorkspacePermissions {
    pub fn rules(&self) -> PermissionRules {
        PermissionRules { allow: self.allow.clone(), ask: self.ask.clone(), deny: self.deny.clone() }
    }
}

// ---------------------------------------------------------------------------------------------
// Workspace settings
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct WorkspaceSettings {
    pub name: String,
    pub projects: Vec<ProjectEntry>,
    pub routing: Routing,
    pub instructions: Instructions,
    pub yolo: YoloSettings,
    pub permissions: WorkspacePermissions,
    pub notifications: NotificationSettings,
    pub limits: Limits,
}

/// Spend and parallelism limits. They exist because a fan-out stage (init scouts, skill
/// generation, explores) can otherwise start dozens of top-tier executions at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Limits {
    /// Executions that may run at once in this workspace. Further spawns wait their turn.
    pub max_parallel_executions: u32,
    /// Dollars one session may spend before it pauses for a decision. `0` means no limit.
    pub session_budget_usd: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_parallel_executions: 3, session_budget_usd: 25.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectEntry {
    pub key: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Stack chosen in project settings, used to seed skills for an empty folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Routing {
    pub executor: ExecutorRouting,
    pub model: ModelRouting,
    /// Reasoning effort per agent, overriding the agent definition's default.
    pub effort: BTreeMap<String, crate::model::Effort>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ExecutorRouting {
    #[serde(rename = "byAgent")]
    #[ts(rename = "byAgent")]
    pub by_agent: BTreeMap<String, ExecutorKind>,
    #[serde(rename = "byPhaseComplexity")]
    #[ts(rename = "byPhaseComplexity")]
    pub by_phase_complexity: BTreeMap<String, BTreeMap<Complexity, ExecutorKind>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ModelRouting {
    #[serde(rename = "byAgent")]
    #[ts(rename = "byAgent")]
    pub by_agent: BTreeMap<String, ModelChoice>,
    #[serde(rename = "byPhaseComplexity")]
    #[ts(rename = "byPhaseComplexity")]
    pub by_phase_complexity: BTreeMap<String, BTreeMap<Complexity, ModelChoice>>,
}

/// A route value: a tier name, `default` (the agent's own default tier), a concrete model, or an
/// explicit per-executor table such as `{ native = "anthropic:x", codex = "gpt-y" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum ModelChoice {
    Value(String),
    PerExecutor(BTreeMap<String, String>),
}

impl From<&str> for ModelChoice {
    fn from(value: &str) -> Self {
        ModelChoice::Value(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Instructions {
    pub all: Option<String>,
    pub agents: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct YoloSettings {
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct NotificationSettings {
    pub push: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings { push: true }
    }
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        WorkspaceSettings::seeded("workspace")
    }
}

impl WorkspaceSettings {
    /// Seeded defaults for a new workspace (handover section 7.2). Every agent has a route.
    pub fn seeded(name: &str) -> Self {
        let mut by_agent: BTreeMap<String, ModelChoice> = BTreeMap::new();
        for (agent, tier) in [
            ("explore", "advanced"),
            ("generate-spec", "advanced"),
            ("plan", "advanced"),
            ("fact-check", "advanced"),
            ("code-reviewer", "balanced"),
            ("execution-path-analyzer", "balanced"),
            ("module-documentation", "advanced"),
            ("prompt-generation", "advanced"),
            ("initializer", "balanced"),
            ("judge", "fast"),
            ("quick-answer", "balanced"),
        ] {
            by_agent.insert(agent.into(), tier.into());
        }
        let complexity_map = || {
            let mut m = BTreeMap::new();
            m.insert(Complexity::Low, ModelChoice::from("fast"));
            m.insert(Complexity::Medium, ModelChoice::from("fast"));
            m.insert(Complexity::High, ModelChoice::from("balanced"));
            m
        };
        let mut by_phase = BTreeMap::new();
        by_phase.insert("implementer".into(), complexity_map());
        by_phase.insert("write-test".into(), complexity_map());
        WorkspaceSettings {
            name: name.to_string(),
            projects: vec![],
            routing: Routing {
                executor: ExecutorRouting::default(),
                model: ModelRouting { by_agent, by_phase_complexity: by_phase },
                effort: BTreeMap::new(),
            },
            instructions: Instructions::default(),
            yolo: YoloSettings::default(),
            permissions: WorkspacePermissions::default(),
            notifications: NotificationSettings::default(),
            limits: Limits::default(),
        }
    }

    pub fn project(&self, key: &str) -> Option<&ProjectEntry> {
        self.projects.iter().find(|p| p.key == key)
    }

    /// Custom instructions for one agent: `instructions.all`, then the agent's own entry.
    pub fn instructions_for(&self, agent: AgentName) -> Vec<String> {
        let mut out = vec![];
        if let Some(all) = self.instructions.all.as_ref().filter(|s| !s.trim().is_empty()) {
            out.push(all.clone());
        }
        if let Some(own) = self.instructions.agents.get(agent.as_str()).filter(|s| !s.trim().is_empty()) {
            out.push(own.clone());
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Route resolution
// ---------------------------------------------------------------------------------------------

/// A resolved route for one execution: which executor runs it and which concrete model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResolvedRoute {
    pub executor: ExecutorKind,
    /// For native: `provider:model`. For a harness: the CLI's own model slug.
    pub model: String,
    pub tier: Option<Tier>,
}

impl ResolvedRoute {
    /// Split a native model into `(provider, model)`.
    pub fn native_parts(&self) -> Option<(&str, &str)> {
        self.model.split_once(':')
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RouteError(pub String);

/// What to resolve a route for.
#[derive(Debug, Clone, Copy)]
pub struct RouteQuery<'a> {
    /// Agent name or `judge`.
    pub key: &'a str,
    /// The agent's default tier from `agent.toml`, used when the route says `default`.
    pub default_tier: Tier,
    /// Phase complexity. `None` means no phase file, which counts as `low`.
    pub complexity: Option<Complexity>,
    /// Forces a tier regardless of the model route (the initializer's generate-skill runs on
    /// `advanced`).
    pub tier_override: Option<Tier>,
    /// Forces an executor (a harness failure re-routed to native).
    pub executor_override: Option<ExecutorKind>,
}

impl<'a> RouteQuery<'a> {
    pub fn new(key: &'a str, default_tier: Tier) -> Self {
        RouteQuery { key, default_tier, complexity: None, tier_override: None, executor_override: None }
    }
}

/// Resolve the executor for a route key. `byPhaseComplexity` wins over `byAgent`, absent means
/// native. The judge always runs natively: it is an engine call, not an agent execution.
pub fn resolve_executor(ws: &WorkspaceSettings, key: &str, complexity: Option<Complexity>) -> ExecutorKind {
    if key == JUDGE_ROUTE {
        return ExecutorKind::Native;
    }
    let complexity = complexity.unwrap_or_default();
    if let Some(e) = ws.routing.executor.by_phase_complexity.get(key).and_then(|m| m.get(&complexity)) {
        return *e;
    }
    ws.routing.executor.by_agent.get(key).copied().unwrap_or(ExecutorKind::Native)
}

fn model_choice<'w>(ws: &'w WorkspaceSettings, key: &str, complexity: Option<Complexity>) -> Option<&'w ModelChoice> {
    let complexity = complexity.unwrap_or_default();
    ws.routing
        .model
        .by_phase_complexity
        .get(key)
        .and_then(|m| m.get(&complexity))
        .or_else(|| ws.routing.model.by_agent.get(key))
}

pub fn resolve_route(
    global: &GlobalConfig,
    ws: &WorkspaceSettings,
    q: RouteQuery<'_>,
) -> Result<ResolvedRoute, RouteError> {
    let executor = q.executor_override.unwrap_or_else(|| resolve_executor(ws, q.key, q.complexity));
    let table_name = executor.tier_table();
    let table = global.tiers.get(table_name);
    let from_tier = |tier: Tier| -> Result<ResolvedRoute, RouteError> {
        let model = table.and_then(|t| t.get(tier)).ok_or_else(|| {
            RouteError(format!(
                "`{}` routes to tier `{tier}` on `{executor}`, but `[tiers.{table_name}]` in the global config has no `{tier}` entry",
                q.key
            ))
        })?;
        check_model(executor, model, q.key)?;
        Ok(ResolvedRoute { executor, model: model.to_string(), tier: Some(tier) })
    };
    if let Some(tier) = q.tier_override {
        return from_tier(tier);
    }
    let choice = model_choice(ws, q.key, q.complexity).ok_or_else(|| {
        RouteError(format!(
            "`{}` has no model route. Add `{}` under `[routing.model.byAgent]` (a tier, `default`, or a model)",
            q.key, q.key
        ))
    })?;
    match choice {
        ModelChoice::Value(v) => {
            let v = v.trim();
            if v == "default" {
                return from_tier(q.default_tier);
            }
            if let Ok(tier) = v.parse::<Tier>() {
                return from_tier(tier);
            }
            check_model(executor, v, q.key)?;
            Ok(ResolvedRoute { executor, model: v.to_string(), tier: None })
        }
        ModelChoice::PerExecutor(map) => {
            let v = map.get(table_name).ok_or_else(|| {
                RouteError(format!(
                    "`{}` has a per-executor model table without a `{table_name}` entry, and it runs on `{executor}`",
                    q.key
                ))
            })?;
            if let Ok(tier) = v.parse::<Tier>() {
                return from_tier(tier);
            }
            check_model(executor, v, q.key)?;
            Ok(ResolvedRoute { executor, model: v.to_string(), tier: None })
        }
    }
}

fn check_model(executor: ExecutorKind, model: &str, key: &str) -> Result<(), RouteError> {
    if model.trim().is_empty() {
        return Err(RouteError(format!("`{key}` resolves to an empty model")));
    }
    if executor == ExecutorKind::Native {
        match model.split_once(':') {
            Some((provider, m)) if matches!(provider, "anthropic" | "openai" | "mock") && !m.is_empty() => {}
            _ => {
                return Err(RouteError(format!(
                    "`{key}` resolves to native model `{model}`; native models are written `anthropic:<model>` or `openai:<model>`"
                )));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ValidationIssue {
    /// Dotted settings path, for example `routing.model.byAgent.plan`.
    pub path: String,
    pub message: String,
}

/// Facts about the machine that validation needs.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub installed_harnesses: Vec<HarnessKind>,
    /// Providers with a usable key.
    pub providers_with_keys: Vec<String>,
}

/// Validate workspace settings against the global config. Returns every problem found.
pub fn validate_workspace(
    global: &GlobalConfig,
    ws: &WorkspaceSettings,
    env: &Environment,
    default_tier: impl Fn(&str) -> Tier,
) -> Vec<ValidationIssue> {
    let mut issues = vec![];
    let issue = |path: String, message: String| ValidationIssue { path, message };

    if ws.name.trim().is_empty() {
        issues.push(issue("name".into(), "The workspace needs a name.".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (i, p) in ws.projects.iter().enumerate() {
        if !is_project_key(&p.key) {
            issues.push(issue(
                format!("projects[{i}].key"),
                format!("`{}` is not a project key. Use lowercase letters, digits, and dashes, starting with a letter or digit.", p.key),
            ));
        }
        if !seen.insert(p.key.clone()) {
            issues.push(issue(format!("projects[{i}].key"), format!("Project key `{}` is used twice.", p.key)));
        }
        if !p.path.is_absolute() {
            issues.push(issue(format!("projects[{i}].path"), "Project paths must be absolute.".into()));
        }
    }

    for key in ws.routing.executor.by_agent.keys() {
        if !route_keys().contains(&key.as_str()) {
            issues.push(issue(format!("routing.executor.byAgent.{key}"), format!("`{key}` is not an agent.")));
        }
    }
    for key in ws.routing.model.by_agent.keys() {
        if !route_keys().contains(&key.as_str()) {
            issues.push(issue(format!("routing.model.byAgent.{key}"), format!("`{key}` is not an agent.")));
        }
    }
    for key in ws.routing.effort.keys() {
        if !AgentName::ALL.iter().any(|a| a.as_str() == key) {
            issues.push(issue(format!("routing.effort.{key}"), format!("`{key}` is not an agent.")));
        }
    }
    if ws.limits.max_parallel_executions == 0 {
        issues.push(issue("limits.max_parallel_executions".into(), "Allow at least one execution at a time.".into()));
    }
    if !ws.limits.session_budget_usd.is_finite() || ws.limits.session_budget_usd < 0.0 {
        issues.push(issue("limits.session_budget_usd".into(), "The budget is a dollar amount of 0 or more; 0 means no limit.".into()));
    }
    if ws.routing.executor.by_agent.contains_key(JUDGE_ROUTE) {
        issues.push(issue(
            "routing.executor.byAgent.judge".into(),
            "Judge calls always run natively; remove this route.".into(),
        ));
    }
    if ws.routing.executor.by_agent.get("quick-answer").is_some_and(|e| *e != ExecutorKind::Native) {
        issues.push(issue(
            "routing.executor.byAgent.quick-answer".into(),
            "The side panel answers on the native executor only.".into(),
        ));
    }

    // Every route key, on every complexity, must resolve on the executor it would run on.
    for key in route_keys() {
        let complexities: Vec<Option<Complexity>> = if AgentName::ALL
            .iter()
            .any(|a| a.as_str() == key && a.routes_by_complexity())
        {
            Complexity::ALL.iter().copied().map(Some).collect()
        } else {
            vec![None]
        };
        for c in complexities {
            let q = RouteQuery { complexity: c, ..RouteQuery::new(key, default_tier(key)) };
            match resolve_route(global, ws, q) {
                Ok(route) => {
                    if let ExecutorKind::Harness(h) = route.executor
                        && !env.installed_harnesses.contains(&h)
                    {
                        issues.push(issue(
                            format!("routing.executor.byAgent.{key}"),
                            format!("`{key}` routes to {}, which is not installed on this machine.", h.display_name()),
                        ));
                    }
                    if let Some((provider, _)) = route.native_parts()
                        && route.executor == ExecutorKind::Native
                        && provider != "mock"
                        && !env.providers_with_keys.iter().any(|p| p == provider)
                    {
                        issues.push(issue(
                            format!("routing.model.byAgent.{key}"),
                            format!("`{key}` runs on `{}`, but the `{provider}` provider has no usable API key.", route.model),
                        ));
                    }
                }
                Err(e) => {
                    let suffix = c.map(|c| format!(" ({c})")).unwrap_or_default();
                    issues.push(issue(format!("routing.model.byAgent.{key}"), format!("{}{suffix}", e.0)));
                }
            }
        }
    }
    issues.sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
    issues.dedup();
    issues
}

// ---------------------------------------------------------------------------------------------
// Project profile (`<project>/.ostra/project.toml`)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ProjectProfile {
    pub schema_version: u32,
    pub generated_at: Option<String>,
    pub stack: Stack,
    pub commands: Commands,
    pub test_framework: Option<String>,
    pub test_types: BTreeMap<String, TestType>,
    pub module_map: Vec<ModuleRow>,
    pub skills: Vec<SkillEntry>,
    pub conventions: Conventions,
    pub review_rules: Vec<ReviewRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Stack {
    pub language: Option<String>,
    pub frameworks: Vec<String>,
    pub build_tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Commands {
    pub build: Option<String>,
    pub test: Option<String>,
    pub test_one: Option<String>,
    pub format: Option<String>,
    pub lint: Option<String>,
    pub typecheck: Option<String>,
    pub run: Option<String>,
}

impl Commands {
    /// Non-empty commands as `(purpose, command)`.
    pub fn entries(&self) -> Vec<(&'static str, &str)> {
        [
            ("build", &self.build),
            ("test", &self.test),
            ("testOne", &self.test_one),
            ("format", &self.format),
            ("lint", &self.lint),
            ("typecheck", &self.typecheck),
            ("run", &self.run),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(str::trim).filter(|s| !s.is_empty() && *s != "—").map(|s| (k, s)))
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct TestType {
    pub command: Option<String>,
    pub command_one: Option<String>,
    pub matches: Vec<String>,
    pub note: Option<String>,
    pub reports: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ModuleRow {
    pub glob: String,
    pub area: String,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct SkillEntry {
    pub name: String,
    /// `convention`, `module-hub`, `creation`, `test`, or `other`.
    pub kind: String,
    /// Relative to the project root, for example `.ostra/skills/convention/SKILL.md`.
    pub path: String,
    pub component_type: Option<String>,
    /// `generated` or `reused`.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Conventions {
    pub immutability_keyword: Option<String>,
    pub naming: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct ReviewRule {
    pub id: String,
    pub rule: String,
    /// `H`, `M`, or `L`.
    pub severity: String,
    pub auto_fixable: bool,
}

impl ProjectProfile {
    /// Rule IDs the engine may apply directly. `SEC-BLOCK-*` and `PHASE-REQ-*` never qualify.
    pub fn auto_fixable_ids(&self) -> Vec<String> {
        self.review_rules
            .iter()
            .filter(|r| r.auto_fixable && !r.id.starts_with("SEC-BLOCK") && !r.id.starts_with("PHASE-REQ"))
            .map(|r| r.id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Loading and saving
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("parsing {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

pub fn load_toml<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(ConfigError::Io { path: path.to_path_buf(), source: e }),
    }
}

pub fn load_toml_required<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io { path: path.to_path_buf(), source: e })?;
    toml::from_str(&text).map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string() })
}

/// Write TOML atomically (temp file plus rename).
pub fn save_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(value)
        .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string() })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io { path: parent.to_path_buf(), source: e })?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| ConfigError::Io { path: tmp.clone(), source: e })?;
    std::fs::rename(&tmp, path).map_err(|e| ConfigError::Io { path: path.to_path_buf(), source: e })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name = "shop"

[[projects]]
key = "backend"
path = "/home/me/code/shop-backend"

[routing.executor.byAgent]
implementer = "harness:codex"
write-test = "harness:codex"

[routing.executor.byPhaseComplexity.implementer]
low = "harness:codex"
medium = "harness:codex"
high = "native"

[routing.model.byAgent]
explore = "advanced"
generate-spec = "advanced"
plan = "advanced"
fact-check = "advanced"
code-reviewer = "balanced"
execution-path-analyzer = "balanced"
module-documentation = "advanced"
prompt-generation = "advanced"
initializer = "balanced"
judge = "fast"
quick-answer = { native = "anthropic:claude-sonnet-5" }

[routing.model.byPhaseComplexity.implementer]
low = "fast"
medium = "fast"
high = "balanced"

[routing.model.byPhaseComplexity.write-test]
low = "fast"
medium = "fast"
high = "balanced"

[instructions]
all = "Write British English in comments and docs."

[instructions.agents]
implementer = "Keep functions under 40 lines."

[yolo]
default = false

[permissions]
mode = "default"
allow = ["Bash(./mvnw *)"]
deny = ["Bash(git push *)"]
"#;

    fn tier(_: &str) -> Tier {
        Tier::Balanced
    }

    #[test]
    fn parses_sample_and_routes() {
        let ws: WorkspaceSettings = toml::from_str(SAMPLE).unwrap();
        let g = GlobalConfig::default();
        let low = resolve_route(&g, &ws, RouteQuery::new("implementer", Tier::Balanced)).unwrap();
        assert_eq!(low.executor, ExecutorKind::Harness(HarnessKind::Codex));
        assert_eq!(low.model, "gpt-5.6-luna");
        let high = resolve_route(
            &g,
            &ws,
            RouteQuery { complexity: Some(Complexity::High), ..RouteQuery::new("implementer", Tier::Balanced) },
        )
        .unwrap();
        assert_eq!(high.executor, ExecutorKind::Native);
        assert_eq!(high.model, "anthropic:claude-sonnet-5");
        let wt = resolve_route(&g, &ws, RouteQuery::new("write-test", Tier::Balanced)).unwrap();
        assert_eq!(wt.executor, ExecutorKind::Harness(HarnessKind::Codex));
        let qa = resolve_route(&g, &ws, RouteQuery::new("quick-answer", Tier::Balanced)).unwrap();
        assert_eq!(qa.model, "anthropic:claude-sonnet-5");
        assert_eq!(ws.instructions_for(AgentName::Implementer).len(), 2);
    }

    #[test]
    fn missing_route_is_an_error() {
        let mut ws = WorkspaceSettings::seeded("x");
        ws.routing.model.by_agent.remove("plan");
        let g = GlobalConfig::default();
        assert!(resolve_route(&g, &ws, RouteQuery::new("plan", Tier::Advanced)).is_err());
        let env = Environment { installed_harnesses: vec![], providers_with_keys: vec!["anthropic".into()] };
        let issues = validate_workspace(&g, &ws, &env, tier);
        assert!(issues.iter().any(|i| i.path == "routing.model.byAgent.plan"));
    }

    #[test]
    fn seeded_defaults_validate() {
        let ws = WorkspaceSettings::seeded("x");
        let g = GlobalConfig::default();
        let env = Environment { installed_harnesses: vec![], providers_with_keys: vec!["anthropic".into()] };
        assert_eq!(validate_workspace(&g, &ws, &env, tier), vec![]);
    }

    #[test]
    fn uninstalled_harness_is_refused() {
        let ws: WorkspaceSettings = toml::from_str(SAMPLE).unwrap();
        let g = GlobalConfig::default();
        let env = Environment { installed_harnesses: vec![], providers_with_keys: vec!["anthropic".into()] };
        let issues = validate_workspace(&g, &ws, &env, tier);
        assert!(issues.iter().any(|i| i.message.contains("Codex")));
    }

    #[test]
    fn native_model_needs_provider() {
        let mut ws = WorkspaceSettings::seeded("x");
        ws.routing.model.by_agent.insert("plan".into(), "claude-opus-5-5".into());
        let g = GlobalConfig::default();
        assert!(resolve_route(&g, &ws, RouteQuery::new("plan", Tier::Advanced)).is_err());
    }

    #[test]
    fn default_and_override() {
        let mut ws = WorkspaceSettings::seeded("x");
        ws.routing.model.by_agent.insert("plan".into(), "default".into());
        let g = GlobalConfig::default();
        let r = resolve_route(&g, &ws, RouteQuery::new("plan", Tier::Advanced)).unwrap();
        assert_eq!(r.tier, Some(Tier::Advanced));
        let r = resolve_route(
            &g,
            &ws,
            RouteQuery { tier_override: Some(Tier::Advanced), ..RouteQuery::new("initializer", Tier::Balanced) },
        )
        .unwrap();
        assert_eq!(r.model, "anthropic:claude-opus-5-5");
    }

    #[test]
    fn global_round_trip() {
        let g = GlobalConfig::default();
        let text = toml::to_string_pretty(&g).unwrap();
        let back: GlobalConfig = toml::from_str(&text).unwrap();
        assert_eq!(g, back);
    }
}
