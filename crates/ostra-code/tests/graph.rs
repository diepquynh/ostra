use ostra_code::graph::{Direction, Hop, Link};
use ostra_code::{Indexes, ProjectIndex};
use ostra_core::api::FileIndex;
use ostra_core::code::SymbolKind;
use pretty_assertions::assert_eq;
use std::sync::Arc;

const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    ("crates/core/Cargo.toml", "[package]\nname = \"my-core\"\n"),
    (
        "crates/core/src/lib.rs",
        "pub mod api;\npub mod other;\npub use api::Thing;\n",
    ),
    (
        "crates/core/src/api.rs",
        "pub struct Thing;\nimpl Thing {\n    pub fn go() {}\n    pub fn new() -> Self { Thing }\n}\n",
    ),
    (
        "crates/core/src/other.rs",
        "pub struct Other;\nimpl Other {\n    pub fn new() -> Self { Other }\n}\n",
    ),
    ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
    (
        "crates/app/src/main.rs",
        "use my_core::api::Thing;\nmod util;\nfn main() {\n    Thing::go();\n    util::run();\n}\n",
    ),
    (
        "crates/app/src/util.rs",
        "use super::Thing;\npub fn run() {\n    let t = Thing::new();\n}\npub struct A;\nimpl A { fn new() {} }\n",
    ),
    (
        "crates/app/src/extra.rs",
        "pub struct B;\nimpl B { fn new() {} }\nfn f(v: Vec<u8>) { v.push(1); v.go(); let id = 1; }\n",
    ),
    (
        "crates/app/src/fields.rs",
        "pub struct C {\n    pub id: u32,\n}\npub fn push() {}\n",
    ),
    (
        "crates/app/tests/it.rs",
        "use my_core::api::Thing;\n#[test]\nfn t() { Thing::go(); }\n",
    ),
    ("web/package.json", "{}"),
    (
        "web/src/a.ts",
        "import { c } from \"./c\";\nexport function a() { return c(); }\n",
    ),
    (
        "web/src/c.ts",
        "import { a } from \"./a\";\nexport function c() { return 1; }\nexport function d() { return a(); }\n",
    ),
    (
        "web/src/api/gen/Item.ts",
        "export type Item = { label: string };\n",
    ),
    (
        "web/src/api/types.ts",
        "export type * from \"./gen/Item\";\n",
    ),
    (
        "web/src/ui/Button.tsx",
        "export function Button({ label }: { label: string }) {\n  return <button>{label}</button>;\n}\n",
    ),
    (
        "web/src/ui/index.ts",
        "export { Button } from \"./Button\";\n",
    ),
    (
        "web/src/cost.ts",
        "export function label(n: number) {\n  return `${n}`;\n}\n",
    ),
    (
        "web/src/List.tsx",
        "import type { Item } from \"./api/types\";\nimport { Button } from \"./ui\";\nexport const List = ({ items }: { items: Item[] }) => (\n  <ul>\n    {items.map((i) => (\n      <Button label={i.label} />\n    ))}\n  </ul>\n);\nexport function Empty() {\n  return <Button label=\"none\" />;\n}\n",
    ),
    ("go.mod", "module example.com/app\n\ngo 1.22\n"),
    ("server/server.go", "package server\n\nfunc Start() {}\n"),
    ("server/other.go", "package server\n\nfunc Unused() {}\n"),
    (
        "cmd/main.go",
        "package main\n\nimport \"example.com/app/server\"\n\nfunc main() { server.Start() }\n",
    ),
];

fn with<R>(f: impl FnOnce(&mut ProjectIndex) -> R) -> R {
    with_files(FILES, f)
}

