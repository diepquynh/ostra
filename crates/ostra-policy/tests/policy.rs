//! Guard and permission cases ported from Ultracode's tests/test_definitions.test.js (scope,
//! bash scope, artifacts, ledgers, tool self-protection, reports, lesson gate, build streak), plus
//! the permission model.

use ostra_core::AgentName;
use ostra_core::config::{PermissionMode, PermissionRules};
use ostra_core::exec::ExecContext;
use ostra_core::policy::{PolicyDecision, ToolCall, ToolOutcome};
use ostra_policy::{ExecutionPolicy, Observation, PolicyInputs};
use serde_json::json;
use std::path::{Path, PathBuf};

struct Fx {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    ws: PathBuf,
    session_root: PathBuf,
    session_dir: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::Builder::new()
        .prefix("ostra-policy-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let root = ostra_core::paths::canonical(tmp.path()).unwrap();
    let repo = root.join("repo");
    let ws = root.join("ws");
    let session_root = ws.join(".ostra/sessions/s1");
    let session_dir = session_root.join("backend");
    for d in [
        repo.join("src"),
        session_dir.clone(),
        root.join("bin"),
        root.join("config"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    let bin = root.join("bin/ostra");
    let config = root.join("config/config.toml");
    std::fs::write(&bin, "").unwrap();
    std::fs::write(&config, "").unwrap();
    Fx {
        _tmp: tmp,
        repo,
        ws,
        session_root,
        session_dir,
        bin,
        config,
    }
}

impl Fx {
    /// A context for a standard agent, with the scope, contract, and grants its definition holds.
    fn ctx(&self, agent: AgentName) -> ExecContext {
        let def = ostra_agents::builtin_def(agent);
        ExecContext {
            work_dirs: Vec::new(),
            execution_id: "x_test".into(),
            session_id: Some("s1".into()),
            agent,
            initializer_mode: None,
            executor: ostra_core::ExecutorKind::Native,
            workspace_root: self.ws.clone(),
            repo_root: self.repo.clone(),
            project_key: "backend".into(),
            session_dir: self.session_dir.clone(),
            session_root: self.session_root.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Bypass,
            permissions: PermissionRules::default(),
            protected_paths: vec![self.bin.clone(), self.config.clone()],
            memory_db: self.repo.join(".ostra/memory/knowledge.sqlite3"),
            sandbox_mode: None,
            enforce_tool_calls: true,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_readable: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
            owes_reply: false,
            write_scope: def.map(|d| d.write_scope),
            contract: def
                .map(|d| d.returns)
                .unwrap_or(ostra_core::Contract::Stage),
            capabilities: def.map(|d| d.capabilities.clone()).unwrap_or_default(),
        }
    }

    fn policy(&self, agent: AgentName) -> ExecutionPolicy {
        ExecutionPolicy::new(self.ctx(agent), PolicyInputs::default())
    }
}

fn write(p: impl AsRef<Path>) -> ToolCall {
    ToolCall::new(
        "Write",
        json!({"file_path": p.as_ref().to_string_lossy(), "content": "x"}),
    )
}

fn bash(cmd: impl AsRef<str>) -> ToolCall {
    ToolCall::new("Bash", json!({"command": cmd.as_ref()}))
}

/// A path rendered for embedding in a bash command: the `\\?\` prefix dropped and separators made
/// forward slashes, as an agent would type under Git Bash, so backslashes are not eaten by the
/// shell. On Unix it is the plain path.
fn shp(p: impl AsRef<Path>) -> String {
    let s = p.as_ref().to_string_lossy().into_owned();
    if cfg!(windows) {
        s.trim_start_matches(r"\\?\").replace('\\', "/")
    } else {
        s
    }
}

fn deny_reason(d: &PolicyDecision) -> Option<String> {
    match d {
        PolicyDecision::Deny { reason, .. } => Some(reason.clone()),
        _ => None,
    }
}

#[track_caller]
fn allowed(p: &ExecutionPolicy, call: &ToolCall) {
    let d = p.check(call);
    assert!(
        d.is_allow(),
        "{} should be allowed for {}, got {d:?}",
        call.input,
        p.ctx().agent
    );
}

#[track_caller]
fn denied(p: &ExecutionPolicy, call: &ToolCall, pattern: &str) -> String {
    let d = p.check(call);
    let reason = deny_reason(&d).unwrap_or_else(|| {
        panic!(
            "{} should be denied for {}, got {d:?}",
            call.input,
            p.ctx().agent
        )
    });
    assert!(
        reason.contains(pattern),
        "reason for {} should mention {pattern:?}: {reason}",
        call.input
    );
    reason
}

#[track_caller]
fn guard_of(p: &ExecutionPolicy, call: &ToolCall) -> String {
    match p.check(call) {
        PolicyDecision::Deny { rule, .. } => rule.rule,
        other => panic!("{} should be denied, got {other:?}", call.input),
    }
}

fn outcome(text: &str) -> ToolOutcome {
    ToolOutcome {
        output: text.into(),
        is_error: false,
        exit_code: None,
        result_known: true,
    }
}

const BUILD_FAIL: &str = "Exit code 1\n[ERROR] /r/src/main/java/Foo.java:[11,2] cannot find symbol\n[ERROR] Failed to execute goal maven-compiler-plugin:3.14.1:compile (default-compile) on project core: Compilation failure";

// ---------------------------------------------------------------------------------------------
// Write scope (scope-guard confines each subagent to its documented write scope)
// ---------------------------------------------------------------------------------------------

#[test]
fn write_scope_per_agent() {
    let f = fx();
    denied(
        &f.policy(AgentName::PromptGeneration),
        &write("/etc/passwd"),
        "outside both",
    );
    denied(
        &f.policy(AgentName::WriteTest),
        &write(f.repo.join("../outside.txt")),
        "outside both",
    );

    for agent in [
        AgentName::CodeReviewer,
        AgentName::Plan,
        AgentName::ExecutionPathAnalyzer,
        AgentName::Explore,
        AgentName::FactCheck,
        AgentName::GenerateSpec,
    ] {
        let p = f.policy(agent);
        allowed(&p, &write(f.session_dir.join("report.md")));
        allowed(&p, &write(f.session_root.join("cross.md")));
        denied(
            &p,
            &write(f.repo.join("src/App.ts")),
            "never modifies project source",
        );
    }

    let init = f.policy(AgentName::Initializer);
    denied(
        &init,
        &write(f.repo.join(".ostra/skills/convention/SKILL.md")),
        "older skills dir",
    );
    allowed(&init, &write(f.repo.join(".agents/skills/entity/SKILL.md")));
    allowed(&init, &write(f.repo.join(".ostra/INVENTORY.md")));
    denied(
        &init,
        &write(f.repo.join(".agents/rules.md")),
        "outside the scope of initializer",
    );
    denied(
        &init,
        &write(f.repo.join("src/App.ts")),
        "outside the scope of initializer",
    );

    // Rule B1: a docs writer returns the book in its submit call and writes no project file.
    let docs = f.policy(AgentName::Documentation);
    denied(
        &docs,
        &write(f.repo.join(".agents/skills/convention/SKILL.md")),
        "never modifies project source",
    );
    denied(
        &docs,
        &write(f.repo.join("src/App.ts")),
        "never modifies project source",
    );

    let imp = f.policy(AgentName::Implementer);
    allowed(&imp, &write(f.repo.join("src/App.ts")));
    let r = denied(&imp, &write(f.repo.join("src/App.test.ts")), "Constraint 6");
    assert!(r.starts_with("Leave test files to the test stage"), "{r}");
    assert!(r.contains("`test_files`"), "{r}");
    assert_eq!(
        guard_of(&imp, &write(f.repo.join("core/src/test/java/FooTest.java"))),
        "no-tests-from-implementer"
    );
    // A phase path list is a hint, not an allowlist: any work-repo path stays writable.
    allowed(
        &imp,
        &write(f.repo.join("billing/src/main/java/Invoice.java")),
    );

    allowed(
        &f.policy(AgentName::WriteTest),
        &write(f.repo.join("src/App.test.ts")),
    );
    allowed(
        &f.policy(AgentName::PromptGeneration),
        &write(f.repo.join("src/prompts/system.md")),
    );
    denied(
        &f.policy(AgentName::QuickAnswer),
        &write(f.session_dir.join("x.md")),
        "read-only",
    );
}

#[cfg(unix)]
#[test]
fn initializer_writes_through_linked_skills_dirs() {
    use std::os::unix::fs::symlink;
    // `.agents/skills` linked to a skills dir elsewhere in the project.
    for target in ["skills", ".claude/skills", ".ostra/skills"] {
        let f = fx();
        std::fs::create_dir_all(f.repo.join(target)).unwrap();
        std::fs::create_dir_all(f.repo.join(".agents")).unwrap();
        symlink(f.repo.join(target), f.repo.join(".agents/skills")).unwrap();
        let init = f.policy(AgentName::Initializer);
        allowed(&init, &write(f.repo.join(".agents/skills/entity/SKILL.md")));
        allowed(&init, &write(f.repo.join(target).join("entity/SKILL.md")));
        allowed(
            &init,
            &bash("mkdir -p .agents/skills/api && echo x > .agents/skills/api/SKILL.md"),
        );
        denied(
            &init,
            &write(f.repo.join("src/App.ts")),
            "outside the scope of initializer",
        );
    }

    // `skills` at the root with `.agents/skills` and the legacy dir both linked to it.
    let f = fx();
    std::fs::create_dir_all(f.repo.join("skills")).unwrap();
    std::fs::create_dir_all(f.repo.join(".agents")).unwrap();
    symlink(f.repo.join("skills"), f.repo.join(".agents/skills")).unwrap();
    std::fs::create_dir_all(f.repo.join(".ostra")).unwrap();
    symlink(f.repo.join("skills"), f.repo.join(".ostra/skills")).unwrap();
    allowed(
        &f.policy(AgentName::Initializer),
        &write(f.repo.join(".agents/skills/entity/SKILL.md")),
    );

    // A legacy dir linked to `.agents/skills` does not make `.agents/skills` the legacy dir.
    let f = fx();
    std::fs::create_dir_all(f.repo.join(".agents/skills")).unwrap();
    std::fs::create_dir_all(f.repo.join(".ostra")).unwrap();
    symlink(f.repo.join(".agents/skills"), f.repo.join(".ostra/skills")).unwrap();
    allowed(
        &f.policy(AgentName::Initializer),
        &write(f.repo.join(".agents/skills/entity/SKILL.md")),
    );
}

#[test]
fn edit_and_apply_patch_are_writes() {
    let f = fx();
    let rev = f.policy(AgentName::CodeReviewer);
    let edit = ToolCall::new(
        "Edit",
        json!({"file_path": f.repo.join("src/a.ts").to_string_lossy(), "old_string": "a", "new_string": "b"}),
    );
    denied(&rev, &edit, "never modifies project source");
    let patch = ToolCall::new(
        "ApplyPatch",
        json!({"patch": "*** Begin Patch\n*** Update File: src/a.ts\n@@\n-a\n+b\n*** End Patch"}),
    );
    denied(&rev, &patch, "never modifies project source");
    let imp = f.policy(AgentName::Implementer);
    allowed(&imp, &patch);
    let test_patch = ToolCall::new(
        "ApplyPatch",
        json!({"patch": "*** Begin Patch\n*** Add File: src/a.test.ts\n+x\n*** End Patch"}),
    );
    denied(&imp, &test_patch, "Constraint 6");
}

// ---------------------------------------------------------------------------------------------
// Bash scope (bash-scope-guard confines each subagent's shell writes)
// ---------------------------------------------------------------------------------------------

#[test]
fn bash_scope_per_agent() {
    let f = fx();
    let rev = f.policy(AgentName::CodeReviewer);
    allowed(&rev, &bash("git status"));
    allowed(&f.policy(AgentName::Implementer), &bash("npm test"));
    allowed(
        &rev,
        &bash(format!(
            "cat <<'EOF' > {}\nhi\nEOF",
            shp(f.session_dir.join("ledger.md"))
        )),
    );
    allowed(
        &f.policy(AgentName::Plan),
        &bash(format!(
            "cat > {} <<'EOF'\nthe `<!-- AWS START --> ... <!-- AWS END -->` group\nrm -rf /somewhere\nEOF",
            shp(f.session_root.join("plan-phase-1.md"))
        )),
    );
    allowed(
        &f.policy(AgentName::GenerateSpec),
        &bash("sort spec.md > /tmp/ostra-test-got.txt"),
    );
    denied(
        &f.policy(AgentName::Implementer),
        &bash("echo x > /tmp/ostra-test-scratch.log"),
        "outside both",
    );
    denied(
        &rev,
        &bash(format!("echo bad > {}", shp(f.repo.join("src/App.ts")))),
        "never modifies project source",
    );
    denied(
        &f.policy(AgentName::Implementer),
        &bash(format!(
            "cat <<'EOF' > {}\nhi\nEOF",
            shp(f.repo.join("src/App.test.ts"))
        )),
        "Constraint 6",
    );
    denied(
        &f.policy(AgentName::WriteTest),
        &bash(format!("rm -rf {}", shp(f.repo.join("../sibling")))),
        "outside both",
    );
    // A relative target after `cd` resolves against the new directory.
    denied(
        &rev,
        &bash("cd src && echo x > App.ts"),
        "never modifies project source",
    );
    // Git commands that rewrite the working tree are writes to it.
    denied(&rev, &bash("git stash"), "never modifies project source");
    // A body fed to a shell runs, so its writes count.
    denied(
        &rev,
        &bash("cat <<'EOF' | bash\necho x > src/hacked.ts\nEOF"),
        "never modifies project source",
    );
    denied(
        &rev,
        &bash("echo $(rm src/App.ts)"),
        "never modifies project source",
    );
}

// ---------------------------------------------------------------------------------------------
// Reports (a report may be written by any tool, but only at the declared path)
// ---------------------------------------------------------------------------------------------

#[test]
fn declared_report_path_and_lesson_gate() {
    let f = fx();
    let report = f.session_dir.join("ostra-implementer-phase-3.md");
    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.report_file = Some(report.clone());
    let p = ExecutionPolicy::new(
        ctx,
        PolicyInputs {
            build_commands: vec!["./mvnw compile".into()],
            ..Default::default()
        },
    );

    allowed(&p, &write(&report));
    allowed(
        &p,
        &bash(format!(
            "cat > \"{}\" <<'REPORT_EOF'\n# Implementation Report\nDid it.\nREPORT_EOF",
            shp(&report)
        )),
    );
    allowed(&p, &bash(format!("echo \"## More\" >> {}", shp(&report))));

    let invented = f.session_dir.join("ostra-implementer-credentials-uri.md");
    let r = denied(&p, &write(&invented), "declared report path");
    assert!(r.starts_with("Write your report to \""));
    assert!(r.contains("ostra-implementer-phase-3.md"), "{r}");
    denied(
        &p,
        &bash(format!("echo x > {}", shp(&invented))),
        "declared report path",
    );
    denied(
        &p,
        &write(f.session_root.join("frontend/ostra-implementer-phase-3.md")),
        "declared report path",
    );

    allowed(
        &p,
        &write(f.session_dir.join("ostra-implementer-progress.md")),
    );
    allowed(&p, &write(f.session_dir.join("notes.txt")));
    denied(
        &p,
        &bash(format!(
            "echo x > {}",
            shp(f.session_dir.join("ostra-security-block.json"))
        )),
        "security_block",
    );

    // Three failures then a pass: a verified recovery with no lesson blocks the report.
    for _ in 0..3 {
        p.observe(&bash("./mvnw compile"), &outcome(BUILD_FAIL));
    }
    let obs = p.observe(&bash("./mvnw compile"), &outcome("BUILD SUCCESS"));
    assert!(
        matches!(&obs[..], [Observation::AppendNote(n)] if n.contains("passed after 3 consecutive failures"))
    );
    let gated = denied(&p, &write(&report), "cannot find symbol");
    assert!(gated.contains("Memory tool") && gated.contains("`reason`"));
    denied(
        &p,
        &bash(format!("cat > \"{}\" <<'EOF'\nx\nEOF", shp(&report))),
        "cannot find symbol",
    );
    denied(
        &p,
        &ToolCall::new("Report", json!({"content": "# r"})),
        "cannot find symbol",
    );
    denied(
        &p,
        &ToolCall::new("submit_implementer", json!({"status": "ok"})),
        "cannot find symbol",
    );
    allowed(
        &p,
        &ToolCall::new("submit_implementer", json!({"status": "stuck"})),
    );

    // A recorded lesson clears it; an agent asserting it recorded one does not.
    p.observe(
        &ToolCall::new(
            "Memory",
            json!({"area": "core", "lesson": "l", "source": "s"}),
        ),
        &outcome("ok"),
    );
    allowed(&p, &write(&report));
    allowed(
        &p,
        &ToolCall::new("submit_implementer", json!({"status": "ok"})),
    );

    // A stated reason passes the Report tool and waives the pending lesson.
    for _ in 0..3 {
        p.observe(&bash("./mvnw compile"), &outcome(BUILD_FAIL));
    }
    p.observe(&bash("./mvnw compile"), &outcome("BUILD SUCCESS"));
    let with_reason = ToolCall::new(
        "Report",
        json!({"content": "# r", "reason": "situational fix"}),
    );
    allowed(&p, &with_reason);
    p.observe(&with_reason, &outcome("wrote"));
    allowed(
        &p,
        &ToolCall::new("submit_implementer", json!({"status": "ok"})),
    );
}

#[test]
fn undeclared_report_is_unconstrained() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    allowed(
        &p,
        &write(f.session_dir.join("ostra-implementer-credentials-uri.md")),
    );
    allowed(
        &f.policy(AgentName::Explore),
        &document(f.session_dir.join("ostra-research-20260826-auth.md")),
    );
}

// ---------------------------------------------------------------------------------------------
// Build streak (counts consecutive failures and forces escalation at five)
// ---------------------------------------------------------------------------------------------

#[test]
fn build_streak_forces_escalation_at_five() {
    let f = fx();
    let p = ExecutionPolicy::new(
        f.ctx(AgentName::Implementer),
        PolicyInputs {
            build_commands: vec!["./mvnw -q -T1C compile".into()],
            test_commands: vec![
                "./mvnw test -Ptest".into(),
                "./mvnw test -Ptest -pl {MODULE} -am -Dtest={TEST}".into(),
            ],
            ..Default::default()
        },
    );
    assert!(
        p.observe(&bash("ls -la src"), &outcome("Exit code 1"))
            .is_empty()
    );
    assert_eq!(p.build_streak(), 0);

    for attempt in 1..=4 {
        allowed(&p, &bash("./mvnw -q -T1C compile"));
        let obs = p.observe(&bash("./mvnw -q -T1C compile"), &outcome(BUILD_FAIL));
        assert_eq!(p.build_streak(), attempt);
        let note = obs.iter().find_map(|o| match o {
            Observation::AppendNote(n) => Some(n.clone()),
            _ => None,
        });
        if attempt < 3 {
            assert!(note.is_none(), "no warning before the third failure");
        } else {
            let n = note.unwrap();
            assert!(n.contains("consecutive failing build or test commands"));
            assert!(n.contains("same diagnostic is repeating"));
            assert!(n.contains("stuck"));
        }
        if attempt >= 2 {
            assert!(obs.iter().any(|o| matches!(o, Observation::RecallLessons { query } if query.contains("cannot find symbol"))));
        }
    }
    allowed(&p, &bash("./mvnw -q -T1C compile"));
    p.observe(&bash("./mvnw -q -T1C compile"), &outcome(BUILD_FAIL));
    assert_eq!(p.build_streak(), 5);
    let r = denied(
        &p,
        &bash("./mvnw -q -T1C compile"),
        "5 consecutive build or test failures",
    );
    assert!(r.starts_with("Call submit_implementer with status `stuck` (the STUCK: hand-back)"));
    denied(
        &p,
        &bash("./mvnw test -Ptest -pl core -am -Dtest=FooTest"),
        "STUCK",
    );
    allowed(&p, &bash("cat src/main/java/Foo.java"));

    p.observe(&bash("./mvnw -q -T1C compile"), &outcome("BUILD SUCCESS"));
    assert_eq!(p.build_streak(), 0);
    assert!(p.lesson_pending());
    allowed(&p, &bash("./mvnw -q -T1C compile"));

    // An interrupted call is no evidence either way.
    p.observe(&bash("./mvnw -q -T1C compile"), &outcome(BUILD_FAIL));
    assert_eq!(p.build_streak(), 1);
    p.observe(
        &bash("./mvnw -q -T1C compile"),
        &ToolOutcome {
            output: "Command timed out after 120s".into(),
            is_error: true,
            exit_code: None,
            result_known: true,
        },
    );
    assert_eq!(p.build_streak(), 1);
}

#[test]
fn exit_code_wins_over_output() {
    let f = fx();
    let p = ExecutionPolicy::new(
        f.ctx(AgentName::WriteTest),
        PolicyInputs {
            build_commands: vec!["npm test".into()],
            ..Default::default()
        },
    );
    p.observe(
        &bash("npm test"),
        &ToolOutcome {
            output: "3 failed".into(),
            is_error: false,
            exit_code: Some(0),
            result_known: true,
        },
    );
    assert_eq!(p.build_streak(), 0);
    p.observe(
        &bash("npm test"),
        &ToolOutcome {
            output: "".into(),
            is_error: true,
            exit_code: Some(1),
            result_known: true,
        },
    );
    assert_eq!(p.build_streak(), 1);
}

// ---------------------------------------------------------------------------------------------
// Tool self-protection (plugin-guard denies running, loading, or patching the tool's own code)
// ---------------------------------------------------------------------------------------------

#[test]
fn self_protection() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    let bin = shp(&f.bin);
    let config = shp(&f.config);
    for cmd in [
        format!("{bin} hook --execution x"),
        "ostra mcp-stdio --execution x".to_string(),
        format!("bash -c \"cat /tmp/p.json > {config}\""),
        format!("sed -i 's/deny/allow/' {config}"),
        format!("rm {config}"),
        format!("chmod -x {bin}"),
        format!("echo '{{}}' > {config}"),
        format!("cd {} && ./ostra hook", shp(f.bin.parent().unwrap())),
        format!("cp /tmp/evil {bin}"),
    ] {
        assert_eq!(guard_of(&p, &bash(&cmd)), "self-protection", "{cmd}");
    }
    for cmd in [
        format!("cat {config}"),
        format!("grep -n deny {config}"),
        format!("head -40 {config}"),
        format!("sed -n '1,20p' {config}"),
    ] {
        allowed(&p, &bash(&cmd));
    }

    let state = shp(f.session_root.join(".state/gates.json"));
    denied(
        &p,
        &bash(format!(
            "node -e \"require('fs').writeFileSync('{state}','{{}}')\""
        )),
        "pipeline state",
    );
    denied(
        &p,
        &bash(format!(
            "python3 -c \"open('{}/ostra-review-ledger-phase-1.md','w').write('x')\"",
            shp(&f.session_dir)
        )),
        "pipeline state",
    );
    denied(
        &p,
        &bash("python3 - <<'PY'\nopen('out.json','w').write('{}')\nPY"),
        "writes to the filesystem",
    );
    denied(
        &p,
        &bash("node -e \"require('child_process').execSync('ls')\""),
        "spawns another process",
    );
    denied(
        &p,
        &bash("echo \"require('fs').writeFileSync('a','b')\" | node"),
        "writes to the filesystem",
    );
    for cmd in [
        "node -e \"console.log(process.version)\"",
        "node -p \"require('./package.json').version\"",
        "python3 -c \"print(open('README.md').read())\"",
        "npm test",
        "./mvnw -q compile",
        "git status",
        "python3 scripts/gen.py",
    ] {
        allowed(&p, &bash(cmd));
    }
    // A data heredoc that merely mentions interpreter code is content.
    allowed(
        &p,
        &bash(format!(
            "cat > {} <<'EOF'\nRun node -e \"require('fs').writeFileSync('x','y')\" to reproduce.\nEOF",
            shp(f.session_dir.join("notes.md"))
        )),
    );

    denied(&p, &write(&f.config), "part of Ostra itself");
    denied(
        &p,
        &write(f.ws.join(".ostra/workspace.toml")),
        "part of Ostra itself",
    );
    denied(
        &p,
        &write(f.ws.join(".ostra/workspace.db-wal")),
        "part of Ostra itself",
    );
    // Rule WB7: a composite transform an agent wrote would run in later nodes.
    denied(
        &p,
        &write(f.ws.join(".ostra/transforms/high-files.toml")),
        "part of Ostra itself",
    );
    allowed(&p, &write(f.repo.join("src/App.ts")));
    allowed(
        &p,
        &write(f.session_dir.join("ostra-implementer-phase-1.md")),
    );
}

// ---------------------------------------------------------------------------------------------
// State and artifact ownership (ledger-policy, artifact-guard)
// ---------------------------------------------------------------------------------------------

#[test]
fn state_ownership() {
    let f = fx();
    for agent in [
        AgentName::Implementer,
        AgentName::CodeReviewer,
        AgentName::FactCheck,
        AgentName::Initializer,
    ] {
        let p = f.policy(agent);
        assert_eq!(
            guard_of(&p, &write(f.session_root.join(".state/verdicts.json"))),
            "state-ownership",
            "{agent}"
        );
        assert_eq!(
            guard_of(&p, &write(f.repo.join(".ostra/memory/knowledge.sqlite3"))),
            "state-ownership",
            "{agent}"
        );
        assert_eq!(
            guard_of(&p, &write(f.session_dir.join("knowledge.sqlite3-wal"))),
            "state-ownership",
            "{agent}"
        );
        assert_eq!(
            guard_of(
                &p,
                &bash(format!(
                    "rm {}",
                    shp(f.repo.join(".ostra/memory/knowledge.sqlite3"))
                ))
            ),
            "state-ownership",
            "{agent}"
        );
    }
    let memory = denied(
        &f.policy(AgentName::Implementer),
        &write(f.repo.join(".ostra/memory/knowledge.sqlite3")),
        "Memory tool",
    );
    assert!(memory.starts_with("Record lessons with the Memory tool"));
    // Reading engine state with a plain reader is fine; anything else is not.
    allowed(
        &f.policy(AgentName::Implementer),
        &bash(format!("cat {}", shp(f.session_root.join(".state/x")))),
    );
    assert_eq!(
        guard_of(
            &f.policy(AgentName::Implementer),
            &bash(format!(
                "sqlite3 {} 'delete from lessons'",
                shp(f.repo.join(".ostra/memory/knowledge.sqlite3"))
            ))
        ),
        "state-ownership"
    );

    let ledger = f.session_dir.join("ostra-review-ledger-phase-1.md");
    for owner in [
        AgentName::CodeReviewer,
        AgentName::Implementer,
        AgentName::WriteTest,
    ] {
        allowed(&f.policy(owner), &write(&ledger));
    }
    denied(
        &f.policy(AgentName::Explore),
        &write(&ledger),
        "review_ledger",
    );
    denied(
        &f.policy(AgentName::Implementer),
        &write(f.session_dir.join("ostra-security-block.json")),
        "security_block",
    );
    allowed(
        &f.policy(AgentName::CodeReviewer),
        &write(f.session_dir.join("ostra-security-block.json")),
    );
    denied(
        &f.policy(AgentName::CodeReviewer),
        &write(f.session_dir.join("ostra-implementer-progress-phase-2.md")),
        "implementer",
    );
}

#[test]
fn artifact_ownership() {
    let f = fx();
    let spec = f.session_root.join("ostra-spec-20260922-120000-orders.md");
    std::fs::write(&spec, "# spec").unwrap();
    for agent in [AgentName::Plan, AgentName::FactCheck, AgentName::Explore] {
        assert_eq!(
            guard_of(&f.policy(agent), &write(&spec)),
            "artifact-ownership",
            "{agent}"
        );
    }
    denied(
        &f.policy(AgentName::GenerateSpec),
        &write(f.session_root.join("ostra-plan-x.md")),
        "document_plan",
    );
    // Fact-check's snapshot copy is not the artifact.
    let fc = f.policy(AgentName::FactCheck);
    allowed(
        &fc,
        &bash(format!(
            "mkdir -p \"{0}/factcheck-snapshot-spec\" && cp \"{1}\" \"{0}/factcheck-snapshot-spec/\"",
            shp(&f.session_root),
            shp(&spec)
        )),
    );
    denied(
        &fc,
        &bash(format!("cp /tmp/x \"{}\"", shp(&spec))),
        "document_spec",
    );
}

fn document(p: impl AsRef<Path>) -> ToolCall {
    ToolCall::new(
        "Document",
        json!({"path": p.as_ref().to_string_lossy(), "update": {}}),
    )
}

#[test]
fn documents_change_only_through_the_document_tool() {
    let f = fx();
    let spec = f.session_root.join("ostra-spec-20260922-120000-orders.md");
    let gs = f.policy(AgentName::GenerateSpec);
    for target in [
        spec.clone(),
        spec.with_extension("json"),
        f.session_root.join("ostra-plan-1-x-phase-2.md"),
    ] {
        let owner = if target.to_string_lossy().contains("plan") {
            f.policy(AgentName::Plan)
        } else {
            f.policy(AgentName::GenerateSpec)
        };
        assert_eq!(guard_of(&owner, &write(&target)), "document-tool");
        denied(&owner, &write(&target), "Document tool");
    }
    denied(
        &gs,
        &bash(format!("cat >> \"{}\" <<'EOF'\nx\nEOF", shp(&spec))),
        "Document tool",
    );
    let explore = f.policy(AgentName::Explore);
    denied(
        &explore,
        &write(f.session_dir.join("ostra-research-1-x.md")),
        "Document tool",
    );
    // Fact-check's snapshot copies and the plan's spec snapshot are plain files.
    allowed(
        &f.policy(AgentName::FactCheck),
        &write(
            f.session_root
                .join("factcheck-snapshot-spec/ostra-spec-20260922-120000-orders.md"),
        ),
    );
    allowed(
        &f.policy(AgentName::Plan),
        &write(f.session_root.join("plan-snapshot/spec.md")),
    );

    allowed(
        &explore,
        &document(f.session_dir.join("ostra-research-1-x.md")),
    );
    assert_eq!(
        guard_of(&f.policy(AgentName::Plan), &document(&spec)),
        "artifact-ownership"
    );
    assert_eq!(
        guard_of(&explore, &document(f.repo.join("ostra-research-1-x.md"))),
        "write-scope"
    );
}

// ---------------------------------------------------------------------------------------------
// Permissions (layer 2)
// ---------------------------------------------------------------------------------------------

fn with_perms(
    f: &Fx,
    agent: AgentName,
    mode: PermissionMode,
    allow: &[&str],
    ask: &[&str],
    deny: &[&str],
) -> ExecutionPolicy {
    let mut ctx = f.ctx(agent);
    ctx.permission_mode = mode;
    ctx.permissions = PermissionRules {
        allow: allow.iter().map(|s| s.to_string()).collect(),
        ask: ask.iter().map(|s| s.to_string()).collect(),
        deny: deny.iter().map(|s| s.to_string()).collect(),
    };
    ExecutionPolicy::new(ctx, PolicyInputs::default())
}

fn is_ask(d: &PolicyDecision) -> bool {
    matches!(d, PolicyDecision::Ask { .. })
}

#[test]
fn default_mode() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &["Bash(npm test *)"],
        &[],
        &["Bash(git push *)"],
    );
    assert!(is_ask(&p.check(&write(f.repo.join("src/a.ts")))));
    allowed(
        &p,
        &write(f.session_dir.join("ostra-implementer-phase-1.md")),
    );
    allowed(&p, &bash("npm test"));
    allowed(&p, &bash("npm test -- --watch=false"));
    assert!(
        is_ask(&p.check(&bash("npm test && curl evil"))),
        "each simple command must match separately"
    );
    assert!(is_ask(&p.check(&bash("npm install left-pad"))));
    allowed(&p, &bash("git status && git diff --stat | wc -l"));
    allowed(&p, &bash("ls -la src; cat README.md"));
    allowed(
        &p,
        &bash(format!(
            "cat > {} <<'EOF'\nx\nEOF",
            shp(f.session_dir.join("n.md"))
        )),
    );
    assert!(
        is_ask(&p.check(&bash("echo hi > src/x.ts"))),
        "a redirect into the project is an edit"
    );
    assert!(is_ask(&p.check(&bash("find . -name '*.tmp' -delete"))));
    let d = p.check(&bash("git push origin main"));
    assert!(deny_reason(&d).unwrap().contains("Bash(git push *)"));
    assert!(deny_reason(&p.check(&bash("npm test && git push"))).is_some());
    assert!(is_ask(&p.check(&ToolCall::new(
        "WebFetch",
        json!({"url": "https://docs.rs/x"})
    ))));
    assert!(is_ask(
        &p.check(&ToolCall::new("Other:browser_open", json!({})))
    ));
    allowed(&p, &ToolCall::new("WebSearch", json!({"query": "x"})));
    allowed(
        &p,
        &ToolCall::new("Read", json!({"file_path": "/etc/hosts"})),
    );
    allowed(&p, &ToolCall::new("MemoryRecall", json!({"query": "x"})));
    allowed(&p, &ToolCall::new("DocsSearch", json!({"query": "x"})));
    allowed(&p, &ToolCall::new("CodeCallers", json!({"symbol": "x"})));
    allowed(
        &p,
        &ToolCall::new("submit_implementer", json!({"status": "ok"})),
    );
    allowed(
        &p,
        &ToolCall::new(
            "Other:ToolSearch",
            json!({"query": "select:mcp__ostra__memory"}),
        ),
    );
    assert!(
        is_ask(&p.check(&bash("npm test $(curl evil)"))),
        "substitutions run too"
    );
    assert!(
        is_ask(&p.check(&bash("if [ x"))),
        "an unparsed command is asked about"
    );
}

