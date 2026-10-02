//! Typed pipeline documents: the research document, the spec, and the plan with its phases. Agents
//! write them through the `Document` tool as JSON, and Ostra renders the markdown the next agent
//! reads, so the browser shows each part natively while the pipeline keeps reading markdown.
//!
//! Field doc comments are the schema descriptions the model sees, so they are written as
//! instructions.

mod check;
mod facts;
mod merge;
mod refs;
mod render;
mod store;

pub use check::{DocIssue, IssueLevel, check, check_submit, check_written};
pub use facts::{changed_since_research, render_code_facts};
pub use merge::{MergeError, Merged, apply_update};
pub use render::render;
pub use store::{DocKind, Written, doc_kind_for, json_path, load, load_view, phase_path, write};

use crate::pipeline::Question;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

// ---------------------------------------------------------------------------------------------
// Shared parts
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RepoRoot {
    /// The repo key, for example `backend`.
    pub key: String,
    /// Absolute root of the repo.
    pub root: String,
}

/// A page retrieved in this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Source {
    /// The page URL.
    pub url: String,
    /// The page's own version or date.
    pub version: String,
    /// The fact the page established, and the decision it settles here.
    pub established: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub enum Level {
    Low,
    Medium,
    High,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Low => "Low",
            Level::Medium => "Medium",
            Level::High => "High",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Research document (explore)
// ---------------------------------------------------------------------------------------------

/// One research document: what the codebase does for one research task, plus what any external
/// technology it depends on documents about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ResearchDoc {
    /// The research topic, for example `Order cancellation`.
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The repo key from `Repo key:`.
    pub repo: String,
    /// Areas from the repo's Module/Area Map.
    #[serde(default)]
    pub areas: Vec<String>,
    /// One or two sentences: the exact `Task:` this spawn was given, and what it does and does not
    /// cover.
    pub scope: String,
    /// The problem, in two to four sentences.
    pub problem: String,
    /// What the request asks for, one demand per entry, in the user's terms. Never requirements.
    #[serde(default)]
    pub asks: Vec<String>,
    /// Files this task touches or depends on.
    #[serde(default)]
    pub files: Vec<FileRef>,
    /// Existing patterns a change here would follow, each shown in full.
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    /// One end-to-end flow traced with real names, one hop per entry, in order.
    #[serde(default)]
    pub data_flow: Vec<FlowStep>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// External technology facts from pages retrieved in this run. Empty when the request touches
    /// nothing the repo does not already do.
    #[serde(default)]
    pub external: Vec<ExternalFinding>,
    /// Two or three approaches when a design decision is implied. Empty when investigative only.
    #[serde(default)]
    pub approaches: Vec<Approach>,
    /// The recommendation, grounded in existing patterns. Absent when investigative only.
    #[serde(default)]
    pub recommendation: Option<String>,
    /// Open questions in question-card form, numbered `Q1`, `Q2`, ...
    #[serde(default)]
    pub open_questions: Vec<Question>,
    /// One entry per page retrieved in this run, never a page you did not open.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Recalled lessons this document relies on.
    #[serde(default)]
    pub lessons: Vec<LessonUsed>,
    /// Anything the task touched that this spawn could not investigate, one item per entry.
    #[serde(default)]
    pub not_covered: Vec<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    /// Written by Ostra, never by the agent, so the schema the model sees leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    #[ts(skip)]
    pub snapshot: Option<FileSnapshot>,
}

