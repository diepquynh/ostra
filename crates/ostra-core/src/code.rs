//! Code navigation contracts: what a code provider returns for one file (display tokens, outline,
//! imports), for a symbol (definitions and references), for a file's dependencies, and for a
//! symbol search. The built-in provider and an external `code_provider` command return the same
//! shapes, so the browser renders either one the same way.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The version of the external provider protocol, sent in every request.
pub const PROTOCOL_VERSION: u32 = 1;

/// Language ids the built-in provider navigates, which a language server entry may name.
pub const NAV_LANGUAGES: &[&str] = &[
    "rust",
    "typescript",
    "javascript",
    "python",
    "go",
    "java",
    "kotlin",
    "scala",
    "csharp",
    "swift",
    "php",
    "c",
    "cpp",
    "ruby",
    "lua",
    "shell",
];

/// Name the built-in provider reports in `provider`.
pub const NATIVE_PROVIDER: &str = "native";

/// Display classes of a token. The browser colors each one and makes the name classes clickable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TokenClass {
    Keyword,
    /// A type, class, trait, or interface name.
    Type,
    /// A called or defined function or method name.
    Function,
    /// Any other identifier.
    Name,
    /// A constant, an enum member, or a literal such as `true` or `None`.
    Constant,
    Macro,
    /// An attribute, decorator, or annotation.
    Attribute,
    String,
    Number,
    Comment,
    Operator,
}

