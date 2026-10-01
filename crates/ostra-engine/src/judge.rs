//! Judge calls: the only places a model makes an orchestration decision (HANDOVER 8.3). Each returns
//! JSON against a schema; the engine stores it as a `DecisionMade` event with its reason.

use ostra_core::event::JudgeKind;
use ostra_core::pipeline::Category;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExploreTaskSpec {
    pub project: String,
    pub task: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OptsIn {
    #[serde(default)]
    pub tests: bool,
    #[serde(default)]
    pub docs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifyOut {
    pub category: Category,
    pub projects: Vec<String>,
    #[serde(default)]
    pub explore_tasks: Vec<ExploreTaskSpec>,
    #[serde(default)]
    pub opts_in: OptsIn,
    #[serde(default)]
    pub reason: String,
    /// A 2 to 5 word label for the session. Older decisions have none.
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SufficiencyItem {
    pub item: String,
    pub needed: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub task: Option<ExploreTaskSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SufficiencyOut {
    #[serde(default)]
    pub items: Vec<SufficiencyItem>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakesOut {
    pub stakes: ostra_core::pipeline::Stakes,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackOut {
    pub track: ostra_core::pipeline::Track,
    #[serde(default)]
    pub reason: String,
}

/// One project a feedback round changes, with the instruction its revision phase gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackTarget {
    pub project: String,
    pub instruction: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackOut {
    pub route: AnswerRoute,
    #[serde(default)]
    pub targets: Vec<FeedbackTarget>,
    /// Rule J1: what happens to the feedback. Older decisions have none, which delivers it.
    #[serde(default)]
    pub items: Vec<AnswerItem>,
    #[serde(default)]
    pub research: Vec<ExploreTaskSpec>,
    #[serde(default)]
    pub forget: Vec<String>,
    #[serde(default)]
    pub reason: String,
}

/// Rule J1: where one answer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// The current stage acts on it now.
    #[default]
    Deliver,
    /// Kept for later stages only.
    Remember,
    /// Dropped. The gate's answer stays in the log.
    Discard,
}

/// A later stage a remembered note reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteStage {
    Implement,
    Tests,
    Docs,
}

/// The judge's decision for one answer: an open question's ID, or `answer` for the gate's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerItem {
    pub id: String,
    #[serde(default)]
    pub disposition: Disposition,
    /// Later stages that also receive `note`. A delivered answer may name some too.
    #[serde(default)]
    pub stages: Vec<NoteStage>,
    #[serde(default)]
    pub note: String,
}

/// The ID a single-text answer's item carries.
pub const ANSWER_ITEM: &str = "answer";
/// Research tasks one answer may queue (Rule J1, a fan-out cap).
pub const MAX_ANSWER_RESEARCH: usize = 3;

/// Rule D2: research tasks one Sufficiency round may add, because each is another full pass.
pub const MAX_SUFFICIENCY_RESEARCH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerRoute {
    RequirementChange,
    ImplementationDetail,
    StageChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAnswerOut {
    pub route: AnswerRoute,
    /// Older decisions have none: every answer is delivered.
    #[serde(default)]
    pub items: Vec<AnswerItem>,
    /// Research that runs before the delivered answers are applied.
    #[serde(default)]
    pub research: Vec<ExploreTaskSpec>,
    /// IDs of notes kept earlier that the user takes back or replaces.
    #[serde(default)]
    pub forget: Vec<String>,
    /// Rule U1: numbers of the unfinished research tasks the user tells Ostra to skip.
    #[serde(default)]
    pub skip: Vec<u32>,
    #[serde(default)]
    pub reason: String,
}

impl RouteAnswerOut {
    pub fn item(&self, id: &str) -> AnswerItem {
        item_for(&self.items, id)
    }
}

/// Every item the judge gave for `id`. A model may split one answer into parts, one item each,
/// and on a gate with one text every item is about that text, whatever ID it carries.
pub fn parts_for<'a>(items: &'a [AnswerItem], id: &str) -> Vec<&'a AnswerItem> {
    items
        .iter()
        .filter(|i| id == ANSWER_ITEM || i.id == id)
        .collect()
}

/// The judge's decision for `id`, its parts merged: delivered when any part is, because the user
/// gave it, then remembered when any part is. The note and stages are the first part's that has
/// stages. An answer the judge did not name is delivered.
pub fn item_for(items: &[AnswerItem], id: &str) -> AnswerItem {
    let parts = parts_for(items, id);
    let has = |d: Disposition| parts.iter().any(|p| p.disposition == d);
    let disposition = if parts.is_empty() || has(Disposition::Deliver) {
        Disposition::Deliver
    } else if has(Disposition::Remember) {
        Disposition::Remember
    } else {
        Disposition::Discard
    };
    let first = parts.iter().find(|p| !p.stages.is_empty());
    AnswerItem {
        id: id.into(),
        disposition,
        stages: first.map(|p| p.stages.clone()).unwrap_or_default(),
        note: first.map(|p| p.note.clone()).unwrap_or_default(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RescueAction {
    Explore,
    Rerun,
    /// Rule O7: the failure is in the agent's environment, so the advisor looks first.
    Advise,
    Gate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescueOut {
    pub action: RescueAction,
    #[serde(default)]
    pub explore_task: Option<ExploreTaskSpec>,
    #[serde(default)]
    pub fact: Option<String>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveAction {
    Fix,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingInstruction {
    pub finding: String,
    pub instruction: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveReviewOut {
    pub action: ResolveAction,
    #[serde(default)]
    pub instructions: Vec<FindingInstruction>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YoloAnswerOut {
    pub answer: Value,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionOut {
    pub report_markdown: String,
    #[serde(default)]
    pub reason: String,
}

/// Longest session title kept, in characters.
pub const TITLE_CHARS: usize = 60;

/// A judge-written session title on one line, without wrapping quotes or a final period, cut to
/// [`TITLE_CHARS`]. `None` when nothing is left.
pub fn clean_title(raw: &str) -> Option<String> {
    let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = line
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`'))
        .trim_end_matches('.')
        .trim();
    let cut: String = line.chars().take(TITLE_CHARS).collect();
    let cut = cut.trim();
    (!cut.is_empty()).then(|| cut.to_string())
}

/// Extract the `reason` every judge output carries.
pub fn reason_of(output: &Value) -> String {
    output
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or_default()
        .to_string()
}

pub fn prompt_name(kind: JudgeKind) -> &'static str {
    kind.as_str()
}

fn answer_items_schema() -> Value {
    json!({"type": "array", "description": "One item per answer: each open question's ID, or `answer` for a single text.", "items": {
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "disposition": {"type": "string", "enum": ["deliver", "remember", "discard"]},
            "note": {"type": "string", "description": "The part later stages receive, self-contained. Empty when stages is empty."},
            "stages": {"type": "array", "items": {"type": "string", "enum": ["implement", "tests", "docs"]}, "description": "Later stages that also receive the note."}
        },
        "required": ["id", "disposition", "note", "stages"]
    }})
}

fn research_schema() -> Value {
    json!({"type": "array", "maxItems": MAX_ANSWER_RESEARCH, "items": explore_task_schema(), "description": "Research to run before the answer is applied. Empty unless the answer asks for research or needs facts no research document has."})
}

fn forget_schema() -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": "IDs (N1, N2, ...) of notes already kept for later stages that this answer takes back or replaces. Empty otherwise."})
}

fn explore_task_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "description": "Project key."},
            "task": {"type": "string", "description": "Self-contained research task."}
        },
        "required": ["project", "task"]
    })
}

/// Output schema for each judge's forced `decide` call. `answer_schema` is the gate-specific answer
/// shape for the YOLO judge.
pub fn output_schema(kind: JudgeKind, answer_schema: Option<Value>) -> Value {
    let reason =
        json!({"type": "string", "description": "One or two sentences the user will read."});
    match kind {
        JudgeKind::Classify => json!({
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": ["RESEARCH","SPEC","PLAN","IMPLEMENT","VERIFY","TEST","DOCS","PROMPT","QUICK_CHANGE","QUICK_ANSWER"]},
                "projects": {"type": "array", "items": {"type": "string"}},
                "explore_tasks": {"type": "array", "items": explore_task_schema()},
                "opts_in": {"type": "object", "properties": {"tests": {"type": "boolean"}, "docs": {"type": "boolean"}}, "required": ["tests", "docs"]},
                "reason": reason,
                "title": {"type": "string", "description": "A 2 to 5 word label for the session, in sentence case, with no final period."}
            },
            "required": ["category", "projects", "explore_tasks", "opts_in", "reason", "title"]
        }),
        JudgeKind::Sufficiency => json!({
            "type": "object",
            "properties": {
                "items": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "item": {"type": "string"},
                        "needed": {"type": "boolean"},
                        "reason": {"type": "string"},
                        "task": {"anyOf": [explore_task_schema(), {"type": "null"}]}
                    },
                    "required": ["item", "needed", "reason"]
                }},
                "reason": reason
            },
            "required": ["items", "reason"]
        }),
        JudgeKind::Stakes => json!({
            "type": "object",
            "properties": {"stakes": {"type": "string", "enum": ["low","medium","high"]}, "reason": reason},
            "required": ["stakes", "reason"]
        }),
        JudgeKind::Track => json!({
            "type": "object",
            "properties": {"track": {"type": "string", "enum": ["light","full"]}, "reason": reason},
            "required": ["track", "reason"]
        }),
        JudgeKind::Feedback => json!({
            "type": "object",
            "properties": {
                "route": {"type": "string", "enum": ["requirement_change","implementation_detail"], "description": "Always required, also when the feedback is remembered or discarded."},
                "targets": {"type": "array", "minItems": 1, "items": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "Project key in scope."},
                        "instruction": {"type": "string", "description": "What this project's revision must change, self-contained."}
                    },
                    "required": ["project", "instruction"]
                }},
                "items": answer_items_schema(),
                "research": research_schema(),
                "forget": forget_schema(),
                "reason": reason
            },
            "required": ["route", "targets", "items", "research", "forget", "reason"]
        }),
        JudgeKind::RouteAnswer => json!({
            "type": "object",
            "properties": {
                "route": {"type": "string", "enum": ["requirement_change","implementation_detail","stage_choice"], "description": "Always required, also when every item is remembered or discarded."},
                "items": answer_items_schema(),
                "research": research_schema(),
                "forget": forget_schema(),
                "skip": {"type": "array", "items": {"type": "integer"}, "description": "Numbers of the unfinished research tasks the user tells Ostra to skip. Empty otherwise."},
                "reason": reason
            },
            "required": ["route", "items", "research", "forget", "skip", "reason"]
        }),
        JudgeKind::Rescue => json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["explore","rerun","advise","gate"]},
                "explore_task": {"anyOf": [explore_task_schema(), {"type": "null"}]},
                "fact": {"type": ["string", "null"]},
                "reason": reason
            },
            "required": ["action", "reason"]
        }),
        JudgeKind::ResolveReview => json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["fix","block"]},
                "instructions": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"finding": {"type": "string"}, "instruction": {"type": "string"}},
                    "required": ["finding", "instruction"]
                }},
                "reason": reason
            },
            "required": ["action", "instructions", "reason"]
        }),
        JudgeKind::YoloAnswer => json!({
            "type": "object",
            "properties": {"answer": answer_schema.unwrap_or(json!({"type": "object"})), "reason": reason},
            "required": ["answer", "reason"]
        }),
        JudgeKind::Completion => json!({
            "type": "object",
            "properties": {"report_markdown": {"type": "string"}, "reason": reason},
            "required": ["report_markdown", "reason"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_parses() {
        let v = json!({"category":"IMPLEMENT","projects":["api"],"explore_tasks":[{"project":"api","task":"t"}],"opts_in":{"tests":true,"docs":false},"reason":"r"});
        let c: ClassifyOut = serde_json::from_value(v).unwrap();
        assert_eq!(c.category, Category::Implement);
        assert!(c.opts_in.tests);
        assert_eq!(c.title, "");
        let v = json!({"category":"RESEARCH","projects":[],"opts_in":{"tests":false,"docs":false},"reason":"r","title":"Order cancellation flow"});
        assert_eq!(
            serde_json::from_value::<ClassifyOut>(v).unwrap().title,
            "Order cancellation flow"
        );
        assert!(
            output_schema(JudgeKind::Classify, None)["required"]
                .as_array()
                .unwrap()
                .contains(&json!("title"))
        );
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(
            clean_title("  \"Order   cancellation flow.\" ").as_deref(),
            Some("Order cancellation flow")
        );
        assert_eq!(clean_title(" \n "), None);
        let long = clean_title(&"word ".repeat(40)).unwrap();
        assert!(long.chars().count() <= TITLE_CHARS);
    }
}
