use ostra_code::lang;
use ostra_code::lex::{Kind, lex};
use ostra_code::outline::{ImportStyle, analyze, expand_use};
use ostra_core::code::SymbolKind;
use pretty_assertions::assert_eq;

/// `name kind container` per definition, in source order.
fn outline(path: &str, src: &str) -> Vec<String> {
    let lang = lang::for_path(path).unwrap();
    let toks = lex(src, lang);
    analyze(src, lang, &toks)
        .symbols
        .into_iter()
        .map(|s| {
            let kind = format!("{:?}", s.kind).to_lowercase();
            match s.container {
                Some(c) => format!("{} {kind} {c}", s.name),
                None => format!("{} {kind}", s.name),
            }
        })
        .collect()
}

fn imports(path: &str, src: &str) -> Vec<String> {
    let lang = lang::for_path(path).unwrap();
    let toks = lex(src, lang);
    analyze(src, lang, &toks)
        .imports
        .into_iter()
        .map(|i| i.spec)
        .collect()
}

#[test]
fn rust_items_members_and_impls() {
    let src = r#"
use crate::files::{tree, git::{self, Status}};
mod watch;
pub const CAP: usize = 5;
static mut COUNT: u32 = 0;
#[derive(Debug)]
pub struct Files {
    pub index: Mutex<u32>,
    live: bool,
}
pub enum Mark { Added, #[serde] Removed(u32), Renamed { from: String } }
pub trait Store { fn get(&self) -> u32; }
impl<T: Clone> Store for Files<T> {
    const LIMIT: u32 = 3;
    fn get(&self) -> u32 { let s = "fn fake() {}"; 1 }
}
pub(crate) async fn run<'a>(x: &'a str) -> Result<(), E> where E: Error {}
macro_rules! row { () => {} }
type Map = HashMap<String, u32>;
"#;
    assert_eq!(
        outline("src/files.rs", src),
        vec![
            "watch module",
            "CAP constant",
            "COUNT constant",
            "Files class",
            "index field Files",
            "live field Files",
            "Mark enum",
            "Added constant Mark",
            "Removed constant Mark",
            "Renamed constant Mark",
            "Store interface",
            "get method Store",
            "LIMIT constant Files",
            "get method Files",
            "run function",
            "row macro",
            "Map type",
        ]
    );
    assert_eq!(
        imports("src/files.rs", src),
        vec![
            "crate::files::tree",
            "crate::files::git",
            "crate::files::git::Status",
            "watch"
        ]
    );
}

#[test]
fn rust_end_lines_follow_braces() {
    let src = "fn a() {\n    if x {\n    }\n}\nstruct B;\nfn c() {}\n";
    let lang = lang::for_path("a.rs").unwrap();
    let toks = lex(src, lang);
    let syms = analyze(src, lang, &toks).symbols;
    let ends: Vec<_> = syms
        .iter()
        .map(|s| (s.name.as_str(), s.line, s.end_line))
        .collect();
    assert_eq!(
        ends,
        vec![("a", 1, Some(4)), ("B", 5, None), ("c", 6, Some(6))]
    );
}

#[test]
fn typescript_declarations() {
    let src = r#"
import { api } from "../api";
import type { Tab } from './tabs';
import "./side-effect.css";
export * from "./gen";
const lazy = () => import("./Screen");
export const MAX = 3;
export const useNav = (): Nav => useContext(Ctx);
export function FileScreen({ ws }: Props) { const inner = 1; }
export default class Store extends Base {
  private items: Item[] = [];
  static create(n: number) { return new Store(n); }
  get size() { return this.items.length; }
  handler = () => { this.go(); };
}
interface Props { ws: string; onOpen?(id: string): void }
export type View = "file" | "diff";
enum Mode { A, B = 2 }
let count = 0;
"#;
    assert_eq!(
        outline("web/src/screens/FileScreen.tsx", src),
        vec![
            "lazy function",
            "MAX constant",
            "useNav function",
            "FileScreen function",
            "Store class",
            "items field Store",
            "create method Store",
            "size method Store",
            "handler field Store",
            "Props interface",
            "ws field Props",
            "onOpen field Props",
            "View type",
            "Mode enum",
            "A constant Mode",
            "B constant Mode",
            "count variable",
        ]
    );
    assert_eq!(
        imports("web/src/screens/FileScreen.tsx", src),
        vec!["../api", "./tabs", "./side-effect.css", "./gen", "./Screen"]
    );
}

