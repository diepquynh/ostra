//! The spawn factory over `ostra-agents`: planner inputs become the agent's typed spawn struct,
//! rendered as the `Label: value` block, plus the repo brief and custom instructions.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::init::STACK_REFERENCE_NAME;
use ostra_agents::AgentCatalog;
use ostra_agents::brief::{ArtifactsBrief, BooksBrief, BriefInput, augment};
use ostra_agents::spawn::*;
use ostra_core::Contract;
use ostra_core::agent::{AgentName, InitializerMode};
use ostra_core::event::{ExecPurpose, FactTarget, WorkKind};
use ostra_core::executor::ExecutorKind;
use ostra_core::pipeline::{Category, Track};
use ostra_engine::plan::{SpawnInputs, SpawnRequest};
use ostra_engine::services::{AgentMeta, BuiltSpawn, SpawnEnv, SpawnFactory};
use ostra_engine::state::SessionState;
use std::path::{Path, PathBuf};

pub struct AgentsFactory;

fn work_source(inputs: &SpawnInputs, s: &SessionState) -> WorkSource {
    match inputs.phase.as_ref().and_then(|p| p.file.clone()) {
        Some(f) => WorkSource::PhaseFile(f),
        None => WorkSource::NoPlan(
            match (inputs.revision, s.category, s.track) {
                (Some(_), _, _) => "A revision the user asked for after reviewing the implementation. The request below and the session context file describe it.",
                (_, Some(Category::Verify), _) => "A verification request: no code change is planned.",
                (_, Some(Category::Test), _) => "The user asked for tests directly, so no plan exists.",
                (_, Some(Category::Docs), _) => "The user asked for documentation directly, so no plan exists.",
                (_, Some(Category::Prompt), _) => "A prompt change: the pipeline has no plan tier for it.",
                (_, Some(Category::QuickChange), _) => "A quick change: the request names the whole edit, so research, spec, plan, and review were skipped.",
                (_, Some(Category::Implement), Some(Track::Light)) => "The light track: research found a contained change, so the spec and plan stages were skipped. Work from the request and the research documents.",
                _ => "The stakes judge rated this request low, so the plan tier was skipped.",
            }
            .into(),
        ),
    }
}