/// Rule D2a: the repo files a research document names, as they were when it was written, so a
/// later stage can tell which of them changed since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSnapshot {
    /// Absolute root of the repo the paths are relative to.
    pub root: String,
    /// Repo-relative path to its content hash, `dir` for a directory, or `missing`.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct FileRef {
    /// Repo-relative path.
    pub path: String,
    /// What the file does for this task.
    pub purpose: String,
    /// Key public signatures or symbols in it, verbatim.
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Pattern {
    pub name: String,
    /// What the pattern is and where it applies, in markdown.
    pub description: String,
    /// Repo-relative paths that use it.
    #[serde(default)]
    pub files: Vec<String>,
    /// The pattern in full, copied from the repo.
    #[serde(default)]
    pub snippet: Option<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Snippet {
    /// Fence language, for example `rust`.
    pub language: String,
    pub code: String,
    /// `path:line` the code was copied from.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct FlowStep {
    /// What happens at this hop, in markdown.
    pub step: String,
    /// `path:Symbol` where it happens.
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum DependencyKind {
    Internal,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Dependency {
    pub name: String,
    pub kind: DependencyKind,
    /// The version the repo resolves, when it pins one.
    #[serde(default)]
    pub version: Option<String>,
    /// What this task needs from it.
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ExternalFinding {
    /// The external technology, for example `DynamoDB transactions`.
    pub technology: String,
    /// What the page states, quoted where it is a signature, limit, key, or ordering rule.
    pub fact: String,
    /// What the fact forces in this repo: the approach it settles or rules out, the limit to respect.
    pub consequence: String,
    /// The URL of the page, which must also be in `sources`.
    pub source: String,
    /// The page's own version or date.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Approach {
    pub name: String,
    pub concept: String,
    #[serde(default)]
    pub pros: Vec<String>,
    #[serde(default)]
    pub cons: Vec<String>,
    /// The codebase precedent (`path:Symbol`), or the retrieved source URL when the repo has none.
    pub precedent: String,
    pub best_for: String,
    /// True on the one approach the recommendation picks.
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct LessonUsed {
    pub area: String,
    pub lesson: String,
    /// Whether current code confirmed it, and where.
    #[serde(default)]
    pub verified: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Spec (generate-spec)
// ---------------------------------------------------------------------------------------------

/// The one specification for the request: criteria, ordered deliverables, EARS requirements with
/// acceptance criteria, contracts, and the external evidence they rest on. Ostra derives the
/// Delivery Order table, the Traceability table, and every count from these lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SpecDoc {
    /// The topic title, for example `Order cancellation`.
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Every research document path, in document order, oldest first.
    #[serde(default)]
    pub research: Vec<String>,
    #[serde(default)]
    pub repos: Vec<RepoRoot>,
    /// Two to four sentences: the outcome this spec delivers and why. No implementation.
    pub objective: String,
    /// What the system does today, grounded in real files and symbols, or `None: this is new
    /// behavior with no existing counterpart.`
    pub current_behavior: String,
    /// One entry per delivered capability.
    #[serde(default)]
    pub in_scope: Vec<ScopeItem>,
    /// One entry per excluded item (K8).
    #[serde(default)]
    pub out_of_scope: Vec<Exclusion>,
    /// The criterion ledger (Step 2A): `C1`, `C2`, ...
    #[serde(default)]
    pub criteria: Vec<Criterion>,
    /// Deliverables in delivery order: `D1`, `D2`, ...
    #[serde(default)]
    pub deliverables: Vec<Deliverable>,
    /// Requirements in one flat sequence `R1`...`R{n}`, D1's first.
    #[serde(default)]
    pub requirements: Vec<Requirement>,
    #[serde(default)]
    pub contracts_provided: Vec<ProvidedContract>,
    #[serde(default)]
    pub contracts_consumed: Vec<ConsumedContract>,
    /// The External Evidence table (Step 2B): `E1`, `E2`, ...
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// New or changed persisted data. Empty when nothing persisted changes.
    #[serde(default)]
    pub data_impact: Vec<DataChange>,
    #[serde(default)]
    pub assumptions: Vec<Assumption>,
    /// Unanswered questions in question-card form, `Q1`, `Q2`, ...
    #[serde(default)]
    pub open_questions: Vec<Question>,
    /// Any S4 size overrun, unresolved cycle, or a finding that does not apply and why.
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ScopeItem {
    pub text: String,
    /// Criterion IDs this item traces to.
    #[serde(default)]
    pub criteria: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Exclusion {
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub enum CriterionKind {
    Functional,
    Data,
    Integration,
    Constraint,
    Quality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Criterion {
    /// `C1`, `C2`, ...
    pub id: String,
    /// One atomic, verifiable demand (K1, K2).
    pub statement: String,
    pub kind: CriterionKind,
    /// The repo key that must change for it (K5).
    pub repo: String,
    /// A real `path:Symbol`, a retrieved `{URL}`, or `new: no precedent found` (K4).
    pub grounding: String,
    /// Criterion IDs it cannot be verified without (K6).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// The question ID a provisional criterion waits on, for example `Q2` (K7). Absent when
    /// confirmed.
    #[serde(default)]
    pub provisional: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Deliverable {
    /// `D1`, `D2`, ... in delivery order.
    pub id: String,
    pub title: String,
    /// The one repo key it changes (S5).
    pub repo: String,
    /// Areas from that repo's Module/Area Map.
    #[serde(default)]
    pub areas: Vec<String>,
    /// Deliverable IDs whose contracts it consumes. Empty for none.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// One sentence: what works end to end once it ships.
    pub outcome: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum EarsPattern {
    /// `THE SYSTEM SHALL {response}.`
    Ubiquitous,
    /// `WHEN {trigger} THE SYSTEM SHALL {response}.`
    EventDriven,
    /// `WHILE {state} THE SYSTEM SHALL {response}.`
    StateDriven,
    /// `IF {undesired condition} THEN THE SYSTEM SHALL {response}.`
    Unwanted,
    /// `WHERE {feature is present} THE SYSTEM SHALL {response}.`
    Optional,
    /// `WHILE {state} WHEN {trigger} THE SYSTEM SHALL {response}.`
    Complex,
}

impl EarsPattern {
    pub fn label(self) -> &'static str {
        match self {
            EarsPattern::Ubiquitous => "Ubiquitous",
            EarsPattern::EventDriven => "Event-driven",
            EarsPattern::StateDriven => "State-driven",
            EarsPattern::Unwanted => "Unwanted behavior",
            EarsPattern::Optional => "Optional feature",
            EarsPattern::Complex => "State and event",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Requirement {
    /// `R1`, `R2`, ... in one flat sequence across the spec.
    pub id: String,
    /// The deliverable ID it belongs to.
    pub deliverable: String,
    /// A short title.
    pub title: String,
    pub pattern: EarsPattern,
    /// One EARS statement with exactly one `SHALL`.
    pub statement: String,
    /// Criterion IDs it delivers (R-e).
    pub covers: Vec<String>,
    /// Evidence IDs whose Binding rule it depends on. Empty for none.
    #[serde(default)]
    pub rests_on: Vec<String>,
    /// At least one acceptance criterion.
    pub acceptance: Vec<Acceptance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Acceptance {
    /// `AC{n}.{m}`, where `R{n}` is the requirement.
    pub id: String,
    /// The initial state, with real values.
    pub given: String,
    pub when: String,
    /// An assertable outcome.
    pub then: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProvidedContract {
    pub name: String,
    /// The full observable shape, in markdown.
    pub shape: String,
    /// The deliverable ID that creates it.
    pub provided_by: String,
    /// Deliverable IDs, or `external callers`.
    #[serde(default)]
    pub consumed_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ConsumedContract {
    pub name: String,
    /// The full current shape, in markdown.
    pub shape: String,
    /// `real/path:Symbol`.
    pub source: String,
}

/// One external fact the requirements rest on, retrieved by explore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Evidence {
    /// `E1`, `E2`, ...
    pub id: String,
    /// What the page states, in its own terms. Quote signatures, keys, limits, and rules verbatim.
    pub fact: String,
    /// One imperative sentence an implementer must obey in this repo.
    pub rule: String,
    /// The URL from a research document's sources.
    pub source: String,
    /// The page's own version or date.
    pub version: String,
    /// For example `unreachable at spec time`.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DataChange {
    /// The deliverable ID the change belongs to.
    #[serde(default)]
    pub deliverable: Option<String>,
    /// Field or table, type, nullability, default, migration need, and effect on existing rows.
    pub change: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Assumption {
    pub text: String,
    /// The trusted source that supports it, or `no precedent`.
    pub source: String,
}

// ---------------------------------------------------------------------------------------------
// Plan (plan)
// ---------------------------------------------------------------------------------------------

/// The plan: the master plan plus every phase, each phase self-contained. Ostra writes the master
/// plan file and one phase file per phase from it, and derives the Deliverable Index, the Phase
/// Index, the Test Policy Rationale, the Requirement Traceability table, and the step counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PlanDoc {
    /// The topic title.
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The spec file path.
    pub spec: String,
    #[serde(default)]
    pub repos: Vec<RepoRoot>,
    pub stakes: Level,
    /// One sentence.
    pub stakes_rationale: String,
    /// One paragraph: what will be built and why.
    pub summary: String,
    /// One per acceptance criterion in the spec, plus one build criterion per repo.
    #[serde(default)]
    pub success_criteria: Vec<SuccessCriterion>,
    /// Question-card form, `Q1`, `Q2`, ... Empty is the expected value.
    #[serde(default)]
    pub clarifying_questions: Vec<Question>,
    /// The spec's deliverables, so the Deliverable Index can show their titles.
    #[serde(default)]
    pub deliverables: Vec<PlanDeliverable>,
    /// Phases numbered `1`...`{N}` in one sequence, D1's first (P0, P10).
    #[serde(default)]
    pub phases: Vec<Phase>,
    #[serde(default)]
    pub risks: Vec<Risk>,
    /// Verification strategy, one entry per line.
    #[serde(default)]
    pub verification: Vec<String>,
    /// The Step 8 mechanical pre-checks.
    #[serde(default)]
    pub pre_checks: Vec<PreCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SuccessCriterion {
    /// The acceptance criterion ID, for example `AC1.2`. Absent for a build criterion.
    #[serde(default)]
    pub id: Option<String>,
    /// A checkable condition.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PlanDeliverable {
    /// `D1`, `D2`, ...
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub enum TestPolicy {
    Required,
    Skip,
}

impl TestPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            TestPolicy::Required => "Required",
            TestPolicy::Skip => "Skip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Phase {
    /// The bare phase number, one sequence across the plan (P10).
    pub id: u32,
    pub name: String,
    /// The deliverable ID (P0).
    pub deliverable: String,
    /// The repo key it changes (P8).
    pub repo: String,
    /// Absolute root of that repo.
    pub repo_root: String,
    /// The model-routing tier (P9).
    pub complexity: Level,
    /// P12.
    pub test_policy: TestPolicy,
    /// One sentence naming the evidence: the first logic step for `Required`, what every step
    /// contains for `Skip`.
    pub test_rationale: String,
    /// Phase IDs that must complete first (P8). Empty for none.
    #[serde(default)]
    pub depends_on: Vec<u32>,
    #[serde(default)]
    pub areas: Vec<String>,
    /// One sentence for the Phase Index.
    pub description: String,
    /// Two to four sentences: what the phase accomplishes and every prior-phase artifact it needs,
    /// with full paths and signatures.
    pub context: String,
    /// The phase's Required Skills (P7).
    #[serde(default)]
    pub skills: Vec<String>,
    /// Every requirement a step delivers, with its EARS statement quoted verbatim from the spec.
    #[serde(default)]
    pub requirements: Vec<QuotedRequirement>,
    /// Every evidence row a step must obey, copied verbatim from the spec.
    #[serde(default)]
    pub constraints: Vec<Evidence>,
    pub steps: Vec<PlanStep>,
    /// This repo's build command.
    pub verification: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct QuotedRequirement {
    /// `R{n}`.
    pub id: String,
    /// The EARS statement, verbatim.
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub enum FileChange {
    Create,
    Modify,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub enum StepSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PlanStep {
    /// `{phase}.{n}`, for example `2.3`.
    pub id: String,
    /// A brief description.
    pub title: String,
    /// The exact repo-relative path (P2, P3).
    pub file: String,
    pub change: FileChange,
    /// Files and symbols to read first.
    #[serde(default)]
    pub read_first: Vec<String>,
    /// Requirement IDs this step delivers (P11).
    pub delivers: Vec<String>,
    /// Precise prose: names, types, rules, logic, side effects. No code (P4).
    pub action: String,
    /// Each evidence rule this step must obey, with its sentence copied verbatim (P13).
    #[serde(default)]
    pub binding_rules: Vec<BindingRule>,
    /// Skills from the repo's Skill Application Mapping (P6).
    #[serde(default)]
    pub skills: Vec<String>,
    /// The repo's build command (P5).
    pub verify: String,
    pub size: StepSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct BindingRule {
    /// `E{n}`.
    pub id: String,
    /// The Binding rule sentence, verbatim from the spec.
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Risk {
    pub risk: String,
    pub impact: String,
    pub likelihood: Level,
    pub mitigation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PreCheck {
    /// `Surviving callers (8A)` or `Target-module imports (8B)`.
    pub check: String,
    pub scope: String,
    pub result: String,
}

/// One phase as its own file: what the implementer reads, and what the phase artifact shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PhaseDoc {
    /// The plan's title.
    pub plan: String,
    pub date: String,
    pub spec: String,
    /// The deliverable's title, when the plan names it.
    pub deliverable_title: Option<String>,
    pub phase: Phase,
}

// ---------------------------------------------------------------------------------------------
// The browser's view
// ---------------------------------------------------------------------------------------------

/// A typed document as the browser receives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "doc", rename_all = "lowercase")]
#[ts(export)]
pub enum Document {
    Research(ResearchDoc),
    Spec(SpecDoc),
    Plan(PlanDoc),
    Phase(PhaseDoc),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentView {
    pub document: Document,
    /// What Ostra's checks found, errors first.
    pub issues: Vec<DocIssue>,
}

#[cfg(test)]
mod tests;
