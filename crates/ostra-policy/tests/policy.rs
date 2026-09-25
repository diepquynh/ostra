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
    let root = std::fs::canonicalize(tmp.path()).unwrap();
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
    fn ctx(&self, agent: AgentName) -> ExecContext {
        ExecContext {
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

    let docs = f.policy(AgentName::ModuleDocumentation);
    allowed(
        &docs,
        &write(f.repo.join(".ostra/skills/module-hub/references/auth.md")),
    );
    allowed(
        &docs,
        &write(f.repo.join(".agents/skills/module-hub/references/auth.md")),
    );
    denied(
        &docs,
        &write(f.repo.join(".ostra/skills/convention/SKILL.md")),
        "outside the scope",
    );
    denied(
        &docs,
        &write(f.repo.join("src/App.ts")),
        "outside the scope",
    );

    let imp = f.policy(AgentName::Implementer);
    allowed(&imp, &write(f.repo.join("src/App.ts")));
    let r = denied(&imp, &write(f.repo.join("src/App.test.ts")), "Constraint 6");
    assert!(r.starts_with("Leave test files to write-test"));
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
            f.session_dir.join("ledger.md").display()
        )),
    );
    allowed(
        &f.policy(AgentName::Plan),
        &bash(format!(
            "cat > {} <<'EOF'\nthe `<!-- AWS START --> ... <!-- AWS END -->` group\nrm -rf /somewhere\nEOF",
            f.session_root.join("plan-phase-1.md").display()
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
        &bash(format!(
            "echo bad > {}",
            f.repo.join("src/App.ts").display()
        )),
        "never modifies project source",
    );
    denied(
        &f.policy(AgentName::Implementer),
        &bash(format!(
            "cat <<'EOF' > {}\nhi\nEOF",
            f.repo.join("src/App.test.ts").display()
        )),
        "Constraint 6",
    );
    denied(
        &f.policy(AgentName::WriteTest),
        &bash(format!("rm -rf {}", f.repo.join("../sibling").display())),
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
            test_commands: vec![],
        },
    );

    allowed(&p, &write(&report));
    allowed(
        &p,
        &bash(format!(
            "cat > \"{}\" <<'REPORT_EOF'\n# Implementation Report\nDid it.\nREPORT_EOF",
            report.display()
        )),
    );
    allowed(
        &p,
        &bash(format!("echo \"## More\" >> {}", report.display())),
    );

    let invented = f.session_dir.join("ostra-implementer-credentials-uri.md");
    let r = denied(&p, &write(&invented), "declared report path");
    assert!(r.starts_with(&format!(
        "Write your report to \"{}\" instead",
        report.display()
    )));
    denied(
        &p,
        &bash(format!("echo x > {}", invented.display())),
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
            f.session_dir.join("ostra-security-block.json").display()
        )),
        "code-reviewer",
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
        &bash(format!("cat > \"{}\" <<'EOF'\nx\nEOF", report.display())),
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
    let bin = f.bin.display().to_string();
    let config = f.config.display().to_string();
    for cmd in [
        format!("{bin} hook --execution x"),
        "ostra mcp-stdio --execution x".to_string(),
        format!("bash -c \"cat /tmp/p.json > {config}\""),
        format!("sed -i 's/deny/allow/' {config}"),
        format!("rm {config}"),
        format!("chmod -x {bin}"),
        format!("echo '{{}}' > {config}"),
        format!("cd {} && ./ostra hook", f.bin.parent().unwrap().display()),
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

    let state = f
        .session_root
        .join(".state/gates.json")
        .display()
        .to_string();
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
            f.session_dir.display()
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
            f.session_dir.join("notes.md").display()
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
                    f.repo.join(".ostra/memory/knowledge.sqlite3").display()
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
        &bash(format!("cat {}", f.session_root.join(".state/x").display())),
    );
    assert_eq!(
        guard_of(
            &f.policy(AgentName::Implementer),
            &bash(format!(
                "sqlite3 {} 'delete from lessons'",
                f.repo.join(".ostra/memory/knowledge.sqlite3").display()
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
        "code-reviewer, implementer, write-test",
    );
    denied(
        &f.policy(AgentName::Implementer),
        &write(f.session_dir.join("ostra-security-block.json")),
        "code-reviewer",
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
        "plan agent",
    );
    // Fact-check's snapshot copy is not the artifact.
    let fc = f.policy(AgentName::FactCheck);
    allowed(
        &fc,
        &bash(format!(
            "mkdir -p \"{0}/factcheck-snapshot-spec\" && cp \"{1}\" \"{0}/factcheck-snapshot-spec/\"",
            f.session_root.display(),
            spec.display()
        )),
    );
    denied(
        &fc,
        &bash(format!("cp /tmp/x \"{}\"", spec.display())),
        "generate-spec",
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
        &bash(format!("cat >> \"{}\" <<'EOF'\nx\nEOF", spec.display())),
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
            f.session_dir.join("n.md").display()
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
