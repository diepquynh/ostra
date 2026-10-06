//! Rule B10: the docs pipeline's view of a project's track: the survey, the drafts, the rounds,
//! and the project's docs as one run.

use ostra_core::book::{DocSection, DocumentationSubmit, InventoryItem, PlannedPage};
use ostra_engine::state::{CheckState, DocsRound, DocsState, ProjectTrack};
use std::collections::BTreeMap;

pub trait DocsTrack {
    /// Rule B10: the survey, once it is done.
    fn survey_plan(&self) -> Option<&DocumentationSubmit>;

    /// Rule B10: the survey's inventory with every item a synthesis pass added; a later item
    /// with the same ID replaces an earlier one.
    fn current_inventory(&self) -> Vec<InventoryItem>;

    /// Rule B10: the engine's checks over the current drafts, by page, with the inventory's under
    /// the empty key.
    fn engine_issues(&self) -> BTreeMap<String, Vec<String>>;

    /// Rule B10: the pages the survey plans to write in this session, in plan order.
    fn planned_pages(&self) -> Vec<&PlannedPage>;

    /// Rule B10: every current draft, in plan order.
    fn drafts(
        &self,
    ) -> Vec<(
        &PlannedPage,
        &DocSection,
    )>;

    /// Rule B10: the drafts placed under their planned IDs and groups, for the engine's checks.
    fn placed_drafts(&self) -> Vec<DocSection>;

    /// Rule B10: every first draft run settled.
    fn first_drafts_settled(&self) -> bool;

    /// Rule B10: the pages a round's fact-checks cover: every draft in round 1, then the pages
    /// the previous round revised.
    fn pages_to_check(&self, round: u32) -> Vec<String>;

    /// Rule B10: the loop is over: the user accepted the book, the last synthesis pass found it
    /// done with no page to revise, or a round revised nothing that needs a check.
    fn docs_finished(&self) -> bool;

    /// The run a docs execution folds into.
    fn docs_run(&mut self, page: Option<&str>, round: u32) -> &mut DocsState;

    fn round_mut(&mut self, round: u32) -> &mut DocsRound;

    /// The project's docs as one run: a whole-part writer from an older log; else the survey;
    /// else running while any run of the pipeline runs, failed while one waits on its failure,
    /// done with the drafts when the loop is over, and abandoned when no page was written.
    fn docs_aggregate(&self) -> DocsState;
}

impl DocsTrack for ProjectTrack {
    fn survey_plan(&self) -> Option<&DocumentationSubmit> {
        match &self.survey {
            DocsState::Done(s) => Some(s),
            _ => None,
        }
    }

    fn current_inventory(&self) -> Vec<InventoryItem> {
        let mut out: Vec<InventoryItem> = self
            .survey_plan()
            .map(|s| s.inventory.clone())
            .unwrap_or_default();
        for item in &self.inventory_added {
            out.retain(|i| i.id != item.id);
            out.push(item.clone());
        }
        out
    }

