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

/// Rule O5: advisor rounds per failing step of a created project's init before the user is asked.
pub const MAX_ADVICE: usize = 2;

/// `Stack reference:` is resolved by the spawn factory from this key to the extracted refs file.
pub const STACK_REFERENCE_NAME: &str = "stack_reference_name";

fn spawn(
    s: &SessionState,
    i: &InitTrack,
    mode: InitializerMode,
    item: Option<String>,
    mut init: BTreeMap<String, String>,
) -> Step {
    let project = i.project.clone();
    // Rule O5: a step the advisor looked at runs again with its latest guidance.
    let key = format!("{mode}:{}", item.clone().unwrap_or_default());
    if let Some(g) = i.advice.get(&key).and_then(|v| v.last()) {
        init.insert("Advisor guidance".into(), g.clone());
    }
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
        continues: None,
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

/// How an init track ends: the init session completes or fails, while a created project's init
/// records its end and the pipeline goes on (Rule O4).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Session,
    CreatedProject,
}

impl Ending {
    /// A step finished but left nothing to build on. `exec` is the step to run again.
    fn fail(self, project: &str, exec: Option<&ExecutionId>, error: &str) -> Step {
        match (self, exec) {
            (Ending::CreatedProject, Some(exec)) => Step::RecordInitProblem {
                project: project.into(),
                execution: exec.clone(),
                error: error.into(),
            },
            _ => Step::Fail {
                error: error.into(),
            },
        }
    }
}

/// Longest step result the advisor is given; the rest is cut, because the start of a result names
/// its shape and a long tail rarely changes the diagnosis.
pub const MAX_STEP_RESULT_CHARS: usize = 8_000;

/// Everything the advisor's spawn carries about one failed step (Rule O5).
#[derive(Debug, Clone)]
pub struct AdviceInputs {
    pub project: String,
    pub session_dir: std::path::PathBuf,
    /// The failed execution, which a `retry` runs again.
    pub execution: ExecutionId,
    /// The agent and mode, such as `initializer detect`.
    pub failed_step: String,
    pub problem: String,
    /// The failed run's own spawn block.
    pub step_inputs: String,
    /// The failed run's submit payload, when it made one.
    pub step_result: Option<Value>,
    pub context: String,
    /// Guidance earlier advice gave this step, oldest first.
    pub earlier: Vec<String>,
}

/// The label of a failed step, as the advisor sees it: `initializer scout api`.
pub fn failed_step_label(agent: AgentName, purpose: &ExecPurpose) -> String {
    match purpose {
        ExecPurpose::Init { mode, item } => match item {
            Some(it) => format!("initializer {mode} {it}"),
            None => format!("initializer {mode}"),
        },
        _ => agent.to_string(),
    }
}

/// The advisor's spawn for one failed step. The planner builds it from the session, and the advisor
/// evals from a scenario, so both give the advisor the same inputs.
pub fn advisor_request(a: AdviceInputs) -> SpawnRequest {
    let mut init = BTreeMap::new();
    init.insert("Failed step".into(), a.failed_step);
    init.insert("Problem".into(), a.problem);
    init.insert(
        "Step inputs".into(),
        if a.step_inputs.trim().is_empty() {
            "none recorded".into()
        } else {
            a.step_inputs
        },
    );
    if let Some(r) = &a.step_result {
        let text = serde_json::to_string_pretty(r).unwrap_or_default();
        let text = if text.chars().count() > MAX_STEP_RESULT_CHARS {
            let cut: String = text.chars().take(MAX_STEP_RESULT_CHARS).collect();
            format!("{cut}\n(cut at {MAX_STEP_RESULT_CHARS} characters)")
        } else {
            text
        };
        init.insert("Step result".into(), text);
    }
    init.insert("Step context".into(), a.context);
    if !a.earlier.is_empty() {
        init.insert(
            "Earlier guidance".into(),
            serde_json::to_string(&a.earlier).unwrap_or_default(),
        );
    }
    let purpose = ExecPurpose::Advise {
        project: a.project.clone(),
        execution: a.execution,
        round: a.earlier.len() as u32 + 1,
    };
    SpawnRequest {
        agent: AgentName::Advisor,
        stage: crate::state::stage_of(&purpose),
        purpose,
        project: a.project,
        session_dir: a.session_dir,
        inputs: SpawnInputs {
            init,
            ..Default::default()
        },
        resumes: None,
        continues: None,
    }
}