#[test]
fn python_indentation_scopes() {
    let src = r#"
import os, json as j
from .models import User, Group
from pkg.sub import (a,
    b)
MAX_ROWS = 10

@dataclass
class Store(Base):
    name: str = "x"
    def get(self, key):
        def inner():
            pass
        return key

    async def put(self):
        """Doc
        def fake(): pass
        """

def main():
    s = Store()
"#;
    assert_eq!(
        outline("app/store.py", src),
        vec![
            "MAX_ROWS constant",
            "Store class",
            "name field Store",
            "get method Store",
            "inner function get",
            "put method Store",
            "main function",
        ]
    );
    assert_eq!(
        imports("app/store.py", src),
        vec!["os", "json", ".models", "pkg.sub"]
    );
    let lang = lang::for_path("a.py").unwrap();
    let toks = lex(src, lang);
    let an = analyze(src, lang, &toks);
    assert_eq!(an.imports[2].alts, vec![".models.User", ".models.Group"]);
    assert_eq!(an.imports[3].alts, vec!["pkg.sub.a", "pkg.sub.b"]);
    let store = an.symbols.iter().find(|s| s.name == "Store").unwrap();
    assert_eq!((store.line, store.end_line), (9, Some(19)));
}

#[test]
fn go_receivers_blocks_and_structs() {
    let src = r#"
package server

import (
	"fmt"
	web "example.com/app/web"
)

const (
	MaxConn = 10
	minConn = 1
)

type Server struct {
	Addr string
	mu   sync.Mutex
}

type Handler interface {
	Serve(w Writer) error
}

func (s *Server) Start(ctx context.Context) error { return nil }
func New[T any](addr string) *Server { return &Server{Addr: addr} }
var Default = New("")
"#;
    assert_eq!(
        outline("server/server.go", src),
        vec![
            "MaxConn constant",
            "minConn constant",
            "Server class",
            "Addr field Server",
            "mu field Server",
            "Handler interface",
            "Serve method Handler",
            "Start method Server",
            "New function",
            "Default variable",
        ]
    );
    assert_eq!(
        imports("server/server.go", src),
        vec!["fmt", "example.com/app/web"]
    );
}

#[test]
fn java_and_kotlin_members() {
    let java = r#"
package com.shop;
import com.shop.model.Order;
import static java.util.Map.*;
@Service
public class OrderService implements Service {
    private final Map<String, Order> orders = new HashMap<>();
    int count;
    public OrderService(Repo repo) { this.repo = repo; }
    @Override
    public Order find(String id) throws NotFound { return orders.get(id); }
    enum State { OPEN, CLOSED; boolean done() { return this == CLOSED; } }
}
"#;
    assert_eq!(
        outline("src/main/java/com/shop/OrderService.java", java),
        vec![
            "OrderService class",
            "orders field OrderService",
            "count field OrderService",
            "OrderService method OrderService",
            "find method OrderService",
            "State enum OrderService",
            "OPEN constant State",
            "CLOSED constant State",
            "done method State",
        ]
    );
    assert_eq!(
        imports("A.java", java),
        vec!["com.shop.model.Order", "java.util.Map.*"]
    );
    let kotlin = r#"
import com.shop.model.Order
data class Cart(val items: List<Order>) {
    val total: Int = 0
    fun add(o: Order) { val x = 1 }
}
fun String.shout(): String = uppercase()
object Registry
"#;
    assert_eq!(
        outline("Cart.kt", kotlin),
        vec![
            "Cart class",
            "total field Cart",
            "add method Cart",
            "shout function String",
            "Registry class",
        ]
    );
}

