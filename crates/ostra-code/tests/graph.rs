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
fn workspace_packages_resolve_by_name_through_their_barrel() {
    let files: &[(&str, &str)] = &[
        (
            "ui/package.json",
            r#"{"name": "@acme/ui", "exports": {".": "./src/index.ts", "./theme": {"types": "./src/theme.ts"}}}"#,
        ),
        ("ui/src/index.ts", "export { Button } from \"./Button\";\n"),
        (
            "ui/src/Button.tsx",
            "export function Button() { return null; }\n",
        ),
        ("ui/src/theme.ts", "export const dark = 1;\n"),
        ("app/package.json", r#"{"name": "app"}"#),
        (
            "app/src/Page.tsx",
            "import { Button as B2, type Tone } from \"@acme/ui\";\nimport { dark } from \"@acme/ui/theme\";\nimport { Button } from \"@acme/ui\";\nexport function Page() { return Button() + dark; }\n",
        ),
    ];
    with_files(files, |ix| {
        let n = ix.neighbors("app/src/Page.tsx", 50).unwrap();
        assert_eq!(
            paths(&n.uses),
            vec![
                ("ui/src/theme.ts", vec!["dark"]),
                ("ui/src/Button.tsx", vec!["Button"]),
                ("ui/src/index.ts", vec![]),
            ]
        );
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

const IMPLS: &[(&str, &str)] = &[
    ("Cargo.toml", "[package]\nname = \"shapes\"\n"),
    (
        "src/lib.rs",
        "pub mod shape;\npub mod circle;\npub mod square;\n",
    ),
    (
        "src/shape.rs",
        "pub trait Shape {\n    fn area(&self) -> f64;\n    fn name(&self) -> String;\n}\n",
    ),
    (
        "src/circle.rs",
        "use crate::shape::Shape;\npub struct Circle { r: f64 }\nimpl Circle {\n    pub fn new(r: f64) -> Self { Circle { r } }\n}\nimpl Shape for Circle {\n    fn area(&self) -> f64 { 3.14 * self.r }\n    fn name(&self) -> String { String::new() }\n}\n",
    ),
    (
        "src/square.rs",
        "use crate::shape::Shape;\npub struct Square;\nimpl Square {\n    pub fn new() -> Self { Square }\n}\nimpl Shape for Square {\n    fn area(&self) -> f64 { 1.0 }\n    fn name(&self) -> String { String::new() }\n}\n",
    ),
    (
        "java/Pet.java",
        "package zoo;\npublic interface Pet {\n    void feed();\n}\n",
    ),
    (
        "java/Zoo.java",
        "package zoo;\npublic class Zoo {\n    void open() {}\n}\nclass Keeper {\n    void feed() {}\n}\ninterface Feeder {}\n",
    ),
    (
        "java/Cat.java",
        "package zoo;\npublic class Cat implements Pet {\n    public void feed() {}\n    public void feed(int grams) {}\n}\n",
    ),
];

#[test]
fn symbol_view_links_implementations() {
    use ostra_core::code::{CodeGraph, CodeGraphEdgeKind};
    let implements = |g: &CodeGraph, from: &str, to: &str| {
        g.edges.iter().any(|e| {
            e.kind == CodeGraphEdgeKind::Implements && e.from.contains(from) && e.to.contains(to)
        })
    };
    with_files(IMPLS, |ix| {
        let t = ix.symbol_view("src/shape.rs", "Shape", None, 1).unwrap();
        assert!(
            implements(&t, "circle.rs:2:Circle", "shape.rs:1:Shape"),
            "{:#?}",
            t.edges
        );
        assert!(implements(&t, "square.rs:2:Square", "shape.rs:1:Shape"));
        let circle = t
            .nodes
            .iter()
            .find(|n| n.id.contains("circle.rs:2:Circle"))
            .unwrap();
        assert_eq!(circle.column, -1);
        // The `impl Shape for Circle` line is the implementation, not a separate reference.
        assert!(
            !t.nodes.iter().any(|n| n.id == "src/circle.rs"),
            "{:#?}",
            t.nodes
        );
        let focus = t.nodes.iter().find(|n| n.id == t.focus).unwrap();
        let members: Vec<&str> = focus
            .symbol
            .as_ref()
            .unwrap()
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(members, ["area", "name"]);

        // A type lists its methods from every impl block, with the trait each one implements.
        let c = ix.symbol_view("src/circle.rs", "Circle", None, 1).unwrap();
        assert!(implements(&c, "circle.rs:2:Circle", "shape.rs:1:Shape"));
        let sym = c
            .nodes
            .iter()
            .find(|n| n.id == c.focus)
            .unwrap()
            .symbol
            .clone()
            .unwrap();
        let members: Vec<(&str, Option<&str>)> = sym
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.via.as_deref()))
            .collect();
        assert_eq!(
            members,
            [
                ("new", None),
                ("area", Some("Shape")),
                ("name", Some("Shape"))
            ]
        );

        // A trait method links down to each implementation, and an implementation up to it.
        let area = ix.symbol_view("src/shape.rs", "area", None, 1).unwrap();
        assert!(
            implements(&area, "circle.rs:7:area", "shape.rs:2:area"),
            "{:#?}",
            area.edges
        );
        assert!(implements(&area, "square.rs:7:area", "shape.rs:2:area"));
        let up = ix.symbol_view("src/circle.rs", "area", None, 1).unwrap();
        assert!(implements(&up, "circle.rs:7:area", "shape.rs:2:area"));
        assert!(!up.nodes.iter().any(|n| n.id.contains("square.rs")));
        let sig = &up
            .nodes
            .iter()
            .find(|n| n.id == up.focus)
            .unwrap()
            .symbol
            .as_ref()
            .unwrap()
            .signature;
        assert!(sig.contains("fn area(&self) -> f64"), "{sig}");

        // Java: the class implements the interface, and both overloads carry their signatures.
        let pet = ix.symbol_view("java/Pet.java", "Pet", None, 1).unwrap();
        assert!(
            implements(&pet, "Cat.java:2:Cat", "Pet.java:2:Pet"),
            "{:#?}",
            pet.edges
        );
        let feed = ix.symbol_view("java/Pet.java", "feed", None, 1).unwrap();
        assert!(
            implements(&feed, "Cat.java:3:feed", "Pet.java:3:feed"),
            "{:#?}",
            feed.edges
        );
        let cat = ix.symbol_view("java/Cat.java", "Cat", None, 1).unwrap();
        let sigs: Vec<String> = cat
            .nodes
            .iter()
            .find(|n| n.id == cat.focus)
            .unwrap()
            .symbol
            .as_ref()
            .unwrap()
            .members
            .iter()
            .map(|m| m.signature.clone())
            .collect();
        assert_eq!(sigs, ["public void feed()", "public void feed(int grams)"]);
    });
}

#[test]
fn go_interfaces_are_satisfied_by_method_sets() {
    use ostra_core::code::CodeGraphEdgeKind;
    let files: &[(&str, &str)] = &[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "store/store.go",
            "package store\n\ntype Store interface {\n\tSave(id string) error\n\tLoad(id string) (string, error)\n}\n\ntype Any interface{}\n",
        ),
        (
            "store/mem.go",
            "package store\n\ntype Mem struct{}\n\nfunc (m *Mem) Save(id string) error { return nil }\n",
        ),
        (
            "store/mem_load.go",
            "package store\n\nfunc (m *Mem) Load(\n\tkey string,\n) (_ string, err error) {\n\treturn \"\", nil\n}\n",
        ),
        (
            "store/half.go",
            "package store\n\ntype Half struct{}\n\nfunc (h Half) Save(id string) error { return nil }\n",
        ),
        (
            "cache/cache.go",
            "package cache\n\ntype Cache interface {\n\tGet(k string) string\n}\n\ntype Mem struct{}\n\nfunc (m *Mem) Get(k string) string { return k }\n",
        ),
        (
            "store/other.go",
            "package store\n\ntype Other struct{}\n\nfunc (o *Other) Save(n int) error { return nil }\nfunc (o *Other) Load(key string) (string, error) { return \"\", nil }\n",
        ),
    ];
    with_files(files, |ix| {
        let imp = |g: &ostra_core::code::CodeGraph, from: &str, to: &str| {
            g.edges.iter().any(|e| {
                e.kind == CodeGraphEdgeKind::Implements
                    && e.from.contains(from)
                    && e.to.contains(to)
            })
        };
        let s = ix.symbol_view("store/store.go", "Store", None, 1).unwrap();
        assert!(
            imp(&s, "mem.go:3:Mem", "store.go:3:Store"),
            "{:#?}",
            s.edges
        );
        assert!(
            !s.nodes.iter().any(|n| n.id.contains("Half")),
            "Half lacks Load"
        );
        // Other has both names, but Save takes an int.
        assert!(
            !s.nodes.iter().any(|n| n.id.contains("Other")),
            "{:#?}",
            s.nodes
        );
        let m = ix.symbol_view("store/mem.go", "Mem", None, 1).unwrap();
        assert!(imp(&m, "mem.go:3:Mem", "store.go:3:Store"));
        assert!(
            !m.edges.iter().any(|e| e.to.contains(":Any")),
            "the empty interface is left out"
        );
        let members: Vec<String> = m
            .nodes
            .iter()
            .find(|n| n.id == m.focus)
            .unwrap()
            .symbol
            .as_ref()
            .unwrap()
            .members
            .iter()
            .map(|x| format!("{}:{}", x.path, x.name))
            .collect();
        assert_eq!(members, ["store/mem.go:Save", "store/mem_load.go:Load"]);
        let load = ix.symbol_view("store/store.go", "Load", None, 1).unwrap();
        assert!(
            imp(&load, "mem_load.go:3:Load", "store.go:5:Load"),
            "{:#?}",
            load.edges
        );
        // The implementation sits left of the interface method, never among its callees.
        let impl_node = load
            .nodes
            .iter()
            .find(|n| n.id.contains("mem_load.go"))
            .unwrap();
        assert_eq!(impl_node.column, -1, "{:#?}", load.nodes);
        let get = ix.symbol_view("cache/cache.go", "Get", Some(4), 1).unwrap();
        let mem_get = get
            .nodes
            .iter()
            .find(|n| n.id == "symbol:cache/cache.go:9:Get")
            .expect("Mem.Get implements Cache.Get");
        assert_eq!(mem_get.column, -1, "{:#?}", get.nodes);
        assert!(
            !get.edges
                .iter()
                .any(|e| e.from == get.focus && e.to == mem_get.id),
            "{:#?}",
            get.edges
        );
        let sig = &load
            .nodes
            .iter()
            .find(|n| n.id.contains("mem_load.go"))
            .unwrap()
            .symbol
            .as_ref()
            .unwrap()
            .signature;
        assert_eq!(sig, "func (m *Mem) Load(key string) (_ string, err error)");
    });
}

#[test]
fn implementations_tool_answers_in_text() {
    use ostra_code::tools::run;
    use serde_json::json;
    with_files(IMPLS, |ix| {
        let out = run(ix, "CodeImplementations", &json!({"symbol": "Shape"})).unwrap();
        assert!(out.starts_with("src/shape.rs:1 interface Shape\n"), "{out}");
        assert!(out.contains("  implemented or extended by (2):\n    src/circle.rs:2 class Circle\n    src/square.rs:2 class Square\n"), "{out}");
        assert!(out.contains("  implements or extends: none"), "{out}");
        // A method name with several definitions: the trait method, with its implementations, comes first.
        let out = run(ix, "CodeImplementations", &json!({"symbol": "area"})).unwrap();
        assert!(out.starts_with("`area` has 3 definitions"), "{out}");
        let first = out.lines().nth(1).unwrap();
        assert_eq!(first, "src/shape.rs:2 method area in Shape");
        assert!(
            out.contains("    src/circle.rs:7 method area in Circle (impl Shape)"),
            "{out}"
        );
        let out = run(
            ix,
            "CodeImplementations",
            &json!({"symbol": "area", "path": "src/square.rs"}),
        )
        .unwrap();
        assert!(
            out.contains("  implements or extends (1):\n    src/shape.rs:2 method area in Shape"),
            "{out}"
        );
        let out = run(
            ix,
            "CodeImplementations",
            &json!({"symbol": "feed", "path": "java/Cat.java", "line": 4}),
        )
        .unwrap();
        assert!(
            out.starts_with("java/Cat.java:4 method feed in Cat"),
            "{out}"
        );
        // A workspace-relative path that starts with the project's folder names the same file.
        let dir = ix.root().file_name().unwrap().to_str().unwrap().to_string();
        let out = run(
            ix,
            "CodeImplementations",
            &json!({"symbol": "Shape", "path": format!("{dir}/src/shape.rs")}),
        )
        .unwrap();
        assert!(out.starts_with("src/shape.rs:1 interface Shape"), "{out}");
        let out = run(
            ix,
            "CodeOutline",
            &json!({"path": format!("{dir}/src/circle.rs")}),
        )
        .unwrap();
        assert!(out.contains("Circle"), "{out}");
        let err = run(ix, "CodeImplementations", &json!({"symbol": "nope"})).unwrap_err();
        assert!(err.contains("Use CodeFind"), "{err}");
        let err = run(
            ix,
            "CodeImplementations",
            &json!({"symbol": "Shape", "path": "src/circle.rs"}),
        )
        .unwrap_err();
        assert!(err.contains("Use CodeOutline"), "{err}");
    });
}

#[test]
fn nodes_list_the_other_definitions_of_their_file() {
    let names = |g: &ostra_core::code::CodeGraph| -> Vec<String> {
        let n = g.nodes.iter().find(|n| n.id == g.focus).unwrap();
        n.file_defs.iter().map(|t| t.name.clone()).collect()
    };
    with_files(IMPLS, |ix| {
        let z = ix.symbol_view("java/Zoo.java", "Zoo", None, 1).unwrap();
        let focus = z.nodes.iter().find(|n| n.id == z.focus).unwrap();
        let lines: Vec<(&str, u32)> = focus
            .file_defs
            .iter()
            .map(|t| (t.name.as_str(), t.line))
            .collect();
        assert_eq!(lines, [("Keeper", 5), ("Feeder", 8)]);
        assert!(
            !z.nodes.iter().any(|n| n.id.contains(":open")),
            "Zoo's own method is a member, not a callee: {:#?}",
            z.nodes
        );
        let open = &focus.symbol.as_ref().unwrap().members[0];
        assert_eq!((open.name.as_str(), open.end_line), ("open", Some(3)));
        // A method's node names every top-level definition of its file, its own type included.
        assert_eq!(
            names(&ix.symbol_view("java/Zoo.java", "feed", None, 1).unwrap()),
            ["Zoo", "Keeper", "Feeder"]
        );
        let f = ix.file_view("java/Zoo.java", 1).unwrap();
        let n = f.nodes.iter().find(|n| n.id == "java/Zoo.java").unwrap();
        assert_eq!(n.file_defs.len(), 3);
    });
    // Every language: a module of functions lists its functions next to its types.
    let files: &[(&str, &str)] = &[
        (
            "tools/util.py",
            "import os\n\nclass Cache:\n    def get(self):\n        return 1\n\ndef load():\n    return Cache()\n\ndef save():\n    pass\n",
        ),
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "store/store.go",
            "package store\n\ntype Store struct{}\n\nfunc (s *Store) Save() {}\n\nfunc Open() *Store { return &Store{} }\n\nfunc Close() {}\n",
        ),
        (
            "web/src/fmt.ts",
            "export function price(n: number) { return `${n}`; }\nexport const date = (d: Date) => d.toISOString();\nexport interface Money { amount: number }\n",
        ),
    ];
    with_files(files, |ix| {
        assert_eq!(
            names(&ix.symbol_view("tools/util.py", "load", None, 1).unwrap()),
            ["Cache", "save"]
        );
        assert_eq!(
            names(&ix.symbol_view("store/store.go", "Open", None, 1).unwrap()),
            ["Store", "Close"]
        );
        assert_eq!(
            names(&ix.symbol_view("web/src/fmt.ts", "price", None, 1).unwrap()),
            ["date", "Money"]
        );
        let f = ix.file_view("tools/util.py", 1).unwrap();
        let n = f.nodes.iter().find(|n| n.id == "tools/util.py").unwrap();
        assert_eq!(n.file_defs.len(), 3);
        assert_eq!(n.more_file_defs, 0);
    });
}

