//! The judges' shared answers to the engine's `Pipeline` hooks: which outputs parse, which can be
//! overridden, and the completion report when its judge fails.

use crate::judge;
use crate::judge_input;
use ostra_core::event::JudgeKind;
use ostra_engine::state::SessionState;
use serde_json::Value;

pub(crate) fn output_ok(kind: JudgeKind, v: &Value) -> bool {
    match kind {
        JudgeKind::Classify => serde_json::from_value::<judge::ClassifyOut>(v.clone()).is_ok(),
        JudgeKind::Sufficiency => {
            serde_json::from_value::<judge::SufficiencyOut>(v.clone()).is_ok()
        }
        JudgeKind::Stakes => serde_json::from_value::<judge::StakesOut>(v.clone()).is_ok(),
        JudgeKind::Track => serde_json::from_value::<judge::TrackOut>(v.clone()).is_ok(),
        JudgeKind::Feedback => serde_json::from_value::<judge::FeedbackOut>(v.clone())
            .is_ok_and(|o| !o.targets.is_empty()),
        JudgeKind::RouteAnswer => {
            serde_json::from_value::<judge::RouteAnswerOut>(v.clone()).is_ok()
        }
        JudgeKind::Rescue => serde_json::from_value::<judge::RescueOut>(v.clone()).is_ok(),
        JudgeKind::ResolveReview => {
            serde_json::from_value::<judge::ResolveReviewOut>(v.clone()).is_ok()
        }
        JudgeKind::Completion => serde_json::from_value::<judge::CompletionOut>(v.clone()).is_ok(),
        JudgeKind::YoloAnswer => true,
    }
}

pub(crate) fn override_ok(kind: JudgeKind, output: &Value) -> bool {
    match kind {
        JudgeKind::Classify => serde_json::from_value::<judge::ClassifyOut>(output.clone()).is_ok(),
        JudgeKind::Stakes => serde_json::from_value::<judge::StakesOut>(output.clone()).is_ok(),
        JudgeKind::Track => serde_json::from_value::<judge::TrackOut>(output.clone()).is_ok(),
        JudgeKind::Sufficiency => {
            serde_json::from_value::<judge::SufficiencyOut>(output.clone()).is_ok()
        }
        _ => false,
    }
}

pub(crate) fn fallback(s: &SessionState, kind: JudgeKind, error: &str) -> Option<Value> {
    if kind != JudgeKind::Completion {
        return None;
    }
    // The session's work is done; a report without prose beats no report.
    let md = format!(
        "# Session complete\n\nThe completion judge failed ({error}), so this report lists the state only.\n\n{}",
        judge_input::judge_input(s, JudgeKind::Completion, None, &[]).0
    );
    Some(serde_json::json!({"report_markdown": md, "reason": "The completion judge failed."}))
}