#[test]
fn rules_on_reads_and_fetches() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Explore,
        PermissionMode::Default,
        &["WebFetch(domain:docs.rs)"],
        &["Read(secrets/**)"],
        &["Read(~/.ssh/**)", "WebSearch"],
    );
    let home = std::env::var("HOME").unwrap();
    assert!(
        deny_reason(&p.check(&ToolCall::new(
            "Read",
            json!({"file_path": format!("{home}/.ssh/id_rsa")})
        )))
        .is_some()
    );
    assert!(
        deny_reason(&p.check(&ToolCall::new(
            "Grep",
            json!({"pattern": "x", "path": format!("{home}/.ssh")})
        )))
        .is_some()
    );
    assert!(is_ask(&p.check(&ToolCall::new(
        "Read",
        json!({"file_path": f.repo.join("secrets/a.txt").to_string_lossy()})
    ))));
    allowed(
        &p,
        &ToolCall::new("WebFetch", json!({"url": "https://docs.rs/tokio/latest"})),
    );
    assert!(is_ask(&p.check(&ToolCall::new(
        "WebFetch",
        json!({"url": "https://evil.example/x"})
    ))));
    assert!(deny_reason(&p.check(&ToolCall::new("WebSearch", json!({"query": "x"})))).is_some());
}