fn with_files<R>(files: &[(&str, &str)], f: impl FnOnce(&mut ProjectIndex) -> R) -> R {
    let dir = tempfile::tempdir().unwrap();
    for (p, text) in files {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut paths: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
    paths.sort();
    let list = Arc::new(FileIndex {
        paths,
        truncated: false,
    });
    Indexes::default().with(&"p", dir.path(), &list, f)
}

fn paths(links: &[Link]) -> Vec<(&str, Vec<&str>)> {
    links
        .iter()
        .map(|l| {
            (
                l.path.as_str(),
                l.names.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

#[test]
fn neighbors_join_imports_and_names() {
    with(|ix| {
        let n = ix.neighbors("crates/app/src/main.rs", 50).unwrap();
        assert_eq!(n.unit, "crates/app");
        assert_eq!(
            paths(&n.uses),
            vec![
                ("crates/core/src/api.rs", vec!["Thing", "go"]),
                ("crates/app/src/util.rs", vec!["run"]),
            ]
        );
        assert!(n.uses[1].module);
        assert_eq!(n.uses[0].import_lines, vec![1]);

        let util = ix.neighbors("crates/app/src/util.rs", 50).unwrap();
        // `Thing::new` resolves through its qualifier although util.rs defines a `new` too.
        assert_eq!(
            paths(&util.uses),
            vec![
                ("crates/core/src/api.rs", vec!["Thing", "new"]),
                ("crates/app/src/main.rs", vec![]),
            ]
        );

        let api = ix.neighbors("crates/core/src/api.rs", 50).unwrap();
        let mut users: Vec<&str> = api.used_by.iter().map(|l| l.path.as_str()).collect();
        users.sort();
        assert_eq!(
            users,
            vec![
                "crates/app/src/main.rs",
                "crates/app/src/util.rs",
                "crates/app/tests/it.rs",
                "crates/core/src/lib.rs",
            ]
        );
    });
}

#[test]
fn members_and_bare_locals_do_not_link_alone() {
    with(|ix| {
        // `v.push` could be any `push`, `v.go` names a method of a file extra.rs never imports,
        // and a local `id` is not the field `C::id`.
        let n = ix.neighbors("crates/app/src/extra.rs", 50).unwrap();
        assert_eq!(paths(&n.uses), vec![]);
    });
}

#[test]
fn react_components_through_barrels() {
    with(|ix| {
        let n = ix.neighbors("web/src/List.tsx", 50).unwrap();
        // `Item` passes through a star re-export, `Button` through an index module, and the
        // `label` prop never means the `label` function in cost.ts.
        assert_eq!(
            paths(&n.uses),
            vec![
                ("web/src/api/gen/Item.ts", vec!["Item"]),
                ("web/src/ui/Button.tsx", vec!["Button"]),
                ("web/src/api/types.ts", vec![]),
                ("web/src/ui/index.ts", vec![]),
            ]
        );
        let c = ix.callers("Button", Some("web/src/ui/Button.tsx"), 50);
        let got: Vec<(&str, Option<&str>, Vec<u32>)> = c
            .callers
            .iter()
            .map(|c| (c.path.as_str(), c.name.as_deref(), c.lines.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("web/src/List.tsx", None, vec![2]),
                ("web/src/List.tsx", Some("List"), vec![6]),
                ("web/src/List.tsx", Some("Empty"), vec![11]),
                ("web/src/ui/index.ts", None, vec![1]),
            ]
        );
        // Dependents of Item stop at the barrel: its importers reach Item by name.
        let r = ix.reach(
            &["web/src/api/gen/Item.ts".into()],
            Direction::Dependents,
            3,
            50,
        );
        let got: Vec<&str> = r.reached.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(got, vec!["web/src/List.tsx", "web/src/api/types.ts"]);
    });
}

#[test]
fn go_package_import_narrows_to_used_files() {
    with(|ix| {
        let n = ix.neighbors("cmd/main.go", 50).unwrap();
        assert_eq!(paths(&n.uses), vec![("server/server.go", vec!["Start"])]);
        assert_eq!(n.uses[0].import_lines, vec![3]);
    });
}

#[test]
fn dependents_skip_module_declarations() {
    with(|ix| {
        let r = ix.reach(
            &["crates/core/src/api.rs".to_string()],
            Direction::Dependents,
            5,
            100,
        );
        let got: Vec<(&str, u32, &str)> = r
            .reached
            .iter()
            .map(|x| (x.path.as_str(), x.distance, x.via.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("crates/app/src/main.rs", 1, "crates/core/src/api.rs"),
                ("crates/app/src/util.rs", 1, "crates/core/src/api.rs"),
                ("crates/app/tests/it.rs", 1, "crates/core/src/api.rs"),
                ("crates/core/src/lib.rs", 1, "crates/core/src/api.rs"),
            ]
        );

        let missing = ix.reach(&["nope.rs".to_string()], Direction::Dependents, 1, 10);
        assert_eq!(missing.missing, vec!["nope.rs".to_string()]);
    });
}

#[test]
fn path_between_follows_names() {
    with(|ix| {
        assert_eq!(
            ix.path_between("cmd/main.go", "server/server.go").unwrap(),
            vec![
                Hop {
                    path: "cmd/main.go".into(),
                    import_line: Some(3),
                    names: vec!["Start".into()],
                },
                Hop {
                    path: "server/server.go".into(),
                    import_line: None,
                    names: vec![],
                },
            ]
        );
        assert_eq!(ix.path_between("server/server.go", "cmd/main.go"), None);
    });
}

#[test]
fn cycles_and_units() {
    with(|ix| {
        assert_eq!(
            ix.cycles(10),
            vec![vec!["web/src/a.ts".to_string(), "web/src/c.ts".to_string()]]
        );
        let m = ix.units();
        let app = m.units.iter().find(|u| u.path == "crates/app").unwrap();
        assert_eq!(app.files, 5);
        assert_eq!(app.tests, 1);
        assert_eq!(
            app.depends_on
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            vec!["crates/core"]
        );
        let core = m.units.iter().find(|u| u.path == "crates/core").unwrap();
        assert_eq!(core.used_by, vec!["crates/app".to_string()]);
        assert!(m.cycles.is_empty());
    });
}

#[test]
fn hubs_count_dependents() {
    with(|ix| {
        let h = ix.hubs(1);
        assert_eq!(h[0].path, "crates/core/src/api.rs");
        assert_eq!(h[0].dependents, 4);
    });
}

#[test]
fn callers_group_by_enclosing_definition() {
    with(|ix| {
        let c = ix.callers("go", Some("crates/core/src/api.rs"), 50);
        assert_eq!(c.definitions.len(), 1);
        let got: Vec<(&str, Option<&str>, Vec<u32>)> = c
            .callers
            .iter()
            .map(|c| (c.path.as_str(), c.name.as_deref(), c.lines.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("crates/app/src/main.rs", Some("main"), vec![4]),
                ("crates/app/tests/it.rs", Some("t"), vec![3]),
            ]
        );
    });
}

#[test]
fn callees_resolve_body_names() {
    with(|ix| {
        let c = ix
            .callees("crates/app/src/main.rs", "main", None, 50)
            .unwrap();
        assert_eq!((c.line, c.end_line), (3, Some(6)));
        let got: Vec<(&str, &str, Option<SymbolKind>)> = c
            .uses
            .iter()
            .map(|l| (l.name.as_str(), l.path.as_str(), l.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Thing", "crates/core/src/api.rs", Some(SymbolKind::Class)),
                ("go", "crates/core/src/api.rs", Some(SymbolKind::Method)),
                ("run", "crates/app/src/util.rs", Some(SymbolKind::Function)),
            ]
        );
    });
}

#[test]
fn related_ranks_direct_links_first() {
    with(|ix| {
        let r = ix.related("crates/app/src/util.rs", 10).unwrap();
        let direct: Vec<&str> = r
            .iter()
            .filter(|x| x.distance == 1)
            .map(|x| x.path.as_str())
            .collect();
        assert_eq!(
            direct,
            vec!["crates/app/src/main.rs", "crates/core/src/api.rs"]
        );
        assert!(
            r.iter()
                .any(|x| x.path == "crates/app/tests/it.rs" && x.distance == 2)
        );
    });
}

const SHOP: &[(&str, &str)] = &[
    ("package.json", "{}"),
    (
        "src/stock.ts",
        "export function stockLevel(sku: string): number {\n  return 0;\n}\nexport function reserveStock(sku: string, qty: number): boolean {\n  return stockLevel(sku) >= qty;\n}\nexport function releaseStock(sku: string, qty: number): void {\n  stockLevel(sku);\n}\n",
    ),
    (
        "src/orders.ts",
        "import { reserveStock, releaseStock } from \"./stock\";\nexport interface Order { id: string; sku: string; qty: number; }\nexport function placeOrder(o: Order): boolean {\n  return reserveStock(o.sku, o.qty);\n}\nexport function cancelOrder(o: Order): void {\n  releaseStock(o.sku, o.qty);\n}\nexport function reorder(o: Order): boolean {\n  cancelOrder(o);\n  return reserveStock(o.sku, o.qty * 2);\n}\n",
    ),
    (
        "src/cart.ts",
        "import { stockLevel } from \"./stock\";\nexport function canAdd(sku: string, qty: number): boolean {\n  return stockLevel(sku) >= qty;\n}\n",
    ),
    (
        "src/api/routes.ts",
        "import { placeOrder, cancelOrder, type Order } from \"../orders\";\nexport function handleOrder(body: Order): { ok: boolean } {\n  return { ok: placeOrder(body) };\n}\nexport function handleCancel(body: Order): { ok: boolean } {\n  cancelOrder(body);\n  return { ok: true };\n}\n",
    ),
    ("src/api/index.ts", "export * from \"./routes\";\n"),
    (
        "src/server.ts",
        "import { handleOrder, handleCancel } from \"./api\";\nexport function route(path: string, body: any) {\n  if (path === \"/order\") return handleOrder(body);\n  return handleCancel(body);\n}\n",
    ),
];

#[test]
fn symbol_impact_follows_the_call_chain_not_the_file() {
    with_files(SHOP, |ix| {
        let r = ix.symbol_impact("releaseStock", "src/stock.ts", 5, 100);
        let hops: Vec<Vec<(&str, Option<&str>, &str)>> = r
            .hops
            .iter()
            .map(|h| {
                h.iter()
                    .map(|u| (u.path.as_str(), u.name.as_deref(), u.uses.as_str()))
                    .collect()
            })
            .collect();
        // cart.ts uses stock.ts too, but only `stockLevel`; placeOrder and handleOrder never
        // reach releaseStock.
        assert_eq!(
            hops,
            vec![
                vec![("src/orders.ts", Some("cancelOrder"), "releaseStock")],
                vec![
                    ("src/api/routes.ts", Some("handleCancel"), "cancelOrder"),
                    ("src/orders.ts", Some("reorder"), "cancelOrder"),
                ],
                vec![("src/server.ts", Some("route"), "handleCancel")],
            ]
        );
        let text = ostra_code::tools::run(
            ix,
            "CodeImpact",
            &serde_json::json!({"symbol": "releaseStock"}),
        )
        .unwrap();
        assert!(
            text.contains(
                "files to change or recheck (3): src/orders.ts, src/api/routes.ts, src/server.ts"
            ),
            "{text}"
        );
        let err = ostra_code::tools::run(ix, "CodeImpact", &serde_json::json!({})).unwrap_err();
        assert!(err.contains("Give `symbol`"), "{err}");
    });
}
