//! One execution's policy: layer 1 guards, then layer 2 permissions, plus the post-tool observer
//! that drives the build streak and the lesson gate.

use crate::bash::{self, Parsed, SimpleCommand};
use crate::build::{self, Streak};
use crate::guards::{self, Denial, Roots};
use crate::perms::{self, Family, Rule, Subject};
use ostra_core::config::PermissionMode;
use ostra_core::exec::ExecContext;
use ostra_core::policy::{PolicyDecision, RuleRef, ToolCall, ToolOutcome};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Project commands the build streak counts, from `project.toml`.
#[derive(Debug, Clone, Default)]
pub struct PolicyInputs {
    pub build_commands: Vec<String>,
    pub test_commands: Vec<String>,
}

/// Something the caller must do after a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// Append this text to the tool result the model sees.
    AppendNote(String),
    /// Run memory recall with this query and append any lessons found.
    RecallLessons { query: String },
}

#[derive(Debug, Default)]
struct State {
    yolo: bool,
    session_allow: Vec<Rule>,
    streak: Streak,
}

pub struct ExecutionPolicy {
    ctx: ExecContext,
    inputs: PolicyInputs,
    roots: Roots,
    allow: Vec<Rule>,
    ask: Vec<Rule>,
    deny: Vec<Rule>,
    state: Mutex<State>,
}

fn parse_rules(list: &[String]) -> Vec<Rule> {
    list.iter()
        .filter_map(|r| match perms::parse_rule(r) {
            Ok(rule) => Some(rule),
            Err(e) => {
                tracing::warn!(rule = %r, error = %e, "ignoring invalid permission rule");
                None
            }
        })
        .collect()
}

fn from_denial(d: Denial) -> PolicyDecision {
    PolicyDecision::Deny {
        reason: d.reason,
        rule: d.rule,
    }
}

const READ_ONLY: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "grep", "rg", "find", "git", "pwd", "echo", "printf",
    "sort", "uniq", "diff", "stat", "file", "which", "tree", "cd", "true", "test", "[", "basename",
    "dirname", "realpath", "readlink",
];

const GIT_READ: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
];

/// Commands safe to run without asking, in every mode.
fn is_read_only(cmd: &SimpleCommand) -> bool {
    let Some(name) = cmd.effective_name() else {
        return false;
    };
    if !READ_ONLY.contains(&name.as_str()) || cmd.is_dynamic_name() {
        return false;
    }
    let args = cmd.args();
    let has = |f: &str| {
        args.iter()
            .any(|w| w.text() == f || w.text().starts_with(&format!("{f}=")))
    };
    match name.as_str() {
        "find" => ![
            "-exec", "-execdir", "-delete", "-fprint", "-fprint0", "-fprintf", "-fls", "-ok",
            "-okdir",
        ]
        .iter()
        .any(|f| has(f)),
        "git" => {
            let (sub, _) = bash::git_subcommand(args);
            sub.is_some_and(|s| GIT_READ.contains(&s.as_str())) && !has("--output")
        }
        "sort" => !has("-o") && !has("--output"),
        "uniq" => args.iter().filter(|w| !w.text().starts_with('-')).count() <= 1,
        "tree" => !has("-o"),
        _ => true,
    }
}

impl ExecutionPolicy {
    pub fn new(ctx: ExecContext, inputs: PolicyInputs) -> Self {
        let roots = Roots::new(&ctx);
        let allow = parse_rules(&ctx.permissions.allow);
        let ask = parse_rules(&ctx.permissions.ask);
        let deny = parse_rules(&ctx.permissions.deny);
        let state = State {
            yolo: ctx.yolo,
            ..Default::default()
        };
        ExecutionPolicy {
            ctx,
            inputs,
            roots,
            allow,
            ask,
            deny,
            state: Mutex::new(state),
        }
    }

    pub fn ctx(&self) -> &ExecContext {
        &self.ctx
    }

    pub fn set_yolo(&self, on: bool) {
        self.lock().yolo = on;
    }

    pub fn yolo(&self) -> bool {
        self.lock().yolo
    }

    pub fn add_session_allow(&self, rule: String) {
        match perms::parse_rule(&rule) {
            Ok(r) => self.lock().session_allow.push(r),
            Err(e) => {
                tracing::warn!(rule = %rule, error = %e, "ignoring invalid session allow rule")
            }
        }
    }

    /// Consecutive build or test failures counted so far in this execution.
    pub fn build_streak(&self) -> u32 {
        self.lock().streak.consecutive
    }