#[test]
fn accept_edits_mode() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::AcceptEdits,
        &[],
        &[],
        &["Edit(*.env)"],
    );
    allowed(&p, &write(f.repo.join("src/a.ts")));
    allowed(&p, &bash("mkdir -p src/new && touch src/new/x.ts"));
    assert!(is_ask(&p.check(&bash("npm install"))));
    assert!(
        deny_reason(&p.check(&write(f.repo.join("config/.env"))))
            .unwrap()
            .contains("Edit(*.env)")
    );
}

#[test]
fn plan_mode() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Plan,
        &[],
        &[],
        &[],
    );
    let r = deny_reason(&p.check(&write(f.repo.join("src/a.ts")))).unwrap();
    assert!(r.starts_with("Stay read-only"));
    allowed(
        &p,
        &write(f.session_dir.join("ostra-implementer-phase-1.md")),
    );
    assert!(deny_reason(&p.check(&bash("npm test"))).is_some());
    allowed(&p, &bash("cat src/a.ts | head"));
    assert!(deny_reason(&p.check(&ToolCall::new("Other:x", json!({})))).is_some());
}

#[test]
fn bypass_mode_and_explicit_asks() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Bypass,
        &[],
        &["Bash(npm publish *)"],
        &[],
    );
    allowed(&p, &bash("npm install"));
    allowed(
        &p,
        &ToolCall::new("WebFetch", json!({"url": "https://x.example"})),
    );
    assert!(
        is_ask(&p.check(&bash("npm publish --access public"))),
        "an explicit ask rule is honored in bypass"
    );
}

