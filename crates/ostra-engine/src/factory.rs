//! The spawn factory over `ostra-agents`: planner inputs become the agent's typed spawn struct,
//! rendered as the `Label: value` block, plus the repo brief and custom instructions.

use crate::init::STACK_REFERENCE_NAME;
use crate::plan::{SpawnInputs, SpawnRequest};
use crate::services::{AgentMeta, BuiltSpawn, SpawnEnv, SpawnFactory};
use ostra_agents::brief::{ArtifactsBrief, BriefInput, augment};
use ostra_agents::spawn::*;
use ostra_core::agent::{AgentName, InitializerMode};
use ostra_core::event::{ExecPurpose, FactTarget, WorkKind};
use ostra_core::executor::ExecutorKind;
use crate::state::SessionState;
use ostra_core::pipeline::{Category, Track};
use std::path::{Path, PathBuf};

pub struct AgentsFactory;

fn work_source(inputs: &SpawnInputs, s: &SessionState) -> WorkSource {
    match inputs.phase.as_ref().and_then(|p| p.file.clone()) {
        Some(f) => WorkSource::PhaseFile(f),
        None => WorkSource::NoPlan(
            match (inputs.revision, s.category, s.track) {
                (Some(_), _, _) => "A revision the user asked for after reviewing the implementation. The request below and the session context file describe it.",
                (_, Some(Category::Verify), _) => "A verification request: no code change is planned.",
                (_, Some(Category::UnitTest), _) => "The user asked for tests directly, so no plan exists.",
                (_, Some(Category::Prompt), _) => "A prompt change: the pipeline has no plan tier for it.",
                (_, Some(Category::QuickChange), _) => "A quick change: the request names the whole edit, so research, spec, plan, and review were skipped.",
                (_, Some(Category::Implement), Some(Track::Light)) => "The light track: research found a contained change, so the spec and plan stages were skipped. Work from the request and the research documents.",
                _ => "The stakes judge rated this request low, so the plan tier was skipped.",
            }
            .into(),
        ),
    }
}

fn work_extras(inputs: &SpawnInputs) -> Extras {
    let mut e = Extras {
        ledger_file: inputs.ledger_file.clone(),
        research_docs: inputs.research_docs.clone(),
        prior_reports: inputs.prior_reports.clone(),
        context_files: inputs.context_files.clone(),
        user_notes: inputs.user_notes.clone(),
        ..Default::default()
    };
    match inputs.work {
        Some(WorkKind::Fix | WorkKind::BlockerFix) => e.findings = inputs.instructions.clone(),
        Some(WorkKind::Rescue) => e.rescue_context = inputs.instructions.clone(),
        Some(WorkKind::Resume) => e.resume_instructions = inputs.instructions.clone(),
        Some(WorkKind::Rerun) => {
            e.task_note = Some(match &inputs.instructions {
                Some(i) => format!("An earlier run of this step ended before it finished. Continue it. Its instructions were:\n{i}"),
                None => "An earlier run of this step ended before it finished. Check the progress log and continue.".into(),
            })
        }
        Some(WorkKind::Initial) | None => {}
    }
    if e.task_note.is_none()
        && let Some(t) = &inputs.task
    {
        e.task_note = Some(format!("The request: {t}"));
    }
    e
}

fn required(value: Option<PathBuf>, what: &str) -> Result<PathBuf, String> {
    value.ok_or_else(|| format!("missing {what}"))
}

