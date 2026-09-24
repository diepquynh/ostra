//! The spawn contract (`UC/hooks/subagent-parameters.json`) as one struct per agent and per
//! initializer mode. Required parameters are non-`Option` fields, so a missing one does not compile.
//! Each struct renders the `Label: value` block the prompts expect, and [`parse_block`] validates the
//! same block at runtime for harness executors.

use ostra_core::pipeline::QuestionAnswer;
use ostra_core::slug::is_project_key;
use ostra_core::{AgentName, InitializerMode};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Behavior every spawn struct shares.
pub trait SpawnParams {
    fn agent(&self) -> AgentName;
    fn mode(&self) -> Option<InitializerMode> {
        None
    }
    fn common(&self) -> &Common;
    /// The `Label: value` block. Required lines first, then the common four, then optional extras.
    fn render(&self) -> String;
    fn to_json(&self) -> Value;
    fn report_file(&self) -> Option<&Path> {
        None
    }
}

/// The four lines every spawn carries. `Workspace root:` replaces Ultracode's `Primary repo root:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Common {
    pub workspace_root: PathBuf,
    pub repo_root: PathBuf,
    pub session_dir: PathBuf,
    pub repo_key: String,
}

/// Hard rule 13: phase-bound agents declare either their phase file or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSource {
    PhaseFile(PathBuf),
    NoPlan(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    Spec,
    Plan,
}

impl TargetType {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetType::Spec => "spec",
            TargetType::Plan => "plan",
        }
    }
}

/// Rule D3b: `refetch` only on a spec target's first pass whose External Evidence table has rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceCheck {
    Citations,
    Refetch,
}

impl SourceCheck {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceCheck::Citations => "citations",
            SourceCheck::Refetch => "refetch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewContext {
    Implementation,
    Test,
}

/// Optional lines the engine adds below the required ones. Absent fields render nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Extras {
    /// Every research document, oldest first (Rule D2).
    pub research_docs: Vec<PathBuf>,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    /// Answers to open questions, folded in by generate-spec (Rule D3).
    pub user_answers: Vec<QuestionAnswer>,
    /// Fix instructions, review findings, or fact-check findings, verbatim.
    pub findings: Option<String>,
    pub required_skills: Vec<String>,
    /// The review ledger the fix agent records FIXED or WONTFIX in.
    pub ledger_file: Option<PathBuf>,
    /// STUCK diagnostic plus the stated fact (never a plain retry).
    pub rescue_context: Option<String>,
    /// What to do after a HANDOFF specialist finished.
    pub resume_instructions: Option<String>,
    pub prior_reports: Vec<PathBuf>,
    pub context_files: Vec<PathBuf>,
    /// Free-form instructions below the parameters.
    pub task_note: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------------------------

struct Block(String);

impl Block {
    fn new() -> Self {
        Block(String::new())
    }

    fn line(&mut self, label: &str, value: &str) {
        let value = value.trim();
        if value.contains('\n') {
            let _ = writeln!(self.0, "{label}:");
            for l in value.lines() {
                let _ = writeln!(self.0, "  {l}");
            }
        } else {
            let _ = writeln!(self.0, "{label}: {value}");
        }
    }

    fn path(&mut self, label: &str, value: &Path) {
        self.line(label, &value.display().to_string());
    }

    fn list<T: AsRef<str>>(&mut self, label: &str, values: &[T]) {
        if values.is_empty() {
            return;
        }
        let _ = writeln!(self.0, "{label}:");
        for v in values {
            let _ = writeln!(self.0, "  {}", v.as_ref().trim());
        }
    }

    fn paths(&mut self, label: &str, values: &[PathBuf]) {
        let v: Vec<String> = values.iter().map(|p| p.display().to_string()).collect();
        self.list(label, &v);
    }