/// Rule O5: the advisor's next look at a failed step of a created project's init.
fn advise(s: &SessionState, i: &InitTrack, exec: &ExecutionId, err: &str, focus: &str) -> Step {
    let rec = s.executions.get(exec);
    let inputs = AdviceInputs {
        project: i.project.clone(),
        session_dir: s.project_session_dir(&i.project),
        execution: exec.clone(),
        failed_step: rec
            .map(|r| failed_step_label(r.agent, &r.purpose))
            .unwrap_or_default(),
        problem: err.to_string(),
        step_inputs: rec.map(|r| r.spawn_block.clone()).unwrap_or_default(),
        step_result: rec
            .and_then(|r| r.result.as_ref())
            .and_then(|r| r.submit.clone()),
        context: focus.to_string(),
        earlier: i
            .advice
            .get(&s.init_step_key(exec))
            .cloned()
            .unwrap_or_default(),
    };
    Step::Spawn(Box::new(advisor_request(inputs)))
}

pub fn plan_init(s: &SessionState, push: &mut dyn FnMut(Step)) {
    let Some(i) = &s.init else { return };
    plan_track(s, i, s.request.trim(), Ending::Session, push);
}

/// Rule O4: the init of every project an agent created, seeded from its `ProjectCreate` call.
pub fn plan_created(s: &SessionState, push: &mut dyn FnMut(Step)) {
    for c in &s.created_projects {
        if let Some(i) = s.project_inits.get(&c.key).filter(|i| !i.finished) {
            let focus = InitTrack::created_focus(c);
            plan_track(s, i, &focus, Ending::CreatedProject, push);
        }
    }
}

fn plan_track(
    s: &SessionState,
    i: &InitTrack,
    focus: &str,
    ending: Ending,
    push: &mut dyn FnMut(Step),
) {
    let project = i.project.clone();
    if let Some((exec, err)) = &i.failed {
        if i.failed_gate.is_some() || i.advising.is_some() {
            return;
        }
        // Rule O5: the advisor looks at a created project's failed step before the user does.
        let rounds = i.advice.get(&s.init_step_key(exec)).map_or(0, Vec::len);
        if ending == Ending::CreatedProject && !i.escalated && rounds < MAX_ADVICE {
            push(advise(s, i, exec, err, focus));
        } else {
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
            if !focus.is_empty() {
                init.insert("User focus".into(), focus.to_string());
            }
            push(spawn(s, i, InitializerMode::Detect, None, init));
        }
        return;
    };
    let scout_plan = str_at(detect, "scout_plan_path").to_string();
    let reference = str_at(detect, "reference_name").to_string();
    let slices = slices(detect);
    // Rule I1: no slices is a complete existing skill setup, so propose reconciles it without scouts.
    if slices.is_empty() && existing_skill_count(detect) == 0 {
        push(ending.fail(
            &project,
            i.detect.as_ref(),
            "Detect found no slices to scout and no existing skills, so there is nothing to build skills from.",
        ));
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
                push(spawn(
                    s,
                    i,
                    InitializerMode::Scout,
                    Some(slug.clone()),
                    init,
                ));
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
            push(spawn(s, i, InitializerMode::Propose, None, init));
        }
        return;
    };
    let proposal_path = str_at(propose, "proposal_path").to_string();
    let Some(decisions) = &i.decisions else {
        let skills = proposals(propose);
        if skills.is_empty() {
            push(ending.fail(
                &project,
                i.propose.as_ref(),
                "Scouting found no recurring components, so there are no skills to propose.",
            ));
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
                            i,
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
            push(spawn(s, i, InitializerMode::GenerateInventory, None, init));
        }
        return;
    };
    if ending == Ending::CreatedProject {
        push(Step::FinishInit { project });
        return;
    }
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