#[test]
fn yolo_answers_asks_but_not_denials() {
    let f = fx();
    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.permission_mode = PermissionMode::Plan;
    ctx.permissions = PermissionRules {
        ask: vec!["Bash(npm publish *)".into()],
        deny: vec!["Bash(git push *)".into()],
        ..Default::default()
    };
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    assert!(deny_reason(&p.check(&bash("npm install"))).is_some());
    p.set_yolo(true);
    allowed(&p, &bash("npm install"));
    allowed(&p, &bash("npm publish"));
    allowed(&p, &write(f.repo.join("src/a.ts")));
    assert!(
        deny_reason(&p.check(&bash("git push origin main"))).is_some(),
        "explicit deny rules still apply"
    );
    assert_eq!(
        guard_of(&p, &write(f.repo.join("src/a.test.ts"))),
        "no-tests-from-implementer"
    );
    match p.check(&bash("npm install")) {
        PolicyDecision::Allow { rule: Some(r) } => assert_eq!(r.rule, "yolo"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn always_in_workspace() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &[],
    );
    let call = bash("cd src && npm run build --prod");
    assert!(is_ask(&p.check(&call)));
    let s = p.allow_rule_suggestion(&call).unwrap();
    assert_eq!(s, "Bash(npm run *)");
    p.add_session_allow(s);
    allowed(&p, &call);
    assert_eq!(
        p.allow_rule_suggestion(&write(f.repo.join("src/a/b.ts")))
            .unwrap(),
        "Edit(/src/**)"
    );
    assert_eq!(
        p.allow_rule_suggestion(&ToolCall::new(
            "WebFetch",
            json!({"url": "https://docs.rs/x"})
        ))
        .unwrap(),
        "WebFetch(domain:docs.rs)"
    );
}

#[test]
fn every_decision_names_its_rule() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &["Bash(npm test *)"],
        &[],
        &[],
    );
    match p.check(&bash("npm test")) {
        PolicyDecision::Allow { rule: Some(r) } => assert_eq!(
            (r.layer.as_str(), r.rule.as_str()),
            ("permission", "Bash(npm test *)")
        ),
        other => panic!("{other:?}"),
    }
    match p.check(&write(f.repo.join("src/a.ts"))) {
        PolicyDecision::Ask { rule, reason } => {
            assert_eq!(rule.rule, "mode:default");
            assert!(reason.starts_with("Write "));
        }
        other => panic!("{other:?}"),
    }
    match p.check(&write(f.repo.join("src/a.test.ts"))) {
        PolicyDecision::Deny { rule, .. } => assert_eq!(rule.layer, "guard"),
        other => panic!("{other:?}"),
    }
}