    /// True while a verified recovery has no recorded lesson.
    pub fn lesson_pending(&self) -> bool {
        !self.lock().streak.pending.is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn build_commands(&self) -> Vec<String> {
        self.inputs
            .build_commands
            .iter()
            .chain(self.inputs.test_commands.iter())
            .cloned()
            .collect()
    }

    fn start_cwd(&self, call: &ToolCall) -> PathBuf {
        match call.str_field("cwd") {
            Some(c) if Path::new(c).is_absolute() => self.roots.resolve(&self.roots.repo, c),
            _ => self.roots.repo.clone(),
        }
    }

    fn file_path(&self, call: &ToolCall) -> Option<(PathBuf, String)> {
        let raw = call
            .str_field("file_path")
            .or_else(|| call.str_field("path"))?;
        Some((self.roots.resolve(&self.roots.repo, raw), raw.to_string()))
    }

    fn write_paths(&self, call: &ToolCall) -> Vec<(PathBuf, String)> {
        match call.tool.as_str() {
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                self.file_path(call).into_iter().collect()
            }
            "ApplyPatch" => {
                let patch = call
                    .str_field("patch")
                    .or_else(|| call.str_field("input"))
                    .unwrap_or_default();
                let base = self.start_cwd(call);
                crate::apply_patch_paths(patch)
                    .into_iter()
                    .map(|p| (self.roots.resolve(&base, &p), p))
                    .collect()
            }
            _ => vec![],
        }
    }

    /// Layer 1 then layer 2. Under YOLO every ask becomes an allow; denials stand.
    pub fn check(&self, call: &ToolCall) -> PolicyDecision {
        let parsed = (call.tool == "Bash")
            .then(|| bash::parse(call.str_field("command").unwrap_or_default()));
        if let Some(d) = self.layer1(call, parsed.as_ref()) {
            return from_denial(d);
        }
        let decision = self.layer2(call, parsed.as_ref());
        if !self.yolo() {
            return decision;
        }
        match decision {
            PolicyDecision::Ask { .. } => PolicyDecision::Allow {
                rule: Some(RuleRef::permission("yolo")),
            },
            PolicyDecision::Allow { rule: Some(r) } if r.rule == "mode:bypass" => {
                PolicyDecision::Allow {
                    rule: Some(RuleRef::permission("yolo")),
                }
            }
            d => d,
        }
    }

    fn layer1(&self, call: &ToolCall, parsed: Option<&Parsed>) -> Option<Denial> {
        let state = self.lock();
        let pending = state.streak.pending.first().cloned();
        let blocked = state.streak.blocked();
        let streak = state.streak.consecutive;
        let last_sig = state.streak.last_signature.clone();
        drop(state);

        let tool = call.tool.as_str();
        if tool == "Report" {
            let reason = call.str_field("reason").map(str::trim).unwrap_or_default();
            if let Some(rec) = &pending
                && reason.is_empty()
            {
                return Some(guards::lesson_denial(rec, true));
            }
            return None;
        }
        if tool.starts_with("submit_") {
            let status = call.str_field("status").unwrap_or("ok");
            if status == "ok"
                && let Some(rec) = &pending
            {
                return Some(guards::lesson_denial(rec, false));
            }
            return None;
        }
        for (path, raw) in self.write_paths(call) {
            if let Some(d) =
                guards::check_write(&self.ctx, &self.roots, &path, &raw, pending.as_ref())
            {
                return Some(d);
            }
        }
        if let Some(parsed) = parsed {
            let start = self.start_cwd(call);
            if let Some(d) = guards::check_shell(&self.roots, parsed, &start) {
                return Some(d);
            }
            for t in guards::shell_targets(&self.roots, parsed, &start) {
                if let Some(d) =
                    guards::check_write(&self.ctx, &self.roots, &t.path, &t.raw, pending.as_ref())
                {
                    return Some(d);
                }
            }
            // Build streak gate (build-streak-gate.js): only the build loop itself is refused.
            let command = call.str_field("command").unwrap_or_default();
            if blocked && build::is_build_command(command, &self.build_commands()) {
                let submit = self.ctx.agent.submit_tool_name();
                let diag = last_sig
                    .map(|s| format!(", last diagnostic: \"{s}\""))
                    .unwrap_or_default();
                return Some(Denial {
                    rule: RuleRef::guard(guards::BUILD_STREAK),
                    reason: format!(
                        "Call {submit} with status `stuck` (the STUCK: hand-back) instead of another build or test \
                         command: {streak} consecutive build or test failures in this run{diag}. Retrying is not the next \
                         step, because you do not yet have the information the fix needs. In the submit call give what you \
                         were making work, the failing command, the diagnostic verbatim, the hypotheses you ruled out, and \
                         the fact or decision you need. Do not retry under a different spelling of the command."
                    ),
                });
            }
        }
        None
    }

