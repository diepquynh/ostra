//! The graph over Ostra's own source tree, checked against facts its layout guarantees
//! (CLAUDE.md, "Crates and the direction of dependencies") and a few known call sites.

use ostra_code::graph::Direction;
use ostra_code::{Indexes, ProjectIndex};
use ostra_core::api::FileIndex;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn with_ostra<R>(f: impl FnOnce(&mut ProjectIndex) -> R) -> R {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = ostra_core::paths::canonical(&root).unwrap();
    let out = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .expect("git lists the tree");
    let mut paths: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|p| root.join(p).is_file())
        .map(String::from)
        .collect();
    paths.sort();
    paths.dedup();
    let list = Arc::new(FileIndex {
        paths,
        truncated: false,
    });
    Indexes::default().with(&(), &root, &list, f)
}

const ENGINE_MUST_NOT_USE: &[&str] = &[
    "crates/ostra-server",
    "crates/ostra-exec-native",
    "crates/ostra-exec-harness",
    "crates/ostra-providers",
    "crates/ostra-tools",
    "crates/ostra-code",
];

#[test]
fn crate_layering_holds() {
    with_ostra(|ix| {
        let m = ix.units();
        let unit = |p: &str| m.units.iter().find(|u| u.path == p).unwrap().clone();
        assert_eq!(unit("crates/ostra-core").depends_on, vec![]);
        let engine: Vec<String> = unit("crates/ostra-engine")
            .depends_on
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert!(engine.contains(&"crates/ostra-core".to_string()));
        for bad in ENGINE_MUST_NOT_USE {
            assert!(!engine.iter().any(|d| d == bad), "engine uses {bad}");
        }
        let server: Vec<String> = unit("crates/ostra-server")
            .depends_on
            .into_iter()
            .map(|d| d.path)
            .collect();
        for dep in [
            "crates/ostra-engine",
            "crates/ostra-code",
            "crates/ostra-core",
        ] {
            assert!(server.iter().any(|d| d == dep), "server lacks {dep}");
        }
        assert_eq!(m.cycles, Vec::<Vec<String>>::new());
    });
}

#[test]
fn planner_links() {
    with_ostra(|ix| {
        let n = ix
            .neighbors("crates/ostra-engine/src/plan.rs", 100)
            .unwrap();
        assert_eq!(n.unit, "crates/ostra-engine");
        let runner = n
            .used_by
            .iter()
            .find(|l| l.path == "crates/ostra-engine/src/runner/driver.rs")
            .expect("the runner uses the planner");
        for name in ["PlanCtx", "Step", "next_steps"] {
            assert!(
                runner.names.iter().any(|x| x == name),
                "runner lacks {name}"
            );
        }
        assert!(
            n.uses
                .iter()
                .any(|l| l.path == "crates/ostra-engine/src/state.rs")
        );
        assert!(
            n.uses
                .iter()
                .any(|l| l.path.starts_with("crates/ostra-core/"))
        );

        let r = ix.reach(
            &["crates/ostra-engine/src/plan.rs".to_string()],
            Direction::Dependents,
            1,
            500,
        );
        assert!(
            r.reached
                .iter()
                .any(|x| x.path == "tests/conformance/main.rs")
        );
        let hops = ix
            .path_between(
                "tests/conformance/main.rs",
                "crates/ostra-engine/src/plan.rs",
            )
            .unwrap();
        assert_eq!(hops.len(), 2);
    });
}

#[test]
fn engine_never_reaches_executors() {
    with_ostra(|ix| {
        let r = ix.reach(
            &["crates/ostra-engine/src/runner/driver.rs".to_string()],
            Direction::Dependencies,
            10,
            5_000,
        );
        for x in &r.reached {
            assert!(
                !ENGINE_MUST_NOT_USE.iter().any(|b| x.path.starts_with(b)),
                "runner reaches {} via {}",
                x.path,
                x.via
            );
        }
    });
}

#[test]
fn call_sites() {
    with_ostra(|ix| {
        let c = ix.callers("next_steps", Some("crates/ostra-engine/src/plan.rs"), 100);
        assert_eq!(c.definitions.len(), 1);
        assert!(
            c.callers.iter().any(|x| {
                x.path == "crates/ostra-engine/src/runner/driver.rs" && x.name.is_some()
            })
        );
        assert!(
            c.callers
                .iter()
                .any(|x| x.path == "tests/conformance/main.rs")
        );

        let fold = ix
            .callees("crates/ostra-engine/src/state.rs", "fold", None, 100)
            .unwrap();
        assert!(fold.end_line.is_some());
        assert!(
            fold.uses
                .iter()
                .any(|l| l.name == "apply" && l.path == "crates/ostra-engine/src/state/apply.rs")
        );
    });
}

