use crate::model::Complexity;
use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

/// Request category from `orchestrate/prompt.md` Step 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum Category {
    Research,
    Spec,
    Plan,
    Implement,
    Verify,
    UnitTest,
    Prompt,
    /// A small edit the request fully describes: one implementer pass on the native executor.
    QuickChange,
    QuickAnswer,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Research => "RESEARCH",
            Category::Spec => "SPEC",
            Category::Plan => "PLAN",
            Category::Implement => "IMPLEMENT",
            Category::Verify => "VERIFY",
            Category::UnitTest => "UNIT_TEST",
            Category::Prompt => "PROMPT",
            Category::QuickChange => "QUICK_CHANGE",
            Category::QuickAnswer => "QUICK_ANSWER",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Stakes {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "PascalCase")]
#[ts(export)]
pub enum TestPolicy {
    /// Any doubt resolves to covering the phase (Rule T4).
    #[default]
    Required,
    Skip,
}

/// Session board lanes, in SDLC order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Lane {
    Research,
    Requirements,
    Verification,
    Design,
    Build,
    Review,
    Test,
    Docs,
    Done,
}

impl Lane {
    pub const ALL: [Lane; 9] = [
        Lane::Research,
        Lane::Requirements,
        Lane::Verification,
        Lane::Design,
        Lane::Build,
        Lane::Review,
        Lane::Test,
        Lane::Docs,
        Lane::Done,
    ];
}

/// One node kind of the pipeline. Stage instances carry a project key, a phase, and a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum StageKind {
    Intake,
    Classify,
    Explore,
    Sufficiency,
    Spec,
    OpenQuestions,
    FactCheckSpec,
    SpecApproval,
    Stakes,
    Plan,
    FactCheckPlan,
    PlanApproval,
    Implement,
    Review,
    Autofix,
    Staging,
    Handoff,
    Rescue,
    Format,
    ClosingGate,
    Epa,
    WriteTest,
    TestReview,
    ModuleDocs,
    Verify,
    PromptGen,
    QuickAnswer,
    Completion,
    // Init flow.
    Detect,
    Scout,
    Propose,
    SkillApproval,
    GenerateSkill,
    GenerateInventory,
}

impl StageKind {
    pub fn lane(self) -> Lane {
        use StageKind::*;
        match self {
            Intake | Classify | Explore | Sufficiency | QuickAnswer | Detect | Scout => {
                Lane::Research
            }
            Spec | OpenQuestions | Propose | SkillApproval => Lane::Requirements,
            FactCheckSpec | FactCheckPlan | SpecApproval => Lane::Verification,
            Stakes | Plan | PlanApproval => Lane::Design,
            Implement | Autofix | Handoff | Rescue | Verify | PromptGen | GenerateSkill
            | GenerateInventory => Lane::Build,
            Review | Staging | Format => Lane::Review,
            ClosingGate | Epa | WriteTest | TestReview => Lane::Test,
            ModuleDocs => Lane::Docs,
            Completion => Lane::Done,
        }
    }
}

/// A phase from the plan's Phase Index (or a synthetic inline phase when plan was skipped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PhaseInfo {
    pub id: u32,
    pub deliverable: Option<String>,
    pub project: String,
    pub title: String,
    pub complexity: Complexity,
    pub test_policy: TestPolicy,
    /// Phase IDs this phase depends on. `None` means unreadable, which counts as depending on
    /// every earlier phase (Rule M5).
    pub depends_on: Option<Vec<u32>>,
    /// Absent for inline no-plan work.
    #[ts(type = "string | null")]
    pub file: Option<std::path::PathBuf>,
    /// Plan's one-sentence reason for `Test policy: Skip`.
    pub test_rationale: Option<String>,
}

/// An open question in AskUserQuestion shape. The recommended option is listed first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct Question {
    /// `Q1`, `Q2`, ...
    pub id: String,
    pub question: String,
    /// 12 characters or fewer.
    pub tag: String,
    pub options: Vec<QuestionOption>,
    /// Index into `options` of the recommended option.
    #[serde(default)]
    pub recommended: usize,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// A user's (or the YOLO judge's) answer to one question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuestionAnswer {
    pub id: String,
    pub question: String,
    /// Chosen option labels, or free text for "Other".
    pub answer: String,
}