    fn mode(&self) -> PermissionMode {
        if self.yolo() {
            PermissionMode::Bypass
        } else {
            self.ctx.permission_mode
        }
    }

    fn rule_decision(&self, subject: &Subject<'_>, allow_extra: &[Rule]) -> Option<PolicyDecision> {
        let (repo, home) = (&self.roots.repo, &self.roots.home);
        if let Some(r) = self.deny.iter().find(|r| r.matches(subject, repo, home)) {
            return Some(PolicyDecision::deny(
                RuleRef::permission(&r.raw),
                format!(
                    "Do not {}: the permission rule `{}` refuses it. If the task needs it, say so in your report.",
                    describe(subject),
                    r.raw
                ),
            ));
        }
        if let Some(r) = self.ask.iter().find(|r| r.matches(subject, repo, home)) {
            return Some(PolicyDecision::ask(
                RuleRef::permission(&r.raw),
                capitalize(&describe(subject)),
            ));
        }
        if let Some(r) = self
            .allow
            .iter()
            .chain(allow_extra.iter())
            .find(|r| r.matches(subject, repo, home))
        {
            return Some(PolicyDecision::Allow {
                rule: Some(RuleRef::permission(&r.raw)),
            });
        }
        None
    }

    fn mode_default(&self, subject: &Subject<'_>, mode: PermissionMode) -> PolicyDecision {
        match mode {
            PermissionMode::Bypass => PolicyDecision::Allow {
                rule: Some(RuleRef::permission("mode:bypass")),
            },
            PermissionMode::Plan => PolicyDecision::deny(
                RuleRef::permission("mode:plan"),
                format!(
                    "Stay read-only: this session is in plan mode, so Ostra refuses to {}. Put what you found in your \
                     report instead.",
                    describe(subject)
                ),
            ),
            PermissionMode::Default | PermissionMode::AcceptEdits => PolicyDecision::ask(
                RuleRef::permission(if mode == PermissionMode::Default {
                    "mode:default"
                } else {
                    "mode:acceptEdits"
                }),
                capitalize(&describe(subject)),
            ),
        }
    }

    fn layer2(&self, call: &ToolCall, parsed: Option<&Parsed>) -> PolicyDecision {
        let session_allow = self.lock().session_allow.clone();
        let mode = self.mode();
        let tool = call.tool.as_str();
        match perms::family(tool) {
            Family::Bash => {
                self.bash_permission(call, parsed.expect("parsed for Bash"), mode, &session_allow)
            }
            Family::Edit => {
                let targets = self.write_paths(call);
                let mut ask = None;
                let mut allowed_by: Option<RuleRef> = None;
                for (path, _) in &targets {
                    let subject = Subject::Path { tool, path };
                    let d = match self.rule_decision(&subject, &session_allow) {
                        Some(d @ (PolicyDecision::Deny { .. } | PolicyDecision::Ask { .. })) => d,
                        _ if self.roots.in_session(path) || self.roots.in_temp(path) => continue,
                        Some(d) => d,
                        None if mode == PermissionMode::AcceptEdits && self.roots.in_repo(path) => {
                            continue;
                        }
                        None => self.mode_default(&subject, mode),
                    };
                    match d {
                        PolicyDecision::Deny { .. } => return d,
                        PolicyDecision::Ask { .. } => ask = ask.or(Some(d)),
                        PolicyDecision::Allow { rule } => allowed_by = allowed_by.or(rule),
                    }
                }
                ask.unwrap_or_else(|| {
                    // Name the rule that actually allowed the write, so the Activity view shows it.
                    let rule = allowed_by.unwrap_or_else(|| {
                        RuleRef::permission(if mode == PermissionMode::AcceptEdits {
                            "mode:acceptEdits"
                        } else {
                            "session-dir"
                        })
                    });
                    PolicyDecision::Allow { rule: Some(rule) }
                })
            }
            Family::Read => {
                let path = match self.file_path(call) {
                    Some((p, _)) => p,
                    None => self.roots.repo.clone(),
                };
                match self.rule_decision(&Subject::Path { tool, path: &path }, &session_allow) {
                    Some(d @ (PolicyDecision::Deny { .. } | PolicyDecision::Ask { .. })) => d,
                    _ => PolicyDecision::allow(),
                }
            }
            Family::WebFetch => {
                let url = call.str_field("url").unwrap_or_default();
                let subject = Subject::Url { url };
                self.rule_decision(&subject, &session_allow)
                    .unwrap_or_else(|| match mode {
                        PermissionMode::Bypass => self.mode_default(&subject, mode),
                        _ => PolicyDecision::ask(
                            RuleRef::permission(if mode == PermissionMode::AcceptEdits {
                                "mode:acceptEdits"
                            } else {
                                "mode:default"
                            }),
                            capitalize(&describe(&subject)),
                        ),
                    })
            }
            Family::Other => {
                let subject = Subject::Tool { tool };
                // Harness bookkeeping tools, and calls to Ostra's own MCP server, which checks
                // each call again when it runs.
                let harness_internal = tool.strip_prefix("Other:").is_some_and(|t| {
                    matches!(t, "search_tool" | "TodoWrite" | "todo_write" | "update_plan" | "update_todos")
                        || t.starts_with("mcp__ostra__")
                        || t.starts_with("ostra__")
                        || t.starts_with("submit_")
                });
                let known = matches!(
                    tool,
                    "WebSearch" | "Skill" | "Report" | "Memory" | "MemoryRecall"
                ) || tool.starts_with("submit_")
                    || harness_internal;
                match self.rule_decision(&subject, &session_allow) {
                    Some(d) => d,
                    None if known => PolicyDecision::allow(),
                    None => self.mode_default(&subject, mode),
                }
            }
        }
    }