    fn opt(&mut self, label: &str, value: Option<&str>) {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            self.line(label, v);
        }
    }

    fn opt_path(&mut self, label: &str, value: Option<&Path>) {
        if let Some(v) = value {
            self.path(label, v);
        }
    }

    fn common(&mut self, c: &Common) {
        self.path("Workspace root", &c.workspace_root);
        self.path("Repo root", &c.repo_root);
        self.path("Session dir", &c.session_dir);
        self.line("Repo key", &c.repo_key);
    }

    fn work(&mut self, w: &WorkSource) {
        match w {
            WorkSource::PhaseFile(p) => self.path("Phase file", p),
            WorkSource::NoPlan(why) => self.line("No plan", why),
        }
    }

    fn scope(&mut self, projects: &[(String, PathBuf)]) {
        let v: Vec<String> = projects.iter().map(|(k, p)| format!("{k} -> {}", p.display())).collect();
        self.list("Repos in scope", &v);
    }

    fn extras(&mut self, e: &Extras) {
        self.paths("Research docs", &e.research_docs);
        self.scope(&e.projects_in_scope);
        if !e.user_answers.is_empty() {
            let v: Vec<String> =
                e.user_answers.iter().map(|a| format!("{} {}: {}", a.id, a.question, a.answer)).collect();
            self.list("User answers", &v);
        }
        self.opt("Findings", e.findings.as_deref());
        if !e.required_skills.is_empty() {
            self.line("Required skills", &e.required_skills.join(", "));
        }
        self.opt_path("Review ledger", e.ledger_file.as_deref());
        self.opt("Rescue context", e.rescue_context.as_deref());
        self.opt("Resume instructions", e.resume_instructions.as_deref());
        self.paths("Prior phase reports", &e.prior_reports);
        self.paths("Context files", &e.context_files);
        if let Some(note) = e.task_note.as_deref().filter(|n| !n.trim().is_empty()) {
            let _ = writeln!(self.0, "\n{}", note.trim());
        }
    }

    fn finish(self) -> String {
        self.0
    }
}