    fn engine_issues(&self) -> BTreeMap<String, Vec<String>> {
        let modules = self
            .docs_scan
            .as_ref()
            .map(|s| s.modules.as_slice())
            .unwrap_or_default();
        let kept: Vec<String> = self
            .survey_plan()
            .map(|s| {
                s.pages
                    .iter()
                    .filter(|p| !p.rewrite)
                    .map(|p| p.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        super::checks::mechanical_issues(
            &self.placed_drafts(),
            &kept,
            &self.current_inventory(),
            modules,
        )
    }

    fn planned_pages(&self) -> Vec<&PlannedPage> {
        self.survey_plan()
            .map(|s| s.pages.iter().filter(|p| p.rewrite).collect())
            .unwrap_or_default()
    }

    fn drafts(
        &self,
    ) -> Vec<(
        &PlannedPage,
        &DocSection,
    )> {
        self.planned_pages()
            .into_iter()
            .filter_map(|p| {
                self.page_docs
                    .get(&p.id)
                    .and_then(|d| d.draft.as_ref())
                    .map(|d| (p, d))
            })
            .collect()
    }

    fn placed_drafts(&self) -> Vec<DocSection> {
        self.drafts()
            .into_iter()
            .map(|(p, d)| {
                let mut d = d.clone();
                d.id = p.id.clone();
                d.group = p.group.clone();
                d
            })
            .collect()
    }

    fn first_drafts_settled(&self) -> bool {
        self.planned_pages().iter().all(|p| {
            self.page_docs
                .get(&p.id)
                .is_some_and(|d| d.run.is_settled())
        })
    }

    fn pages_to_check(&self, round: u32) -> Vec<String> {
        if round <= 1 {
            return self
                .drafts()
                .into_iter()
                .map(|(p, _)| p.id.clone())
                .collect();
        }
        self.docs_rounds
            .get(round as usize - 2)
            .map(|r| {
                r.revisions
                    .iter()
                    .filter(|(_, st)| matches!(st, DocsState::Done(_)))
                    .map(|(p, _)| p.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn docs_finished(&self) -> bool {
        if self.docs_accepted {
            return true;
        }
        let Some(last) = self.docs_rounds.last() else {
            return false;
        };
        // Rule B10: a done pass finishes the loop only when no inventory item or module lacks an
        // owner; otherwise the next round runs another synthesis pass.
        if let DocsState::Done(syn) = &last.synthesis
            && syn.done
            && last.targets.is_empty()
            && !self.engine_issues().contains_key("")
        {
            return true;
        }
        let revised = last.synthesis.is_settled()
            && !last.targets.is_empty()
            && last
                .targets
                .keys()
                .all(|p| last.revisions.get(p).is_some_and(|r| r.is_settled()));
        revised
            && self
                .pages_to_check(self.docs_rounds.len() as u32 + 1)
                .is_empty()
    }

    fn docs_run(&mut self, page: Option<&str>, round: u32) -> &mut DocsState {
        match page {
            None => &mut self.docs,
            Some(p) if round == 0 => &mut self.page_docs.entry(p.to_string()).or_default().run,
            Some(p) => self
                .round_mut(round)
                .revisions
                .entry(p.to_string())
                .or_default(),
        }
    }

    fn round_mut(&mut self, round: u32) -> &mut DocsRound {
        while self.docs_rounds.len() < round as usize {
            self.docs_rounds.push(DocsRound::default());
        }
        &mut self.docs_rounds[round as usize - 1]
    }

    fn docs_aggregate(&self) -> DocsState {
        if !matches!(self.docs, DocsState::NotStarted) {
            return self.docs.clone();
        }
        let Some(survey) = self.survey_plan() else {
            return self.survey.clone();
        };
        let mut runs: Vec<&DocsState> = self.page_docs.values().map(|d| &d.run).collect();
        for r in &self.docs_rounds {
            runs.push(&r.synthesis);
            runs.extend(r.revisions.values());
        }
        if let Some(r) = runs.iter().find(|r| matches!(r, DocsState::Running(_))) {
            return (*r).clone();
        }
        for r in &self.docs_rounds {
            if let Some(CheckState::Running(id)) = r
                .checks
                .values()
                .find(|c| matches!(c, CheckState::Running(_)))
            {
                return DocsState::Running(id.clone());
            }
        }
        if let Some(f) = runs.iter().find(|r| matches!(r, DocsState::Failed { .. })) {
            return (*f).clone();
        }
        if !self.first_drafts_settled() {
            return DocsState::NotStarted;
        }
        let drafts = self.drafts();
        if drafts.is_empty() && survey.pages.iter().all(|p| p.rewrite) {
            return DocsState::Abandoned;
        }
        if !self.docs_finished() {
            return DocsState::NotStarted;
        }
        let glossary = self
            .page_docs
            .values()
            .flat_map(|d| d.glossary.iter().cloned())
            .chain(survey.glossary.iter().cloned())
            .collect();
        DocsState::Done(Box::new(super::combine_pages(
            survey,
            &drafts,
            glossary,
            self.current_inventory(),
        )))
    }
}