#[test]
fn std_method_names_do_not_link() {
    with_ostra(|ix| {
        // index.rs calls `.entry()`, `.insert()`, `.remove()` on std maps; none of them may link
        // it to a project file that happens to define a method by that name.
        let n = ix.neighbors("crates/ostra-code/src/index.rs", 100).unwrap();
        for l in &n.uses {
            for name in [
                "entry", "insert", "remove", "push", "len", "clone", "is_empty",
            ] {
                assert!(
                    !l.names.iter().any(|x| x == name),
                    "{} links through {name}",
                    l.path
                );
            }
        }
    });
}

#[test]
fn typescript_barrel_and_types() {
    with_ostra(|ix| {
        // A barrel passes names on; its importers depend on the files that define them.
        let hubs = ix.hubs(20);
        assert!(!hubs.iter().any(|h| h.path == "web/src/api/types.ts"));
        // `Button` comes through the `@ostra/design` package's barrel, design/src/index.ts.
        let n = ix
            .neighbors("design/src/components/core/Button.tsx", 200)
            .unwrap();
        assert!(
            n.used_by
                .iter()
                .any(|l| l.path == "web/src/shell/QuickDock.tsx")
        );
        let n = ix.neighbors("web/src/api/index.ts", 200).unwrap();
        assert!(n.used_by.len() > 10);
        // `CodeFile` comes through the `export type * from` barrel in api/types.ts.
        let n = ix
            .neighbors("web/src/screens/project/code/SourceView.tsx", 100)
            .unwrap();
        let file = n
            .uses
            .iter()
            .find(|l| l.path == "web/src/api/gen/CodeFile.ts")
            .expect("SourceView uses the generated CodeFile");
        assert_eq!(file.names, vec!["CodeFile".to_string()]);
        let c = ix.callers("Button", Some("design/src/components/core/Button.tsx"), 500);
        assert!(c.callers.iter().filter(|x| x.name.is_some()).count() > 10);
    });
}

#[test]
fn tools_answer_in_text() {
    use ostra_code::tools::run;
    use serde_json::json;
    with_ostra(|ix| {
        let mut t = |tool: &str, input: serde_json::Value| run(ix, tool, &input);
        let out = t("CodeFind", json!({"query": "next_steps"})).unwrap();
        assert!(out.contains("crates/ostra-engine/src/plan.rs:"), "{out}");
        let out = t("CodeCallers", json!({"symbol": "next_steps"})).unwrap();
        assert!(out.contains("several definitions: pass `path`"), "{out}");
        let out = t(
            "CodeCallers",
            json!({"symbol": "next_steps", "path": "crates/ostra-engine/src/plan.rs"}),
        )
        .unwrap();
        assert!(
            out.contains("defined at:\n  crates/ostra-engine/src/plan.rs:"),
            "{out}"
        );
        assert!(
            out.contains("crates/ostra-engine/src/runner/driver.rs  "),
            "{out}"
        );
        let out = t(
            "CodeOutline",
            json!({"path": "crates/ostra-engine/src/plan.rs"}),
        )
        .unwrap();
        assert!(out.contains("fn next_steps"), "{out}");
        assert!(out.contains("-> crates/ostra-engine/src/state.rs"), "{out}");
        let out = t(
            "CodeCallees",
            json!({"path": "crates/ostra-engine/src/state.rs", "symbol": "fold"}),
        )
        .unwrap();
        assert!(out.contains("method apply"), "{out}");
        let out = t(
            "CodeNeighbors",
            json!({"path": "crates/ostra-engine/src/plan.rs"}),
        )
        .unwrap();
        assert!(out.contains("used by ("), "{out}");
        let out = t(
            "CodeImpact",
            json!({"paths": ["crates/ostra-engine/src/plan.rs"], "depth": 1}),
        )
        .unwrap();
        assert!(out.contains("hop 1:\n"), "{out}");
        assert!(out.contains("tests/conformance/main.rs"), "{out}");
        let out = t(
            "CodeImplementations",
            json!({"symbol": "Executor", "path": "crates/ostra-core/src/exec.rs"}),
        )
        .unwrap();
        assert!(out.contains("implemented or extended by ("), "{out}");
        assert!(out.contains("NativeExecutor"), "{out}");
        let out = t("CodeMap", json!({"prefix": "crates/ostra-engine"})).unwrap();
        assert!(out.contains("crates/ostra-engine  "), "{out}");
        let err = t("CodeFind", json!({"q": "x"})).unwrap_err();
        assert!(err.starts_with("Invalid input"), "{err}");
    });
}

