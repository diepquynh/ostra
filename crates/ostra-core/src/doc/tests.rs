use super::*;
use crate::AgentName;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) const RESEARCH: &str = include_str!("fixtures/research.json");
pub(crate) const SPEC: &str = include_str!("fixtures/spec.json");
pub(crate) const PLAN: &str = include_str!("fixtures/plan.json");

fn value(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

fn errors(kind: DocKind, v: &Value) -> Vec<String> {
    let doc = kind.parse(v).unwrap();
    check(&doc)
        .into_iter()
        .filter(|i| i.level == IssueLevel::Error)
        .map(|i| i.line())
        .collect()
}

#[test]
fn fixtures_parse_and_pass() {
    for (kind, text) in [
        (DocKind::Research, RESEARCH),
        (DocKind::Spec, SPEC),
        (DocKind::Plan, PLAN),
    ] {
        let v = value(text);
        let doc = kind.parse(&v).unwrap();
        let issues = check(&doc);
        assert!(issues.is_empty(), "{kind:?}: {issues:#?}");
    }
}

#[test]
fn spec_renders_the_sections_downstream_agents_read() {
    let doc = DocKind::Spec.parse(&value(SPEC)).unwrap();
    let md = render(&doc, Path::new("/s/ostra-spec-1.md"));
    for heading in [
        "# Specification: Order cancellation",
        "## Delivery Order",
        "### D1: Cancellation endpoint",
        "#### R3 Publish the cancellation",
        "## Contracts Provided",
        "## External Evidence",
        "## Traceability",
    ] {
        assert!(md.contains(heading), "missing {heading}\n{md}");
    }
    assert!(md.contains("**Criteria covered:** 4 of 4"), "{md}");
    assert!(
        md.contains(
            "| D1 | Cancellation endpoint | backend | orders, events | none | R1–R3 | C1, C2, C3 |"
        ),
        "{md}"
    );
    assert!(md.contains("| C4 | D2 | R4 | AC4.1, AC4.2 |"), "{md}");
    assert!(md.contains("- **AC2.1** GIVEN order `o-1`"), "{md}");
}

#[test]
fn plan_writes_master_and_phase_files() {
    let dir = tempfile_dir();
    let md = dir.join("ostra-plan-20260728-141530-order-cancellation.md");
    let w = write(DocKind::Plan, &md, &value(PLAN)).unwrap();
    assert_eq!(w.errors(), 0);
    let phase1 = phase_path(&md, 1);
    assert_eq!(
        w.files,
        vec![md.clone(), phase1.clone(), phase_path(&md, 2)]
    );
    let master = std::fs::read_to_string(&md).unwrap();
    assert!(
        master.contains(&format!(
            "| 1 | Cancel transition | D1 | backend | Medium | Required | none | `{}` | 2 |",
            phase1.display()
        )),
        "{master}"
    );
    assert!(
        master.contains("| R3 | D1 | phase 1 step 1.1 | AC3.1 |"),
        "{master}"
    );
    let p1 = std::fs::read_to_string(&phase1).unwrap();
    assert!(p1.contains("**Complexity:** Medium"), "{p1}");
    assert!(
        p1.contains(
            "- **Binding rules**: E1: Set the order id as the message group of `order.cancelled`."
        ),
        "{p1}"
    );
    let view = load_view(&phase1).unwrap();
    assert!(
        matches!(view.document, Document::Phase(ref p) if p.phase.id == 1 && p.deliverable_title.as_deref() == Some("Cancellation endpoint"))
    );

    // A revision that removes phase 2 also removes its files.
    let merged = apply_update(load(&md), &json!({}), &["2".into()]).unwrap();
    write(DocKind::Plan, &md, &merged).unwrap();
    assert!(!phase_path(&md, 2).exists());
    assert!(!json_path(&phase_path(&md, 2)).exists());
    assert!(phase1.exists());
}

#[test]
fn broken_references_are_errors() {
    let mut v = value(SPEC);
    v["requirements"][3]["covers"] = json!(["C9"]);
    v["requirements"][2]["rests_on"] = json!(["E7"]);
    v["requirements"][1]["acceptance"][0]["id"] = json!("AC3.1");
    let errs = errors(DocKind::Spec, &v).join("\n");
    assert!(errs.contains("(C4): Cover this criterion"), "{errs}");
    assert!(errs.contains("`covers` names `C9`"), "{errs}");
    assert!(errs.contains("`rests_on` names `E7`"), "{errs}");
    assert!(
        errs.contains("(AC3.1): Give each acceptance criterion its own id"),
        "{errs}"
    );

    let mut v = value(SPEC);
    v["deliverables"][0]["depends_on"] = json!(["D2"]);
    assert!(
        errors(DocKind::Spec, &v)
            .join("\n")
            .contains("cycle D1 -> D2 -> D1")
    );

    let mut v = value(PLAN);
    v["phases"][0]["steps"][0]["binding_rules"][0]["rule"] = json!("Use a group.");
    v["phases"][1]["id"] = json!(3);
    let errs = errors(DocKind::Plan, &v).join("\n");
    assert!(errs.contains("verbatim"), "{errs}");
    assert!(errs.contains("expected phase 2 here (P10)"), "{errs}");
}

#[test]
fn schema_errors_name_the_field() {
    let mut v = value(SPEC);
    v["requirements"][0]["acceptance"][0]
        .as_object_mut()
        .unwrap()
        .remove("then");
    let err = DocKind::Spec.parse(&v).unwrap_err();
    assert!(
        err.starts_with("requirements[0].acceptance[0]: missing field `then`"),
        "{err}"
    );
    let mut v = value(RESEARCH);
    v["surprise"] = json!(1);
    assert!(
        DocKind::Research
            .parse(&v)
            .unwrap_err()
            .contains("unknown field `surprise`")
    );
}

#[test]
fn submit_needs_a_clean_document_that_matches() {
    let dir = tempfile_dir();
    let md = dir.join("ostra-spec-1.md");
    let submit = json!({"spec_path": md, "open_questions": [], "external_evidence_rows": 1, "deliverables": 2, "requirements": 4, "summary": "s"});
    let err = check_submit(AgentName::GenerateSpec, &submit).unwrap_err();
    assert!(
        err.contains("Write the spec with the Document tool"),
        "{err}"
    );
    write(DocKind::Spec, &md, &value(SPEC)).unwrap();
    check_submit(AgentName::GenerateSpec, &submit).unwrap();
    let mut wrong = submit.clone();
    wrong["requirements"] = json!(5);
    assert!(
        check_submit(AgentName::GenerateSpec, &wrong)
            .unwrap_err()
            .contains("`requirements` is 5, but the spec has 4")
    );
    assert!(check_submit(AgentName::Implementer, &json!({})).is_ok());
}

#[test]
fn plan_submit_must_match_the_phases() {
    let dir = tempfile_dir();
    let md = dir.join("ostra-plan-1.md");
    write(DocKind::Plan, &md, &value(PLAN)).unwrap();
    let phase = |id: u32, deliverable: &str, project: &str, complexity: &str, deps: Vec<u32>| json!({"id": id, "deliverable": deliverable, "project": project, "title": "t", "complexity": complexity, "test_policy": "Required", "depends_on": deps, "file": phase_path(&md, id)});
    let mut submit = json!({"spec_path": "/s", "master_plan_path": md, "phases": [phase(1, "D1", "backend", "Medium", vec![]), phase(2, "D2", "web", "Low", vec![1])], "stakes": "Medium", "summary": "s", "step_count": 3, "requirement_coverage": "4 of 4"});
    check_submit(AgentName::Plan, &submit).unwrap();
    submit["phases"][1]["complexity"] = json!("High");
    let err = check_submit(AgentName::Plan, &submit).unwrap_err();
    assert!(
        err.contains("Phase 2 in `phases` disagrees with the plan")
            && err.contains("complexity Low"),
        "{err}"
    );
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ostra-doc-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
