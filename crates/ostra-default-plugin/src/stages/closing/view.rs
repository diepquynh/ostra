//! The closing stage on the board: format, the closing gate, each phase's tests, and the label of
//! a running test run.

use crate::data::{EpaState, PhaseRun, ProjectTrack};
use crate::view::{Artifacts, advised_loop, advising_detail, card, exec_ids, loop_status};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::{CommandPurpose, ExecPurpose as P};
use ostra_core::pipeline::{Lane, StageKind};
use ostra_engine::state::SessionState;

/// The format and closing-gate cards of project `key`.
pub(crate) fn cards(key: &str, t: &ProjectTrack, out: &mut Vec<StageCard>) {
    if t.format.is_none()
        && let Some((CommandPurpose::Format, cmd)) = &t.running
    {
        let mut c = card(
            StageKind::Format,
            format!("Format {key}"),
            StageStatus::Running,
        );
        c.project = Some(key.to_string());
        c.detail = Some(cmd.clone());
        out.push(c);
    }
    if let Some(f) = t.format {
        let mut c = card(
            StageKind::Format,
            format!("Format {key}"),
            StageStatus::Done,
        );
        c.project = Some(key.to_string());
        c.detail = Some(match f {
            None => "no format command".into(),
            Some(code) => format!("exit {code}"),
        });
        out.push(c);
    }
    if t.closing_gate.is_some() || t.closing.is_some() {
        let mut c = card(
            StageKind::ClosingGate,
            format!("Tests and docs for {key}"),
            if t.closing.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Waiting
            },
        );
        c.project = Some(key.to_string());
        c.gate = t.closing_gate.clone();
        c.detail = t.closing.map(|(a, b)| {
            format!(
                "tests {}, docs {}",
                if a { "yes" } else { "no" },
                if b { "yes" } else { "no" }
            )
        });
        out.push(c);
    }
}

/// The test card of phase `p`, once its path analysis or test loop started.
pub(crate) fn test_card(s: &SessionState, p: &PhaseRun, out: &mut Vec<StageCard>) {
    if matches!(p.epa, EpaState::NotStarted) && p.test_loop.is_idle() {
        return;
    }
    let id = p.info.id;
    let mut c = card(
        StageKind::WriteTest,
        format!("Tests for phase {id}"),
        loop_status(&p.test_loop),
    );
    if matches!(p.epa, EpaState::Running(_)) {
        c.status = StageStatus::Running;
        c.stage = StageKind::Epa;
        c.lane = Lane::Test;
    }
    c.project = Some(p.info.project.clone());
    c.phase = Some(id);
    c.executions = exec_ids(s, |e| {
        e.loop_key == Some((id, true))
            || matches!(e.purpose, P::Epa { phase } if phase == id)
            || advised_loop(s, &e.id) == Some((id, true))
    });
    c.detail = advising_detail(&p.test_loop);
    out.push(c);
}

/// The path analysis and test reports of phase `p`.
pub(crate) fn phase_artifacts(p: &PhaseRun, a: &mut Artifacts) {
    if let EpaState::Done(path) = &p.epa {
        a.add(
            path.clone(),
            "report",
            format!("EPA report, phase {}", p.info.id),
            Some(p.info.project.clone()),
        );
    }
    if let Some(r) = &p.test_loop.report {
        a.add(
            r.clone(),
            "report",
            format!("Test report, phase {}", p.info.id),
            Some(p.info.project.clone()),
        );
    }
}

/// The label of a running test-writing or test-review run of a phase.
pub(crate) fn running_label(purpose: &P) -> Option<String> {
    match purpose {
        P::Review {
            phase,
            tests: true,
            iteration,
        } => Some(format!(
            "Reviewing tests of phase {phase}, pass {iteration}"
        )),
        P::WriteTest { phase, .. } => Some(format!("Writing tests for phase {phase}")),
        _ => None,
    }
}