    fn bash_permission(
        &self,
        call: &ToolCall,
        parsed: &Parsed,
        mode: PermissionMode,
        session_allow: &[Rule],
    ) -> PolicyDecision {
        let start = self.start_cwd(call);
        let targets = guards::shell_targets(&self.roots, parsed, &start);
        let (repo, home) = (&self.roots.repo, &self.roots.home);

        // Deny beats ask beats allow, across every simple command.
        for cmd in &parsed.commands {
            let subject = Subject::Bash(cmd);
            if let Some(r) = self.deny.iter().find(|r| r.matches(&subject, repo, home)) {
                return PolicyDecision::deny(
                    RuleRef::permission(&r.raw),
                    format!(
                        "Do not run `{}`: the permission rule `{}` refuses it. If the task needs it, say so in your report.",
                        cmd.words_text(),
                        r.raw
                    ),
                );
            }
        }
        for cmd in &parsed.commands {
            if let Some(r) = self
                .ask
                .iter()
                .find(|r| r.matches(&Subject::Bash(cmd), repo, home))
            {
                return PolicyDecision::ask(
                    RuleRef::permission(&r.raw),
                    format!("Run `{}`", cmd.words_text()),
                );
            }
        }
        if !parsed.complete {
            let command = call.str_field("command").unwrap_or_default();
            let subject = Subject::Tool { tool: "Bash" };
            return match mode {
                PermissionMode::Bypass => self.mode_default(&subject, mode),
                PermissionMode::Plan => PolicyDecision::deny(
                    RuleRef::permission("mode:plan"),
                    "Stay read-only: this session is in plan mode, and Ostra could not parse this command to confirm it \
                     only reads."
                        .to_string(),
                ),
                _ => PolicyDecision::ask(
                    RuleRef::permission("unparsed"),
                    format!("Run `{}` (Ostra could not split it into simple commands)", truncate(command, 200)),
                ),
            };
        }

        let mut last_allow = None;
        for (i, cmd) in parsed.commands.iter().enumerate() {
            let cmd_targets: Vec<&PathBuf> = targets
                .iter()
                .filter(|t| t.command == i)
                .map(|t| &t.path)
                .collect();
            let writes_ok = cmd_targets
                .iter()
                .all(|p| self.roots.in_session(p) || self.roots.in_temp(p));
            let subject = Subject::Bash(cmd);
            let allowed_by_rule = if cmd.is_dynamic_name() {
                None
            } else {
                self.allow
                    .iter()
                    .chain(session_allow.iter())
                    .find(|r| r.matches(&subject, repo, home))
                    .map(|r| r.raw.clone())
            };
            if let Some(rule) = allowed_by_rule {
                last_allow = Some(RuleRef::permission(&rule));
                continue;
            }
            if is_read_only(cmd) && writes_ok {
                continue;
            }
            if mode == PermissionMode::AcceptEdits {
                let fs = cmd
                    .effective_name()
                    .is_some_and(|n| matches!(n.as_str(), "mkdir" | "touch" | "cp" | "mv"));
                let in_scope = cmd_targets.iter().all(|p| {
                    self.roots.in_repo(p) || self.roots.in_session(p) || self.roots.in_temp(p)
                });
                if (fs || is_read_only(cmd)) && in_scope {
                    continue;
                }
            }
            match mode {
                PermissionMode::Bypass => continue,
                PermissionMode::Plan => {
                    return PolicyDecision::deny(
                        RuleRef::permission("mode:plan"),
                        format!(
                            "Stay read-only: this session is in plan mode, so Ostra refuses `{}`, which is not a read-only \
                             command. Put what you found in your report instead.",
                            cmd.words_text()
                        ),
                    );
                }
                _ => {
                    return PolicyDecision::ask(
                        RuleRef::permission(if mode == PermissionMode::Default {
                            "mode:default"
                        } else {
                            "mode:acceptEdits"
                        }),
                        format!("Run `{}`", cmd.words_text()),
                    );
                }
            }
        }
        PolicyDecision::Allow {
            rule: Some(last_allow.unwrap_or_else(|| {
                RuleRef::permission(if mode == PermissionMode::Bypass {
                    "mode:bypass"
                } else {
                    "read-only"
                })
            })),
        }
    }

