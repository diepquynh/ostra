use ostra_code::{Indexes, ProjectIndex};
use ostra_core::api::FileIndex;
use ostra_core::code::CodeUsages;
use std::sync::Arc;

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

/// Usages of the `nth` occurrence of `symbol` on the line of `path` that contains `line_has`.
fn at(
    ix: &mut ProjectIndex,
    files: &[(&str, &str)],
    path: &str,
    line_has: &str,
    symbol: &str,
) -> CodeUsages {
    let text = files.iter().find(|(p, _)| *p == path).unwrap().1;
    let (n, l) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(line_has))
        .unwrap_or_else(|| panic!("no line with {line_has}"));
    let col = l[l.find(line_has).unwrap()..].find(symbol).unwrap() + l.find(line_has).unwrap();
    ix.usages_at(symbol, path, n as u32 + 1, Some(col as u32 + 1), 100)
}

fn refs(u: &CodeUsages) -> Vec<String> {
    u.references
        .iter()
        .map(|r| format!("{}:{} {}", r.path, r.line, r.preview.trim()))
        .collect()
}

fn defs(u: &CodeUsages) -> Vec<String> {
    u.definitions
        .iter()
        .map(|d| {
            format!(
                "{}:{} {}",
                d.path,
                d.line,
                d.container.as_deref().unwrap_or("")
            )
        })
        .collect()
}

const JAVA: &[(&str, &str)] = &[
    (
        "src/model/User.java",
        "package model;\n\npublic class User {\n    private String name;\n\n    public String getName() {\n        return name;\n    }\n\n    public void setName(String name) {\n        this.name = name;\n    }\n}\n",
    ),
    (
        "src/model/Product.java",
        "package model;\n\npublic class Product {\n    private String name;\n\n    public String getName() {\n        return name;\n    }\n}\n",
    ),
    (
        "src/model/Admin.java",
        "package model;\n\npublic class Admin extends User {\n    public String label() {\n        return getName();\n    }\n}\n",
    ),
    (
        "src/service/Shop.java",
        "package service;\n\nimport model.Admin;\nimport model.Product;\nimport model.User;\n\npublic class Shop {\n    private User owner;\n\n    public String describe(User u, Product p, Admin a) {\n        String name = p.getName();\n        name.length();\n        a.getName();\n        return u.getName() + owner.getName() + name;\n    }\n}\n",
    ),
];

#[test]
fn java_member_usages_follow_the_receiver_type() {
    with_files(JAVA, |ix| {
        let u = at(ix, JAVA, "src/model/User.java", "String getName", "getName");
        assert_eq!(defs(&u), ["src/model/User.java:6 User"]);
        assert_eq!(
            refs(&u),
            [
                "src/model/Admin.java:5 return getName();",
                "src/service/Shop.java:13 a.getName();",
                "src/service/Shop.java:14 return u.getName() + owner.getName() + name;",
                "src/service/Shop.java:14 return u.getName() + owner.getName() + name;",
            ]
        );

        let p = at(
            ix,
            JAVA,
            "src/model/Product.java",
            "String getName",
            "getName",
        );
        assert_eq!(
            refs(&p),
            ["src/service/Shop.java:11 String name = p.getName();"]
        );

        // A use leads to the definition its receiver's type has.
        let jump = at(ix, JAVA, "src/service/Shop.java", "p.getName", "getName");
        assert_eq!(defs(&jump), ["src/model/Product.java:6 Product"]);
        let inherited = at(ix, JAVA, "src/service/Shop.java", "a.getName", "getName");
        assert_eq!(defs(&inherited), ["src/model/User.java:6 User"]);
    });
}

#[test]
fn java_field_usages_skip_locals_and_other_classes() {
    with_files(JAVA, |ix| {
        let u = at(
            ix,
            JAVA,
            "src/model/User.java",
            "private String name",
            "name",
        );
        assert_eq!(defs(&u), ["src/model/User.java:4 User"]);
        assert_eq!(
            refs(&u),
            [
                "src/model/User.java:7 return name;",
                "src/model/User.java:11 this.name = name;",
            ]
        );
        assert_eq!(u.references[1].col, 13);

        // A local of the same name means nothing in the project.
        let local = at(ix, JAVA, "src/service/Shop.java", "name.length", "name");
        assert!(local.definitions.is_empty(), "{:?}", defs(&local));
        assert!(local.references.is_empty(), "{:?}", refs(&local));
    });
}

const TS: &[(&str, &str)] = &[
    ("package.json", "{}"),
    (
        "src/a.ts",
        "export class Cart {\n  save(): void {}\n}\nexport class Draft {\n  save(): void {}\n}\n",
    ),
    (
        "src/b.ts",
        "import { Cart, Draft } from \"./a\";\n\nexport function run(d: Draft) {\n  const c = new Cart();\n  c.save();\n  d.save();\n}\n",
    ),
];

#[test]
fn typescript_members_follow_declared_and_constructed_types() {
    with_files(TS, |ix| {
        let u = at(ix, TS, "src/a.ts", "save", "save");
        assert_eq!(defs(&u), ["src/a.ts:2 Cart"]);
        assert_eq!(refs(&u), ["src/b.ts:5 c.save();"]);
    });
}