// Rule M1: workspace MCP tools are allowed by default, rules narrow them, plan mode refuses them.
#[test]
fn workspace_mcp_tools() {
    let f = fx();
    let issue = ToolCall::new("mcp__github__create_issue", json!({"title": "x"}));
    let search = ToolCall::new("mcp__github__search", json!({}));
    let other_server = ToolCall::new("mcp__githubx__search", json!({}));
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &[],
    );
    allowed(&p, &issue);
    // A harness's own MCP server is not a workspace server.
    assert!(is_ask(
        &p.check(&ToolCall::new("Other:mcp__github__search", json!({})))
    ));

    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &["mcp__github__create_issue"],
        &["mcp__githubx"],
    );
    assert!(is_ask(&p.check(&issue)));
    allowed(&p, &search);
    assert!(deny_reason(&p.check(&other_server)).is_some());

    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Plan,
        &["mcp__github__search"],
        &[],
        &[],
    );
    assert!(
        deny_reason(&p.check(&issue))
            .unwrap()
            .starts_with("Stay read-only")
    );
    allowed(&p, &search);
    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.permission_mode = PermissionMode::Plan;
    let p = ExecutionPolicy::new(
        ctx,
        PolicyInputs {
            read_only_mcp_tools: vec!["mcp__githubx__search".into()],
            ..Default::default()
        },
    );
    allowed(&p, &other_server);
    assert!(deny_reason(&p.check(&search)).is_some());

    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &["mcp__github"],
    );
    assert!(deny_reason(&p.check(&issue)).is_some());
    assert!(deny_reason(&p.check(&search)).is_some());
    allowed(&p, &other_server);
}

// ---------------------------------------------------------------------------------------------
// Hardening against prompt-injected agents
// ---------------------------------------------------------------------------------------------

fn not_auto_allowed(p: &ExecutionPolicy, cmd: &str) {
    let d = p.check(&bash(cmd));
    assert!(!d.is_allow(), "`{cmd}` must not run unasked, got {d:?}");
}

#[test]
fn only_fully_understood_commands_are_read_only() {
    let f = fx();
    for mode in [PermissionMode::Default, PermissionMode::Plan] {
        let p = with_perms(&f, AgentName::Implementer, mode, &[], &[], &[]);
        for cmd in [
            "env --unset cat bash -c id",
            "env -S 'bash -c id' cat",
            "exec -a cat bash -c id",
            "strace -o cat bash -c id",
            "nice cat x",
            "LD_PRELOAD=./x.so cat x",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=id git status",
            "GIT_EXTERNAL_DIFF=id git diff",
            "git -c core.fsmonitor=id status",
            "git --config-env=core.pager=X log",
            "git --exec-path=/tmp status",
            "git --git-dir=/tmp/x status",
            "git -p log",
            "git diff --ext-diff",
            "git log --out=/tmp/x",
            "rg --pre sh foo",
            "rg --pre=sh foo",
            "sort --compress-program=sh x",
            "sort --compress-prog=sh x",
            "sort -uo out.txt x",
            "sort --out=out.txt x",
            "tree -ao out.txt",
            "file -C -m x",
            "./cat x",
            "/tmp/cat x",
            "rg $PAT src",
            "rg foo *",
        ] {
            not_auto_allowed(&p, cmd);
        }
        for cmd in [
            "git status",
            "git log --oneline -5",
            "git diff --stat",
            "git --no-pager log -1",
            "rg foo src",
            "cat README.md",
            "ls -la",
            "sort -u x",
            "echo $HOME",
        ] {
            allowed(&p, &bash(cmd));
        }
    }
}

#[test]
fn unresolved_write_targets_are_never_auto_allowed() {
    let f = fx();
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
    ] {
        let p = with_perms(&f, AgentName::Implementer, mode, &[], &[], &[]);
        for cmd in [
            "echo pwn >> \"$HOME/.bashrc\"",
            "echo pwn >> ~/.bashr?",
            "echo pwn > $(echo ~/.bashrc)",
            "cd \"$D\" && echo x > a",
            "tee \"$X\" < /dev/null",
            "cp a \"$DEST\"",
            "dd if=/dev/zero of=\"$T\"",
            "echo x > ~root/.bashrc",
        ] {
            not_auto_allowed(&p, cmd);
        }
    }
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &[],
    );
    allowed(
        &p,
        &bash(format!("echo x > {}", shp(f.session_dir.join("n.md")))),
    );
}

#[test]
fn git_runs_unasked_only_inside_the_project() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &[],
    );
    allowed(&p, &bash("git status"));
    allowed(&p, &bash("git -C src log -1"));
    not_auto_allowed(&p, &format!("git -C {} status", shp(&f.session_dir)));
    not_auto_allowed(&p, "cd /tmp && git status");
    not_auto_allowed(&p, "git -C \"$R\" status");
}

#[test]
fn git_metadata_is_written_only_by_git() {
    let f = fx();
    let yolo = {
        let mut ctx = f.ctx(AgentName::Implementer);
        ctx.yolo = true;
        ExecutionPolicy::new(ctx, PolicyInputs::default())
    };
    for call in [
        write(f.repo.join(".git/config")),
        write(f.repo.join(".git/hooks/pre-commit")),
        write(f.repo.join(".git")),
        write(f.session_dir.join("r/.git/config")),
        bash("echo '[core] fsmonitor = id' >> .git/config"),
        bash(format!(
            "cp hook {}",
            shp(f.repo.join(".git/hooks/post-merge"))
        )),
    ] {
        assert_eq!(guard_of(&yolo, &call), "git-metadata", "{}", call.input);
    }
    allowed(&yolo, &write(f.repo.join(".gitignore")));
}

