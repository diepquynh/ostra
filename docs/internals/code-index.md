# Code index

Ostra reads the code of each project that it works on and keeps an index of that code. The index holds three
things for each file: the names that the file defines, the names that it mentions, and the files that it
imports. Two parts of Ostra read that index:

- The Files view in the browser. It uses the index to color code, draw an outline, go to a definition, and list
  usages.
- Agents. They use the index through eight code tools. With these tools, an agent can ask "who calls this" or
  "what breaks if I change this", and it does not read dozens of files.

The index is in the `ostra-code` crate. It has no compiler and no type checker. It uses three parts:

- A tokenizer that a table controls.
- A set of definition and import rules for each language family.
- A resolver that knows how each language maps an import to a file.

This page tells what these parts get right and where they stop. It also tells how a project can add a language
server or its own program when it needs more.

## Why Ostra has its own index

Agents spend most of their tokens when they find their way around a codebase. A model that must know where a
function is used runs `grep` and reads every hit. Then it discards the hits that name a different function
with the same name. Then it opens each caller to see what it is. On a large repository, this costs many tool
calls before the real work starts.

The index answers those questions in one call. The answer is in a compact text form that a model reads
quickly. The index also works the same way on every executor. A native run calls the tools directly. A harness
run (Claude Code, Codex, and the others) gets the same tools through Ostra's MCP server. No harness needs an
editor plugin or a language server of its own.

The index does not need a successful build. It reads source text, so it answers when the project fails to
compile. An implementer needs the index most at that time.

## Languages

The tokenizer covers these languages for navigation (outline, definitions, usages, and imports):

| Family | Languages | Family-specific rules |
| --- | --- | --- |
| Rust | Rust | `impl` blocks as containers, `impl Trait for Type`, `mod` declarations, `use` paths |
| JavaScript | TypeScript, JavaScript | `export * from` re-exports, `tsconfig.json` paths such as `@/` |
| Python | Python | Indentation sets the contents of a class body, base classes in the `class` line |
| Go | Go | Receivers (`func (r *Repo) Save`), `go.mod` module paths, interface satisfaction by method set |
| JVM and friends | Java, Kotlin, Scala, C#, Swift, PHP | Class bodies with members, dotted imports, `extends` and `implements` |
| C | C, C++ | Function bodies, `#include` |
| Others | Ruby, Lua, shell | Keyword definitions |

Ostra colors SQL, TOML, YAML, JSON, CSS, and HTML for display. These languages are not part of navigation.

Each language is one entry in a table (`crates/ostra-code/src/lang.rs`). The entry holds these data:

- The comment and string syntax.
- The keywords.
- The built-in types and constants.
- The keywords that start a definition, with the kind of thing that each one defines.

To add a language, add a row. If the family of the language is new, also add a small set of definition and
import rules. The lexer and the index do not change.

## What the index keeps

Ostra keeps one index for each project. It builds the index on the first request that needs it. Then it
updates the index in increments.

For each file, the index keeps:

- The set of names that the file mentions.
- The definitions of the file. Each definition has a kind (function, method, class, field, and other kinds), a
  line range, the enclosing type, and a one-line signature.
- The imports of the file. The index resolves each import to a project file when the rules of the language
  allow it.
- The supertype declarations of the file (`extends`, `implements`, `impl Trait for Type`, Python base
  classes).

The index never keeps the text of a file. A usages query reads again only the files whose name set contains
the name that the query looks for. Thus the index uses little memory on large repositories. The answer also
always shows what is on disk now.

Imports resolve from the rules of the language and from the manifests of the project:

- `Cargo.toml` package names.
- `go.mod` module paths.
- `package.json` and `tsconfig.json` for aliased paths.

A `package.json` also names a workspace package. An import of `@scope/ui` or `@scope/ui/theme` resolves in this
order:

1. To the file that its `exports` entry names (the first of the `types`, `import`, `default`, `module`, and
   `require` conditions).
2. If there is no such entry and the import names the package itself, to the top-level `types`, `module`, or
   `main` of the package.
3. If there is no such entry and the import names a subpath, to that path in the package folder.

