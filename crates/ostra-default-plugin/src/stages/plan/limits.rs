//! Rule WD3: what the plan agent learns about the later stages, so it splits work by them. Only
//! the native executor works in several projects, so a phase that names more than one project
//! must run only agents that work natively.

use crate::data::SINGLE_PROJECT_PARAM;
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::workflow::{BuiltinStage, StageRun, StageScope};
use ostra_engine::plan::PlanCtx;
use ostra_engine::state::SessionState;
use serde_json::Value;

/// One stage's agents, and whether its runs serve one plan phase.
struct LaterStage {
    name: String,
    agents: Vec<AgentName>,
    per_phase: bool,
}

fn later_stages(s: &SessionState) -> Vec<LaterStage> {
    let build = |s: &SessionState| {
        [
            LaterStage {
                name: "build".into(),
                agents: vec![s.agent_for(BuiltinStage::Build, Contract::Implementation)],
                per_phase: true,
            },
            LaterStage {
                name: "review".into(),
                agents: vec![s.agent_for(BuiltinStage::Build, Contract::Review)],
                per_phase: true,
            },
        ]
    };
    let test = |s: &SessionState| LaterStage {
        name: "test".into(),
        agents: vec![
            s.agent_for(BuiltinStage::Closing, Contract::PathAnalysis),
            s.agent_for(BuiltinStage::Closing, Contract::Tests),
            s.agent_for(BuiltinStage::Closing, Contract::Review),
        ],
        per_phase: true,
    };
    let Some(wf) = &s.workflow else {
        let mut all: Vec<LaterStage> = build(s).into();
        all.push(test(s));
        return all;
    };
    let mut out = vec![];
    for d in &wf.stages {
        match &d.run {
            StageRun::Builtin {
                stage: BuiltinStage::Build,
            } => out.extend(build(s)),
            StageRun::Builtin {
                stage: BuiltinStage::Closing,
            } => out.push(test(s)),
            StageRun::Agent { agent } => out.push(LaterStage {
                name: d.id.clone(),
                agents: vec![*agent],
                per_phase: d.scope == StageScope::Phase,
            }),
            _ => {}
        }
    }
    out
}

/// Rule WD3: one line per later stage for the plan's spawn, and the `(agent, executor)` pairs
/// that a multi-project phase would run and that work in one project.
pub fn stage_limits(s: &SessionState, ctx: &PlanCtx) -> (Vec<String>, Vec<(String, String)>) {
    let mut lines = vec![];
    let mut single: Vec<(String, String)> = vec![];
    for st in later_stages(s) {
        let agents: Vec<String> = st.agents.iter().map(|a| ctx.agent_limit(*a)).collect();
        lines.push(format!("{}: {}", st.name, agents.join(", ")));
        if !st.per_phase {
            continue;
        }
        for a in st.agents.iter().filter(|a| !ctx.several_projects(**a)) {
            let executor = ctx
                .executors
                .get(a)
                .map(|e| e.to_string())
                .unwrap_or_default();
            let pair = (a.to_string(), executor);
            if !single.contains(&pair) {
                single.push(pair);
            }
        }
    }
    (lines, single)
}

/// Rule WD3: the pairs a plan run recorded in its params.
pub fn recorded(params: &Value) -> Vec<(String, String)> {
    params
        .get(SINGLE_PROJECT_PARAM)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Rule WD3: the correction for a phase that names several projects while a single-project agent
/// would run it.
pub fn split_message(phase: u32, single: &[(String, String)]) -> String {
    let (agent, executor) = &single[0];
    format!(
        "Split phase {phase} into one phase per project and link them with Depends on: {agent} runs on {executor} and works in one project."
    )
}

/// Rule WD3: the plan's phases that name several projects while a single-project agent would run
/// them, each with its correction.
pub fn check_plan(input: &Value, params: &Value) -> Vec<String> {
    let single = recorded(params);
    if single.is_empty() {
        return vec![];
    }
    let Some(phases) = input.get("phases").and_then(Value::as_array) else {
        return vec![];
    };
    phases
        .iter()
        .filter(|p| {
            let cell = p.get("project").and_then(Value::as_str).unwrap_or_default();
            !crate::fold::phase_projects(cell).1.is_empty()
        })
        .map(|p| {
            let id = p.get("id").and_then(Value::as_u64).unwrap_or_default() as u32;
            split_message(id, &single)
        })
        .collect()
}