/// The session's book, when an earlier session already wrote it. Read at spawn time, outside the fold.
fn existing_book(s: &SessionState) -> Option<PathBuf> {
    let path = ostra_core::book::book_json(&s.workspace_root, &s.book_id());
    path.is_file().then_some(path)
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

/// Rules D2a and D4a: the code facts an agent gets, as the changed files or the facts file.
fn code_facts(i: &SpawnInputs, s: &SessionState) -> (Option<Vec<String>>, Option<PathBuf>) {
    if i.code_facts.is_empty() {
        (None, None)
    } else if i.wants_code_facts_file() {
        (None, Some(s.code_facts_path()))
    } else {
        (
            Some(ostra_core::doc::changed_since_research(&i.code_facts)),
            None,
        )
    }
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
    fn agent_meta(&self, agent: AgentName, agents: &AgentCatalog) -> Option<AgentMeta> {
        let d = agents.def(agent)?;
        Some(AgentMeta {
            default_tier: d.default_tier,
            capabilities: d.capabilities.clone(),
            timeout_secs: d.timeout_secs,
            write_scope: d.write_scope,
            submit_schema: d.submit_schema(),
            programmatic: d.programmatic,
            returns: d.returns,
        })
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
        let (changed_since_research, facts_file) = code_facts(i, s);
        // Rule CA5: the spawn is the contract's, whichever agent fills it.
        let def = env
            .agents
            .def(req.agent)
            .ok_or_else(|| format!("the workspace defines no agent `{}`", req.agent))?;
        let agent = req.agent;
        let params: Box<dyn SpawnParams> = match def.returns {
            // A workflow stage or a helper gets the stage spawn, whatever the agent returns.
            _ if matches!(
                req.purpose,
                ExecPurpose::Stage { .. } | ExecPurpose::Helper { .. }
            ) =>
            {
                Box::new(ostra_engine::workflow::custom_params(req, s, common)?)
            }
            _ if matches!(req.purpose, ExecPurpose::Consult { .. }) => Box::new(ConsultParams {
                common,
                agent,
                asked_by: i.task.clone().ok_or("missing asker")?,
                question: i.question.clone().ok_or("missing question")?,
            }),
            Contract::Research => Box::new(ExploreParams {
                common,
                task: i.task.clone().ok_or("missing task")?,
                extra: Extras::default(),
            }),
            Contract::Spec => Box::new(GenerateSpecParams {
                common,
                task: i.task.clone().ok_or("missing task")?,
                spec_file: i.spec_file.clone(),
                new_research_docs: i.new_research_docs.clone(),
                requirement_changes: i.changes.clone(),
                changed_since_research,
                extra: Extras {
                    research_docs: i.research_docs.clone(),
                    projects_in_scope: i.projects_in_scope.clone(),
                    user_answers: i.answers.clone(),
                    findings: i.findings.clone(),
                    ..Default::default()
                },
            }),
            Contract::FactCheck => Box::new(FactCheckParams {
                common,
                target: required(i.target.clone(), "target")?,
                target_type: match i.target_type {
                    _ if i.docs_check => TargetType::Page,
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
                changed_since_research,
                code_facts: facts_file,
            }),
            Contract::Plan => Box::new(PlanParams {
                common,
                spec_file: required(i.spec_file.clone(), "spec file")?,
                projects_in_scope: i.projects_in_scope.clone(),
                code_facts: facts_file,
                findings: i.findings.clone(),
                phases_to_revise: i.revise_phases.clone(),
                master_plan: i.target.clone(),
            }),
            // Rule O8: an implementer the user sent to a stuck run fixes only what stopped it.
            Contract::Implementation if matches!(req.purpose, ExecPurpose::Unblock { .. }) => {
                Box::new(ImplementerParams {
                    common,
                    report_file: required(i.report_file.clone(), "report file")?,
                    work: WorkSource::NoPlan(
                        "A fix for another run of this phase that is stuck. The Unblock line says what to fix.".into(),
                    ),
                    unblock: Some(i.instructions.clone().ok_or("missing unblock")?),
                    extra: Extras {
                        context_files: i.context_files.clone(),
                        user_notes: i.user_notes.clone(),
                        ..Default::default()
                    },
                })
            }
            Contract::Implementation => Box::new(ImplementerParams {
                common,
                report_file: required(i.report_file.clone(), "report file")?,
                work: work_source(i, s),
                unblock: None,
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
            Contract::Review => {
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
            Contract::PathAnalysis => Box::new(EpaParams {
                common,
                implementer_report: required(i.implementer_report.clone(), "implementer report")?,
                report_file: required(i.report_file.clone(), "report file")?,
                phase_file: i.phase.as_ref().and_then(|p| p.file.clone()),
                extra: Extras {
                    user_notes: i.user_notes.clone(),
                    ..Default::default()
                },
            }),
            Contract::Tests => Box::new(WriteTestParams {
                common,
                implementer_report: required(i.implementer_report.clone(), "implementer report")?,
                epa_report: required(i.epa_report.clone(), "EPA report")?,
                report_file: required(i.report_file.clone(), "report file")?,
                work: work_source(i, s),
                extra: work_extras(i),
            }),
            // Rule B2: the request and the docs-stage notes steer the writer, and the brief adds the
            // workspace instructions for the agent.
            Contract::Documentation => Box::new(DocumentationParams {
                common,
                implementer_reports: i.implementer_reports.clone(),
                existing_book: existing_book(s),
                reference: i.docs_reference.clone(),
                mode: match (i.docs_step, &i.docs_page) {
                    (Some(ostra_core::book::DocsStep::Survey), _) => DocsMode::Survey,
                    (Some(ostra_core::book::DocsStep::Synthesis), _) => DocsMode::Synthesis {
                        round: i.docs_round,
                        drafts: s.docs_drafts_dir(&req.project),
                        findings: i.docs_instructions.clone(),
                    },
                    (Some(ostra_core::book::DocsStep::Page), Some(p)) => {
                        DocsMode::Page(DocsPageScope {
                            id: p.id.clone(),
                            title: p.title.clone(),
                            group: p.group.clone(),
                            covers: p.covers.clone(),
                            sources: p.sources.clone(),
                            inventory: i.docs_inventory.clone(),
                            others: i
                                .docs_pages
                                .iter()
                                .filter(|o| o.id != p.id)
                                .map(|o| (o.id.clone(), o.title.clone(), o.covers.clone()))
                                .collect(),
                            drafts: s.docs_drafts_dir(&req.project),
                            draft: i.target.clone(),
                            instructions: i.docs_instructions.clone(),
                        })
                    }
                    _ => DocsMode::Part,
                },
                extra: Extras {
                    user_notes: i.user_notes.clone(),
                    task_note: Some(format!("The request: {}", s.full_request())),
                    ..Default::default()
                },
            }),
            Contract::Prompt => Box::new(PromptGenParams {
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
            Contract::Advice => Box::new(AdvisorParams {
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
            Contract::Answer => Box::new(QuickAnswerParams {
                common,
                question: i.question.clone().ok_or("missing question")?,
                projects_in_scope: s
                    .projects
                    .iter()
                    .map(|p| (p.key.clone(), p.path.clone()))
                    .collect(),
                session_artifacts: vec![],
            }),
            Contract::Setup => {
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
            Contract::Stage | Contract::Plugin(_) => {
                Box::new(ostra_engine::workflow::custom_params(req, s, common)?)
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
        let books = BooksBrief::read(ws).map(|mut b| {
            if env
                .agents
                .def(req.agent)
                .is_some_and(|d| d.capabilities.contains(&ostra_core::Capability::DocsSearch))
            {
                b.search_tool = ostra_agents::tool_name("docs_search", req.agent, env.executor);
            }
            b
        });
        let first_message = augment(
            &block,
            &BriefInput {
                agent: req.agent,
                contract: def.returns,
                sections: &def.brief,
                prompt: &block,
                repo_root: env.repo_root,
                profile: env.profile,
                inventory: env.inventory,
                instructions: &instructions,
                project_docs: env.project_docs,
                artifacts: artifacts.as_ref(),
                books: books.as_ref(),
                new_projects: &s.created_projects,
            },
        );
        let system_prompt = env
            .agents
            .render_prompt(req.agent, env.executor)
            .map_err(|e| e.to_string())?;
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
            .unwrap_or_else(|| {
                env.agents
                    .def(req.agent)
                    .map(|d| d.effort_on(env.executor))
                    .unwrap_or(ostra_core::Effort::High)
            }),
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
