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
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RescueAction {
    Explore,
    Rerun,
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

/// Extract the `reason` every judge output carries.
pub fn reason_of(output: &Value) -> String {
    output.get("reason").and_then(|r| r.as_str()).unwrap_or_default().to_string()
}

pub fn prompt_name(kind: JudgeKind) -> &'static str {
    kind.as_str()
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
    let reason = json!({"type": "string", "description": "One or two sentences the user will read."});
    match kind {
        JudgeKind::Classify => json!({
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": ["RESEARCH","SPEC","PLAN","IMPLEMENT","VERIFY","UNIT_TEST","PROMPT","QUICK_ANSWER"]},
                "projects": {"type": "array", "items": {"type": "string"}},
                "explore_tasks": {"type": "array", "items": explore_task_schema()},
                "opts_in": {"type": "object", "properties": {"tests": {"type": "boolean"}, "docs": {"type": "boolean"}}, "required": ["tests", "docs"]},
                "reason": reason
            },
            "required": ["category", "projects", "explore_tasks", "opts_in", "reason"]
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
        JudgeKind::RouteAnswer => json!({
            "type": "object",
            "properties": {"route": {"type": "string", "enum": ["requirement_change","implementation_detail","stage_choice"]}, "reason": reason},
            "required": ["route", "reason"]
        }),
        JudgeKind::Rescue => json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["explore","rerun","gate"]},
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
    }
}
