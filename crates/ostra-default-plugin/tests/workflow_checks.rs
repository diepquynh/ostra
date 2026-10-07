//! Workflow checks (Rules CA5, WF8, and WB6) against the standard plugin's default workflows and
//! agents.

use ostra_core::workflow::{WorkflowDef, WorkflowFile, WorkflowSet};
use ostra_engine::workflow::{check_runnable, check_types};

fn resolve(text: &str) -> WorkflowDef {
    let file: WorkflowFile = toml::from_str(text).unwrap();
    WorkflowSet {
        files: [("w".to_string(), file)].into_iter().collect(),
        ..ostra_default_plugin::workflow_set()
    }
    .resolve("w")
    .unwrap()
}

/// Rules CA5 and WF8: stages take agents by contract, never by name.
#[test]
fn stages_check_agents_by_contract() {
    let agents = ostra_agents::AgentCatalog::builtin();
    // A standard agent with the stage's contract may be bound, whatever its name.
    let ok = resolve("extends = \"implement\"\n[agents]\nreview = \"code-reviewer\"\n");
    assert!(check_runnable(&ok, &agents, &[]).is_ok());
    // An agent bound to a contract it does not return is refused.
    let wrong = resolve("extends = \"implement\"\n[agents]\nreview = \"plan\"\n");
    let e = check_runnable(&wrong, &agents, &[]).unwrap_err();
    assert!(e.contains("returns `plan`"), "{e}");
    // A custom stage reads a verdict, so a standard agent that returns `spec` cannot run it.
    let custom =
        resolve("extends = \"research\"\n[[stage]]\nid = \"x\"\nagent = \"generate-spec\"\n");
    let e = check_runnable(&custom, &agents, &[]).unwrap_err();
    assert!(e.contains("returns `spec`"), "{e}");
}

/// Rule WB6: references are followed through each node's shape before anything runs.
#[test]
fn wb6_references_read_fields_of_the_right_kind() {
    let def = ostra_agents::catalog::parse_markdown(
        std::path::Path::new("/w/.ostra/agents/auditor.md"),
        "+++\ndescription = \"Audits.\"\n[data_schema]\ntype = \"object\"\n[data_schema.properties.risk]\ntype = \"string\"\n+++\nAudit.\n",
    )
    .unwrap();
    let agents = ostra_agents::AgentCatalog::builtin()
        .with_workspace_def(def.name, Some(def))
        .unwrap();
    let check = |stages: &str| {
        let wf = resolve(&format!(
            "extends = \"research\"\n[[stage]]\nid = \"audit\"\nagent = \"auditor\"\n{stages}"
        ));
        check_types(&wf, &agents).join(" ")
    };
    assert_eq!(
        check(
            "[[stage]]\nid = \"n\"\ntransform = \"count\"\nafter = [\"audit\"]\ninputs = { items = \"audit.findings\" }\n[[stage]]\nid = \"r\"\ntransform = \"compare\"\nafter = [\"audit\"]\ninputs = { value = \"audit.data.risk\" }\nargs = { op = \"eq\", to = \"high\" }\n"
        ),
        ""
    );
    let e = check(
        "[[stage]]\nid = \"r\"\ntransform = \"compare\"\nafter = [\"audit\"]\ninputs = { value = \"audit.data.riks\" }\nargs = { op = \"eq\", to = \"high\" }\n",
    );
    assert!(
        e.contains("`audit.data` has no field `riks`; it has `risk`"),
        "{e}"
    );
    let e = check(
        "[[stage]]\nid = \"n\"\ntransform = \"count\"\nafter = [\"audit\"]\ninputs = { items = \"audit.findings\" }\n[[stage]]\nid = \"f\"\ntransform = \"filter\"\nafter = [\"n\"]\ninputs = { items = \"n.output\" }\nargs = { op = \"truthy\" }\n",
    );
    assert!(
        e.contains("Wire a list into input `items` of node `f`: `n.output` is a number"),
        "{e}"
    );
    let e = check(
        "[[stage]]\nid = \"x\"\nagent = \"auditor\"\nafter = [\"audit\"]\nwhen = [{ ref = \"audit.findings.file\", op = \"exists\" }]\n",
    );
    assert!(e.contains("is a list, so read an item by its index"), "{e}");
    // A filter keeps the shape of its items, so a later reference into them is checked too.
    let e = check(
        "[[stage]]\nid = \"f\"\ntransform = \"filter\"\nafter = [\"audit\"]\ninputs = { items = \"audit.findings\" }\nargs = { op = \"truthy\" }\n[[stage]]\nid = \"x\"\nagent = \"auditor\"\nafter = [\"f\"]\ninputs = { first = \"f.output.0.line\" }\n",
    );
    assert!(e.contains("has no field `line`"), "{e}");
}