#[test]
fn files_that_run_code_later_always_ask() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::AcceptEdits,
        &[],
        &[],
        &[],
    );
    for rel in [
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".mcp.json",
        ".codex/config.toml",
        ".vscode/tasks.json",
        ".envrc",
        ".github/workflows/ci.yml",
        ".husky/pre-commit",
    ] {
        assert!(is_ask(&p.check(&write(f.repo.join(rel)))), "{rel}");
        assert!(
            is_ask(&p.check(&bash(format!("echo x > {rel}")))),
            "shell write to {rel}"
        );
    }
    allowed(&p, &write(f.repo.join("src/a.ts")));
    let plan = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Plan,
        &[],
        &[],
        &[],
    );
    assert!(deny_reason(&plan.check(&write(f.repo.join(".mcp.json")))).is_some());
}

#[test]
fn credentials_and_process_memory_are_never_read() {
    let f = fx();
    let data = ostra_core::paths::data_dir();
    let yolo = {
        let mut ctx = f.ctx(AgentName::Implementer);
        ctx.yolo = true;
        ExecutionPolicy::new(ctx, PolicyInputs::default())
    };
    let read = |p: &str| ToolCall::new("Read", json!({"file_path": p}));
    for call in [
        read(&data.join("registry.db").to_string_lossy()),
        read(&data.join("master.key").to_string_lossy()),
        read("/proc/self/environ"),
        read("/proc/1/environ"),
        ToolCall::new(
            "Grep",
            json!({"pattern": "sk-", "path": data.to_string_lossy()}),
        ),
        ToolCall::new(
            "Skill",
            json!({"path": data.join("registry.db").to_string_lossy()}),
        ),
        read(&ostra_core::paths::workspace_db(&f.ws).to_string_lossy()),
        // Quoted, because the macOS data dir is under `Application Support`.
        bash(format!("cat '{}'", data.join("registry.db").display())),
        bash("xargs -0 -n1 < /proc/$PPID/environ"),
        bash("strings /proc/1/environ"),
    ] {
        assert_eq!(guard_of(&yolo, &call), "secret-read", "{}", call.input);
    }
    let home = std::env::var("HOME").unwrap();
    for rel in [
        ".ssh/id_ed25519",
        ".aws/credentials",
        ".claude/.credentials.json",
    ] {
        let call = read(&format!("{home}/{rel}"));
        assert_eq!(guard_of(&yolo, &call), "secret-read", "{rel}");
    }
    allowed(
        &yolo,
        &read(&data.join("assets/agents/x.md").to_string_lossy()),
    );
    allowed(&yolo, &read("/proc/cpuinfo"));
}

#[test]
fn readable_entries_open_only_the_credentials_they_name() {
    let f = fx();
    let mut ctx = f.ctx(AgentName::Implementer);
    let data = ostra_core::paths::data_dir();
    ctx.sandbox_readable = vec![
        "~/.netrc".into(),
        "~/.docker/config.json".into(),
        "~/".into(),
        data.to_string_lossy().into_owned(),
    ];
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    let home = std::env::var("HOME").unwrap();
    let read = |p: &str| ToolCall::new("Read", json!({"file_path": p}));
    allowed(&p, &read(&format!("{home}/.netrc")));
    allowed(&p, &read(&format!("{home}/.docker/config.json")));
    allowed(&p, &bash("cat ~/.netrc"));
    for path in [
        format!("{home}/.docker/other.json"),
        format!("{home}/.ssh/id_ed25519"),
        data.join("registry.db").to_string_lossy().into_owned(),
    ] {
        assert_eq!(guard_of(&p, &read(&path)), "secret-read", "{path}");
    }
}

#[test]
fn skill_paths_follow_read_rules() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &[],
        &[],
        &["Read(~/.ssh/**)"],
    );
    let home = std::env::var("HOME").unwrap();
    let d = p.check(&ToolCall::new(
        "Skill",
        json!({"path": format!("{home}/.ssh/id_ed25519")}),
    ));
    assert!(deny_reason(&d).is_some(), "{d:?}");
}

#[test]
fn webfetch_rules_match_the_host_reqwest_connects_to() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        &["WebFetch(domain:docs.rs)"],
        &[],
        &[],
    );
    allowed(
        &p,
        &ToolCall::new("WebFetch", json!({"url": "https://docs.rs/x"})),
    );
    let d = p.check(&ToolCall::new(
        "WebFetch",
        json!({"url": "https://evil.example\\@docs.rs/x"}),
    ));
    assert!(!d.is_allow(), "{d:?}");
}

#[test]
fn statements_outside_the_command_model_are_not_read_only() {
    let f = fx();
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
    ] {
        let p = with_perms(&f, AgentName::Implementer, mode, &[], &[], &[]);
        for cmd in [
            "export GIT_CONFIG_VALUE_0=x; git status",
            "FOO=bar; git status",
            "declare -x PATH=/tmp; ls",
            "unset HOME; ls",
            "printf -v PATH '%s' /tmp; ls",
            "[[ -v 'a[$(id)]' ]]",
            "test -v 'a[$(id)]'",
            "for f in a b; do cat $f; done",
            "f() { cat x; }; f",
        ] {
            not_auto_allowed(&p, cmd);
        }
        for cmd in [
            "test -f README.md",
            "[ -d src ]",
            "if test -f x; then cat x; fi",
        ] {
            allowed(&p, &bash(cmd));
        }
    }
}

#[test]
fn every_spelling_of_a_copy_target_is_checked() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::AcceptEdits,
        &[],
        &[],
        &[],
    );
    let outside = f
        .session_root
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("elsewhere");
    let o = shp(&outside);
    for cmd in [
        format!("cp --target-directory {o} x"),
        format!("cp --target-directory={o} x"),
        format!("cp --target-dir={o} x"),
        format!("cp -t{o} x"),
        format!("mv --target-directory={o} x"),
        "cp x {src/a.ts,/tmp/../etc/cron.d/x}".to_string(),
    ] {
        not_auto_allowed(&p, &cmd);
    }
    allowed(&p, &bash("cp src/a.ts src/b.ts"));
    allowed(&p, &bash("mv src/a.ts src/b.ts"));
}

#[test]
fn listing_forms_of_git_stay_read_only() {
    let f = fx();
    let p = with_perms(
        &f,
        AgentName::Implementer,
        PermissionMode::Plan,
        &[],
        &[],
        &[],
    );
    for cmd in [
        "git branch",
        "git branch -a",
        "git remote -v",
        "git tag",
        "git describe --tags",
        "git config --get user.name",
    ] {
        allowed(&p, &bash(cmd));
    }
    for cmd in [
        "git branch evil",
        "git tag v9",
        "git remote add x y",
        "git config core.fsmonitor id",
        "rg --hostname-bin=sh x",
        "tree -R",
    ] {
        not_auto_allowed(&p, cmd);
    }
    allowed(&p, &bash("echo see ~/.ssh/config"));
}

#[test]
fn workspace_artifacts_are_read_but_never_written() {
    // Rule W1.
    let f = fx();
    let dir = ostra_core::artifacts::dir(&f.ws);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sample.csv"), "a,b\n").unwrap();
    let p = f.policy(AgentName::Implementer);
    let file = dir.join("sample.csv");
    allowed(
        &p,
        &ToolCall::new("Read", json!({"file_path": file.to_string_lossy()})),
    );
    allowed(&p, &bash(format!("python3 scripts/load.py {}", shp(&file))));
    assert_eq!(guard_of(&p, &write(&file)), "workspace-artifacts");
    assert_eq!(
        guard_of(&p, &bash(format!("cp src/x {}/y.md", shp(&dir)))),
        "workspace-artifacts"
    );
    assert_eq!(
        guard_of(&p, &bash(format!("rm {}", shp(&file)))),
        "workspace-artifacts"
    );
}

