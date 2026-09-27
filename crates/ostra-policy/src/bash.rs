//! Shell command parsing with tree-sitter-bash: simple commands in execution order, their
//! redirects, heredoc bodies, and the paths a command would create, overwrite, move, or delete.
//!
//! Replaces the regex heuristics of Ultracode's `hooks/lib/shell-paths.js`, keeping its rules:
//! a heredoc body handed to a data sink is content, a body fed to a shell runs, `<ID>`
//! placeholders are not redirects, dot-only tokens are prose, and dynamic tokens are skipped.

use tree_sitter::{Node, Parser};

const MAX_DEPTH: usize = 4;

/// One shell word. `value` is the unquoted text when the word is static.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub raw: String,
    pub value: Option<String>,
    /// Contains an unquoted glob character.
    pub glob: bool,
}

impl Word {
    fn literal(text: &str) -> Self {
        Word {
            raw: text.to_string(),
            value: Some(text.to_string()),
            glob: false,
        }
    }

    /// The static value, or the raw source when the word is dynamic.
    pub fn text(&self) -> &str {
        self.value.as_deref().unwrap_or(&self.raw)
    }

    pub fn is_static(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub op: String,
    pub fd: Option<String>,
    pub target: Option<Word>,
}

impl Redirect {
    /// True when the redirect creates or overwrites a file.
    pub fn writes(&self) -> bool {
        match self.op.as_str() {
            ">" | ">>" | ">|" | "&>" | "&>>" | "<>" => true,
            ">&" => self
                .target
                .as_ref()
                .is_some_and(|t| !t.text().chars().all(|c| c.is_ascii_digit()) && t.text() != "-"),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleCommand {
    pub assignments: Vec<String>,
    /// Command name followed by its arguments.
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
    /// Heredoc body handed to this command, when it is data or interpreter code.
    pub heredoc_body: Option<String>,
    /// Source text of the command, redirects included, heredoc bodies excluded.
    pub text: String,
    /// Reads stdin from a pipe.
    pub piped_input: bool,
    /// Source text of the whole pipeline this command belongs to.
    pub pipeline_text: String,
}

impl SimpleCommand {
    /// Index of the word that actually runs, after assignments and wrappers.
    pub fn effective_index(&self) -> Option<usize> {
        effective_index(&self.words)
    }

    /// The command word that runs, without any directory part.
    pub fn effective_name(&self) -> Option<String> {
        self.effective_index()
            .map(|i| basename(self.words[i].text()).to_string())
    }

    /// Arguments after the effective command word.
    pub fn args(&self) -> &[Word] {
        match self.effective_index() {
            Some(i) => &self.words[i + 1..],
            None => &[],
        }
    }

    /// The command as a plain string of words, used for permission matching.
    pub fn words_text(&self) -> String {
        self.words
            .iter()
            .map(|w| w.text().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Words from the effective command on, wrappers dropped.
    pub fn effective_text(&self) -> String {
        match self.effective_index() {
            Some(i) => self.words[i..]
                .iter()
                .map(|w| w.text().to_string())
                .collect::<Vec<_>>()
                .join(" "),
            None => String::new(),
        }
    }

    pub fn is_dynamic_name(&self) -> bool {
        self.effective_index()
            .is_none_or(|i| !self.words[i].is_static())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub commands: Vec<SimpleCommand>,
    /// False when the parser hit a syntax error, so the split may be wrong.
    pub complete: bool,
    /// False when the source holds a statement the command list does not model (a standalone
    /// assignment, `export`, `[[ ]]`, a loop header, a function), whose effect on later commands
    /// or whose own evaluation is not checked.
    pub modelled: bool,
}

pub const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "mksh"];

pub const INTERPRETERS: &[&str] = &[
    "node",
    "nodejs",
    "bun",
    "deno",
    "ts-node",
    "tsx",
    "python",
    "python2",
    "python3",
    "perl",
    "ruby",
    "php",
    "Rscript",
    "osascript",
];

pub fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name)
}

/// Code interpreters, including versioned Python names such as `python3.12`.
pub fn is_interpreter(name: &str) -> bool {
    INTERPRETERS.contains(&name)
        || name.strip_prefix("python").is_some_and(|rest| {
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "nice", "ionice", "stdbuf", "time", "timeout", "command",
    "exec", "xargs", "strace", "watch", "builtin",
];

fn wrapper_value_flags(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "sudo" => &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"],
        "nice" => &["-n"],
        "ionice" => &["-c", "-n", "-p"],
        "timeout" => &["-s", "-k", "--signal", "--kill-after"],
        "watch" => &["-n", "-d"],
        "xargs" => &["-I", "-n", "-P", "-d", "-E", "-L", "-s", "-a"],
        "env" => &["-u", "-C", "-S"],
        _ => &[],
    }
}

pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_assignment(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if c == '=' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    false
}

/// Index of the word that runs, skipping `VAR=value` words and wrappers with their options.
pub fn effective_index(words: &[Word]) -> Option<usize> {
    let mut i = 0;
    while i < words.len() {
        let text = words[i].text();
        if is_assignment(text) {
            i += 1;
            continue;
        }
        let name = basename(text);
        if WRAPPERS.contains(&name) {
            let value_flags = wrapper_value_flags(name);
            i += 1;
            let mut took_duration = name != "timeout";
            while i < words.len() {
                let t = words[i].text();
                if t.starts_with('-') {
                    i += if value_flags.contains(&t) { 2 } else { 1 };
                } else if name == "env" && is_assignment(t) {
                    i += 1;
                } else if !took_duration {
                    took_duration = true;
                    i += 1;
                } else {
                    break;
                }
            }
            continue;
        }
        return Some(i);
    }
    None
}

fn unescape_double(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '"' | '\\' | '$' | '`' | '\n')
        {
            chars.next();
            if next != '\n' {
                out.push(next);
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn unescape_word(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// A glob or a brace expansion (`{a,b}`, `{1..3}`): the word names paths only the shell knows.
fn has_unquoted_glob(text: &str) -> bool {
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '*' | '?' | '[' => return true,
            '{' => {
                let inner = text[i + 1..].split('}').next().unwrap_or_default();
                if text[i + 1..].contains('}') && (inner.contains(',') || inner.contains("..")) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

struct Walker<'s> {
    src: &'s str,
    out: Vec<SimpleCommand>,
    complete: bool,
    modelled: bool,
    depth: usize,
}

/// Statement nodes that only sequence or group the commands they contain.
const STRUCTURAL: &[&str] = &[
    "program",
    "list",
    "subshell",
    "compound_statement",
    "if_statement",
    "elif_clause",
    "else_clause",
    "while_statement",
    "do_group",
    "negated_command",
];

impl<'s> Walker<'s> {
    fn text(&self, node: Node) -> &'s str {
        &self.src[node.byte_range()]
    }

    fn word(&self, node: Node) -> Word {
        let raw = self.text(node).to_string();
        let value = self.static_value(node);
        let glob = node.kind() == "word" && has_unquoted_glob(&raw)
            || node.kind() == "concatenation" && {
                let mut c = node.walk();
                let parts: Vec<Node> = node.named_children(&mut c).collect();
                // A brace expansion splits into several words (`{a`, `,`, `b}`), so the
                // unquoted parts are joined before looking for one.
                let unquoted: String = parts
                    .iter()
                    .map(|n| {
                        if n.kind() == "word" {
                            self.text(*n)
                        } else {
                            "\u{0}"
                        }
                    })
                    .collect();
                parts
                    .iter()
                    .any(|n| n.kind() == "word" && has_unquoted_glob(self.text(*n)))
                    || has_unquoted_glob(&unquoted)
            };
        Word { raw, value, glob }
    }

    fn static_value(&self, node: Node) -> Option<String> {
        match node.kind() {
            "word" => Some(unescape_word(self.text(node))),
            "number" => Some(self.text(node).to_string()),
            "raw_string" => {
                let t = self.text(node);
                Some(
                    t.strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                        .unwrap_or(t)
                        .to_string(),
                )
            }
            "string" => {
                let mut c = node.walk();
                let mut value = String::new();
                for child in node.named_children(&mut c) {
                    if child.kind() != "string_content" {
                        return None;
                    }
                    value.push_str(&unescape_double(self.text(child)));
                }
                Some(value)
            }
            "concatenation" => {
                let mut c = node.walk();
                let mut value = String::new();
                for child in node.named_children(&mut c) {
                    value.push_str(&self.static_value(child)?);
                }
                Some(value)
            }
            "command_name" => {
                let mut c = node.walk();
                let child = node.named_children(&mut c).next()?;
                self.static_value(child)
            }
            _ => None,
        }
    }

    fn redirect(&mut self, node: Node) -> Option<Redirect> {
        if node.kind() != "file_redirect" {
            return None;
        }
        let mut op = String::new();
        let mut fd = None;
        let mut target = None;
        let mut c = node.walk();
        for (i, child) in node.children(&mut c).enumerate() {
            let field = node.field_name_for_child(i as u32);
            if field == Some("descriptor") {
                fd = Some(self.text(child).to_string());
            } else if field == Some("destination") {
                if target.is_none() {
                    target = Some(self.word(child));
                }
            } else if !child.is_named() {
                op.push_str(self.text(child));
            }
        }
        Some(Redirect { op, fd, target })
    }

    fn visit_children(&mut self, node: Node) {
        let mut c = node.walk();
        let children: Vec<Node> = node.named_children(&mut c).collect();
        for child in children {
            self.visit(child);
        }
    }

    /// Commands nested in words (command and process substitutions) run too.
    fn visit_substitutions(&mut self, node: Node) {
        match node.kind() {
            "command_substitution" | "process_substitution" => self.visit_children(node),
            _ => {
                let mut c = node.walk();
                let children: Vec<Node> = node.named_children(&mut c).collect();
                for child in children {
                    self.visit_substitutions(child);
                }
            }
        }
    }

    fn command(&mut self, node: Node) {
        let mut cmd = SimpleCommand {
            text: self.text(node).to_string(),
            ..Default::default()
        };
        let mut nested = vec![];
        let mut c = node.walk();
        let children: Vec<(Option<&str>, Node)> = node
            .children(&mut c)
            .enumerate()
            .map(|(i, n)| (node.field_name_for_child(i as u32), n))
            .collect();
        for (field, child) in children {
            match (field, child.kind()) {
                (_, "variable_assignment") => {
                    cmd.assignments.push(self.text(child).to_string());
                    nested.push(child);
                }
                (Some("name"), _) | (Some("argument"), _) => {
                    cmd.words.push(self.word(child));
                    nested.push(child);
                }
                (Some("redirect"), "file_redirect") => {
                    if let Some(r) = self.redirect(child) {
                        cmd.redirects.push(r);
                    }
                    nested.push(child);
                }
                _ => {}
            }
        }
        let inner = self.inline_shell_code(&cmd);
        self.out.push(cmd);
        for n in nested {
            self.visit_substitutions(n);
        }
        if let Some(code) = inner {
            self.nested_parse(&code);
        }
    }

    /// `bash -c "<code>"` and `eval <code>` run their argument as shell.
    fn inline_shell_code(&self, cmd: &SimpleCommand) -> Option<String> {
        let idx = cmd.effective_index()?;
        let name = basename(cmd.words[idx].text());
        let args = &cmd.words[idx + 1..];
        if is_shell(name) {
            let pos = args.iter().position(|w| {
                let t = w.text();
                t.starts_with('-') && !t.starts_with("--") && t.contains('c')
            })?;
            return args.get(pos + 1).and_then(|w| w.value.clone());
        }
        if name == "eval" {
            let parts: Option<Vec<String>> = args.iter().map(|w| w.value.clone()).collect();
            return parts.map(|p| p.join(" "));
        }
        None
    }

    fn nested_parse(&mut self, code: &str) {
        if self.depth >= MAX_DEPTH {
            self.complete = false;
            return;
        }
        let inner = parse_depth(code, self.depth + 1);
        self.complete &= inner.complete;
        self.modelled &= inner.modelled;
        self.out.extend(inner.commands);
    }

    fn redirected_statement(&mut self, node: Node) {
        let before = self.out.len();
        let mut c = node.walk();
        let children: Vec<(Option<&str>, Node)> = node
            .children(&mut c)
            .enumerate()
            .map(|(i, n)| (node.field_name_for_child(i as u32), n))
            .collect();
        let mut redirects = vec![];
        let mut heredocs = vec![];
        for (field, child) in &children {
            match (*field, child.kind()) {
                (Some("body"), _) => self.visit(*child),
                (_, "file_redirect") => {
                    if let Some(r) = self.redirect(*child) {
                        redirects.push(r);
                    }
                    self.visit_substitutions(*child);
                }
                (_, "heredoc_redirect") => heredocs.push(*child),
                (_, "herestring_redirect") => self.visit_substitutions(*child),
                _ => {}
            }
        }
        let body_end = self.out.len();
        if body_end > before {
            let last = &mut self.out[body_end - 1];
            last.redirects.extend(redirects);
            last.text = self.src[node.byte_range()]
                .split('\n')
                .next()
                .unwrap_or_default()
                .to_string();
        }
        for h in heredocs {
            self.heredoc(node, h, before, body_end);
        }
    }

    fn heredoc(&mut self, stmt: Node, h: Node, before: usize, body_end: usize) {
        let mut body = String::new();
        let mut piped = None;
        let mut c = h.walk();
        let children: Vec<Node> = h.children(&mut c).collect();
        for child in children {
            match child.kind() {
                "heredoc_body" => body = self.text(child).to_string(),
                "pipeline" => piped = Some(child),
                "file_redirect" => {
                    if let Some(r) = self.redirect(child)
                        && body_end > before
                    {
                        self.out[body_end - 1].redirects.push(r);
                    }
                }
                _ => {}
            }
        }
        let owner = (body_end > before).then(|| body_end - 1);
        let piped_start = self.out.len();
        if let Some(p) = piped {
            // The continuation after `<<EOF |` is a pipeline whose first command reads our output.
            let mut pc = p.walk();
            let stages: Vec<Node> = p.named_children(&mut pc).collect();
            for stage in stages {
                let start = self.out.len();
                self.visit(stage);
                if let Some(first) = self.out.get_mut(start) {
                    first.piped_input = true;
                }
            }
            let text = self.text(stmt).to_string();
            for cmd in &mut self.out[piped_start..] {
                cmd.pipeline_text = text.clone();
            }
            if let Some(o) = owner {
                self.out[o].pipeline_text = text;
            }
        }
        let runs_as_shell = owner
            .into_iter()
            .chain(piped_start..self.out.len())
            .any(|i| self.out[i].effective_name().is_some_and(|n| is_shell(&n)));
        if runs_as_shell {
            self.nested_parse(&body);
            return;
        }
        let interpreter = owner
            .into_iter()
            .chain(piped_start..self.out.len())
            .find(|&i| {
                self.out[i]
                    .effective_name()
                    .is_some_and(|n| is_interpreter(&n))
            });
        if let Some(i) = interpreter.or(owner) {
            self.out[i].heredoc_body = Some(body);
        }
    }

    fn pipeline(&mut self, node: Node) {
        let start_all = self.out.len();
        let mut c = node.walk();
        let stages: Vec<Node> = node.named_children(&mut c).collect();
        for (i, stage) in stages.into_iter().enumerate() {
            let start = self.out.len();
            self.visit(stage);
            if i > 0
                && let Some(first) = self.out.get_mut(start)
            {
                first.piped_input = true;
            }
        }
        let text = self.text(node).to_string();
        for cmd in &mut self.out[start_all..] {
            if cmd.pipeline_text.is_empty() {
                cmd.pipeline_text = text.clone();
            }
        }
    }

    fn visit(&mut self, node: Node) {
        if node.is_error() || node.is_missing() {
            self.complete = false;
        }
        match node.kind() {
            "command" => self.command(node),
            "redirected_statement" => self.redirected_statement(node),
            "pipeline" => self.pipeline(node),
            "heredoc_redirect" => {
                let n = self.out.len();
                self.heredoc(node, node, n, n);
            }
            "comment" => {}
            // `[ -f x ]` and `[[ -d src ]]` over plain words only: an expansion or a subscript
            // in a test is evaluated, and can run a command.
            "test_command"
                if !self
                    .text(node)
                    .trim_matches(['[', ']', ' '])
                    .contains(['$', '`', '[', ']', '(']) => {}
            kind => {
                if node.is_named() && !STRUCTURAL.contains(&kind) {
                    self.modelled = false;
                }
                self.visit_children(node)
            }
        }
    }
}

fn parse_depth(src: &str, depth: usize) -> Parsed {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return Parsed {
            commands: vec![],
            complete: false,
            modelled: false,
        };
    }
    let Some(tree) = parser.parse(src, None) else {
        return Parsed {
            commands: vec![],
            complete: false,
            modelled: false,
        };
    };
    let root = tree.root_node();
    let mut w = Walker {
        src,
        out: vec![],
        complete: !root.has_error(),
        modelled: true,
        depth,
    };
    w.visit(root);
    for cmd in &mut w.out {
        if cmd.pipeline_text.is_empty() {
            cmd.pipeline_text = cmd.text.clone();
        }
    }
    Parsed {
        commands: w.out,
        complete: w.complete,
        modelled: w.modelled,
    }
}

/// Parse a shell command into simple commands in execution order. Commands inside substitutions,
/// `bash -c` strings, `eval`, and heredoc bodies fed to a shell are included.
pub fn parse(src: &str) -> Parsed {
    parse_depth(src, 0)
}

/// How a write target was named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
    /// A path the command writes.
    Path(String),
    /// `cp src dest`: `dest/src_name` when `dest` is a directory, else `dest` itself.
    CopyInto {
        dest: String,
        src_name: String,
        force_dir: bool,
    },
    /// A git command that changes the working tree at this directory (empty means the cwd).
    GitTree(Option<String>),
}

/// A write target with the working directory it resolves against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteTarget {
    pub spec: TargetSpec,
    /// `cd` target in effect, relative to the starting cwd when not absolute. `None` means the
    /// starting cwd; `Some("")` never occurs.
    pub cwd: Option<String>,
    /// Index into `Parsed::commands`.
    pub command: usize,
    /// The command word or redirect operator that writes.
    pub via: String,
}

enum Cand {
    Skip,
    Path(String),
    /// A variable, substitution, or glob: the path is only known when the shell runs.
    Unresolved,
}

fn candidate(word: &Word) -> Option<String> {
    match classify(word) {
        Cand::Path(p) => Some(p),
        _ => None,
    }
}

fn classify(word: &Word) -> Cand {
    let Some(value) = word.value.as_ref() else {
        return Cand::Unresolved;
    };
    if word.glob {
        return Cand::Unresolved;
    }
    let v = value.trim();
    if v.is_empty()
        || matches!(
            v,
            "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "-" | "&1" | "&2"
        )
    {
        return Cand::Skip;
    }
    // A dot-only token is prose (`--> ...`), not a path.
    if v.chars().all(|c| c == '.') {
        return Cand::Skip;
    }
    // `~user/...` expands to another account's home, which the resolver does not model.
    if v.starts_with('~') && !(v == "~" || v.starts_with("~/")) {
        return Cand::Unresolved;
    }
    Cand::Path(v.to_string())
}

const GIT_WRITE_SUBCOMMANDS: &[&str] = &[
    "checkout",
    "switch",
    "restore",
    "reset",
    "apply",
    "am",
    "stash",
    "clean",
    "rm",
    "mv",
    "merge",
    "rebase",
    "pull",
    "cherry-pick",
    "revert",
    "commit",
    "add",
];

/// Git global options that take a value.
fn git_value_opt(t: &str) -> bool {
    matches!(t, "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace")
}

/// Git subcommand and its `-C` directory, if any.
pub fn git_subcommand(args: &[Word]) -> (Option<String>, Option<String>) {
    let mut dir = None;
    let mut i = 0;
    while i < args.len() {
        let t = args[i].text();
        if git_value_opt(t) {
            if t == "-C" {
                dir = args.get(i + 1).and_then(|w| w.value.clone());
            }
            i += 2;
            continue;
        }
        if t.starts_with('-') {
            i += 1;
            continue;
        }
        return (Some(t.to_string()), dir);
    }
    (None, dir)
}

/// A `-C`, `--git-dir`, or `--work-tree` value that is only known when the shell runs.
pub fn git_dir_dynamic(args: &[Word]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let t = args[i].text();
        if git_value_opt(t) {
            if t != "-c" && args.get(i + 1).is_some_and(|w| !w.is_static()) {
                return true;
            }
            i += 2;
            continue;
        }
        if !t.starts_with('-') {
            return false;
        }
        i += 1;
    }
    false
}

fn non_flag(args: &[Word]) -> impl Iterator<Item = &Word> {
    args.iter()
        .filter(|w| !w.text().starts_with('-') || w.text() == "-")
}

fn targets_of(cmd: &SimpleCommand, unresolved: &mut bool) -> Vec<(TargetSpec, String)> {
    let mut out = vec![];
    for r in &cmd.redirects {
        if !r.writes() {
            continue;
        }
        match r.target.as_ref().map(classify) {
            Some(Cand::Path(t)) => out.push((TargetSpec::Path(t), r.op.clone())),
            Some(Cand::Unresolved) | None => *unresolved = true,
            Some(Cand::Skip) => {}
        }
    }
    let Some(name) = cmd.effective_name() else {
        return out;
    };
    let args = cmd.args();
    let via = name.clone();
    let push_all = |out: &mut Vec<(TargetSpec, String)>,
                    unresolved: &mut bool,
                    words: &mut dyn Iterator<Item = &Word>| {
        for w in words {
            match classify(w) {
                Cand::Path(t) => out.push((TargetSpec::Path(t), via.clone())),
                Cand::Unresolved => *unresolved = true,
                Cand::Skip => {}
            }
        }
    };
    match name.as_str() {
        "tee" | "rm" | "rmdir" | "shred" | "truncate" | "touch" | "mkdir" | "unlink" => {
            push_all(&mut out, unresolved, &mut non_flag(args));
        }
        "cp" | "install" | "ln" | "mv" => {
            let mut target_dir = None;
            let mut rest = vec![];
            let mut i = 0;
            while i < args.len() {
                let t = args[i].text();
                // Every spelling of the target directory: `-t D`, `-tD`, `--target-directory D`,
                // `--target-directory=D`, and its unique abbreviations.
                let long = t.split_once('=').map_or(t, |(n, _)| n);
                let is_long = long.len() > 3 && "--target-directory".starts_with(long);
                if t == "-t" || (is_long && !t.contains('=')) {
                    target_dir = args.get(i + 1).cloned();
                    i += 2;
                    continue;
                }
                if is_long {
                    let d = t.split_once('=').map(|(_, v)| v).unwrap_or_default();
                    target_dir = Some(if args[i].is_static() {
                        Word::literal(d)
                    } else {
                        args[i].clone()
                    });
                } else if let Some(d) = t
                    .strip_prefix("-t")
                    .filter(|d| !d.is_empty() && !t.starts_with("--"))
                {
                    target_dir = Some(if args[i].is_static() {
                        Word::literal(d)
                    } else {
                        args[i].clone()
                    });
                } else if !t.starts_with('-') {
                    rest.push(&args[i]);
                }
                i += 1;
            }
            let (dest, sources): (Option<&Word>, Vec<&Word>) = match &target_dir {
                Some(d) => (Some(d), rest),
                None => match rest.split_last() {
                    Some((d, s)) => (Some(*d), s.to_vec()),
                    None => (None, vec![]),
                },
            };
            if dest.is_some_and(|d| matches!(classify(d), Cand::Unresolved)) {
                *unresolved = true;
            }
            // A move also removes each source.
            if name == "mv" {
                push_all(&mut out, unresolved, &mut sources.iter().copied());
            }
            if let Some(dest) = dest.and_then(candidate) {
                if sources.is_empty() {
                    out.push((TargetSpec::Path(dest.clone()), via.clone()));
                }
                let force_dir = target_dir.is_some() || sources.len() > 1 || dest.ends_with('/');
                for s in sources {
                    let src_name = basename(s.text().trim_end_matches('/')).to_string();
                    out.push((
                        TargetSpec::CopyInto {
                            dest: dest.clone(),
                            src_name,
                            force_dir,
                        },
                        via.clone(),
                    ));
                }
            }
        }
        "sed" | "perl" => {
            let in_place = args.iter().any(|w| {
                let t = w.text();
                t == "--in-place"
                    || t.starts_with("--in-place=")
                    || (t.starts_with('-') && !t.starts_with("--") && t.contains('i'))
            });
            if in_place {
                let mut i = 0;
                let mut script_given = false;
                let mut files = vec![];
                while i < args.len() {
                    let t = args[i].text();
                    if matches!(t, "-e" | "-f" | "--expression" | "--file" | "-E")
                        && name == "sed"
                        && t != "-E"
                    {
                        script_given = true;
                        i += 2;
                        continue;
                    }
                    if name == "perl" && (t == "-e" || t == "-E") {
                        script_given = true;
                        i += 2;
                        continue;
                    }
                    if name == "perl"
                        && t.starts_with('-')
                        && (t.ends_with('e') || t.ends_with('E'))
                        && !t.starts_with("--")
                    {
                        script_given = true;
                        i += 2;
                        continue;
                    }
                    if !t.starts_with('-') {
                        files.push(&args[i]);
                    }
                    i += 1;
                }
                let skip = usize::from(!script_given && name == "sed");
                push_all(&mut out, unresolved, &mut files.into_iter().skip(skip));
            }
        }
        "dd" => {
            for w in args {
                if w.value.is_none() && w.raw.contains("of=") {
                    *unresolved = true;
                }
                if let Some(of) = w.value.as_deref().and_then(|v| v.strip_prefix("of=")) {
                    let w = Word::literal(of);
                    if let Some(t) = candidate(&w) {
                        out.push((TargetSpec::Path(t), via.clone()));
                    }
                }
            }
        }
        "git" => {
            let (sub, dir) = git_subcommand(args);
            if git_dir_dynamic(args) {
                *unresolved = true;
            }
            if sub
                .as_deref()
                .is_some_and(|s| GIT_WRITE_SUBCOMMANDS.contains(&s))
            {
                out.push((
                    TargetSpec::GitTree(dir),
                    format!("git {}", sub.unwrap_or_default()),
                ));
            }
        }
        _ => {}
    }
    out
}

/// Every path the command would create, overwrite, move, or delete, with the `cd` in effect.
pub fn write_targets(parsed: &Parsed) -> Vec<WriteTarget> {
    let cwds = command_cwds(parsed);
    let mut out = vec![];
    for (i, cmd) in parsed.commands.iter().enumerate() {
        for (spec, via) in targets_of(cmd, &mut false) {
            out.push(WriteTarget {
                spec,
                cwd: cwds[i].clone(),
                command: i,
                via,
            });
        }
    }
    out
}

/// Indexes of commands that write somewhere the parser cannot name: a variable, substitution, or
/// glob in a write position, or a relative write after a `cd` whose target is only known at run
/// time. Such a write is never auto-allowed, because its real target was not checked.
pub fn unresolved_writes(parsed: &Parsed) -> Vec<usize> {
    let unknown = cwd_unknown(parsed);
    let mut out = vec![];
    for (i, cmd) in parsed.commands.iter().enumerate() {
        let mut unresolved = false;
        let targets = targets_of(cmd, &mut unresolved);
        let relative = targets.iter().any(|(spec, _)| match spec {
            TargetSpec::Path(p) | TargetSpec::CopyInto { dest: p, .. } => {
                !p.starts_with('/') && !p.starts_with('~')
            }
            TargetSpec::GitTree(d) => d.as_deref().is_none_or(|d| !d.starts_with('/')),
        });
        if unresolved || (unknown[i] && relative) {
            out.push(i);
        }
    }
    out
}

/// True for each command that runs after a `cd` or `pushd` whose target is dynamic, or after
/// `popd`, whose target the parser does not track.
pub fn cwd_unknown(parsed: &Parsed) -> Vec<bool> {
    let mut out = Vec::with_capacity(parsed.commands.len());
    let mut unknown = false;
    for cmd in &parsed.commands {
        out.push(unknown);
        match cmd.effective_name().as_deref() {
            Some("cd") | Some("pushd") => match non_flag(cmd.args()).next() {
                Some(w) if !w.is_static() => unknown = true,
                Some(w) if w.text().starts_with('/') => unknown = false,
                _ => {}
            },
            Some("popd") => unknown = true,
            _ => {}
        }
    }
    out
}

/// The `cd` target in effect when each command runs. `None` is the starting cwd; relative
/// values are relative to it, and `~` means home. An unknown (dynamic) `cd` keeps the last
/// known value. Subshell scoping is not modelled.
pub fn command_cwds(parsed: &Parsed) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(parsed.commands.len());
    let mut cwd: Option<String> = None;
    for cmd in &parsed.commands {
        out.push(cwd.clone());
        if matches!(cmd.effective_name().as_deref(), Some("cd") | Some("pushd")) {
            let arg = non_flag(cmd.args()).next();
            cwd = match arg {
                None => Some("~".into()),
                Some(w) => match &w.value {
                    Some(v) => Some(match &cwd {
                        Some(prev) if !v.starts_with('/') && !v.starts_with('~') => {
                            format!("{prev}/{v}")
                        }
                        _ => v.clone(),
                    }),
                    None => cwd.clone(),
                },
            };
        }
    }
    out
}

/// Convenience for tests and callers that only need raw target strings.
pub fn extract_write_targets(command: &str) -> Vec<String> {
    write_targets(&parse(command))
        .into_iter()
        .map(|t| match t.spec {
            TargetSpec::Path(p) => p,
            TargetSpec::CopyInto {
                dest,
                src_name,
                force_dir,
            } => {
                if force_dir {
                    format!("{}/{}", dest.trim_end_matches('/'), src_name)
                } else {
                    dest
                }
            }
            TargetSpec::GitTree(dir) => dir.unwrap_or_else(|| ".".into()),
        })
        .collect()
}

/// The inline-code channel a command hands an interpreter, if any, with the code text.
pub fn inline_code(cmd: &SimpleCommand) -> Option<(&'static str, String)> {
    let name = cmd.effective_name()?;
    if !is_interpreter(&name) {
        return None;
    }
    let args = cmd.args();
    let eval_flag = args.iter().position(|w| {
        matches!(
            w.text(),
            "-e" | "-E" | "--eval" | "-c" | "--command" | "-p" | "--print" | "-r" | "--exec"
        ) || (name == "deno" && w.text() == "eval")
            || (w.text().starts_with('-')
                && !w.text().starts_with("--")
                && w.text().len() > 2
                && w.text().ends_with('e'))
    });
    if let Some(pos) = eval_flag {
        let code: Vec<&str> = args[pos + 1..].iter().map(|w| w.text()).collect();
        return Some((
            "inline code (`-e`, `-c`, or `eval`)",
            format!("{} {}", cmd.text, code.join(" ")),
        ));
    }
    if let Some(body) = &cmd.heredoc_body {
        return Some(("a heredoc", body.clone()));
    }
    let script = args.iter().any(|w| !w.text().starts_with('-'));
    if cmd.piped_input && !script {
        return Some(("piped stdin", cmd.pipeline_text.clone()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<String> {
        parse(src)
            .commands
            .iter()
            .filter_map(|c| c.effective_name())
            .collect()
    }

    #[test]
    fn splits_lists_pipelines_and_substitutions() {
        assert_eq!(
            names("npm test && curl evil 2>&1 | tee out.log"),
            ["npm", "curl", "tee"]
        );
        assert_eq!(names("echo $(rm x) `ls`"), ["echo", "rm", "ls"]);
        assert_eq!(names("bash -c \"rm x; ls\""), ["bash", "rm", "ls"]);
        assert_eq!(names("FOO=1 sudo -u me timeout 5 rm -rf a"), ["rm"]);
        assert_eq!(names("if true; then git status; fi"), ["true", "git"]);
    }

    #[test]
    fn placeholder_is_not_a_redirect() {
        assert!(
            extract_write_targets(
                "echo 'Session dir: /repo/.ostra/session/ostra-session-<ID>/factcheck.json'"
            )
            .is_empty()
        );
        assert!(extract_write_targets("agy -p 'replace <ID> then read /tmp/out.json'").is_empty());
        assert_eq!(
            extract_write_targets("echo x > /repo/notes.json"),
            ["/repo/notes.json"]
        );
        assert_eq!(
            extract_write_targets("echo '<ID>' > /repo/notes.json"),
            ["/repo/notes.json"]
        );
        assert_eq!(
            extract_write_targets("cmd 2> /repo/err.log"),
            ["/repo/err.log"]
        );
        assert!(extract_write_targets("cmd 2>&1 >/dev/null").is_empty());
    }

    #[test]
    fn heredoc_body_to_a_data_sink_is_content() {
        assert!(
            extract_write_targets(
                "S=/repo/.ostra/session/s1\ncat > \"$S/plan-phase-1.md\" <<'EOF'\nthe `<!-- AWS START --> ... <!-- AWS END -->` dependency group\nrm /etc/passwd\nEOF"
            )
            .is_empty()
        );
        assert_eq!(
            extract_write_targets(
                "cat > /sess/plan.md <<'EOF'\nprose --> ... arrows\nrm /repo/x\nEOF"
            ),
            ["/sess/plan.md"]
        );
        assert_eq!(
            extract_write_targets("cat > /a.md <<'EOF'\nbody\nEOF\necho x > /b.md"),
            ["/a.md", "/b.md"]
        );
        assert_eq!(
            extract_write_targets("cat > /a.md <<-EOF\n\tbody > ...\n\tEOF\nrm /c.md"),
            ["/a.md", "/c.md"]
        );
    }

    #[test]
    fn heredoc_body_fed_to_a_shell_runs() {
        assert_eq!(
            extract_write_targets("bash <<'EOF'\nrm /repo/src/App.ts\nEOF"),
            ["/repo/src/App.ts"]
        );
        assert_eq!(
            extract_write_targets("cat <<'EOF' | bash\necho x > /repo/hacked.ts\nEOF"),
            ["/repo/hacked.ts"]
        );
    }

    #[test]
    fn herestring_and_dots_are_not_targets() {
        assert_eq!(
            extract_write_targets("grep x <<< 'EOF'\nrm /d.md"),
            ["/d.md"]
        );
        assert!(extract_write_targets("echo 'a --> ... done'").is_empty());
        assert!(extract_write_targets("echo a > ...").is_empty());
    }

    #[test]
    fn destructive_commands_and_dynamic_tokens() {
        assert_eq!(extract_write_targets("rm -rf a b"), ["a", "b"]);
        assert_eq!(extract_write_targets("mv a b"), ["a", "b"]);
        assert_eq!(extract_write_targets("cp a b"), ["b"]);
        assert_eq!(extract_write_targets("cp a b dir"), ["dir/a", "dir/b"]);
        assert_eq!(extract_write_targets("cp a dir/"), ["dir/a"]);
        assert_eq!(extract_write_targets("tee -a x.log"), ["x.log"]);
        assert_eq!(extract_write_targets("sed -i 's/a/b/' f.txt"), ["f.txt"]);
        assert_eq!(
            extract_write_targets("sed -i -e 's/a/b/' f.txt g.txt"),
            ["f.txt", "g.txt"]
        );
        assert!(extract_write_targets("sed -n '1,20p' f.txt").is_empty());
        assert_eq!(
            extract_write_targets("dd if=/dev/zero of=disk.img"),
            ["disk.img"]
        );
        assert!(extract_write_targets("rm \"$X/file\" *.log").is_empty());
        assert_eq!(extract_write_targets("git stash"), ["."]);
        assert_eq!(extract_write_targets("git -C sub checkout -- a"), ["sub"]);
        assert!(extract_write_targets("git status").is_empty());
    }

    #[test]
    fn cd_is_tracked() {
        let t = write_targets(&parse("cd sub && echo x >> y"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].cwd.as_deref(), Some("sub"));
        assert_eq!(t[0].spec, TargetSpec::Path("y".into()));
    }

    #[test]
    fn interpreter_channels() {
        let p = parse("node -e \"require('fs').writeFileSync('a','b')\"");
        assert!(inline_code(&p.commands[0]).is_some());
        let p = parse("python3 - <<'PY'\nopen('out.json','w').write('{}')\nPY");
        let (ch, code) = inline_code(&p.commands[0]).unwrap();
        assert_eq!(ch, "a heredoc");
        assert!(code.contains("open('out.json'"));
        let p = parse("echo 'print(1)' | python3");
        assert!(inline_code(&p.commands[1]).is_some());
        let p = parse("python3 script.py");
        assert!(inline_code(&p.commands[0]).is_none());
        let p = parse("cat <<'EOF' | python3\nprint(1)\nEOF");
        assert_eq!(inline_code(&p.commands[1]).unwrap().0, "a heredoc");
    }
}
