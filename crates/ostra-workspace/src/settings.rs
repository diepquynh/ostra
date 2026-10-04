//! Settings checks a workspace adds to `validate_workspace`, and the agents' routes under them.

use ostra_core::agent::JUDGE_ROUTE;
use ostra_core::api::AgentInfo;
use ostra_core::config::{
    Environment, GlobalConfig, RouteQuery, ValidationIssue, WorkspaceSettings, resolve_executor,
    resolve_route, validate_workspace,
};
use ostra_core::model::Tier;

pub fn default_tier(key: &str) -> Tier {
    if key == JUDGE_ROUTE {
        return Tier::Advanced;
    }
    key.parse()
        .ok()
        .and_then(ostra_agents::builtin_def)
        .map(|d| d.default_tier)
        .unwrap_or(Tier::Balanced)
}

/// Rule CA4: what routing knows of every agent of a catalog, built-in and custom alike.
pub fn agent_routes(agents: &ostra_agents::AgentCatalog) -> Vec<ostra_core::config::AgentRoute> {
    agents
        .names()
        .into_iter()
        .filter_map(|n| agents.def(n))
        .map(|d| ostra_core::config::AgentRoute {
            name: d.name.to_string(),
            default_tier: d.default_tier,
            per_phase: d.returns.per_phase(),
            native_only: d.returns == ostra_core::Contract::Answer,
        })
        .collect()
}

/// The fixes Ostra can apply on its own: an agent with no model route gets `default`, which
/// writes down the default tier it already runs on (Rule CA4).
pub fn settings_fixes(
    settings: &WorkspaceSettings,
    agents: &[ostra_core::config::AgentRoute],
) -> Vec<ostra_core::api::SettingsFix> {
    ostra_core::config::keys_without_route(settings, agents)
        .into_iter()
        .map(|key| {
            let tier = agents
                .iter()
                .find(|a| a.name == key)
                .map(|a| a.default_tier)
                .unwrap_or_else(|| default_tier(key));
            ostra_core::api::SettingsFix {
                path: format!("routing.model.byAgent.{key}"),
                value: "default".into(),
                label: format!("Route `{key}` to its default tier ({tier})"),
            }
        })
        .collect()
}

/// Settings validation: routes, harness availability, keys, projects, sandbox entries, and
/// permission rules.
pub fn validate_settings(
    global: &GlobalConfig,
    env: &Environment,
    settings: &WorkspaceSettings,
) -> Vec<ValidationIssue> {
    let mut issues = ostra_sandbox::validate::workspace(settings);
    issues.extend(validate_workspace(global, settings, env, default_tier));
    let lists = [
        ("allow", &settings.permissions.allow),
        ("ask", &settings.permissions.ask),
        ("deny", &settings.permissions.deny),
    ];
    for (name, list) in lists {
        for (i, rule) in list.iter().enumerate() {
            if let Err(e) = ostra_policy::validate_rule(rule) {
                issues.push(ValidationIssue {
                    path: format!("permissions.{name}[{i}]"),
                    message: e,
                });
            }
        }
    }
    for (i, p) in settings.projects.iter().enumerate() {
        // A relative path already has its own issue from `validate_workspace`.
        if p.path.is_absolute() && !p.path.is_dir() {
            issues.push(ValidationIssue {
                path: format!("projects[{i}].path"),
                message: format!("{} is not a folder.", p.path.display()),
            });
        }
    }
    issues
}

pub fn field_issue(path: &str, message: String) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message,
    }
}

/// Every agent's definition, built-in and custom, with its routes under `settings`.
pub fn agent_infos(
    global: &GlobalConfig,
    settings: &WorkspaceSettings,
    agents: &ostra_agents::AgentCatalog,
) -> Vec<AgentInfo> {
    agents
        .names()
        .into_iter()
        .filter_map(|name| agents.def(name).map(|d| (name, d)))
        .map(|(name, def)| {
            let q = RouteQuery::new(name.as_str(), def.default_tier);
            let resolved = resolve_route(global, settings, q).ok();
            let default_route = resolve_route(
                global,
                settings,
                RouteQuery {
                    tier_override: Some(def.default_tier),
                    ..q
                },
            )
            .ok();
            let executor = resolve_executor(settings, name.as_str(), None);
            AgentInfo {
                name,
                label: name.label(),
                description: def.description.clone(),
                default_tier: def.default_tier,
                effort: def.effort.clone(),
                default_effort: def.effort_on(executor),
                capabilities: def.capabilities.clone(),
                timeout_secs: def.timeout_secs,
                resolved,
                default_route,
                source: agent_source(&def.origin),
                returns: def.returns,
                write_scope: def.write_scope,
                helper: def.helper,
                programmatic: def.programmatic,
            }
        })
        .collect()
}

/// Rule AG1: where a definition comes from, with a workspace file named under the workspace.
pub fn agent_source(origin: &ostra_agents::AgentOrigin) -> ostra_core::api::AgentSource {
    use ostra_agents::AgentOrigin;
    use ostra_core::api::AgentSource;
    match origin {
        AgentOrigin::Standard => AgentSource::Ostra,
        AgentOrigin::Workspace(p) => AgentSource::Workspace {
            file: format!(
                ".ostra/agents/{}",
                p.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
        },
        AgentOrigin::Plugin(p) => AgentSource::Plugin { plugin: p.clone() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::AgentName;
    use ostra_core::executor::{ExecutorKind, HarnessKind};

    #[test]
    fn agent_infos_resolve_current_and_default_routes() {
        let global = GlobalConfig::default();
        let mut settings = WorkspaceSettings::seeded("x");
        settings
            .routing
            .model
            .by_agent
            .insert("plan".into(), "fast".into());
        settings.routing.executor.by_agent.insert(
            "implementer".into(),
            ExecutorKind::Harness(HarnessKind::Codex),
        );
        settings.routing.model.by_agent.remove("explore");
        let infos = agent_infos(&global, &settings, &ostra_agents::AgentCatalog::builtin());
        assert_eq!(infos.len(), AgentName::ALL.len());
        let get = |n: AgentName| infos.iter().find(|i| i.name == n).unwrap();

        let plan = get(AgentName::Plan);
        assert_eq!(
            (plan.label.as_str(), plan.default_tier),
            ("Plan", Tier::Advanced)
        );
        assert_eq!(
            plan.resolved.as_ref().map(|r| (r.tier, r.model.as_str())),
            Some((Some(Tier::Fast), "anthropic:claude-haiku-4-5-20251001"))
        );
        assert_eq!(
            plan.default_route.as_ref().map(|r| r.model.as_str()),
            Some("anthropic:claude-opus-5-5"),
            "Agent default is the agent's own tier"
        );
        assert!(plan.capabilities.contains(&ostra_core::Capability::Read));

        let imp = get(AgentName::Implementer);
        assert_eq!(
            imp.resolved.as_ref().map(|r| r.executor),
            Some(ExecutorKind::Harness(HarnessKind::Codex))
        );
        assert_eq!(
            imp.default_effort,
            ostra_agents::effort_for(
                AgentName::Implementer,
                ExecutorKind::Harness(HarnessKind::Codex)
            )
        );

        // Rule CA4: an agent without a route runs on its default tier.
        let explore = get(AgentName::Explore);
        assert_eq!(explore.resolved, explore.default_route);
        assert!(explore.default_route.is_some());
    }
}