fn init_list(inputs: &SpawnInputs, key: &str) -> Vec<PathBuf> {
    inputs
        .init
        .get(key)
        .map(|v| {
            v.lines()
                .filter(|l| !l.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn init_str(inputs: &SpawnInputs, key: &str) -> Result<String, String> {
    inputs
        .init
        .get(key)
        .cloned()
        .ok_or_else(|| format!("missing {key}"))
}

impl SpawnFactory for AgentsFactory {
    fn agent_meta(&self, agent: AgentName) -> AgentMeta {
        let d = ostra_agents::agent_def(agent);
        AgentMeta {
            default_tier: d.default_tier,
            capabilities: d.capabilities.clone(),
            timeout_secs: d.timeout_secs,
        }
    }

    fn judge_prompt(&self, name: &str) -> Option<String> {
        if let Some(agent) = name.strip_prefix("__agent:") {
            let agent = agent.parse().ok()?;
            return ostra_agents::render_prompt(agent, ExecutorKind::Native).ok();
        }
        ostra_agents::judge_prompt(name).map(String::from)
    }

    fn build(&self, req: &SpawnRequest, env: &SpawnEnv<'_>) -> Result<BuiltSpawn, String> {
        let s = env.state;
        let i = &req.inputs;
        let common = Common {
            workspace_root: s.workspace_root.clone(),
            repo_root: env.repo_root.to_path_buf(),
            session_dir: req.session_dir.clone(),
            repo_key: req.project.clone(),
        };
        let params: Box<dyn SpawnParams> = match req.agent {
            AgentName::Explore => Box::new(ExploreParams {
                common,
                task: i.task.clone().ok_or("missing task")?,
                extra: Extras::default(),
            }),
            AgentName::GenerateSpec => Box::new(GenerateSpecParams {
                common,
                task: i.task.clone().ok_or("missing task")?,
                spec_file: i.spec_file.clone(),
                new_research_docs: i.new_research_docs.clone(),
                requirement_changes: i.changes.clone(),
                extra: Extras {
                    research_docs: i.research_docs.clone(),
                    projects_in_scope: i.projects_in_scope.clone(),
                    user_answers: i.answers.clone(),
                    findings: i.findings.clone(),
                    ..Default::default()
                },
            }),
            AgentName::FactCheck => Box::new(FactCheckParams {
                common,
                target: required(i.target.clone(), "target")?,
                target_type: match i.target_type {
                    Some(FactTarget::Plan) => TargetType::Plan,
                    _ => TargetType::Spec,
                },
                prior_findings: i.prior_findings.clone().unwrap_or_else(|| "none".into()),
                spec_file: required(i.spec_file.clone(), "spec file")?,
                source_check: if i.source_check.as_deref() == Some("refetch") {
                    SourceCheck::Refetch
                } else {
                    SourceCheck::Citations
                },
                research_docs: i.research_docs.clone(),
            }),
            AgentName::Plan => Box::new(PlanParams {
                common,
                spec_file: required(i.spec_file.clone(), "spec file")?,
                projects_in_scope: i.projects_in_scope.clone(),
                findings: i.findings.clone(),
                master_plan: i.target.clone(),
            }),
            AgentName::Implementer => Box::new(ImplementerParams {
                common,
                report_file: required(i.report_file.clone(), "report file")?,
                work: work_source(i, s),
                extra: Extras {
                    // Rule O2: the phase's project does not exist yet.
                    new_project: s.project_to_create(&req.project).map(|folder| {
                        format!(
                            "`{}` does not exist yet. Create it with ProjectCreate before anything else, in folder `{}`.",
                            req.project,
                            folder.display()
                        )
                    }),
                    ..work_extras(i)
                },
            }),
            AgentName::CodeReviewer => {
                let tests = matches!(req.purpose, ExecPurpose::Review { tests: true, .. });
                Box::new(CodeReviewerParams {
                    common,
                    phase: i.phase_value.clone().ok_or("missing phase")?,
                    changed_files: i.changed_files.clone(),
                    change_rationale: i.rationale.clone().unwrap_or_default(),
                    work: work_source(i, s),
                    // Staging keeps each review on its own change (Step 2).
                    review_scope: Some("unstaged".into()),
                    context: Some(if tests {
                        ReviewContext::Test
                    } else {
                        ReviewContext::Implementation
                    }),
                    epa_report: i.epa_report.clone(),
                })
            }
            AgentName::ExecutionPathAnalyzer => Box::new(EpaParams {
                common,
                implementer_report: required(i.implementer_report.clone(), "implementer report")?,
                report_file: required(i.report_file.clone(), "report file")?,
                phase_file: i.phase.as_ref().and_then(|p| p.file.clone()),
                extra: Extras {
                    user_notes: i.user_notes.clone(),
                    ..Default::default()
                },
            }),
            AgentName::WriteTest => Box::new(WriteTestParams {
                common,
                implementer_report: required(i.implementer_report.clone(), "implementer report")?,
                epa_report: required(i.epa_report.clone(), "EPA report")?,
                report_file: required(i.report_file.clone(), "report file")?,
                work: work_source(i, s),
                extra: work_extras(i),
            }),
            AgentName::ModuleDocumentation => Box::new(ModuleDocsParams {
                common,
                implementer_reports: i.implementer_reports.clone(),
                report_file: required(i.report_file.clone(), "report file")?,
                extra: Extras {
                    user_notes: i.user_notes.clone(),
                    ..Default::default()
                },
            }),
            AgentName::PromptGeneration => Box::new(PromptGenParams {
                common,
                task: i.task.clone().ok_or("missing task")?,
                target_files: i
                    .target_files
                    .clone()
                    .unwrap_or_default()
                    .lines()
                    .map(String::from)
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
                report_file: required(i.report_file.clone(), "report file")?,
                extra: Extras::default(),
            }),
            AgentName::Advisor => Box::new(AdvisorParams {
                common,
                failed_step: init_str(i, "Failed step")?,
                problem: init_str(i, "Problem")?,
                step_inputs: init_str(i, "Step inputs")?,
                step_result: i.init.get("Step result").cloned(),
                context: i.init.get("Step context").cloned(),
                earlier_guidance: i
                    .init
                    .get("Earlier guidance")
                    .and_then(|g| serde_json::from_str(g).ok())
                    .unwrap_or_default(),
            }),
            AgentName::QuickAnswer => Box::new(QuickAnswerParams {
                common,
                question: i.question.clone().ok_or("missing question")?,
                projects_in_scope: s
                    .projects
                    .iter()
                    .map(|p| (p.key.clone(), p.path.clone()))
                    .collect(),
                session_artifacts: vec![],
            }),
            AgentName::Initializer => {
                let ExecPurpose::Init { mode, .. } = &req.purpose else {
                    return Err("initializer without a mode".into());
                };
                match mode {
                    InitializerMode::Detect => Box::new(InitDetectParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        user_focus: i.init.get("User focus").cloned(),
                    }),
                    InitializerMode::Adopt => Box::new(InitAdoptParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        source_harness: "ultracode".into(),
                        source_runtime_dir: ".ultracode".into(),
                        source_skills_dir: ".ultracode/skills".into(),
                    }),
                    InitializerMode::Scout => Box::new(InitScoutParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        slice: init_str(i, "Slice")?,
                        slice_paths: init_str(i, "Slice paths")?
                            .lines()
                            .map(String::from)
                            .collect(),
                        stack_reference: ostra_agents::reference_path(&init_str(
                            i,
                            STACK_REFERENCE_NAME,
                        )?),
                        scout_plan: PathBuf::from(init_str(i, "Scout plan")?),
                    }),
                    InitializerMode::Propose => Box::new(InitProposeParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        scout_findings: init_list(i, "Scout findings"),
                        scout_plan: PathBuf::from(init_str(i, "Scout plan")?),
                    }),
                    InitializerMode::GenerateSkill => Box::new(InitGenerateSkillParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        skill_name: init_str(i, "Skill name")?,
                        skill_kind: init_str(i, "Skill kind")?,
                        disposition: init_str(i, "Disposition")?,
                        proposal: PathBuf::from(init_str(i, "Proposal")?),
                        scout_findings: init_list(i, "Scout findings"),
                    }),
                    InitializerMode::GenerateInventory => Box::new(InitGenerateInventoryParams {
                        common,
                        advisor_guidance: i.init.get("Advisor guidance").cloned(),
                        generated_skills: init_str(i, "Generated skills")?,
                        reused_skills: init_str(i, "Reused skills")?,
                        proposal: PathBuf::from(init_str(i, "Proposal")?),
                        scout_findings: init_list(i, "Scout findings"),
                    }),
                }
            }
        };
        let block = params.render();
        let ws = &env.state.workspace_root;
        let projects: Vec<(String, PathBuf)> = env
            .settings
            .projects
            .iter()
            .map(|p| (p.key.clone(), p.path.clone()))
            .collect();
        let instructions: Vec<String> = env
            .settings
            .instructions_for(req.agent)
            .into_iter()
            .map(|text| with_tagged_files(text, ws, &projects))
            .collect();
        let artifacts = ArtifactsBrief::read(ws);
        let first_message = augment(
            &block,
            &BriefInput {
                agent: req.agent,
                prompt: &block,
                repo_root: env.repo_root,
                profile: env.profile,
                inventory: env.inventory,
                instructions: &instructions,
                project_docs: env.project_docs,
                artifacts: artifacts.as_ref(),
                new_projects: &s.created_projects,
            },
        );
        let system_prompt =
            ostra_agents::render_prompt(req.agent, env.executor).map_err(|e| e.to_string())?;
        Ok(BuiltSpawn {
            system_prompt,
            first_message,
            spawn_block: block,
            params: params.to_json(),
            report_file: params.report_file().map(PathBuf::from),
            effort: ostra_core::config::resolve_effort(
                env.settings,
                req.agent.as_str(),
                req.complexity(),
            )
            .unwrap_or_else(|| ostra_agents::effort_for(req.agent, env.executor)),
        })
    }
}

/// Rule W3: an instruction that tags files or artifacts carries their absolute paths, so every
/// executor can read them. A tag naming a hidden or missing artifact resolves to nothing.
fn with_tagged_files(text: String, workspace: &Path, projects: &[(String, PathBuf)]) -> String {
    let files = ostra_core::artifacts::tagged_files(&text, workspace, projects);
    if files.is_empty() {
        return text;
    }
    let rows: Vec<String> = files
        .iter()
        .map(|(tag, path)| format!("- `{}` ({tag})", path.display()))
        .collect();
    format!(
        "{}\n\nFiles these instructions name. Read each before you start, because the user chose them:\n{}",
        text.trim_end(),
        rows.join("\n")
    )
}