fn json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------
// Pipeline agents
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExploreParams {
    pub common: Common,
    pub task: String,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerateSpecParams {
    pub common: Common,
    /// The complete request as it now stands.
    pub task: String,
    /// On a revision, the spec file to rewrite in place.
    pub spec_file: Option<PathBuf>,
    /// On a revision, the research documents the spec was written without.
    pub new_research_docs: Vec<PathBuf>,
    /// Requirement changes from the user that the spec does not reflect yet.
    pub requirement_changes: Vec<String>,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FactCheckParams {
    pub common: Common,
    pub target: PathBuf,
    pub target_type: TargetType,
    /// `none` on the first pass, then the previous pass's findings verbatim or `NO_PRIOR_FINDINGS` (Rule D3a).
    pub prior_findings: String,
    pub spec_file: PathBuf,
    pub source_check: SourceCheck,
    /// Research documents, on a spec target only (Rule D5 withholds them from a plan target).
    pub research_docs: Vec<PathBuf>,
}

/// Rule D4, Hard rule 16: the plan agent gets the spec and nothing else. The only other inputs are
/// the fact-check findings of a re-pass (Rule D5) and the master plan being revised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanParams {
    pub common: Common,
    pub spec_file: PathBuf,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    pub findings: Option<String>,
    pub master_plan: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImplementerParams {
    pub common: Common,
    pub report_file: PathBuf,
    pub work: WorkSource,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeReviewerParams {
    pub common: Common,
    /// `N`, `N-tests`, or `none`: the loop, not the pass.
    pub phase: String,
    pub changed_files: Vec<String>,
    pub change_rationale: String,
    pub work: WorkSource,
    /// `unstaged` whenever staging is in effect.
    pub review_scope: Option<String>,
    pub context: Option<ReviewContext>,
    pub epa_report: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpaParams {
    pub common: Common,
    pub implementer_report: PathBuf,
    pub report_file: PathBuf,
    pub phase_file: Option<PathBuf>,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteTestParams {
    pub common: Common,
    pub implementer_report: PathBuf,
    pub epa_report: PathBuf,
    pub report_file: PathBuf,
    pub work: WorkSource,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleDocsParams {
    pub common: Common,
    pub implementer_reports: Vec<PathBuf>,
    pub report_file: PathBuf,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptGenParams {
    pub common: Common,
    pub task: String,
    pub target_files: Vec<String>,
    pub report_file: PathBuf,
    pub extra: Extras,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuickAnswerParams {
    pub common: Common,
    pub question: String,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    pub session_artifacts: Vec<PathBuf>,
}

impl SpawnParams for ExploreParams {
    fn agent(&self) -> AgentName {
        AgentName::Explore
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.line("Task", &self.task);
        b.common(&self.common);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

impl SpawnParams for GenerateSpecParams {
    fn agent(&self) -> AgentName {
        AgentName::GenerateSpec
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.line("Task", &self.task);
        b.common(&self.common);
        b.opt_path("Spec file", self.spec_file.as_deref());
        b.paths("New research docs", &self.new_research_docs);
        b.list("Requirement changes", &self.requirement_changes);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

impl SpawnParams for FactCheckParams {
    fn agent(&self) -> AgentName {
        AgentName::FactCheck
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.path("Target", &self.target);
        b.line("Target type", self.target_type.as_str());
        b.line("Prior findings", &self.prior_findings);
        b.path("Spec file", &self.spec_file);
        b.line("Source check", self.source_check.as_str());
        b.common(&self.common);
        b.paths("Research docs", &self.research_docs);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

impl SpawnParams for PlanParams {
    fn agent(&self) -> AgentName {
        AgentName::Plan
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.path("Spec file", &self.spec_file);
        b.common(&self.common);
        b.scope(&self.projects_in_scope);
        b.opt("Findings", self.findings.as_deref());
        b.opt_path("Master plan", self.master_plan.as_deref());
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

impl SpawnParams for ImplementerParams {
    fn agent(&self) -> AgentName {
        AgentName::Implementer
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.path("Report file", &self.report_file);
        b.work(&self.work);
        b.common(&self.common);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
    fn report_file(&self) -> Option<&Path> {
        Some(&self.report_file)
    }
}

impl SpawnParams for CodeReviewerParams {
    fn agent(&self) -> AgentName {
        AgentName::CodeReviewer
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.line("Phase", &self.phase);
        b.list("Changed files", &self.changed_files);
        b.line("Change rationale", &self.change_rationale);
        b.work(&self.work);
        b.common(&self.common);
        b.opt("Review scope", self.review_scope.as_deref());
        match self.context {
            Some(ReviewContext::Implementation) => b.line("Review context", "Review implementation code"),
            Some(ReviewContext::Test) => b.line("Review context", "Review test code"),
            None => {}
        }
        b.opt_path("EPA report", self.epa_report.as_deref());
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

impl SpawnParams for EpaParams {
    fn agent(&self) -> AgentName {
        AgentName::ExecutionPathAnalyzer
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.path("Implementer report", &self.implementer_report);
        b.path("Report file", &self.report_file);
        b.common(&self.common);
        b.opt_path("Phase file", self.phase_file.as_deref());
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
    fn report_file(&self) -> Option<&Path> {
        Some(&self.report_file)
    }
}

impl SpawnParams for WriteTestParams {
    fn agent(&self) -> AgentName {
        AgentName::WriteTest
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.path("Implementer report", &self.implementer_report);
        b.path("EPA report", &self.epa_report);
        b.path("Report file", &self.report_file);
        b.work(&self.work);
        b.common(&self.common);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
    fn report_file(&self) -> Option<&Path> {
        Some(&self.report_file)
    }
}

impl SpawnParams for ModuleDocsParams {
    fn agent(&self) -> AgentName {
        AgentName::ModuleDocumentation
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.paths("Implementer reports", &self.implementer_reports);
        b.path("Report file", &self.report_file);
        b.common(&self.common);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
    fn report_file(&self) -> Option<&Path> {
        Some(&self.report_file)
    }
}

impl SpawnParams for PromptGenParams {
    fn agent(&self) -> AgentName {
        AgentName::PromptGeneration
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.line("Task", &self.task);
        b.list("Target files", &self.target_files);
        b.path("Report file", &self.report_file);
        b.common(&self.common);
        b.extras(&self.extra);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
    fn report_file(&self) -> Option<&Path> {
        Some(&self.report_file)
    }
}

impl SpawnParams for QuickAnswerParams {
    fn agent(&self) -> AgentName {
        AgentName::QuickAnswer
    }
    fn common(&self) -> &Common {
        &self.common
    }
    fn render(&self) -> String {
        let mut b = Block::new();
        b.line("Question", &self.question);
        b.common(&self.common);
        b.scope(&self.projects_in_scope);
        b.paths("Session artifacts", &self.session_artifacts);
        b.finish()
    }
    fn to_json(&self) -> Value {
        json(self)
    }
}

// ---------------------------------------------------------------------------------------------
// Initializer modes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitDetectParams {
    pub common: Common,
    pub user_focus: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitAdoptParams {
    pub common: Common,
    pub source_harness: String,
    pub source_runtime_dir: String,
    pub source_skills_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitScoutParams {
    pub common: Common,
    pub slice: String,
    pub slice_paths: Vec<String>,
    pub stack_reference: PathBuf,
    pub scout_plan: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitProposeParams {
    pub common: Common,
    pub scout_findings: Vec<PathBuf>,
    pub scout_plan: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitGenerateSkillParams {
    pub common: Common,
    pub skill_name: String,
    /// `creation`, `convention`, or `module-hub`.
    pub skill_kind: String,
    /// `generate` or `regenerate`.
    pub disposition: String,
    pub proposal: PathBuf,
    pub scout_findings: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InitGenerateInventoryParams {
    pub common: Common,
    /// JSON array of `{name, kind, component_type, path}`.
    pub generated_skills: String,
    /// JSON array of `{name, kind, component_type, path}`.
    pub reused_skills: String,
    pub proposal: PathBuf,
    pub scout_findings: Vec<PathBuf>,
}

macro_rules! init_impl {
    ($ty:ty, $mode:expr, |$s:ident, $b:ident| $body:block) => {
        impl SpawnParams for $ty {
            fn agent(&self) -> AgentName {
                AgentName::Initializer
            }
            fn mode(&self) -> Option<InitializerMode> {
                Some($mode)
            }
            fn common(&self) -> &Common {
                &self.common
            }
            fn render(&self) -> String {
                let $s = self;
                let mut $b = Block::new();
                $b.line("Mode", $mode.as_str());
                $body
                $b.common(&$s.common);
                $b.finish()
            }
            fn to_json(&self) -> Value {
                let mut v = json(self);
                if let Value::Object(m) = &mut v {
                    m.insert("mode".into(), Value::String($mode.as_str().into()));
                }
                v
            }
        }
    };
}

init_impl!(InitDetectParams, InitializerMode::Detect, |s, b| {
    b.opt("User focus", s.user_focus.as_deref());
});
init_impl!(InitAdoptParams, InitializerMode::Adopt, |s, b| {
    b.line("Source harness", &s.source_harness);
    b.line("Source runtime dir", &s.source_runtime_dir);
    b.line("Source skills dir", &s.source_skills_dir);
});
init_impl!(InitScoutParams, InitializerMode::Scout, |s, b| {
    b.line("Slice", &s.slice);
    b.list("Slice paths", &s.slice_paths);
    b.path("Stack reference", &s.stack_reference);
    b.path("Scout plan", &s.scout_plan);
});
init_impl!(InitProposeParams, InitializerMode::Propose, |s, b| {
    b.paths("Scout findings", &s.scout_findings);
    b.path("Scout plan", &s.scout_plan);
});
init_impl!(InitGenerateSkillParams, InitializerMode::GenerateSkill, |s, b| {
    b.line("Skill name", &s.skill_name);
    b.line("Skill kind", &s.skill_kind);
    b.line("Disposition", &s.disposition);
    b.path("Proposal", &s.proposal);
    b.paths("Scout findings", &s.scout_findings);
});
init_impl!(InitGenerateInventoryParams, InitializerMode::GenerateInventory, |s, b| {
    b.line("Generated skills", &s.generated_skills);
    b.line("Reused skills", &s.reused_skills);
    b.path("Proposal", &s.proposal);
    b.paths("Scout findings", &s.scout_findings);
});

// ---------------------------------------------------------------------------------------------
// Runtime validation (harness executors)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AbsoluteDir,
    AbsolutePath,
    Path,
    RepoKey,
    Phase,
    Text,
    Enum(&'static [&'static str]),
}

/// Parameter id, its accepted labels, and its type. Ported from `subagent-parameters.json`.
const PARAMS: &[(&str, &[&str], Kind)] = &[
    ("workspace_root", &["Workspace root"], Kind::AbsoluteDir),
    ("repo_root", &["Repo root"], Kind::AbsoluteDir),
    ("session_dir", &["Session dir"], Kind::AbsolutePath),
    ("repo_key", &["Repo key"], Kind::RepoKey),
    ("phase", &["Phase"], Kind::Phase),
    ("task", &["Task"], Kind::Text),
    ("question", &["Question"], Kind::Text),
    (
        "mode",
        &["Mode"],
        Kind::Enum(&["detect", "adopt", "scout", "propose", "generate-skill", "generate-inventory"]),
    ),
    ("user_focus", &["User focus"], Kind::Text),
    ("source_harness", &["Source harness"], Kind::Text),
    ("source_runtime_dir", &["Source runtime dir"], Kind::Path),
    ("source_skills_dir", &["Source skills dir"], Kind::Path),
    ("slice", &["Slice"], Kind::Text),
    ("slice_paths", &["Slice paths"], Kind::Text),
    ("stack_reference", &["Stack reference"], Kind::Path),
    ("scout_plan", &["Scout plan"], Kind::Path),
    ("scout_findings", &["Scout findings"], Kind::Text),
    ("skill_name", &["Skill name"], Kind::Text),
    ("skill_kind", &["Skill kind"], Kind::Text),
    ("disposition", &["Disposition"], Kind::Text),
    ("proposal", &["Proposal"], Kind::Path),
    ("generated_skills", &["Generated skills"], Kind::Text),
    ("reused_skills", &["Reused skills"], Kind::Text),
    ("target", &["Target"], Kind::Path),
    ("target_type", &["Target type"], Kind::Enum(&["spec", "plan"])),
    ("source_check", &["Source check"], Kind::Enum(&["citations", "refetch"])),
    ("prior_findings", &["Prior findings"], Kind::Text),
    ("spec_file", &["Spec file"], Kind::Path),
    ("phase_file", &["Phase file"], Kind::Path),
    ("no_plan", &["No plan"], Kind::Text),
    ("report_file", &["Report file"], Kind::AbsolutePath),
    ("implementer_report", &["Implementer report"], Kind::Path),
    ("implementer_reports", &["Implementer reports"], Kind::Text),
    ("epa_report", &["EPA report"], Kind::Path),
    ("changed_files", &["Changed files"], Kind::Text),
    ("change_rationale", &["Change rationale"], Kind::Text),
    ("target_files", &["Target files", "Target file"], Kind::Text),
    ("research_docs", &["Research docs"], Kind::Text),
    ("projects_in_scope", &["Repos in scope"], Kind::Text),
    ("master_plan", &["Master plan"], Kind::Path),
    ("findings", &["Findings"], Kind::Text),
];

const COMMON: [&str; 4] = ["workspace_root", "repo_root", "session_dir", "repo_key"];

/// `(required, one_of)` per agent, beyond the common four.
fn contract(agent: AgentName, mode: Option<&str>) -> Result<(Vec<&'static str>, Vec<&'static str>), String> {
    let one_of_work = vec!["phase_file", "no_plan"];
    Ok(match agent {
        AgentName::Explore | AgentName::GenerateSpec => (vec!["task"], vec![]),
        AgentName::FactCheck => {
            (vec!["target", "target_type", "prior_findings", "spec_file", "source_check"], vec![])
        }
        AgentName::Plan => (vec!["spec_file"], vec![]),
        AgentName::Implementer => (vec!["report_file"], one_of_work),
        AgentName::ExecutionPathAnalyzer => (vec!["implementer_report", "report_file"], vec![]),
        AgentName::WriteTest => (vec!["implementer_report", "epa_report", "report_file"], one_of_work),
        AgentName::CodeReviewer => (vec!["phase", "changed_files", "change_rationale"], one_of_work),
        AgentName::PromptGeneration => (vec!["task", "target_files", "report_file"], vec![]),
        AgentName::ModuleDocumentation => (vec!["implementer_reports", "report_file"], vec![]),
        AgentName::QuickAnswer => (vec!["question"], vec![]),
        AgentName::Initializer => {
            let extra: Vec<&'static str> = match mode {
                Some("detect") => vec![],
                Some("adopt") => vec!["source_harness", "source_runtime_dir", "source_skills_dir"],
                Some("scout") => vec!["slice", "slice_paths", "stack_reference", "scout_plan"],
                Some("propose") => vec!["scout_findings", "scout_plan"],
                Some("generate-skill") => {
                    vec!["skill_name", "skill_kind", "disposition", "proposal", "scout_findings"]
                }
                Some("generate-inventory") => {
                    vec!["generated_skills", "reused_skills", "proposal", "scout_findings"]
                }
                Some(other) => return Err(format!("`Mode: {other}` is not an initializer mode")),
                None => return Err("missing required parameter `Mode:`".into()),
            };
            let mut req = vec!["mode"];
            req.extend(extra);
            (req, vec![])
        }
    })
}

fn label_index() -> BTreeMap<&'static str, (&'static str, Kind)> {
    let mut m = BTreeMap::new();
    for (id, labels, kind) in PARAMS {
        for l in *labels {
            m.insert(*l, (*id, *kind));
        }
    }
    m
}

fn check(id: &str, kind: Kind, value: &str) -> Result<(), String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(format!("`{id}` is empty"));
    }
    match kind {
        Kind::AbsoluteDir | Kind::AbsolutePath => {
            if !Path::new(v).is_absolute() {
                return Err(format!("`{id}` must be an absolute path, got `{v}`"));
            }
        }
        Kind::RepoKey => {
            if !is_project_key(v) {
                return Err(format!("`{id}` must be a lowercase slug, got `{v}`"));
            }
        }
        Kind::Phase => {
            let base = v.strip_suffix("-tests").unwrap_or(v);
            let ok = v == "none" || (!base.is_empty() && base.chars().all(|c| c.is_ascii_digit()));
            if !ok {
                return Err(format!("`Phase:` must be `N`, `N-tests`, or `none`, got `{v}`"));
            }
        }
        Kind::Enum(values) => {
            if !values.contains(&v) {
                return Err(format!("`{id}` must be one of {}, got `{v}`", values.join(", ")));
            }
        }
        Kind::Path | Kind::Text => {}
    }
    Ok(())
}

/// Parse and validate a `Label: value` block for `agent`. Continuation lines (indented) append to
/// the label above them. Parsing stops at a `---` rule or a `## ` heading, where the brief starts.
/// Returns the values by parameter id.
pub fn parse_block(agent: AgentName, text: &str) -> Result<BTreeMap<String, String>, String> {
    let index = label_index();
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.trim() == "---" || line.starts_with("## ") {
            break;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && current.is_some() {
            let id = current.clone().unwrap_or_default();
            let entry = values.entry(id).or_default();
            if !entry.is_empty() {
                entry.push('\n');
            }
            entry.push_str(line.trim());
            continue;
        }
        current = None;
        let Some((label, rest)) = line.split_once(':') else { continue };
        if let Some((id, _)) = index.get(label.trim()) {
            values.entry(id.to_string()).or_insert_with(|| rest.trim().to_string());
            current = Some(id.to_string());
        }
    }
    let (required, one_of) = contract(agent, values.get("mode").map(String::as_str))?;
    let kinds: BTreeMap<&str, Kind> = PARAMS.iter().map(|(id, _, k)| (*id, *k)).collect();
    for id in COMMON.iter().chain(required.iter()) {
        let label = PARAMS.iter().find(|(p, _, _)| p == id).map(|(_, l, _)| l[0]).unwrap_or(id);
        match values.get(*id) {
            None => return Err(format!("missing required parameter `{label}:`")),
            Some(v) => check(id, kinds[id], v)?,
        }
    }
    if !one_of.is_empty() {
        let present: Vec<&&str> = one_of.iter().filter(|id| values.contains_key(**id)).collect();
        match present.len() {
            1 => check(present[0], kinds[*present[0]], &values[*present[0]])?,
            0 => return Err("missing required parameter: one of `Phase file:` or `No plan:`".into()),
            _ => return Err("give exactly one of `Phase file:` or `No plan:`, not both".into()),
        }
    }
    Ok(values)
}
