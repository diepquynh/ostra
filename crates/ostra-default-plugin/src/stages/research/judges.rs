//! How the Classify and Sufficiency judges' decisions change the research stage (Rules D1, D2).

use crate::fold::*;
use crate::judge::{ClassifyOut, MAX_SUFFICIENCY_RESEARCH, SufficiencyOut};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::ids::DecisionId;
use ostra_engine::state::*;
use serde_json::Value;

/// How the Classify and Sufficiency judges' decisions change the research stage.
pub trait ResearchJudges {
    fn classify_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool);

    fn sufficiency_decided(&mut self, subject: Option<&str>, output: &Value, overriding: bool);
}

impl ResearchJudges for SessionState {
    fn classify_decided(&mut self, id: &DecisionId, output: &Value, overriding: bool) {
        if overriding && !self.can_override_classify_now() {
            return;
        }
        if let Ok(out) = serde_json::from_value::<ClassifyOut>(output.clone()) {
            self.classify = Some(id.clone());
            self.apply_classify(&out);
        } else if !overriding {
            self.classify = Some(id.clone());
            self.failed =
                Some("The Classify judge returned output that does not match its schema.".into());
        }
    }

    fn sufficiency_decided(&mut self, subject: Option<&str>, output: &Value, overriding: bool) {
        let covered: Vec<u32> = subject
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !overriding {
            self.ext.os_mut().sufficiency_rounds += 1;
        }
        for t in self
            .ext
            .os_mut()
            .explore
            .iter_mut()
            .filter(|t| covered.contains(&t.idx))
        {
            t.judged = true;
        }
        if overriding {
            self.ext
                .os_mut()
                .explore
                .retain(|t| !(t.origin == ExploreOrigin::Sufficiency && t.exec.is_none()));
        }
        if let Ok(out) = serde_json::from_value::<SufficiencyOut>(output.clone()) {
            let mut added = 0;
            for item in out.items.into_iter().filter(|i| i.needed) {
                if added == MAX_SUFFICIENCY_RESEARCH {
                    break;
                }
                let (project, task) = match item.task {
                    Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                    _ => (self.primary(), item.item.clone()),
                };
                // Rule D2: the judge often maps several items to one task; research it once.
                if self
                    .ext
                    .os()
                    .explore
                    .iter()
                    .any(|t| !t.finished() && t.project == project && t.task == task)
                {
                    continue;
                }
                self.push_explore(project, task, ExploreOrigin::Sufficiency);
                added += 1;
            }
        }
    }
}