The index marks an import that goes out of the project as `external`. Examples are `std::path::Path` and a
third-party package.

### Staying current

Two mechanisms keep the index the same as the files:

1. **Writes by executions.** When an agent writes or edits a file, Ostra marks that file to read again. Ostra
   does the same when the user saves a file in the browser.
2. **A periodic check.** After 30 seconds, Ostra compares the size and modification time of each file with the
   disk again. Thus Ostra finds edits from outside Ostra, for example from an editor, a `git pull`, or a
   formatter. Ostra analyzes again only the files whose stamp changed, and it analyzes them in parallel.

### Limits

The index has fixed limits. Without limits, an index on a monorepo can use all the memory of the server:

| Limit | Value | Where |
| --- | --- | --- |
| Largest file indexed | 1 MB | `MAX_FILE_BYTES` in `crates/ostra-code/src/lib.rs` |
| Source read per project | 256 MB | `MAX_INDEX_BYTES` in `index.rs` |
| Files one usages query reads | 4,000 | `MAX_SCAN_FILES` in `index.rs` |
| Files a transitive walk visits | 5,000 | `MAX_REACH` in `graph.rs` |

The index skips minified files and the files that the ignore rules of the project exclude. When the index
stops at the 256 MB mark, its answers say so. Thus no reader thinks that a partial answer is complete.

## Finding the right definition

The difficult part of "who uses this" is a name that occurs many times. `getName` exists on many unrelated
classes, and a text search lists all of them. The index makes the list smaller in several steps.

When a request names a position (a file and a line), the index first finds the one definition that the name
means at that position:

- A position on a definition means that definition.
- A member after `.`, `->`, or `::` resolves through the type of its receiver. The index gets the type from
  the declaration of the receiver in the file (`Repo repo`, `repo: Repo`, `repo *Repo`, `repo = new Repo()`).
  It can also get the type from the return type in the signature of the call before it
  (`Order.builder().build()`). Then it walks the supertypes of that type.
- `self.x`, `this.x`, and a bare member name in a class body mean the enclosing class. The exception is a local
  that the method declares earlier with the same name.
- A receiver whose type the project does not define, such as `String`, means none of the members of the
  project.

Then the index lists only the mentions that can mean that definition. The usages of a method also include
calls to the interface method or trait method that it implements. A mention whose receiver type is not known
still counts in two cases:

- The file names the class of the definition.
- No other thing in the language has that name.

When the name resolves to nothing that the index can identify, the answer uses a match by name.

This method is a heuristic, not type checking. The index does not see members that code generation adds, such
as Lombok getters. If a project needs exact answers, it can add a language server (below).

When the user clicks a name in the Files view, the view asks for its usages at that position. The code pane
lists the one definition and the mentions that can mean it:

![The Usages tab of the code pane for the cancel method](../images/console/usages.png)

## The dependency graph

From the data for each file, the index builds a graph of the project (`crates/ostra-code/src/graph.rs`). It
does not read files for this graph. The graph has two kinds of edge:

- **Import edges** come from resolved imports.
- **Reference edges** link a file to the files that define the names that it mentions. The index looks for a
  definition in the order that a person uses: the file itself, the files that it imports, its folder, its
  package, and then the packages that its package imports.

These rules keep the graph correct:

- If a name is defined in more than three files, the name links none of them. The exception is a file that
  mentions the name and imports a file that defines it directly. Without this rule, a common name connects all files to all
  files.
- A Rust `mod` declaration shows containment, not use. Thus a transitive walk does not follow it.
- Index modules (`lib.rs`, `mod.rs`, `index.ts`, `__init__.py`, and files with `export * from`) re-export
  names. A walk does not go through them on an import alone, because the names already link to the files that
  define them.
- Across packages, a bare name links to a function only when an import spells it. In Rust, this import is
  `use other::name`. In JavaScript and TypeScript, it is `import { name } from "@scope/ui"`, and every name
  from another file needs an import. In Rust, a type, macro, or top-level constant from a crate that the
  package depends on links without an import.
