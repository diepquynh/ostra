//! Payloads of the `submit_<agent>` tools. Each agent ends its run by calling its submit tool, so
//! the engine reads structured data instead of scraping a final message (Hard rule 4).

use crate::agent::AgentName;
use crate::pipeline::Question;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a leaf execution ended, from the agent's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SubmitStatus {
    #[default]
    Ok,
    /// Hit the retry ceiling on the same failure. Carries a diagnostic and a question.
    Stuck,
    /// Needs a specialist (prompt-generation) before it can continue.
    Handoff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct StuckInfo {
    /// The exact failing command output or error, verbatim.
    pub diagnostic: String,
    /// The specific fact or decision needed to continue.
    pub need: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct HandoffInfo {
    /// Always `prompt-generation` today.
    pub specialist: String,
    /// What the specialist must author.
    pub request: String,
    /// Instruction files the specialist should create or edit.
    #[serde(default)]
    pub target_files: Vec<String>,
    /// How to continue once the specialist is done.
    pub resume_instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ExploreSubmit {
    /// Absolute path of the research document.
    pub research_path: String,
    /// One sentence: the task answered and what was deliberately left out.
    pub scope_covered: String,
    /// Three to five sentences.
    pub findings_summary: String,
    pub sources_retrieved: u32,
    pub open_questions: u32,
    /// Items touched on but not investigated. Empty when none.
    #[serde(default)]
    pub not_covered: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct GenerateSpecSubmit {
    /// Absolute path of the one spec file.
    pub spec_path: String,
    /// Open questions still unanswered, in AskUserQuestion shape.
    #[serde(default)]
    pub open_questions: Vec<Question>,
    /// Rows in the External Evidence table. Zero when it says `None`.
    pub external_evidence_rows: u32,
    pub deliverables: u32,
    pub requirements: u32,
    /// Two or three sentences.
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export)]
pub enum Verdict {
    Pass,
    Fail,
    Error,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "UPPERCASE")]
#[ts(export)]
pub enum Severity {
    Blocker,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Blocker => "BLOCKER",
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct FactCheckFinding {
    pub severity: Severity,
    pub location: String,
    /// The ID of the element the claim sits in: `R3`, `AC3.2`, `E2`, `D1`, `C4`, `phase 2`, or
    /// `step 2.3`. Ostra shows the finding on that element.
    #[serde(default)]
    pub element: Option<String>,
    pub claim: String,
    pub issue: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct FactCheckSubmit {
    /// `PASS` only when no finding is HIGH or MEDIUM.
    pub verdict: Verdict,
    /// `spec` or `plan`.
    pub target: String,
    #[serde(default)]
    pub findings: Vec<FactCheckFinding>,
}

impl FactCheckSubmit {
    /// Render findings verbatim for the next pass's `Prior findings:` line (Rule D3a).
    pub fn findings_text(&self) -> String {
        if self.findings.is_empty() {
            return "none".into();
        }
        self.findings
            .iter()
            .map(|f| {
                format!(
                    "{}, {}: {} {}",
                    f.severity.as_str(),
                    f.location,
                    f.claim,
                    f.issue
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct PlanPhaseSubmit {
    pub id: u32,
    pub deliverable: String,
    /// Project key.
    pub project: String,
    pub title: String,
    /// `Low`, `Medium`, or `High`.
    pub complexity: String,
    /// `Required` or `Skip`.
    pub test_policy: String,
    /// The plan's one-sentence reason when `Skip`.
    #[serde(default)]
    pub test_rationale: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<u32>,
    /// Absolute path of the phase file.
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct PlanSubmit {
    pub spec_path: String,
    pub master_plan_path: String,
    pub phases: Vec<PlanPhaseSubmit>,
    /// `Low`, `Medium`, or `High`.
    pub stakes: String,
    pub summary: String,
    pub step_count: u32,
    /// `{M} of {M}`.
    pub requirement_coverage: String,
    #[serde(default)]
    pub clarifying_questions: Vec<Question>,
    /// Rule O2: keys of projects the plan puts phases in that do not exist yet. The implementer of
    /// the first such phase creates each one.
    #[serde(default)]
    pub new_projects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ImplementerSubmit {
    pub status: SubmitStatus,
    /// Absolute path of the change report (the declared `Report file:`).
    pub report_path: String,
    /// Repo-relative paths of every file created, modified, or deleted.
    #[serde(default)]
    pub changed_files: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub stuck: Option<StuckInfo>,
    #[serde(default)]
    pub handoff: Option<HandoffInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ReviewFinding {
    pub severity: Severity,
    /// Path relative to the repo root.
    pub file: String,
    /// `SEC-BLOCK-*`, `PHASE-REQ-*`, or a Review Rule Set ID.
    pub rule: String,
    /// What is wrong.
    pub description: String,
    /// Exact replacement. Auto-fixable rules use ``Change `old` to `new` on line N.`` or
    /// ``Add `text` above line N: `anchor`.``
    pub fix: String,
    /// BLOCKER only: risk, failure scenario, and what to research. Never a ready-made fix.
    #[serde(default)]
    pub guidance: Option<String>,
}

impl ReviewFinding {
    /// The one-line format Ultracode's ledger and fix prompts use.
    pub fn line(&self) -> String {
        let mut s = format!(
            "[{}] {} ({}) - {} Fix: {}",
            self.severity.as_str(),
            self.file,
            self.rule,
            self.description.trim_end(),
            self.fix.trim_end()
        );
        if let Some(g) = self
            .guidance
            .as_ref()
            .filter(|_| self.severity == Severity::Blocker)
        {
            s.push_str(" Guidance: ");
            s.push_str(g.trim_end());
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct CodeReviewerSubmit {
    /// BLOCKER findings first.
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
    /// True when any BLOCKER finding is present.
    pub security_block: bool,
    /// Absolute path of this loop's review ledger.
    pub ledger_path: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ReportSubmit {
    pub status: SubmitStatus,
    /// Absolute path of the report (the declared `Report file:`).
    pub report_path: String,
    #[serde(default)]
    pub changed_files: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub stuck: Option<StuckInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct QuickAnswerSubmit {
    /// Markdown answer.
    pub answer: String,
    /// Files and URLs the answer is based on.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// Initializer output, per mode. Free-form JSON keyed by mode, validated loosely because modes
/// differ; `summary` and `files` are always present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct InitializerSubmit {
    pub status: SubmitStatus,
    pub summary: String,
    /// Absolute paths of files written.
    #[serde(default)]
    pub files: Vec<String>,
    /// Mode-specific result (detect: slices; scout: findings path; propose: proposal path and
    /// skills; generate-skill: skill path; generate-inventory: inventory and profile paths).
    #[serde(default)]
    #[ts(type = "unknown")]
    #[schemars(extend("type" = "object"))]
    pub result: serde_json::Value,
    #[serde(default)]
    pub stuck: Option<StuckInfo>,
}

/// What the advisor tells the engine to do with a failed step (Rule O5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AdviceAction {
    /// Run the step again with `guidance`.
    Retry,
    /// Ask the user, because the fix needs a decision or a fact no agent has.
    Escalate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct AdvisorSubmit {
    pub action: AdviceAction,
    /// For `retry`: what the step's next run must do differently, as instructions to that agent.
    #[serde(default)]
    pub guidance: String,
    /// Why the step failed and why this action, for the user.
    pub reason: String,
}

/// JSON schema for an agent's submit tool input.
pub fn submit_schema(agent: AgentName) -> serde_json::Value {
    let schema = match agent {
        AgentName::Explore => schemars::schema_for!(ExploreSubmit),
        AgentName::GenerateSpec => schemars::schema_for!(GenerateSpecSubmit),
        AgentName::FactCheck => schemars::schema_for!(FactCheckSubmit),
        AgentName::Plan => schemars::schema_for!(PlanSubmit),
        AgentName::Implementer => schemars::schema_for!(ImplementerSubmit),
        AgentName::CodeReviewer => schemars::schema_for!(CodeReviewerSubmit),
        AgentName::ExecutionPathAnalyzer
        | AgentName::WriteTest
        | AgentName::ModuleDocumentation
        | AgentName::PromptGeneration => schemars::schema_for!(ReportSubmit),
        AgentName::Initializer => schemars::schema_for!(InitializerSubmit),
        AgentName::QuickAnswer => schemars::schema_for!(QuickAnswerSubmit),
        AgentName::Advisor => schemars::schema_for!(AdvisorSubmit),
    };
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// Description shown to the model for its submit tool.
pub fn submit_description(agent: AgentName) -> String {
    format!(
        "Return your result to Ostra. Call this exactly once, as your last action, after every file you \
         were asked to write exists. The engine reads only this call, so any result not in it is lost. \
         Agent: {agent}."
    )
}

/// Validate a submit payload for an agent. Returns the parsed value or a message for the model.
pub fn validate_submit(agent: AgentName, input: &serde_json::Value) -> Result<(), String> {
    fn check<T: serde::de::DeserializeOwned>(v: &serde_json::Value) -> Result<(), String> {
        serde_json::from_value::<T>(v.clone())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    match agent {
        AgentName::Explore => check::<ExploreSubmit>(input),
        AgentName::GenerateSpec => check::<GenerateSpecSubmit>(input),
        AgentName::FactCheck => check::<FactCheckSubmit>(input),
        AgentName::Plan => check::<PlanSubmit>(input),
        AgentName::Implementer => check::<ImplementerSubmit>(input),
        AgentName::CodeReviewer => {
            let r: CodeReviewerSubmit =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            let has_blocker = r.findings.iter().any(|f| f.severity == Severity::Blocker);
            if has_blocker != r.security_block {
                return Err(
                    "security_block must be true exactly when a BLOCKER finding is present".into(),
                );
            }
            Ok(())
        }
        AgentName::ExecutionPathAnalyzer
        | AgentName::WriteTest
        | AgentName::ModuleDocumentation
        | AgentName::PromptGeneration => check::<ReportSubmit>(input),
        AgentName::Initializer => check::<InitializerSubmit>(input),
        AgentName::QuickAnswer => check::<QuickAnswerSubmit>(input),
        AgentName::Advisor => {
            let a: AdvisorSubmit =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            if a.action == AdviceAction::Retry && a.guidance.trim().is_empty() {
                return Err(
                    "give `guidance` for a retry: what the step's next run must do differently"
                        .into(),
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_exist() {
        for a in AgentName::ALL {
            let s = submit_schema(a);
            assert!(s.get("properties").is_some(), "{a}");
        }
    }

    #[test]
    fn initializer_result_is_an_object() {
        let s = submit_schema(AgentName::Initializer);
        assert_eq!(s["properties"]["result"]["type"], "object", "{s}");
    }

    #[test]
    fn reviewer_block_consistency() {
        let v = serde_json::json!({
            "findings": [{"severity":"BLOCKER","file":"a.js","rule":"SEC-BLOCK-EXFIL","description":"x","fix":"remove"}],
            "security_block": false, "ledger_path": "/l", "summary": "s"
        });
        assert!(validate_submit(AgentName::CodeReviewer, &v).is_err());
    }
}