#[test]
fn c_and_cpp_definitions() {
    let src = r#"
#include "util.h"
#include <stdio.h>
#define MAX_LEN 64
struct point { int x; int y; };
static int add(int a, int b) { return a + b; }
int main(void) {
    if (add(1, 2)) { printf("hi"); }
    return 0;
}
void Widget::draw() const { }
namespace ui { class Button : public Widget { public: void click(); int size; }; }
int declared(int a);
"#;
    assert_eq!(
        outline("src/main.cpp", src),
        vec![
            "MAX_LEN macro",
            "point class",
            "x field point",
            "y field point",
            "add function",
            "main function",
            "draw method Widget",
            "ui module",
            "Button class ui",
            "click method Button",
            "size field Button",
        ]
    );
    let lang = lang::for_path("a.c").unwrap();
    let toks = lex(src, lang);
    let an = analyze(src, lang, &toks);
    let inc: Vec<_> = an
        .imports
        .iter()
        .map(|i| (i.spec.as_str(), i.style))
        .collect();
    assert_eq!(
        inc,
        vec![
            ("util.h", ImportStyle::Include { quoted: true }),
            ("stdio.h", ImportStyle::Include { quoted: false }),
        ]
    );
}

#[test]
fn ruby_lua_shell_php_swift() {
    assert_eq!(
        outline(
            "lib/a.rb",
            "require_relative 'b'\nclass Cart < Base\n  def self.build; end\n  def empty?; end\nend\nmodule Util; end\n"
        ),
        vec![
            "Cart class",
            "build function",
            "empty? function",
            "Util module"
        ]
    );
    assert_eq!(
        imports("lib/a.rb", "require_relative 'b'\nrequire \"json\"\n"),
        vec!["b", "json"]
    );
    assert_eq!(
        outline(
            "a.lua",
            "local M = require('util.str')\nfunction M.trim(s) end\nlocal function helper() end\n"
        ),
        vec!["trim function M", "helper function"]
    );
    assert_eq!(
        imports("a.lua", "local M = require('util.str')\n"),
        vec!["util.str"]
    );
    assert_eq!(
        outline(
            "run.sh",
            "source ./lib.sh\nbuild() {\n  echo $HOME # not a def\n}\nfunction deploy {\n}\n"
        ),
        vec!["build function", "deploy function"]
    );
    assert_eq!(imports("run.sh", "source ./lib.sh\n"), vec!["./lib.sh"]);
    assert_eq!(
        outline(
            "src/Cart.php",
            "<?php\nnamespace App;\nuse App\\Models\\Order;\nclass Cart {\n  const MAX = 3;\n  public function add(Order $o) {}\n}\n"
        ),
        vec![
            "App module",
            "Cart class",
            "MAX constant Cart",
            "add method Cart"
        ]
    );
    assert_eq!(
        imports("src/Cart.php", "<?php\nuse App\\Models\\Order;\n"),
        vec!["App\\Models\\Order"]
    );
    assert_eq!(
        outline(
            "Cart.swift",
            "import Foundation\nstruct Cart {\n  let items: [Item]\n  func total() -> Int { 0 }\n}\nextension Cart { func clear() {} }\nprotocol Store {}\n"
        ),
        vec![
            "Cart class",
            "items field Cart",
            "total method Cart",
            "clear method Cart",
            "Store interface"
        ]
    );
}