- The index identifies test files by folder (`tests/`, `__tests__/`, `spec/`, `fixtures/`) and by name
  (`_test.go`, `.spec.ts`, `Test.java`, and others). Thus answers can separate code from its tests.

Ostra checks this graph against its own source tree in `crates/ostra-code/tests/graph_ostra.rs`. One test
asserts that `ostra-engine` depends on none of the server, executor, provider, or tool crates. This is the
dependency rule in the contributor notes. If the index makes the graph incorrectly, that test fails.

The Dependencies tab of a project draws this graph. When the user double-clicks a package, the tab opens its
files and the imports between them:

![The dependency graph of the crates package with one import link](../images/console/project-deps.png)

## The code tools agents use

Each agent that reads code has the `code` capability, which gives it eight tools. The description of each tool
tells the model when to use it in place of `Grep` or `Read`.

| Tool | What it answers |
| --- | --- |
| `CodeOutline` | The imports of a file (with the project file that each one resolves to) and its definitions with line ranges, without a read of the file |
| `CodeFind` | Definitions by name across the project, in this order: exact match, prefix, substring, initials (`pn` finds `parse_name`) |
| `CodeCallers` | Each place that uses a symbol, grouped by the enclosing function or type |
| `CodeCallees` | What the body of one function or type uses: the functions that it calls, the types that it names, the fields that it reads or writes |
| `CodeImplementations` | What a type implements or extends, and what implements it. For a method, the interface method that it implements and the methods that implement it |
| `CodeNeighbors` | The files that one file uses and the files that use it, with the import lines and the names that link each pair |
| `CodeImpact` | What a change can break, hop by hop, and the list of files to change or check again |
| `CodeMap` | The packages of the project and their dependencies, the files with the most dependents, and import cycles |

The tool definitions are in `crates/ostra-tools/src/defs.rs`. The code that formats the answers is in
`crates/ostra-code/src/tools.rs`. On a harness, the same tools have the names `mcp__ostra__code_outline`,
`mcp__ostra__code_callers`, and so on, and they run the same code.

The code tools always read the built-in index, also when the project has a language server. The reason is
that the question of an agent must get the same answer if an external server runs or not.

### What an answer looks like

These are real answers from the index on the repository of Ostra.

`CodeOutline` on the project memory store lists imports and definitions. It indents methods under their type.
An agent reads this first. Then it opens only the line range that it needs:

```text
crates/ostra-store/src/memory.rs (rust, 314 lines)
imports:
  L5 crate::StoreError -> crates/ostra-store/src/lib.rs
  L6 ostra_core::api::Lesson -> crates/ostra-core/src/api.rs
  L7 rusqlite::Connection -> external
  ...
symbols:
  L11 const DEFAULT_RECALL_LIMIT
  L38-40 class MemoryStore
    L39 field path in MemoryStore
    L90-99 method record in MemoryStore
    L103-163 method recall in MemoryStore
  ...
```

`CodeCallers` groups each use by the function that contains it:

```text
defined at:
  crates/ostra-policy/src/guards.rs:654 fn lesson_denial
used by (2 places in 2 files):
  crates/ostra-policy/src/guards.rs  fn check_write  lines 507
  crates/ostra-policy/src/policy.rs  method layer1 in ExecutionPolicy  lines 386,400
```

`CodeImpact` follows names, not full files. The answer does not include a file that uses only other things
from the same file. This is the difference from "every file that imports this file":

```text
changing crates/ostra-store/src/memory.rs:103 method recall in MemoryStore affects:
hop 1 (uses it directly):
  crates/ostra-exec-native/src/lib.rs  method tool in Run  lines 808
  crates/ostra-server/src/bridge.rs  method policy_observe in ServerBridge  lines 345
  crates/ostra-tools/src/misc.rs  fn memory_recall  lines 122
  ...
hop 2 (uses something in hop 1):
  crates/ostra-exec-native/src/lib.rs  method run_tools in Run  lines 627,635  via tool
  crates/ostra-server/src/bridge.rs  method mcp_call in ServerBridge  lines 401  via policy_observe
  crates/ostra-tools/src/lib.rs  fn execute  lines 253  via memory_recall
files to change or recheck (5): ...
```

