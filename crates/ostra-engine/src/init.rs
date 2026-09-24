//! The init flow, `UC/commands/init-kit/prompt.md` as engine stages (HANDOVER 8.4):
//! detect (existing skills and instruction files first), scout ×N (parallel, max 6, skipped when
//! existing skills cover the project), propose, skill approval gate, generate-skill ×N (parallel,
//! advanced tier), generate-inventory.

use crate::plan::{SpawnInputs, SpawnRequest, Step};
use crate::state::{InitTrack, SessionState};
use ostra_core::agent::{AgentName, InitializerMode};
use ostra_core::event::{ExecPurpose, GatePayload, SkillProposal};
use ostra_core::ids::ExecutionId;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Scouts run in parallel, so each one costs a full execution; Ultracode allowed 12.
pub const MAX_SCOUTS: usize = 6;
/// Skills the proposal defaults to generating. The rest default to `drop`, and the user can still
/// choose them at the approval gate.
pub const MAX_DEFAULT_GENERATE: usize = 8;

/// `Stack reference:` is resolved by the spawn factory from this key to the extracted refs file.
pub const STACK_REFERENCE_NAME: &str = "stack_reference_name";

fn spawn(
    s: &SessionState,
    mode: InitializerMode,
    item: Option<String>,
    init: BTreeMap<String, String>,
) -> Step {
    let project = s
        .init
        .as_ref()
        .map(|i| i.project.clone())
        .unwrap_or_default();
    let purpose = ExecPurpose::Init {
        mode,
        item: item.clone(),
    };
    Step::Spawn(Box::new(SpawnRequest {
        agent: AgentName::Initializer,
        stage: crate::state::stage_of(&purpose),
        purpose,
        project: project.clone(),
        session_dir: s.project_session_dir(&project),
        inputs: SpawnInputs {
            init,
            init_item: item,
            ..Default::default()
        },
        resumes: None,
    }))
}

fn str_at<'v>(v: &'v Value, key: &str) -> &'v str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default()
}

pub fn slices(detect: &Value) -> Vec<(String, String, Vec<String>)> {
    detect
        .get("slices")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .map(|x| {
                    let paths = x
                        .get("paths")
                        .and_then(|p| p.as_array())
                        .map(|p| {
                            p.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        str_at(x, "slug").to_string(),
                        str_at(x, "descriptor").to_string(),
                        paths,
                    )
                })
                .filter(|(slug, _, _)| !slug.is_empty())
                .take(MAX_SCOUTS)
                .collect()
        })
        .unwrap_or_default()
}

fn existing_skill_count(detect: &Value) -> usize {
    detect
        .get("existing_skills")
        .and_then(|s| s.as_array())
        .map_or(0, |a| {
            a.iter().filter(|x| !str_at(x, "name").is_empty()).count()
        })
}

/// The proposal's skills with the defaults the propose mode sets: new and recommended generate,
/// new and not recommended drop, existing reuse.
pub fn proposals(propose: &Value) -> Vec<SkillProposal> {
    propose
        .get("skills")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .map(|x| {
                    let existing = str_at(x, "status") == "existing";
                    let recommend = x
                        .get("recommend")
                        .and_then(|r| r.as_bool())
                        .unwrap_or(false);
                    let disposition = if existing {
                        "reuse"
                    } else if recommend {
                        "generate"
                    } else {
                        "drop"
                    };
                    let description = [str_at(x, "description"), str_at(x, "rationale")]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    SkillProposal {
                        name: str_at(x, "name").to_string(),
                        kind: str_at(x, "kind").to_string(),
                        description,
                        disposition: disposition.into(),
                        exemplars: x
                            .get("existing_path")
                            .and_then(|p| p.as_str())
                            .filter(|p| !p.is_empty())
                            .map(|p| vec![p.to_string()])
                            .unwrap_or_default(),
                    }
                })
                .filter(|p| !p.name.is_empty())
                .collect::<Vec<_>>()
        })
        .map(cap_generate)
        .unwrap_or_default()
}

/// Keep the first `MAX_DEFAULT_GENERATE` generate defaults, in proposal order (ranked by
/// ubiquity), and default the rest to drop.
fn cap_generate(mut skills: Vec<SkillProposal>) -> Vec<SkillProposal> {
    let mut kept = 0;
    for s in skills.iter_mut().filter(|s| s.disposition == "generate") {
        kept += 1;
        if kept > MAX_DEFAULT_GENERATE {
            s.disposition = "drop".into();
            s.description = format!(
                "{} (Dropped by default to limit cost; choose generate to include it.)",
                s.description
            );
        }
    }
    skills
}