#[test]
fn workspace_books_are_read_but_never_written() {
    // Rule B5.
    let f = fx();
    let dir = ostra_core::book::book_dir(&f.ws, "api_web");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.md"), "# Book\n").unwrap();
    let file = dir.join("index.md");
    for agent in [AgentName::Implementer, AgentName::Documentation] {
        let p = f.policy(agent);
        allowed(
            &p,
            &ToolCall::new("Read", json!({"file_path": file.to_string_lossy()})),
        );
        assert_eq!(guard_of(&p, &write(&file)), "workspace-docs", "{agent}");
        assert_eq!(
            guard_of(&p, &bash(format!("rm {}", shp(&file)))),
            "workspace-docs",
            "{agent}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Windows path forms (WINDOWS_HANDOVER 1.4): the guards compare paths, and Windows has more ways
// to spell one file than Linux. Each case names a protected file under a spelling a naive guard
// would miss. These run on Windows only; on Unix the spellings are ordinary path text.
// ---------------------------------------------------------------------------------------------

/// Writes to the same protected file named several ways all hit the git-metadata guard.
#[cfg(windows)]
#[test]
fn windows_spellings_of_a_git_path_are_all_caught() {
    let f = fx();
    std::fs::create_dir_all(f.repo.join(".git/hooks")).unwrap();
    let p = f.policy(AgentName::Implementer);
    // A backslash path, a mixed-case path (NTFS folds case), a path with a trailing dot Win32
    // strips, and an 8.3-style parent all name `.git\hooks\pre-commit`.
    for raw in [
        format!(r"{}\.git\hooks\pre-commit", shp_win(&f.repo)),
        format!(r"{}\.GIT\Hooks\pre-commit", shp_win(&f.repo)),
        format!(r"{}\.git\hooks\pre-commit.", shp_win(&f.repo)),
        format!("{}/.git/hooks/pre-commit", shp(&f.repo)),
    ] {
        assert_eq!(
            guard_of(
                &p,
                &ToolCall::new("Write", json!({"file_path": raw, "content": "x"}))
            ),
            "git-metadata",
            "{raw}"
        );
    }
}

/// The self-protection guard covers the tool's own files named with a drive path, a mixed case,
/// and a trailing space, because NTFS opens all three as the same file.
#[cfg(windows)]
#[test]
fn windows_spellings_of_a_protected_file_are_all_caught() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    for raw in [
        shp_win(&f.config),
        shp_win(&f.config).to_uppercase(),
        format!("{} ", shp_win(&f.config)),
        shp(&f.config),
    ] {
        assert_eq!(
            guard_of(
                &p,
                &ToolCall::new("Write", json!({"file_path": raw, "content": "x"}))
            ),
            "self-protection",
            "{raw}"
        );
    }
}

/// An MSYS path a Git Bash command uses names the same file as its Windows form, so a write to the
/// tool's config under `/c/...` is refused the same way.
#[cfg(windows)]
#[test]
fn msys_paths_in_bash_resolve_to_windows_paths() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    let msys = format!(
        "/{}/{}",
        shp(&f.config).replace(':', "").chars().next().unwrap(),
        &shp(&f.config)[3..]
    );
    assert_eq!(
        guard_of(&p, &bash(format!("echo x > {msys}"))),
        "self-protection",
        "{msys}"
    );
}

/// The path-form refusals apply to file tools only: in a shell word a `:` is a URL or a git
/// revision, not an alternate data stream.
#[cfg(windows)]
#[test]
fn urls_and_git_revisions_in_shell_words_are_not_path_forms() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    allowed(&p, &bash("curl -fsSL https://example.com/x.json"));
    allowed(&p, &bash("git show HEAD:src/a.rs"));
    let read = |raw: &str| ToolCall::new("Read", json!({"file_path": raw}));
    assert_eq!(
        guard_of(&p, &read(r"C:\x\notes.txt:hidden")),
        "windows-path"
    );
    assert_eq!(guard_of(&p, &read(r"\\localhost\C$\x")), "windows-path");
}

/// The Windows credential stores under the profile (DPAPI keys, Credential Manager, browser
/// profiles) are secret for every tool, in any letter case, as a tool path or a shell word.
#[cfg(windows)]
#[test]
fn windows_credential_stores_are_never_read() {
    let f = fx();
    let p = f.policy(AgentName::Implementer);
    let home = ostra_core::paths::home().unwrap();
    let read = |path: String| ToolCall::new("Read", json!({"file_path": path}));
    for rel in [
        r"AppData\Roaming\Microsoft\Protect\CREDHIST",
        r"APPDATA\roaming\microsoft\credentials\x",
        r"AppData\Local\Google\Chrome\User Data\Default\Login Data",
        r"AppData\Roaming\Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt",
        r".ssh\id_ed25519",
    ] {
        let path = home.join(rel).display().to_string();
        assert_eq!(guard_of(&p, &read(path.clone())), "secret-read", "{path}");
    }
    assert_eq!(
        guard_of(
            &p,
            &bash(format!(
                "cat '{}'",
                shp(home.join(r"AppData\Roaming\Microsoft\Protect\CREDHIST"))
            ))
        ),
        "secret-read"
    );
}

/// A junction needs no privilege on Windows, so an agent can make one inside the repo that leads
/// out of it. A write through it resolves to the target and is refused, and a Bash path with a
/// `..` right after it is refused because Git Bash and Win32 read that `..` differently.
#[cfg(windows)]
#[test]
fn junctions_cannot_lead_writes_out_of_the_repo() {
    let f = fx();
    let outside = f.repo.parent().unwrap().join("outside");
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    ostra_core::paths::link(&outside, &f.repo.join("jn")).unwrap();
    let p = f.policy(AgentName::Implementer);
    assert_eq!(
        guard_of(&p, &write(f.repo.join(r"jn\x.txt"))),
        "write-scope"
    );
    assert_eq!(
        guard_of(&p, &bash(format!("echo x > {}/jn/x.txt", shp(&f.repo)))),
        "write-scope"
    );
    assert_eq!(
        guard_of(&p, &bash("echo x > jn/deep/../../src/y.txt")),
        "windows-path"
    );
    // A plain write inside the repo is unaffected.
    allowed(&p, &write(f.repo.join(r"src\ok.txt")));
}

/// PowerShell and cmd commands are opaque: never auto-allowed, and refused when they name Ostra's
/// own files or state, even though Ostra cannot parse them.
#[cfg(windows)]
#[test]
fn powershell_and_cmd_are_opaque_and_hardened() {
    let f = fx();
    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.permission_mode = PermissionMode::Default;
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    let ps = |c: &str| ToolCall::new("PowerShell", json!({"command": c}));
    let cmd = |c: &str| ToolCall::new("Cmd", json!({"command": c}));
    // Even a plain read is not auto-allowed under the default mode: it asks.
    assert!(
        is_ask(&p.check(&ps("Get-ChildItem"))),
        "{:?}",
        p.check(&ps("Get-ChildItem"))
    );
    assert!(is_ask(&p.check(&cmd("dir"))));
    // Naming Ostra's own config is refused outright.
    assert_eq!(
        guard_of(&p, &ps(&format!("Set-Content '{}' x", shp_win(&f.config)))),
        "self-protection"
    );
    // Naming a state file by its name is refused as state, and by an engine-state path as
    // self-protection; either way an opaque command cannot touch it.
    assert_eq!(
        guard_of(&p, &cmd("type ostra-review-ledger.md")),
        "state-ownership"
    );
    assert_eq!(
        guard_of(
            &p,
            &cmd(&format!(
                "type {}\\.state\\gates.json",
                shp_win(&f.session_root)
            ))
        ),
        "self-protection"
    );
    // Under bypass an ordinary command runs.
    let yolo = {
        let mut ctx = f.ctx(AgentName::Implementer);
        ctx.yolo = true;
        ExecutionPolicy::new(ctx, PolicyInputs::default())
    };
    allowed(&yolo, &ps("Get-Date"));
}

/// A drive path renderer that keeps backslashes, for the Windows spelling cases. Verbatim prefix
/// dropped so the guard sees a plain drive path.
#[cfg(windows)]
fn shp_win(p: impl AsRef<Path>) -> String {
    p.as_ref()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

fn project_create() -> ToolCall {
    ToolCall::new(
        "ProjectCreate",
        json!({"key": "notes-mcp", "stack": "rust", "purpose": "An MCP server for the team's notes.",
               "requirements": ["Rust 2024", "rmcp 3.5 over stdio"]}),
    )
}

/// The implementer of a phase the plan puts in `notes-mcp`, which does not exist yet.
fn creator(f: &Fx) -> ExecContext {
    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.project_key = "notes-mcp".into();
    ctx.repo_root = f.session_dir.clone();
    ctx.creates_project = true;
    ctx
}

// Rule O1: creating a project asks the user in every mode that allows writes, even bypass and an
// allow rule; a deny rule refuses it, plan mode refuses it, and only YOLO answers it.
#[test]
fn project_create_always_asks_unless_yolo() {
    let f = fx();
    let call = project_create();
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Bypass,
    ] {
        let mut ctx = creator(&f);
        ctx.permission_mode = mode;
        ctx.permissions.allow = vec!["ProjectCreate".into()];
        let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
        match p.check(&call) {
            PolicyDecision::Ask { reason, rule } => {
                assert_eq!(rule.rule, "manage");
                assert!(
                    reason.starts_with("Create project `notes-mcp` (rust)"),
                    "{reason}"
                );
            }
            other => panic!("{mode:?}: expected an ask, got {other:?}"),
        }
        assert_eq!(
            p.allow_rule_suggestion(&call),
            None,
            "no rule stands in for the user"
        );
        p.set_yolo(true);
        match p.check(&call) {
            PolicyDecision::Allow { rule: Some(r) } => assert_eq!(r.rule, "yolo"),
            other => panic!("{mode:?} under YOLO: expected allow, got {other:?}"),
        }
    }
    let mut ctx = creator(&f);
    ctx.permissions.deny = vec!["ProjectCreate".into()];
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    p.set_yolo(true);
    denied(&p, &call, "permission rule `ProjectCreate`");
    let mut ctx = creator(&f);
    ctx.permission_mode = PermissionMode::Plan;
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    denied(&p, &call, "plan mode");
}