const GO: &[(&str, &str)] = &[
    ("go.mod", "module example.com/app\n\ngo 1.22\n"),
    (
        "store/store.go",
        "package store\n\ntype DB struct{}\n\nfunc (d *DB) Close() {}\n\ntype File struct{}\n\nfunc (f *File) Close() {}\n",
    ),
    (
        "app/app.go",
        "package app\n\nimport \"example.com/app/store\"\n\ntype App struct {\n\tdb *store.DB\n}\n\nfunc (a *App) Stop(f *store.File) {\n\ta.db.Close()\n\tf.Close()\n}\n",
    ),
];

#[test]
fn go_members_follow_receivers_and_fields() {
    with_files(GO, |ix| {
        let u = at(ix, GO, "store/store.go", "(d *DB) Close", "Close");
        assert_eq!(defs(&u), ["store/store.go:5 DB"]);
        assert_eq!(refs(&u), ["app/app.go:10 a.db.Close()"]);
        let f = at(ix, GO, "store/store.go", "(f *File) Close", "Close");
        assert_eq!(refs(&f), ["app/app.go:11 f.Close()"]);
    });
}

#[test]
fn a_name_without_a_position_matches_by_name() {
    with_files(JAVA, |ix| {
        let u = ix.usages("getName", None, 100);
        assert_eq!(u.definitions.len(), 2);
        assert_eq!(u.references.len(), 5);
    });
}

const CHAINS: &[(&str, &str)] = &[
    (
        "src/Order.java",
        "public class Order {\n    public static Builder builder() {\n        return new Builder();\n    }\n\n    public static class Builder {\n        public Builder id(long id) {\n            return this;\n        }\n\n        public Order build() {\n            return new Order();\n        }\n    }\n}\n",
    ),
    (
        "src/Request.java",
        "public class Request {\n    public static class Builder {\n        public Request build() {\n            return new Request();\n        }\n    }\n}\n",
    ),
    (
        "src/Shop.java",
        "import lombok.Builder;\n\n@Builder\npublic class Shop {\n    void run(Client client) {\n        Order o = Order.builder().id(1).build();\n        var b = Order.builder();\n        b.build();\n    }\n}\n",
    ),
    (
        "src/Api.java",
        "public class Api {\n    void call(Client client) {\n        client.request().build();\n    }\n}\n",
    ),
    (
        "src/Users.java",
        "import java.util.Optional;\n\npublic class Users {\n    public Optional<Order> find(long id) {\n        return Optional.empty();\n    }\n\n    Order.Builder again(Users users) {\n        users.find(1).orElseThrow().build();\n        return null;\n    }\n}\n",
    ),
];

#[test]
fn java_chains_follow_return_types() {
    with_files(CHAINS, |ix| {
        let u = at(ix, CHAINS, "src/Order.java", "Order build", "build");
        assert_eq!(defs(&u), ["src/Order.java:11 Builder"]);
        assert_eq!(
            refs(&u),
            [
                "src/Shop.java:6 Order o = Order.builder().id(1).build();",
                "src/Shop.java:8 b.build();",
            ]
        );
        // `Optional<Order>` passes through `orElseThrow`, and `Order` has no `build`.
        let o = at(ix, CHAINS, "src/Users.java", "orElseThrow().build", "build");
        assert!(o.definitions.is_empty(), "{:?}", defs(&o));
        // A chain the index cannot follow does not count for a nested class the file never names.
        let r = at(ix, CHAINS, "src/Request.java", "Request build", "build");
        assert!(r.references.is_empty(), "{:?}", refs(&r));
    });
}

const IFACE: &[(&str, &str)] = &[
    (
        "src/Greeter.java",
        "public interface Greeter {\n    String greet();\n}\n",
    ),
    (
        "src/Hello.java",
        "public class Hello implements Greeter {\n    public String greet() {\n        return \"hi\";\n    }\n}\n",
    ),
    (
        "src/Bye.java",
        "public class Bye implements Greeter {\n    public String greet() {\n        return \"bye\";\n    }\n}\n",
    ),
    (
        "src/Main.java",
        "public class Main {\n    void run(Greeter g, Hello h, Bye b) {\n        g.greet();\n        h.greet();\n        b.greet();\n        verify(b, times(1)).greet();\n        verify(this.hello).greet();\n    }\n\n    private Hello hello;\n}\n",
    ),
];

#[test]
fn an_implementation_counts_calls_through_its_interface() {
    with_files(IFACE, |ix| {
        let u = at(ix, IFACE, "src/Hello.java", "String greet", "greet");
        assert_eq!(
            defs(&u),
            ["src/Hello.java:2 Hello", "src/Greeter.java:2 Greeter"]
        );
        assert_eq!(
            refs(&u),
            [
                "src/Main.java:3 g.greet();",
                "src/Main.java:4 h.greet();",
                "src/Main.java:7 verify(this.hello).greet();",
            ]
        );
        let i = at(ix, IFACE, "src/Greeter.java", "String greet", "greet");
        assert_eq!(refs(&i), ["src/Main.java:3 g.greet();"]);
    });
}