fn findings_list(i: &InitTrack) -> String {
    i.scouts
        .iter()
        .filter_map(|x| {
            x.result
                .as_ref()
                .map(|r| str_at(r, "findings_path").to_string())
        })
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn skill_entry(propose: &Value, name: &str) -> Value {
    propose
        .get("skills")
        .and_then(|s| s.as_array())
        .and_then(|a| a.iter().find(|x| str_at(x, "name") == name))
        .cloned()
        .unwrap_or(Value::Null)
}

pub fn plan_init(s: &SessionState, push: &mut dyn FnMut(Step)) {
    let Some(i) = &s.init else { return };
    let project = i.project.clone();
    if let Some((exec, err)) = &i.failed {
        if i.failed_gate.is_none() {
            push(failed_gate(exec, &project, err));
        }
        return;
    }
    if i.approval_gate.is_some() {
        return;
    }
    let Some(detect) = &i.detect_result else {
        if i.detect.is_none() {
            let mut init = BTreeMap::new();
            if !s.request.trim().is_empty() {
                init.insert("User focus".into(), s.request.clone());
            }
            push(spawn(s, InitializerMode::Detect, None, init));
        }
        return;
    };
    let scout_plan = str_at(detect, "scout_plan_path").to_string();
    let reference = str_at(detect, "reference_name").to_string();
    let slices = slices(detect);
    // Rule I1: no slices is a complete existing skill setup, so propose reconciles it without scouts.
    if slices.is_empty() && existing_skill_count(detect) == 0 {
        push(Step::Fail {
            error: "Detect found no slices to scout and no existing skills, so there is nothing to build skills from."
                .into(),
        });
        return;
    }
    let mut scouts_done = true;
    for (slug, descriptor, paths) in &slices {
        let item = i.scouts.iter().find(|x| &x.key == slug);
        match item {
            Some(x) if x.result.is_some() => {}
            Some(x) if x.exec.is_some() => scouts_done = false,
            _ => {
                scouts_done = false;
                let mut init = BTreeMap::new();
                init.insert("Slice".into(), descriptor.clone());
                init.insert("Slice paths".into(), paths.join("\n"));
                init.insert(
                    STACK_REFERENCE_NAME.into(),
                    if reference.is_empty() {
                        "_generic".into()
                    } else {
                        reference.clone()
                    },
                );
                init.insert("Scout plan".into(), scout_plan.clone());
                push(spawn(s, InitializerMode::Scout, Some(slug.clone()), init));
            }
        }
    }
    if !scouts_done {
        return;
    }
    let findings = findings_list(i);
    let Some(propose) = &i.propose_result else {
        if i.propose.is_none() {
            let mut init = BTreeMap::new();
            init.insert("Scout findings".into(), findings);
            init.insert("Scout plan".into(), scout_plan);
            push(spawn(s, InitializerMode::Propose, None, init));
        }
        return;
    };
    let proposal_path = str_at(propose, "proposal_path").to_string();
    let Some(decisions) = &i.decisions else {
        let skills = proposals(propose);
        if skills.is_empty() {
            push(Step::Fail {
                error: "Scouting found no recurring components, so there are no skills to propose."
                    .into(),
            });
            return;
        }
        push(Step::OpenGate {
            title: "Approve the skills".into(),
            explanation: "Each skill is grounded in a real exemplar the scouts captured. Choose, per skill, whether to generate it, regenerate an existing one from the current code, reuse it as it is, or drop it.".into(),
            payload: GatePayload::SkillApproval { project, skills },
        });
        return;
    };
    let mut generates_done = true;
    let mut generated = vec![];
    let mut reused = vec![];
    for (name, disposition) in decisions {
        let entry = skill_entry(propose, name);
        let kind = str_at(&entry, "kind").to_string();
        let component = entry.get("component_type").cloned().unwrap_or(Value::Null);
        match disposition.as_str() {
            "generate" | "regenerate" => {
                let item = i.generates.iter().find(|x| &x.key == name);
                match item {
                    Some(x) if x.result.is_some() => {
                        let r = x.result.clone().unwrap_or(Value::Null);
                        generated.push(json!({"name": name, "kind": kind, "componentType": component, "path": r.get("path").cloned().unwrap_or(Value::Null)}));
                    }
                    Some(x) if x.exec.is_some() => generates_done = false,
                    _ => {
                        generates_done = false;
                        let mut init = BTreeMap::new();
                        init.insert("Skill name".into(), name.clone());
                        init.insert("Skill kind".into(), kind.clone());
                        init.insert(
                            "Component type".into(),
                            component.as_str().unwrap_or("none").to_string(),
                        );
                        init.insert("Disposition".into(), disposition.clone());
                        init.insert("Proposal".into(), proposal_path.clone());
                        init.insert("Scout findings".into(), findings.clone());
                        push(spawn(
                            s,
                            InitializerMode::GenerateSkill,
                            Some(name.clone()),
                            init,
                        ));
                    }
                }
            }
            "reuse" => {
                reused.push(json!({"name": name, "kind": kind, "componentType": component, "path": entry.get("existing_path").cloned().unwrap_or(Value::Null)}));
            }
            _ => {}
        }
    }
    if !generates_done {
        return;
    }
    let Some(inventory) = &i.inventory_result else {
        if i.inventory.is_none() {
            let mut init = BTreeMap::new();
            init.insert(
                "Generated skills".into(),
                serde_json::to_string(&generated).unwrap_or_default(),
            );
            init.insert(
                "Reused skills".into(),
                serde_json::to_string(&reused).unwrap_or_default(),
            );
            init.insert("Proposal".into(), proposal_path);
            init.insert("Scout findings".into(), findings);
            push(spawn(s, InitializerMode::GenerateInventory, None, init));
        }
        return;
    };
    let md = format!(
        "# Project initialized\n\nOstra wrote `{}` and `{}`.\n\n- Skills generated: {}\n- Skills reused: {}\n\nLater executions in this project load these skills by path and route work through the inventory.\n\nGeneration report: `{}`\n",
        str_at(inventory, "inventory_path"),
        str_at(inventory, "profile_path"),
        generated.len(),
        reused.len(),
        str_at(inventory, "report_path"),
    );
    push(Step::Complete {
        report_markdown: Some(md),
    });
}

fn failed_gate(exec: &ExecutionId, project: &str, err: &str) -> Step {
    Step::OpenGate {
        title: "The initializer failed".into(),
        explanation: "The execution ended without a usable result. Retry it, or abandon the init."
            .into(),
        payload: GatePayload::ExecutionFailed {
            execution: exec.clone(),
            agent: AgentName::Initializer,
            project: project.to_string(),
            error: err.to_string(),
        },
    }
}
