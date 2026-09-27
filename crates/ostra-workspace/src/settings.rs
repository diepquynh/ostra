//! Settings checks a workspace adds to `validate_workspace`, and the agents' routes under them.

use ostra_core::agent::{AgentName, JUDGE_ROUTE};
use ostra_core::api::AgentInfo;
use ostra_core::config::{
    Environment, GlobalConfig, RouteQuery, ValidationIssue, WorkspaceSettings, resolve_executor,
    resolve_route, validate_workspace,
};
use ostra_core::model::Tier;

pub fn default_tier(key: &str) -> Tier {
    if key == JUDGE_ROUTE {
        return Tier::Fast;
    }
    key.parse()
        .map(|a| ostra_agents::agent_def(a).default_tier)
        .unwrap_or(Tier::Balanced)
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

/// Every agent's definition with its routes under `settings`.
pub fn agent_infos(global: &GlobalConfig, settings: &WorkspaceSettings) -> Vec<AgentInfo> {
    AgentName::ALL
        .into_iter()
        .map(|name| {
            let def = ostra_agents::agent_def(name);
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
                default_effort: ostra_agents::effort_for(name, executor),
                capabilities: def.capabilities.clone(),
                timeout_secs: def.timeout_secs,
                resolved,
                default_route,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let infos = agent_infos(&global, &settings);
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

        let explore = get(AgentName::Explore);
        assert_eq!(
            explore.resolved, None,
            "a route that does not resolve is null"
        );
        assert!(explore.default_route.is_some());
    }
}