impl TokenClass {
    /// Classes that name a symbol, so the browser offers "Find usages" on them.
    pub fn is_symbol(self) -> bool {
        matches!(
            self,
            TokenClass::Type
                | TokenClass::Function
                | TokenClass::Name
                | TokenClass::Constant
                | TokenClass::Macro
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SymbolKind {
    Function,
    Method,
    /// A struct, class, record, or union.
    Class,
    Enum,
    /// A trait, interface, or protocol.
    Interface,
    /// A type alias.
    Type,
    Module,
    Constant,
    Variable,
    Field,
    Macro,
}

/// One symbol a file defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based.
    pub line: u32,
    /// 0-based, in UTF-16 code units, like a browser string index.
    pub col: u32,
    /// Last line of the symbol's body, when the provider knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// The enclosing type, module, or impl, for methods and fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

/// One dependency a file names: an import, use, include, or require.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeImport {
    /// The module as the source spells it, such as `./api` or `crate::files::tree`.
    pub spec: String,
    /// 1-based.
    pub line: u32,
    /// The project-relative file or folder it resolves to. Null for an external package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `target` names a folder (a Go package, for example).
    #[serde(default)]
    pub folder: bool,
}

/// `GET /api/workspaces/:ws/projects/:key/code/file?path=`: display data for one file. The text
/// itself comes from the `file` endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeFile {
    #[serde(default)]
    pub path: String,
    /// Provider name: `native` or the command's program name.
    #[serde(default)]
    pub provider: String,
    /// Language id such as `rust` or `typescript`. Null when the provider does not know the file.
    #[serde(default)]
    pub language: Option<String>,
    /// The classes `tokens` refers to by index.
    #[serde(default)]
    pub classes: Vec<TokenClass>,
    /// Flat runs of four numbers per token, in source order: 1-based line, 0-based UTF-16
    /// column, UTF-16 length, index into `classes`. A token never spans lines.
    #[serde(default)]
    pub tokens: Vec<u32>,
    /// Definitions in source order.
    #[serde(default)]
    pub symbols: Vec<CodeSymbol>,
    #[serde(default)]
    pub imports: Vec<CodeImport>,
    /// Why the provider fell back or cut its answer, shown to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A place in a project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeLocation {
    /// The symbol's name as written at this place. A provider may leave it out of usages.
    #[serde(default)]
    pub name: String,
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 0-based, UTF-16.
    pub col: u32,
    /// UTF-16 length of the matched name.
    pub len: u32,
    /// The trimmed source line, cut to a few hundred characters.
    #[serde(default)]
    pub preview: String,
    /// Set on definitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

/// `GET .../code/usages?symbol=&path=`: where a symbol is defined and used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeUsages {
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub definitions: Vec<CodeLocation>,
    /// Uses outside the definitions. The file named in the request comes first.
    #[serde(default)]
    pub references: Vec<CodeLocation>,
    /// The result cap was reached.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A file that imports the requested one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeImporter {
    pub path: String,
    /// 1-based line of the import.
    pub line: u32,
    pub spec: String,
}

/// `GET .../code/deps?path=`: what a file imports and which files import it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeDeps {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub imports: Vec<CodeImport>,
    #[serde(default)]
    pub importers: Vec<CodeImporter>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Which slice of the dependency graph a [`CodeGraph`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CodeGraphView {
    /// Every package and the links between them.
    Packages,
    /// One package's files, plus the packages they use.
    Package,
    /// One file and the files within a few hops of it.
    File,
    /// One definition, what references it to the left, and what it calls or names to the right.
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CodeGraphNodeKind {
    Package,
    File,
    /// A function, method, type, or other definition.
    Symbol,
}

/// The definition a symbol node stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeGraphSymbol {
    pub path: String,
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based line of the definition.
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeGraphNode {
    /// A project-relative file path, `package:<folder>` for a package, or
    /// `symbol:<path>:<line>:<name>` for a definition.
    pub id: String,
    pub kind: CodeGraphNodeKind,
    pub label: String,
    /// The package folder the node belongs to, `""` for the project top.
    pub package: String,
    /// Left to right: a node uses the nodes in higher columns. The focus of a file view is column 0.
    pub column: i32,
    /// Files in a package; 1 for a file.
    pub files: u32,
    pub test: bool,
    /// Files (or packages) that use this one, across the whole project. For a symbol view, the
    /// links into the node that the view shows.
    pub dependents: u32,
    /// Files (or packages) this one uses, across the whole project. For a symbol view, the links
    /// out of the node that the view shows.
    pub dependencies: u32,
    /// Set on symbol nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<CodeGraphSymbol>,
}

/// `from` uses `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeGraphEdge {
    pub from: String,
    pub to: String,
    /// Names `from` uses from `to`, or file-to-file links between two packages.
    pub weight: u32,
    /// The first few names, for a file edge.
    #[serde(default)]
    pub names: Vec<String>,
    /// An import statement names `to`.
    #[serde(default)]
    pub import: bool,
}

/// `GET .../code/graph?package=&path=&symbol=&line=&depth=`: one view of the project's dependency graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeGraph {
    pub view: CodeGraphView,
    /// The package or file the view is about; empty for the package view.
    #[serde(default)]
    pub focus: String,
    pub nodes: Vec<CodeGraphNode>,
    pub edges: Vec<CodeGraphEdge>,
    /// Files the index holds.
    pub indexed_files: u32,
    /// Nodes were left out to keep the view readable.
    #[serde(default)]
    pub truncated: bool,
}

/// `POST .../code/reindex`: the project's code index, rebuilt from disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeReindex {
    pub indexed_files: u32,
    pub millis: u32,
    /// The index's size cap left files out.
    pub truncated: bool,
}

/// `GET .../code/symbols?q=&limit=`: definitions whose name matches, best match first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CodeSymbols {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub items: Vec<CodeLocation>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// One request to an external provider, written as JSON to its stdin. The provider answers with
/// the response shape of `op` on stdout, or `null` to let the built-in provider answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProviderRequest {
    File {
        version: u32,
        root: String,
        path: String,
    },
    Usages {
        version: u32,
        root: String,
        symbol: String,
        /// The file the user asked from, for ranking and scoping.
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        col: Option<u32>,
        limit: u32,
    },
    Deps {
        version: u32,
        root: String,
        path: String,
    },
    Symbols {
        version: u32,
        root: String,
        query: String,
        limit: u32,
    },
}

impl ProviderRequest {
    pub fn op(&self) -> &'static str {
        match self {
            ProviderRequest::File { .. } => "file",
            ProviderRequest::Usages { .. } => "usages",
            ProviderRequest::Deps { .. } => "deps",
            ProviderRequest::Symbols { .. } => "symbols",
        }
    }
}