## The provider chain in the Files view

For each request, the Files view in the browser asks a chain of providers and shows the first answer. Each
answer names the provider that gave it.

```text
code_provider program  ->  language servers  ->  built-in index
```

The footer of the code pane names the provider that answered. This screenshot comes from the demo data of the
console, so the provider is `mock`:

![A Rust file with its outline in the code pane and the provider named in the footer](../images/console/file.png)

### A project's own program

A project entry can set `code_provider.command`. Ostra runs this program one time for each request, in the
project folder. Ostra writes one JSON request to its standard input. Then Ostra reads one JSON value from its
standard output:

```json
{"op": "usages", "version": 1, "root": "/home/me/src/shop", "symbol": "Order", "limit": 40, "path": "src/order.rs", "line": 12}
```

The operations are `file`, `usages`, `deps`, and `symbols`. The answer has the shape of the matching API type
(`CodeFile`, `CodeUsages`, `CodeDeps`, `CodeSymbols` in `crates/ostra-core/src/code.rs`). The program sends
only the fields that it knows. To give a request to the next provider, the program answers `null`.

In these cases, Ostra also gives the request to the next provider:

- The program exits with a non-zero status.
- The program times out (10 seconds by default, up to 120).
- The program returns an answer that fails the checks of Ostra.

The answer that the view shows then includes the reason. A silent fallback can hide a broken provider.
`OSTRA_CODE_PROTOCOL` in the environment of the program holds the protocol version.

### Language servers

Each `[[projects.language_servers]]` entry names a Language Server Protocol server, the languages that it
answers for, a timeout, and optional initialization options:

```toml
[[projects.language_servers]]
command = ["rust-analyzer"]
languages = ["rust"]
initialization_options = { workspace = { symbol = { search = { kind = "all_symbols" } } } }
```

Ostra starts a server in the project folder on the first request for one of its languages. Ostra keeps one
server for each project and command. It stops a server after 10 minutes without requests. It runs at most 8
servers across all projects. When it must start a ninth server, it stops the least recently used server,
because each server holds a full project in memory. If a server fails to start, Ostra does not try it again for
30 seconds.

A language server makes the Files view better, but it does not replace the built-in answer:

- A file answer starts from the built-in answer. Then Ostra adds the semantic tokens and document symbols of the
  server on top. Imports stay built-in.
- For usages at a position, Ostra asks the server for `definition` and `references`.
- For symbol search, Ostra asks `workspace/symbol`.
- Writes by executions and saves in the browser go to running servers as `workspace/didChangeWatchedFiles`.

A server can also answer with a definition outside the project. Examples are a library in `~/.cargo/registry`,
in `~/go/pkg/mod`, or in a Java jar. Ostra opens the definition as a read-only tab. When the user follows a link
from that tab, Ostra asks the same server. Thus navigation continues through the library and back. The browser
cannot name a random file this way. Ostra reads only the URIs that a server of that project returned after the
Ostra server started.

Some servers need options to give full answers. gopls colors only with `semanticTokens = true`. rust-analyzer
searches functions only with `workspace.symbol.search.kind = "all_symbols"`.

## Safety

The index and its helpers run code only when the user approved it:

- A `code_provider` or `language_servers` entry can arrive in a `workspace.toml` with a repository or a
  `git pull`. This entry does not start until the user approves that exact content (Rule A1). Settings shows
  the commands to approve.
- Language servers run in the same sandbox as agent commands, if the platform supports it.
- The `code_provider` of a project cannot point the browser at files outside the project. Ostra clears the
  `uri` field in its answers.

## Where to read next

- [Agents](agents.md): the `code` capability and the agents that have it.
- [Tools](tools.md): the other native tools, which agents use together with the code tools.
- [MCP servers](mcp.md): how harness runs get the code tools through Ostra's MCP server.
- [Settings and routing](settings-and-routing.md): where `code_provider` and `language_servers` go in a project entry.
- [Agent containment](../security/agent-containment.md): the sandbox that language servers run in.