#[test]
fn lexer_positions_are_utf16_and_split_per_line() {
    let src = "let é = \"a\nb\"; /* x\ny */ 𝔸z";
    let lang = lang::for_path("a.rs").unwrap();
    let toks = lex(src, lang);
    let view: Vec<_> = toks
        .iter()
        .map(|t| (t.kind, t.line, t.col, t.len, t.text(src)))
        .collect();
    assert_eq!(
        view,
        vec![
            (Kind::Keyword, 1, 0, 3, "let"),
            (Kind::Ident, 1, 4, 1, "é"),
            (Kind::Punct(b'='), 1, 6, 1, "="),
            (Kind::Str, 1, 8, 2, "\"a"),
            (Kind::Str, 2, 0, 2, "b\""),
            (Kind::Punct(b';'), 2, 2, 1, ";"),
            (Kind::Comment, 2, 4, 4, "/* x"),
            (Kind::Comment, 3, 0, 4, "y */"),
            (Kind::Ident, 3, 5, 3, "𝔸z"),
        ]
    );
}

#[test]
fn lexer_family_forms() {
    let kinds = |path: &str, src: &str| -> Vec<(Kind, String)> {
        let lang = lang::for_path(path).unwrap();
        lex(src, lang)
            .into_iter()
            .filter(|t| !matches!(t.kind, Kind::Punct(_)))
            .map(|t| (t.kind, t.text(src).to_string()))
            .collect()
    };
    assert_eq!(
        kinds("a.rs", "#[cfg(test)] r#\"q\"# 'a' &'b x println!(\"{}\")"),
        vec![
            (Kind::Attr, "#[cfg(test)]".into()),
            (Kind::Str, "r#\"q\"#".into()),
            (Kind::Str, "'a'".into()),
            (Kind::Lifetime, "'b".into()),
            (Kind::Ident, "x".into()),
            (Kind::Macro, "println!".into()),
            (Kind::Str, "\"{}\"".into()),
        ]
    );
    assert_eq!(
        kinds("a.py", "x = f'{a}' + \"\"\"doc\"\"\" # note"),
        vec![
            (Kind::Ident, "x".into()),
            (Kind::Str, "f'{a}'".into()),
            (Kind::Str, "\"\"\"doc\"\"\"".into()),
            (Kind::Comment, "# note".into()),
        ]
    );
    assert_eq!(
        kinds("a.sh", "echo $# ${HOME}/x # c"),
        vec![
            (Kind::Ident, "echo".into()),
            (Kind::Ident, "HOME".into()),
            (Kind::Ident, "x".into()),
            (Kind::Comment, "# c".into()),
        ]
    );
    assert_eq!(
        kinds("a.sql", "SELECT id FROM t -- c"),
        vec![
            (Kind::Keyword, "SELECT".into()),
            (Kind::Ident, "id".into()),
            (Kind::Keyword, "FROM".into()),
            (Kind::Ident, "t".into()),
            (Kind::Comment, "-- c".into()),
        ]
    );
}

#[test]
fn use_trees_expand() {
    let mut out = vec![];
    expand_use("", "a::{b, c::{d, e as f}, self, g::*}", &mut out);
    assert_eq!(out, vec!["a::b", "a::c::d", "a::c::e", "a", "a::g"]);
}

#[test]
fn definition_classes_color_names() {
    use ostra_core::code::TokenClass;
    let f = ostra_code::render("a.rs", "fn go(x: Point) { go(X_MAX) }", None);
    let cls = |i: usize| f.classes[f.tokens[i * 4 + 3] as usize];
    let at: Vec<_> = f
        .tokens
        .chunks(4)
        .map(|c| (c[1], f.classes[c[3] as usize]))
        .collect();
    assert_eq!(
        at,
        vec![
            (0, TokenClass::Keyword),
            (3, TokenClass::Function),
            (6, TokenClass::Name),
            (9, TokenClass::Type),
            (18, TokenClass::Function),
            (21, TokenClass::Constant),
        ]
    );
    assert_eq!(cls(1), TokenClass::Function);
    assert_eq!(f.symbols[0].kind, SymbolKind::Function);
    assert_eq!(f.language.as_deref(), Some("rust"));
    assert!(
        ostra_code::render("notes.unknown", "x", None)
            .tokens
            .is_empty()
    );
}