    /// The rule an "always in this workspace" answer adds for this call.
    pub fn allow_rule_suggestion(&self, call: &ToolCall) -> Option<String> {
        match perms::family(&call.tool) {
            Family::Bash => {
                let parsed = bash::parse(call.str_field("command").unwrap_or_default());
                let session_allow = self.lock().session_allow.clone();
                let (repo, home) = (&self.roots.repo, &self.roots.home);
                parsed
                    .commands
                    .iter()
                    .find(|c| {
                        !is_read_only(c)
                            && !self
                                .allow
                                .iter()
                                .chain(session_allow.iter())
                                .any(|r| r.matches(&Subject::Bash(c), repo, home))
                    })
                    .or(parsed.commands.first())
                    .and_then(perms::suggestion_for_bash)
            }
            Family::Edit => self
                .write_paths(call)
                .first()
                .map(|(p, _)| perms::suggestion_for_path(&call.tool, p, &self.roots.repo)),
            Family::Read => self
                .file_path(call)
                .map(|(p, _)| perms::suggestion_for_path("Read", &p, &self.roots.repo)),
            Family::WebFetch => call
                .str_field("url")
                .and_then(perms::url_host)
                .map(|h| format!("WebFetch(domain:{h})")),
            Family::Other => Some(call.tool.clone()),
        }
    }

    /// Post-tool: the build streak and the lesson gate.
    pub fn observe(&self, call: &ToolCall, outcome: &ToolOutcome) -> Vec<Observation> {
        let mut out = vec![];
        match call.tool.as_str() {
            "Bash" => {
                let command = call.str_field("command").unwrap_or_default();
                if !build::is_build_command(command, &self.build_commands()) {
                    return out;
                }
                let Some(failed) = build::failed_from(outcome) else {
                    return out;
                };
                let ev = self.lock().streak.record(failed, &outcome.output);
                if let Some(q) = ev.recall {
                    out.push(Observation::RecallLessons { query: q });
                }
                if let Some(n) = ev.note {
                    out.push(Observation::AppendNote(n));
                }
            }
            "Memory" if !outcome.is_error => self.lock().streak.pending.clear(),
            "Report"
                if !outcome.is_error
                    && call
                        .str_field("reason")
                        .is_some_and(|r| !r.trim().is_empty()) =>
            {
                // An explicitly stated reason waives the pending lessons, and later submits pass.
                self.lock().streak.pending.clear();
            }
            _ => {}
        }
        out
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn describe(subject: &Subject<'_>) -> String {
    match subject {
        Subject::Bash(cmd) => format!("run `{}`", truncate(&cmd.words_text(), 200)),
        Subject::Path { tool, path } => {
            let verb = match perms::family(tool) {
                Family::Read => "read",
                _ => "write",
            };
            format!("{verb} {}", path.display())
        }
        Subject::Url { url } => format!("fetch {}", truncate(url, 200)),
        Subject::Tool { tool } => format!("use the tool `{tool}`"),
    }
}