#[test]
fn graph_views_for_the_ui() {
    use ostra_core::code::CodeGraph;
    let closed = |g: &CodeGraph| {
        for e in &g.edges {
            assert!(g.nodes.iter().any(|n| n.id == e.from), "{} missing", e.from);
            assert!(g.nodes.iter().any(|n| n.id == e.to), "{} missing", e.to);
        }
    };
    let col = |g: &CodeGraph, id: &str| {
        g.nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("{id} not in the view"))
            .column
    };
    with_ostra(|ix| {
        let p = ix.packages_view();
        closed(&p);
        assert!(col(&p, "package:crates/ostra-server") < col(&p, "package:crates/ostra-engine"));
        assert!(col(&p, "package:crates/ostra-engine") < col(&p, "package:crates/ostra-core"));
        assert!(
            p.edges
                .iter()
                .any(|e| e.from == "package:crates/ostra-engine"
                    && e.to == "package:crates/ostra-core"
                    && e.weight > 10)
        );

        let v = ix.package_view("crates/ostra-engine").unwrap();
        closed(&v);
        assert!(
            v.nodes
                .iter()
                .any(|n| n.id == "crates/ostra-engine/src/plan.rs")
        );
        assert!(v.nodes.iter().any(|n| n.id == "package:crates/ostra-core"));
        assert!(
            !v.nodes
                .iter()
                .any(|n| n.id.starts_with("crates/ostra-core/"))
        );

        let f = ix.file_view("crates/ostra-engine/src/plan.rs", 1).unwrap();
        closed(&f);
        assert_eq!(col(&f, "crates/ostra-engine/src/plan.rs"), 0);
        assert_eq!(col(&f, "crates/ostra-engine/src/runner/driver.rs"), -1);
        assert_eq!(col(&f, "crates/ostra-engine/src/state.rs"), 1);
        assert!(
            f.edges
                .iter()
                .any(|e| e.from == "crates/ostra-engine/src/runner/driver.rs"
                    && e.to == "crates/ostra-engine/src/plan.rs"
                    && e.names.iter().any(|n| n == "DecideStage"))
        );

        let s = ix
            .symbol_view("crates/ostra-engine/src/plan.rs", "next_steps", None, 1)
            .unwrap();
        closed(&s);
        assert_eq!(s.view, ostra_core::code::CodeGraphView::Symbol);
        let focus = s.nodes.iter().find(|n| n.id == s.focus).unwrap();
        assert_eq!(focus.column, 0);
        assert_eq!(focus.symbol.as_ref().unwrap().name, "next_steps");
        let caller = s
            .nodes
            .iter()
            .find(|n| {
                n.symbol
                    .as_ref()
                    .is_some_and(|x| x.path == "crates/ostra-engine/src/runner/driver.rs")
            })
            .expect("a runner function calls next_steps");
        assert_eq!(caller.column, -1);
        assert!(
            s.edges
                .iter()
                .any(|e| e.from == caller.id && e.to == s.focus)
        );

        let fold = ix
            .symbol_view("crates/ostra-engine/src/state.rs", "fold", None, 1)
            .unwrap();
        closed(&fold);
        let apply = fold
            .nodes
            .iter()
            .find(|n| n.symbol.as_ref().is_some_and(|x| x.name == "apply"))
            .expect("fold calls apply");
        assert_eq!(apply.column, 1);
        assert!(
            fold.edges
                .iter()
                .any(|e| e.from == fold.focus && e.to == apply.id)
        );

        assert!(
            ix.symbol_view("crates/ostra-engine/src/plan.rs", "nope", None, 1)
                .is_none()
        );
        assert!(ix.file_view("nope.rs", 1).is_none());
        assert!(ix.package_view("nope").is_none());
    });
}
