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
    /// The test stage alone on existing code. Logs written before the rename say `UNIT_TEST`.
    #[serde(alias = "UNIT_TEST")]
    Test,
    /// The docs stage alone on existing code: a documentation book, no code change.
    Docs,
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
            Category::Test => "TEST",
            Category::Docs => "DOCS",
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

/// How much of the pipeline an `IMPLEMENT` request runs. The light track goes from research
/// straight to reviewed phases; the full track adds the spec, its fact-check, the plan, and the
/// approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Track {
    Light,
    Full,
}

impl Track {
    pub fn as_str(self) -> &'static str {
        match self {
            Track::Light => "light",
            Track::Full => "full",
        }
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
    Track,
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
    ImplementationReview,
    ClosingGate,
    Epa,
    WriteTest,
    TestReview,
    /// Logs and databases written before books say `module-docs`.
    #[serde(alias = "module-docs")]
    Documentation,
    Architecture,
    BookWrite,
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
            Intake | Classify | Explore | Sufficiency | Track | QuickAnswer | Detect | Scout => {
                Lane::Research
            }
            Spec | OpenQuestions | Propose | SkillApproval => Lane::Requirements,
            FactCheckSpec | FactCheckPlan | SpecApproval => Lane::Verification,
            Stakes | Plan | PlanApproval => Lane::Design,
            Implement | Autofix | Handoff | Rescue | Verify | PromptGen | GenerateSkill
            | GenerateInventory => Lane::Build,
            Review | Staging | Format | ImplementationReview => Lane::Review,
            ClosingGate | Epa | WriteTest | TestReview => Lane::Test,
            Documentation | Architecture | BookWrite => Lane::Docs,
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

impl Question {
    /// The options as the console numbers them: the recommended option first, then the rest in
    /// order. Must match `orderedOptions` in `web/src/lib/gateAnswers.ts`, because a typed answer
    /// names options by these numbers.
    pub fn display_options(&self) -> Vec<(usize, &QuestionOption)> {
        let rec = self.options.get(self.recommended).map(|_| self.recommended);
        rec.into_iter()
            .chain((0..self.options.len()).filter(|i| Some(*i) != rec))
            .enumerate()
            .map(|(n, i)| (n + 1, &self.options[i]))
            .collect()
    }

    /// Whether an answer is only option labels, as the console sends a picked option (several
    /// joined by `; ` for a multi-select question).
    pub fn is_option_answer(&self, answer: &str) -> bool {
        answer
            .split("; ")
            .all(|part| self.options.iter().any(|o| o.label == part.trim()))
    }

    /// Rule J1: a typed answer with the options it may name by number or label, so the agent
    /// reading it alone knows what "1 and 3" means. A picked option is returned as is.
    pub fn answer_in_context(&self, answer: &str) -> String {
        if self.is_option_answer(answer) {
            return answer.to_string();
        }
        let opts: Vec<String> = self
            .display_options()
            .iter()
            .map(|(n, o)| format!("{n}. {}: {}", o.label, o.description))
            .collect();
        format!(
            "{answer}\n(The user typed this answer. It may name these options by number or label, combine them, and add to them: {})",
            opts.join("; ")
        )
    }
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

#[cfg(test)]
mod question_tests {
    use super::*;

    fn q(recommended: usize) -> Question {
        Question {
            id: "Q1".into(),
            question: "Which?".into(),
            tag: "t".into(),
            options: ["A", "B", "C"]
                .iter()
                .map(|l| QuestionOption {
                    label: l.to_string(),
                    description: format!("{l} desc"),
                })
                .collect(),
            recommended,
            multi_select: false,
        }
    }

    #[test]
    fn options_are_numbered_recommended_first() {
        let labels = |q: &Question| {
            q.display_options()
                .iter()
                .map(|(n, o)| format!("{n}{}", o.label))
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(&q(0)), ["1A", "2B", "3C"]);
        assert_eq!(labels(&q(2)), ["1C", "2A", "3B"]);
        assert_eq!(labels(&q(9)), ["1A", "2B", "3C"], "an out-of-range recommendation is ignored");
    }

    #[test]
    fn a_typed_answer_carries_the_options() {
        let q = q(1);
        assert_eq!(q.answer_in_context("A"), "A");
        assert_eq!(q.answer_in_context("A; C"), "A; C");
        let typed = q.answer_in_context("1 and 3, plus an audit log");
        assert!(typed.starts_with("1 and 3, plus an audit log\n"));
        assert!(typed.contains("1. B: B desc; 2. A: A desc; 3. C: C desc"));
    }
}