// Rule O2: only the implementer of a phase in a project the plan names as new creates it, only
// that key, and only with a well-formed call, which is refused before the user is asked.
#[test]
fn project_create_is_guarded() {
    let f = fx();
    let call = project_create();
    // Rule CA6: whatever its name, an agent without `manage_projects` creates nothing.
    for agent in [AgentName::GenerateSpec, AgentName::Plan, AgentName::Explore] {
        let mut ctx = creator(&f);
        ctx.agent = agent;
        ctx.capabilities = f.ctx(agent).capabilities;
        let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
        p.set_yolo(true);
        assert_eq!(guard_of(&p, &call), "manage-tools", "{agent}");
    }
    let p = f.policy(AgentName::Implementer);
    p.set_yolo(true);
    denied(&p, &call, "holds `manage_projects`");
    let mut ctx = creator(&f);
    ctx.session_id = None;
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    assert_eq!(guard_of(&p, &call), "manage-tools");
    let p = ExecutionPolicy::new(creator(&f), PolicyInputs::default());
    let other = ToolCall::new(
        "ProjectCreate",
        json!({"key": "other", "stack": "rust", "purpose": "p", "requirements": ["x"]}),
    );
    denied(&p, &other, "Call ProjectCreate with key `notes-mcp`");
    let bad = ToolCall::new(
        "ProjectCreate",
        json!({"key": "Notes", "stack": "rust", "purpose": "p", "requirements": ["x"]}),
    );
    denied(&p, &bad, "Use a project key");
    let outside = ToolCall::new(
        "ProjectCreate",
        json!({"key": "notes-mcp", "stack": "rust", "purpose": "p", "requirements": ["x"], "folder": "../elsewhere"}),
    );
    denied(&p, &outside, "relative to the workspace root");
}

// Rule O2: before the project exists, its phase's run writes only in its session dir and temp.
#[test]
fn a_run_that_creates_a_project_writes_nothing_else_first() {
    let f = fx();
    let p = ExecutionPolicy::new(creator(&f), PolicyInputs::default());
    assert_eq!(
        guard_of(&p, &write(f.ws.join("notes-mcp/Cargo.toml"))),
        "manage-tools"
    );
    denied(
        &p,
        &write(f.repo.join("src/a.rs")),
        "Call ProjectCreate for `notes-mcp` first",
    );
    let shell = ToolCall::new(
        "Bash",
        json!({"command": format!("mkdir -p {}", shp(f.ws.join("notes-mcp")))}),
    );
    assert_eq!(guard_of(&p, &shell), "manage-tools");
    allowed(&p, &write(f.session_dir.join("notes.md")));
}

#[test]
fn project_list_is_allowed_for_every_agent() {
    let f = fx();
    for agent in [AgentName::GenerateSpec, AgentName::Implementer] {
        let mut ctx = f.ctx(agent);
        ctx.permission_mode = PermissionMode::Plan;
        let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
        allowed(&p, &ToolCall::new("ProjectList", json!({})));
    }
}

#[test]
fn sm2_messaging_tools_never_ask_the_user() {
    let f = fx();
    let mut ctx = f.ctx(AgentName::GenerateSpec);
    ctx.permission_mode = PermissionMode::Plan;
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    for tool in ["ListAgents", "SendMessage", "WaitForMessage"] {
        allowed(&p, &ToolCall::new(tool, json!({"message": "m"})));
    }
}

// ---------------------------------------------------------------------------------------------
// Tool enforcement disabled (Rule G1)
// ---------------------------------------------------------------------------------------------

impl Fx {
    fn relaxed(&self, agent: AgentName) -> ExecutionPolicy {
        let ctx = ExecContext {
            enforce_tool_calls: false,
            ..self.ctx(agent)
        };
        ExecutionPolicy::new(ctx, PolicyInputs::default())
    }
}

#[test]
fn without_tool_enforcement_write_scope_report_path_and_self_protection_are_off() {
    let f = fx();
    let config = shp(&f.config);
    let script = format!(
        "python3 - <<'EOF'\nfor p in ['{0}/a.py', '{0}/b.py']:\n    open(p, 'w').write('x')\nEOF",
        shp(f.repo.join("src"))
    );
    let inline = format!(
        "python3 -c \"import subprocess; open('{}', 'w').write('x')\"",
        shp(f.repo.join("src/a.py"))
    );

    let strict = f.policy(AgentName::Implementer);
    assert_eq!(guard_of(&strict, &bash(&script)), "self-protection");
    assert_eq!(guard_of(&strict, &bash(&inline)), "self-protection");

    let p = f.relaxed(AgentName::Implementer);
    allowed(&p, &bash(&script));
    allowed(&p, &bash(&inline));
    allowed(&p, &write(f.repo.join("../outside.txt")));
    allowed(&p, &write(&f.config));
    allowed(&p, &bash(format!("sed -i 's/deny/allow/' {config}")));
    allowed(&p, &bash("ostra --version"));

    let reviewer = f.relaxed(AgentName::CodeReviewer);
    allowed(&reviewer, &write(f.repo.join("src/main.rs")));
    allowed(
        &f.relaxed(AgentName::QuickAnswer),
        &write(f.repo.join("src/main.rs")),
    );
    allowed(
        &f.relaxed(AgentName::Plan),
        &document(f.repo.join("ostra-plan-x.md")),
    );

    let mut ctx = f.ctx(AgentName::Implementer);
    ctx.enforce_tool_calls = false;
    ctx.report_file = Some(f.session_dir.join("ostra-implementer-phase-1.md"));
    let p = ExecutionPolicy::new(ctx, PolicyInputs::default());
    allowed(&p, &write(f.session_dir.join("ostra-implementer-other.md")));
}

#[test]
fn without_tool_enforcement_ownership_secrets_git_and_tests_still_hold() {
    let f = fx();
    let p = f.relaxed(AgentName::Implementer);
    let state = f.session_root.join(".state/gates.json");

    assert_eq!(
        guard_of(&p, &write(f.repo.join("tests/api_test.go"))),
        "no-tests-from-implementer"
    );
    assert_eq!(
        guard_of(&p, &write(f.repo.join(".git/hooks/pre-commit"))),
        "git-metadata"
    );
    assert_eq!(guard_of(&p, &write(&state)), "state-ownership");
    assert_eq!(
        guard_of(&p, &write(f.ws.join("workspace.db"))),
        "state-ownership"
    );
    assert_eq!(
        guard_of(
            &p,
            &bash(format!(
                "python3 -c \"open('{}', 'w').write('x')\"",
                shp(&state)
            ))
        ),
        "state-ownership"
    );
    assert_eq!(
        guard_of(&p, &bash(format!("sed -i 's/a/b/' {}", shp(&state)))),
        "state-ownership"
    );
    assert_eq!(
        guard_of(&p, &write(f.session_dir.join("ostra-spec-x.md"))),
        "artifact-ownership"
    );
    let registry = ostra_core::paths::data_dir().join("registry.db");
    assert_eq!(
        guard_of(
            &p,
            &ToolCall::new("Read", json!({"file_path": registry.to_string_lossy()}))
        ),
        "secret-read"
    );
}

// Rule G2: a sandboxed execution's searches never include what a `.*ignore` file hides.
#[test]
fn sandboxed_searches_skip_ignored_paths() {
    let f = fx();
    let repo = &f.repo;
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("target/debug")).unwrap();
    std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let p = f.policy(AgentName::Implementer).sandboxed(true);
    let grep = |path: &str| ToolCall::new("Grep", json!({"pattern": "x", "path": path}));

    allowed(&p, &grep(&shp(repo.join("src"))));
    allowed(&p, &bash("rg main src"));
    allowed(&p, &bash("grep -rn main src"));
    allowed(&p, &bash("find src -name '*.rs'"));
    allowed(&p, &bash("cat target/debug/out"));
    allowed(&p, &bash("git ls-files --others --exclude-standard"));
    denied(&p, &grep(&shp(repo.join("target/debug"))), ".gitignore");
    denied(&p, &bash("rg main target"), ".gitignore");
    denied(&p, &bash("rg -uu main"), "-uu");
    denied(&p, &bash("rg --no-ignore-vcs main src"), "--no-ignore-vcs");
    denied(&p, &bash("fd -I main"), "-I");
    denied(&p, &bash("grep -rn main ."), "target");
    denied(&p, &bash("grep -R main"), "target");
    denied(&p, &bash("find . -name '*.rs'"), "target");
    denied(&p, &bash("ls -R"), "target");
    denied(&p, &bash("git status --ignored"), "--ignored");
    denied(&p, &bash("bash -c 'grep -rn main .'"), "target");
    denied(&p, &bash("echo . | xargs grep -rn main"), "target");
    denied(&p, &bash("cd target && rg main"), ".gitignore");
    denied(&p, &bash("git ls-files -o"), "--others");

    // Every `.*ignore` file counts, not only the ones ripgrep reads.
    std::fs::write(repo.join(".dockerignore"), "src/secret.txt\n").unwrap();
    std::fs::write(repo.join("src/secret.txt"), "x").unwrap();
    denied(&p, &bash("grep -rn main src"), "secret.txt");
    denied(
        &p,
        &grep(&shp(repo.join("src/secret.txt"))),
        ".dockerignore",
    );

    // Ostra's own state is searchable although its `.gitignore` keeps it out of git.
    std::fs::write(f.session_root.parent().unwrap().join(".gitignore"), "*\n").unwrap();
    std::fs::write(f.session_dir.join("report.md"), "x").unwrap();
    allowed(&p, &grep(&shp(&f.session_dir)));
    allowed(&p, &bash(format!("grep -rn x {}", shp(&f.session_dir))));

    // Without a sandbox the rule does not apply.
    let open = f.policy(AgentName::Implementer);
    allowed(&open, &bash("grep -rn main ."));
    allowed(&open, &grep(&shp(repo.join("target/debug"))));
}