#[test]
fn editor_jumps_follow_implementation_links() {
    use ostra_code::hint::{At, navigate_index};
    use ostra_core::code::NavigateTarget::{Implementations, Supertypes};
    with_files(IMPLS, |ix| {
        let shape = "pub trait Shape {\n    fn area(&self) -> f64;\n}\n";
        let at = |path: &'static str, text: &'static str, line: u32, col: u32| At {
            path,
            text,
            line,
            col,
            trigger: None,
            retrigger: false,
        };
        let found = |ix: &mut ProjectIndex, a: &At<'_>, t| -> Vec<(String, u32)> {
            navigate_index(ix, a, t)
                .unwrap()
                .locations
                .into_iter()
                .map(|l| (l.path, l.line))
                .collect()
        };
        let impls = found(ix, &at("src/shape.rs", shape, 1, 12), Implementations);
        assert_eq!(
            impls,
            [
                ("src/circle.rs".to_string(), 2),
                ("src/square.rs".to_string(), 2)
            ]
        );
        // A trait method leads to the methods that implement it.
        let area = found(ix, &at("src/shape.rs", shape, 2, 8), Implementations);
        assert_eq!(
            area,
            [
                ("src/circle.rs".to_string(), 7),
                ("src/square.rs".to_string(), 7)
            ]
        );
        let circle = "use crate::shape::Shape;\npub struct Circle { r: f64 }\n";
        let up = found(ix, &at("src/circle.rs", circle, 2, 13), Supertypes);
        assert_eq!(up, [("src/shape.rs".to_string(), 1)]);
        // A name the file only uses is asked across the project.
        let used = found(ix, &at("src/circle.rs", circle, 1, 20), Implementations);
        assert_eq!(used.len(), 2);
        let cat = "package zoo;\npublic class Cat implements Pet {\n";
        let pet = found(ix, &at("java/Cat.java", cat, 2, 29), Supertypes);
        assert!(pet.is_empty(), "an interface extends nothing: {pet:?}");
        assert_eq!(
            found(ix, &at("java/Cat.java", cat, 2, 14), Supertypes),
            [("java/Pet.java".to_string(), 2)]
        );

        // Files and names the index does not know are left to a language server.
        assert!(navigate_index(ix, &at("src/new.rs", circle, 2, 13), Supertypes).is_none());
        let other = "fn unknown_name() {}\n";
        assert!(navigate_index(ix, &at("src/lib.rs", other, 1, 5), Supertypes).is_none());
    });
}
