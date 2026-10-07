use super::*;
use ostra_core::agent::AgentName;
use ostra_core::agent::Capability;
use ostra_core::config::{PermissionMode, PermissionRules, ResolvedRoute};
use ostra_core::exec::Wake;
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::ExecutionId;
use ostra_core::model::Effort;
use ostra_core::policy::RuleRef;
use ostra_providers::ScriptedProvider;
use ostra_providers::mock::response;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct FakeHost {
    deltas: Mutex<Vec<ExecutionDelta>>,
    messages: Mutex<Vec<(String, Value)>>,
    asks: Mutex<Vec<ToolCall>>,
    answer: Mutex<Option<PermissionAnswer>>,
    transcripts: Mutex<HashMap<ExecutionId, Vec<(String, Value)>>>,
    yolo: bool,
    /// Rule SM2: messages queued for the run, handed over one per turn boundary.
    inbox: Mutex<Vec<String>>,
    /// Rule SM6: the run owes a waiting sender a reply until it sends one.
    owes: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl ExecutionHost for FakeHost {
    fn emit(&self, delta: ExecutionDelta) {
        self.deltas.lock().push(delta);
    }
    async fn ask_permission(
        &self,
        call: &ToolCall,
        _reason: &str,
        _rule: &RuleRef,
    ) -> PermissionAnswer {
        self.asks.lock().push(call.clone());
        self.answer.lock().unwrap_or(PermissionAnswer::Deny)
    }
    fn record_message(&self, role: &str, content: &Value) {
        self.messages.lock().push((role.into(), content.clone()));
    }
    fn transcript(&self, execution: &ExecutionId) -> Vec<(String, Value)> {
        self.transcripts
            .lock()
            .get(execution)
            .cloned()
            .unwrap_or_default()
    }
    fn yolo(&self) -> bool {
        self.yolo
    }
    fn take_messages(&self) -> Option<Wake> {
        let mut inbox = self.inbox.lock();
        (!inbox.is_empty()).then(|| Wake {
            note: inbox.remove(0),
            owes_reply: false,
        })
    }
    fn submit_blocked(&self) -> Option<String> {
        self.owes
            .load(std::sync::atomic::Ordering::SeqCst)
            .then(|| ostra_core::coord::reply_instruction("SendMessage", &["x_sender".into()]))
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    session: PathBuf,
    outside: PathBuf,
}

fn fixture() -> Fixture {
    static CACHE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any test here starts a sandboxed program that reads it.
    CACHE.call_once(|| unsafe {
        std::env::set_var(
            "OSTRA_SANDBOX_CACHE",
            std::env::temp_dir().join("ostra-test-sandbox-cache"),
        );
    });
    // Outside OS temp, because the policy never asks for writes there.
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    // Strip the Windows `\\?\` verbatim prefix so derived paths match what the tools resolve to.
    let root =
        ostra_core::paths::strip_verbatim(&ostra_core::paths::canonical(dir.path()).unwrap());
    let repo = root.join("repo");
    let session = root.join("ws/.ostra/sessions/s_1/app");
    let outside = root.join("elsewhere");
    for d in [&repo, &session, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(repo.join(".ostra")).unwrap();
    std::fs::write(repo.join("main.txt"), "hello\n").unwrap();
    Fixture {
        _dir: dir,
        repo,
        session,
        outside,
    }
}

fn spec(
    f: &Fixture,
    agent: AgentName,
    mode: PermissionMode,
    caps: Vec<Capability>,
) -> ExecutionSpec {
    let id = ExecutionId::new();
    let report = f.session.join("ostra-implementer-phase-1.md");
    let def = ostra_agents::builtin_def(agent);
    let contract = def
        .map(|d| d.returns)
        .unwrap_or(ostra_core::Contract::Stage);
    ExecutionSpec {
        id: id.clone(),
        agent,
        route: ResolvedRoute {
            executor: ExecutorKind::Native,
            model: "mock:test".into(),
            tier: None,
        },
        effort: Effort::Medium,
        system_prompt: "You are a test agent.".into(),
        first_message: "Do the task.".into(),
        capabilities: caps.clone(),
        submit_schema: ostra_core::submit::submit_schema(contract),
        timeout_secs: 30,
        ctx: ExecContext {
            work_dirs: Vec::new(),
            execution_id: id,
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Native,
            workspace_root: f
                .session
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf(),
            repo_root: f.repo.clone(),
            project_key: "app".into(),
            session_dir: f.session.clone(),
            session_root: f.session.parent().unwrap().to_path_buf(),
            report_file: Some(report),
            phase: None,
            yolo: false,
            permission_mode: mode,
            permissions: PermissionRules::default(),
            protected_paths: vec![],
            memory_db: f.repo.join(".ostra/memory/knowledge.sqlite3"),
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
            contract,
            capabilities: caps.clone(),
        },
        resume: None,
        harness_session_id: None,
    }
}

fn executor(p: ScriptedProvider) -> (NativeExecutor, Arc<ScriptedProvider>) {
    let p = Arc::new(p);
    let providers = Providers::empty();
    providers.register("mock", p.clone());
    let resolver: SkillResolver = Arc::new(|_: &str| None);
    (NativeExecutor::new(Arc::new(providers), resolver, None), p)
}

fn impl_caps() -> Vec<Capability> {
    vec![
        Capability::Read,
        Capability::Write,
        Capability::Edit,
        Capability::Shell,
        Capability::Report,
        Capability::Memory,
    ]
}

fn submit_ok(report: &Path) -> Value {
    json!({"status": "ok", "report_path": report.display().to_string(), "changed_files": [], "summary": "done"})
}

fn tool_results(req: &ChatRequest) -> Vec<(String, bool)> {
    req.messages
        .last()
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolResult {
                        content, is_error, ..
                    } => Some((content.clone(), *is_error)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn reads_is_denied_outside_scope_reports_and_submits() {
    let f = fixture();
    let s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::AcceptEdits,
        impl_caps(),
    );
    let report = s.ctx.report_file.clone().unwrap();
    let p = ScriptedProvider::new();
    p.push_tool_use(
        "Read",
        json!({"file_path": f.repo.join("main.txt").display().to_string()}),
    );
    p.push_tool_use(
        "Write",
        json!({"file_path": f.outside.join("x.txt").display().to_string(), "content": "no"}),
    );
    p.push_tool_use("Report", json!({"content": "# Report\nDone."}));
    p.push_tool_use("submit_implementer", submit_ok(&report));
    let (exec, p) = executor(p);
    let host = Arc::new(FakeHost::default());
    let r = exec.run(s, host.clone(), CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    assert_eq!(r.submit.unwrap()["summary"], "done");
    let reqs = p.requests();
    assert!(tool_results(&reqs[1])[0].0.contains("hello"));
    let (denied, is_err) = &tool_results(&reqs[2])[0];
    assert!(*is_err && denied.starts_with("Denied by guard"), "{denied}");
    assert!(!f.outside.join("x.txt").exists());
    assert!(std::fs::read_to_string(&report).unwrap().contains("Done."));
    assert!(host.deltas.lock().iter().any(|d| matches!(
        d,
        ExecutionDelta::Policy {
            decision: PolicyDecision::Deny { .. },
            ..
        }
    )));
    assert_eq!(r.usage.tool_calls, 3);
    assert!(host.messages.lock().len() >= 8);
}

#[tokio::test]
async fn invalid_submit_is_retried() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.ctx.report_file = None;
    let p = ScriptedProvider::new();
    p.push_tool_use("submit_quick_answer", json!({"sources": []}));
    p.push_tool_use("submit_quick_answer", json!({"answer": "a", "sources": []}));
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    let (msg, is_err) = &tool_results(&p.requests()[1])[0];
    assert!(*is_err && msg.contains("invalid"), "{msg}");
}

#[tokio::test]
async fn explore_submits_only_after_its_document() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::Explore,
        PermissionMode::Default,
        vec![Capability::Read, Capability::DocumentResearch],
    );
    s.ctx.report_file = None;
    let md = f.session.join("ostra-research-1-greeting.md");
    let submit = json!({"research_path": md, "scope_covered": "a", "findings_summary": "b", "sources_retrieved": 0, "open_questions": 0, "not_covered": []});
    let p = ScriptedProvider::new();
    p.push_tool_use("submit_explore", submit.clone());
    p.push_tool_use(
        "Document",
        json!({"path": md, "document": {"title": "Greeting", "date": "2026-07-28", "repo": "app", "scope": "s", "problem": "p"}}),
    );
    p.push_tool_use("submit_explore", submit);
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    let reqs = p.requests();
    assert!(
        reqs[0]
            .tools
            .iter()
            .any(|t| t.name == "Document" && t.input_schema["$defs"].get("Document").is_some())
    );
    let (msg, is_err) = &tool_results(&reqs[1])[0];
    assert!(*is_err && msg.contains("Document tool"), "{msg}");
    let (msg, is_err) = &tool_results(&reqs[2])[0];
    assert!(
        !*is_err && msg.contains("Wrote the research document"),
        "{msg}"
    );
    assert!(
        std::fs::read_to_string(&md)
            .unwrap()
            .starts_with("# Research: Greeting")
    );
}

#[tokio::test]
async fn missing_submit_gets_one_reminder() {
    let f = fixture();
    let s = spec(
        &f,
        AgentName::Explore,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    let p = ScriptedProvider::new();
    p.push_text("I am done.");
    p.push_text("Really done.");
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    assert!(r.submit.is_none());
    assert_eq!(r.final_text, "Really done.");
    let reqs = p.requests();
    assert_eq!(reqs.len(), 2);
    let last = reqs[1].messages.last().unwrap();
    assert!(matches!(&last.content[0], Block::Text { text } if text.contains("submit_explore")));
}

#[tokio::test]
async fn permission_ask_allow_and_deny() {
    for (answer, expect_written) in [
        (PermissionAnswer::AllowOnce, true),
        (PermissionAnswer::Deny, false),
    ] {
        let f = fixture();
        let mut s = spec(
            &f,
            AgentName::Implementer,
            PermissionMode::Default,
            impl_caps(),
        );
        s.ctx.report_file = None;
        let target = f.repo.join("new.txt");
        let p = ScriptedProvider::new();
        p.push_tool_use(
            "Write",
            json!({"file_path": target.display().to_string(), "content": "x"}),
        );
        p.push_tool_use("submit_implementer", json!({"status": "ok", "report_path": "", "changed_files": ["new.txt"], "summary": "s"}));
        let (exec, _) = executor(p);
        let host = Arc::new(FakeHost {
            answer: Mutex::new(Some(answer)),
            ..Default::default()
        });
        let r = exec.run(s, host.clone(), CancellationToken::new()).await;
        assert_eq!(r.status, ExecutionStatus::Ok);
        assert_eq!(host.asks.lock().len(), 1);
        assert_eq!(target.exists(), expect_written);
    }
}

#[tokio::test]
async fn yolo_skips_asks() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        impl_caps(),
    );
    s.ctx.report_file = None;
    let target = f.repo.join("y.txt");
    let p = ScriptedProvider::new();
    p.push_tool_use(
        "Write",
        json!({"file_path": target.display().to_string(), "content": "x"}),
    );
    p.push_tool_use(
        "submit_implementer",
        json!({"status": "ok", "report_path": "", "changed_files": [], "summary": "s"}),
    );
    let (exec, _) = executor(p);
    let host = Arc::new(FakeHost {
        yolo: true,
        ..Default::default()
    });
    exec.run(s, host.clone(), CancellationToken::new()).await;
    assert!(host.asks.lock().is_empty());
    assert!(target.exists());
}

#[tokio::test]
async fn cancel_is_immediate() {
    let f = fixture();
    let s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Bypass,
        impl_caps(),
    );
    let p = ScriptedProvider::new();
    p.push_tool_use("Bash", json!({"command": "sleep 30"}));
    let (exec, _) = executor(p);
    let token = CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        t2.cancel();
    });
    let started = std::time::Instant::now();
    let r = exec.run(s, Arc::new(FakeHost::default()), token).await;
    assert_eq!(r.status, ExecutionStatus::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn timeout_is_an_error() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Bypass,
        impl_caps(),
    );
    s.timeout_secs = 1;
    let p = ScriptedProvider::new();
    p.push_tool_use("Bash", json!({"command": "sleep 30"}));
    let (exec, _) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Error);
    assert!(r.error.unwrap().contains("timed out after 1 s"));
}

#[tokio::test]
async fn asks_for_the_whole_output_limit_of_the_model() {
    ostra_core::pricing::install_test_prices();
    let f = fixture();
    for (model, expected) in [
        ("mock:claude-haiku-4-5-20251001", 64_000),
        ("mock:unlisted", crate::DEFAULT_MAX_OUTPUT),
    ] {
        let mut s = spec(&f, AgentName::QuickAnswer, PermissionMode::Default, vec![]);
        s.ctx.report_file = None;
        s.route.model = model.into();
        let p = ScriptedProvider::new();
        p.push_tool_use("submit_quick_answer", json!({"answer": "a"}));
        let (exec, p) = executor(p);
        let r = exec
            .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
            .await;
        assert_eq!(r.status, ExecutionStatus::Ok, "{model}: {:?}", r.error);
        assert_eq!(p.requests()[0].max_tokens, expected, "{model}");
    }
}

#[tokio::test]
async fn resumes_from_transcript() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.ctx.report_file = None;
    let from = ExecutionId::new();
    s.resume = Some(ostra_core::exec::ResumeInfo {
        from: from.clone(),
        native_session_id: None,
        note: None,
        inspect: false,
    });
    let host = FakeHost::default();
    host.transcripts.lock().insert(
        from,
        vec![
            ("user".into(), json!([{"type": "text", "text": "Do the task."}])),
            ("assistant".into(), json!([{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/x"}}])),
        ],
    );
    let p = ScriptedProvider::new();
    p.push_tool_use("submit_quick_answer", json!({"answer": "a"}));
    let (exec, p) = executor(p);
    let r = exec.run(s, Arc::new(host), CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    let req = &p.requests()[0];
    assert_eq!(req.messages.len(), 3);
    let last = &req.messages[2];
    assert!(
        matches!(&last.content[0], Block::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "t1")
    );
    assert!(matches!(&last.content[1], Block::Text { text } if text.contains("interrupted")));
}

#[tokio::test]
async fn resuming_in_place_records_only_the_new_turn() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.ctx.report_file = None;
    s.resume = Some(ostra_core::exec::ResumeInfo {
        from: s.id.clone(),
        native_session_id: None,
        note: Some("Continue the workflow.".into()),
        inspect: false,
    });
    let host = Arc::new(FakeHost::default());
    host.transcripts.lock().insert(
        s.id.clone(),
        vec![
            ("user".into(), json!([{"type": "text", "text": "Do the task."}])),
            ("assistant".into(), json!([{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/x"}}])),
            ("user".into(), json!([{"type": "tool_result", "tool_use_id": "t1", "content": "x", "is_error": false}])),
        ],
    );
    let p = ScriptedProvider::new();
    p.push_tool_use("submit_quick_answer", json!({"answer": "a"}));
    let (exec, p) = executor(p);
    let r = exec.run(s, host.clone(), CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    let req = &p.requests()[0];
    assert_eq!(req.messages.len(), 3, "the note joins the last user turn");
    assert!(
        matches!(&req.messages[2].content[..], [Block::ToolResult { .. }, Block::Text { text }] if text == "Continue the workflow.")
    );
    let recorded = host.messages.lock().clone();
    assert_eq!(
        recorded[0],
        (
            "user".to_string(),
            json!([{"type": "text", "text": "Continue the workflow."}])
        ),
        "the stored prefix is not recorded again"
    );
}

#[test]
fn rebuilding_merges_adjacent_turns_of_one_role() {
    let transcript = vec![
        (
            "user".to_string(),
            json!([{"type": "text", "text": "Task."}]),
        ),
        (
            "assistant".to_string(),
            json!([{"type": "text", "text": "Working."}]),
        ),
        (
            "user".to_string(),
            json!([{"type": "text", "text": "Continue the workflow."}]),
        ),
        (
            "user".to_string(),
            json!([{"type": "text", "text": "Continue the workflow."}]),
        ),
    ];
    let m = rebuild_transcript(&transcript, None);
    assert_eq!(m.len(), 3);
    assert_eq!(m[2].content.len(), 3);
}

#[test]
fn resume_note_replaces_the_interruption_notice() {
    let transcript = vec![(
        "assistant".to_string(),
        json!([{"type": "text", "text": "Working."}]),
    )];
    let m = rebuild_transcript(&transcript, Some("Continue the workflow."));
    assert!(
        matches!(&m.last().unwrap().content[..], [Block::Text { text }] if text == "Continue the workflow.")
    );
}

#[tokio::test]
async fn build_streak_recalls_lessons() {
    let f = fixture();
    let script = f.repo.join("build.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"error[E0425]: cannot find value x in this scope\"\nexit 1\n",
    )
    .unwrap();
    std::fs::write(
        f.repo.join(".ostra/project.toml"),
        "[commands]\nbuild = \"sh build.sh\"\n",
    )
    .unwrap();
    let mem = MemoryStore::open(&f.repo.join(".ostra/memory/knowledge.sqlite3")).unwrap();
    mem.record(
        "app",
        "error E0425 cannot find value means the import is missing",
        "test",
    )
    .unwrap();
    let s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Bypass,
        impl_caps(),
    );
    let report = s.ctx.report_file.clone().unwrap();
    let p = ScriptedProvider::new();
    p.push_tool_use("Bash", json!({"command": "sh build.sh"}));
    p.push_tool_use("Bash", json!({"command": "sh build.sh"}));
    p.push_tool_use("submit_implementer", json!({"status": "stuck", "report_path": report.display().to_string(), "changed_files": [], "summary": "s", "stuck": {"diagnostic": "E0425", "need": "the import"}}));
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Stuck);
    let reqs = p.requests();
    let first = &tool_results(&reqs[1])[0].0;
    assert!(!first.contains("Lessons recorded"), "{first}");
    let second = &tool_results(&reqs[2])[0].0;
    assert!(
        second.contains("Lessons recorded earlier") && second.contains("import is missing"),
        "{second}"
    );
    assert!(r.usage.build_ms > 0 || r.usage.tool_calls == 2);
}

#[tokio::test]
async fn a_planted_commondir_is_undone_and_reported() {
    let f = fixture();
    std::fs::create_dir_all(f.repo.join(".git")).unwrap();
    std::fs::write(f.repo.join(".git/config"), "").unwrap();
    // A script, because the guard refuses a command that names `.git/` itself.
    std::fs::write(
        f.repo.join("plant.sh"),
        "#!/bin/sh\nprintf /tmp/evil > .git/commondir\n",
    )
    .unwrap();
    let s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Bypass,
        impl_caps(),
    );
    let report = s.ctx.report_file.clone().unwrap();
    let p = ScriptedProvider::new();
    p.push_tool_use("Bash", json!({"command": "sh plant.sh"}));
    p.push_tool_use("submit_implementer", json!({"status": "stuck", "report_path": report.display().to_string(), "changed_files": [], "summary": "s", "stuck": {"diagnostic": "d", "need": "n"}}));
    let (exec, p) = executor(p);
    let host = Arc::new(FakeHost::default());
    exec.run(s, host.clone(), CancellationToken::new()).await;
    assert!(!f.repo.join(".git/commondir").exists());
    let result = &tool_results(&p.requests()[1])[0].0;
    assert!(result.contains("Ostra undid it"), "{result}");
    let signal = host.deltas.lock().iter().any(|d| {
        ostra_core::containment::classify(d).is_some_and(|s| {
            matches!(s, ostra_core::containment::ContainmentSignal::Guard { rule, .. } if rule == "git-metadata")
        })
    });
    assert!(signal, "the repair counts as a containment signal");
}

#[tokio::test]
async fn submit_needs_the_report_file() {
    let f = fixture();
    let s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::AcceptEdits,
        impl_caps(),
    );
    let report = s.ctx.report_file.clone().unwrap();
    let p = ScriptedProvider::new();
    p.push_tool_use("submit_implementer", submit_ok(&report));
    p.push_tool_use("Report", json!({"content": "r"}));
    p.push_tool_use("submit_implementer", submit_ok(&report));
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok);
    assert!(
        tool_results(&p.requests()[1])[0]
            .0
            .contains("before you call")
    );
}

#[tokio::test]
async fn missing_provider_is_a_clear_error() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::Explore,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.route.model = "anthropic:claude-sonnet-5-5".into();
    let exec = NativeExecutor::new(Arc::new(Providers::empty()), Arc::new(|_: &str| None), None);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Error);
    assert!(r.error.unwrap().contains("no API key"));
}

#[tokio::test]
async fn pause_turn_and_refusal() {
    let f = fixture();
    let s = spec(
        &f,
        AgentName::Explore,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    let p = ScriptedProvider::new();
    p.push(response(
        vec![Block::text("searching")],
        StopReason::PauseTurn,
    ));
    p.push(response(vec![Block::text("no")], StopReason::Refusal));
    let (exec, p) = executor(p);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Error);
    assert!(r.error.unwrap().contains("refused"));
    assert_eq!(p.requests().len(), 2);
}

#[test]
fn coalescer_chunks() {
    let host = Arc::new(FakeHost::default());
    let c = Coalescer {
        host: host.clone(),
        buf: Mutex::new((false, String::new())),
    };
    for _ in 0..10 {
        c.push(false, "abc");
    }
    c.push(true, "think");
    c.flush();
    let d = host.deltas.lock();
    assert_eq!(d.len(), 2);
    assert!(matches!(&d[0], ExecutionDelta::Text { text } if text.len() == 30));
}

#[tokio::test]
async fn relative_paths_are_checked_against_the_shell_cwd() {
    let f = fixture();
    let mut s = spec(
        &f,
        AgentName::Implementer,
        PermissionMode::Default,
        impl_caps(),
    );
    s.ctx.report_file = None;
    s.ctx.protected_paths = vec![f.outside.clone()];
    let victim = f.outside.join("victim.toml");
    std::fs::write(&victim, "orig").unwrap();
    let root = f.repo.parent().unwrap();
    let p = ScriptedProvider::new();
    p.push_tool_use(
        "Bash",
        json!({"command": format!("cd {}", root.display().to_string().replace('\\', "/"))}),
    );
    p.push_tool_use("Bash", json!({"command": "cd elsewhere"}));
    p.push_tool_use(
        "Write",
        json!({"file_path": "victim.toml", "content": "PWNED"}),
    );
    // A model-supplied `cwd` is replaced by the shell's own.
    p.push_tool_use(
        "Bash",
        json!({"command": "echo PWNED > victim.toml", "cwd": f.repo.display().to_string()}),
    );
    p.push_tool_use(
        "submit_implementer",
        json!({"status": "ok", "report_path": "", "changed_files": [], "summary": "s"}),
    );
    let (exec, p) = executor(p);
    let host = Arc::new(FakeHost {
        yolo: true,
        ..Default::default()
    });
    let r = exec.run(s, host.clone(), CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    let reqs = p.requests();
    for req in &reqs[3..5] {
        let (text, is_err) = &tool_results(req)[0];
        assert!(*is_err && text.starts_with("Denied by guard"), "{text}");
    }
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "orig");
    assert!(!f.repo.join("victim.toml").exists());
}

#[test]
fn webfetch_hosts_takes_exact_domain_rules() {
    let rules = vec![
        "WebFetch(domain:localhost)".to_string(),
        "Read(./src/**)".to_string(),
        " WebFetch(domain:10.0.0.5) ".to_string(),
    ];
    assert_eq!(
        ostra_tools::webfetch_hosts(&rules),
        vec!["localhost", "10.0.0.5"]
    );
}

/// Answers messaging calls the way the engine does: a send with `wait` and a wait pause the run,
/// and a send clears the reply the run owes.
struct FakeCoord {
    owes: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl ostra_tools::Coordinate for FakeCoord {
    async fn call(
        &self,
        tool: &str,
        input: &Value,
    ) -> Result<ostra_core::coord::CoordReply, String> {
        let end = match tool {
            "SendMessage" if input["wait"] == json!(true) => RunEnd::Wait,
            "WaitForMessage" => RunEnd::Wait,
            _ => RunEnd::Continue,
        };
        if tool == "SendMessage" {
            self.owes.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(ostra_core::coord::CoordReply {
            text: format!("{tool} done"),
            end,
        })
    }
}

struct FakeCoordConnector {
    owes: Arc<std::sync::atomic::AtomicBool>,
}

impl CoordConnector for FakeCoordConnector {
    fn open(&self, _spec: &ExecutionSpec) -> Option<Arc<dyn ostra_tools::Coordinate>> {
        Some(Arc::new(FakeCoord {
            owes: self.owes.clone(),
        }))
    }
}

fn coord() -> Arc<FakeCoordConnector> {
    Arc::new(FakeCoordConnector {
        owes: Default::default(),
    })
}

#[tokio::test]
async fn sm3_a_send_with_wait_or_a_wait_ends_the_run_waiting() {
    let f = fixture();
    let caps = vec![Capability::Read, Capability::Coordinate];
    let p = ScriptedProvider::new();
    p.push_tool_use("ListAgents", json!({}));
    p.push_tool_use(
        "SendMessage",
        json!({"message": "why?", "agent": "explore", "wait": true}),
    );
    let (exec, p) = executor(p);
    let exec = exec.with_coord(coord());
    let s = spec(
        &f,
        AgentName::GenerateSpec,
        PermissionMode::Default,
        caps.clone(),
    );
    let host = Arc::new(FakeHost::default());
    let r = exec.run(s, host.clone(), CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Waiting, "{:?}", r.error);
    assert_eq!(r.submit.unwrap()["coordination"], "SendMessage");
    assert_eq!(
        p.requests().len(),
        2,
        "the run stops at the send, with no model call after it"
    );
    let last = host.messages.lock().last().cloned().unwrap();
    assert!(
        last.1.to_string().contains("SendMessage done"),
        "the send's result is recorded"
    );

    let p = ScriptedProvider::new();
    p.push_tool_use("WaitForMessage", json!({}));
    let (exec, _) = executor(p);
    let exec = exec.with_coord(coord());
    let s = spec(&f, AgentName::GenerateSpec, PermissionMode::Default, caps);
    let r = exec
        .run(s, Arc::new(FakeHost::default()), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Waiting);
}

#[tokio::test]
async fn sm2_queued_messages_follow_the_tool_results_of_a_turn() {
    let f = fixture();
    let p = ScriptedProvider::new();
    p.push_tool_use(
        "Read",
        json!({"file_path": f.repo.join("main.txt").display().to_string()}),
    );
    p.push_tool_use("submit_quick_answer", json!({"answer": "s", "sources": []}));
    let (exec, p) = executor(p);
    let mut s = spec(
        &f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.ctx.report_file = None;
    let host = Arc::new(FakeHost::default());
    host.inbox
        .lock()
        .push("Message from subagent x_1: check R2.".into());
    let r = exec.run(s, host, CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    let reqs = p.requests();
    let turn = reqs[1].messages.last().unwrap();
    assert!(
        matches!(&turn.content[0], Block::ToolResult { .. }),
        "{turn:?}"
    );
    assert!(
        matches!(turn.content.last().unwrap(), Block::Text { text } if text.contains("check R2")),
        "the message follows the tool result in the same user turn: {turn:?}"
    );
    assert_eq!(
        &reqs[1].messages[..reqs[0].messages.len()],
        &reqs[0].messages[..],
        "the earlier request is a prefix of the next, so the prompt cache holds"
    );
}

#[tokio::test]
async fn sm6_a_run_that_owes_a_reply_is_reminded_and_cannot_submit() {
    let f = fixture();
    let p = ScriptedProvider::new();
    p.push(response(
        vec![Block::text("I think the answer is 42.")],
        StopReason::EndTurn,
    ));
    let submit = json!({"answer": "s", "sources": []});
    p.push_tool_use("submit_quick_answer", submit.clone());
    p.push_tool_use("SendMessage", json!({"message": "42", "to": "x_sender"}));
    p.push_tool_use("submit_quick_answer", submit);
    let (exec, p) = executor(p);
    let owes = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let exec = exec.with_coord(Arc::new(FakeCoordConnector { owes: owes.clone() }));
    let mut s = spec(
        &f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read, Capability::Coordinate],
    );
    s.ctx.report_file = None;
    let host = Arc::new(FakeHost {
        owes,
        ..Default::default()
    });
    let r = exec.run(s, host, CancellationToken::new()).await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    assert_eq!(r.submit.unwrap()["answer"], "s");
    let reqs = p.requests();
    let reminder = reqs[1].messages.last().unwrap();
    assert!(
        matches!(&reminder.content[0], Block::Text { text } if text.contains("SendMessage") && text.contains("x_sender")),
        "the reminder names the send tool and the sender: {reminder:?}"
    );
    let (denied, is_err) = &tool_results(&reqs[2])[0];
    assert!(*is_err && denied.contains("x_sender"), "{denied}");
}

fn read_near_full(f: &Fixture, context: u64) -> ChatResponse {
    let mut r = response(
        vec![Block::ToolUse {
            id: format!("r{context}"),
            name: "Read".into(),
            input: json!({"file_path": f.repo.join("main.txt").display().to_string()}),
        }],
        StopReason::ToolUse,
    );
    r.usage.cache_read_tokens = context;
    r
}

fn quick(f: &Fixture) -> ExecutionSpec {
    let mut s = spec(
        f,
        AgentName::QuickAnswer,
        PermissionMode::Default,
        vec![Capability::Read],
    );
    s.ctx.report_file = None;
    s
}

fn is_compaction_ask(req: &ChatRequest) -> bool {
    req.tool_choice == ToolChoice::None
}

#[tokio::test]
async fn compacts_near_the_context_window_and_continues_from_the_summary() {
    let f = fixture();
    let p = ScriptedProvider::new();
    // 95% of the 200k window assumed for an unlisted model.
    p.push(read_near_full(&f, 190_000));
    p.push_text("Summary: main.txt says hello. Next: answer.");
    p.push_tool_use("submit_quick_answer", json!({"answer": "hello"}));
    let (exec, p) = executor(p);
    let host = Arc::new(FakeHost::default());
    let r = exec
        .run(quick(&f), host.clone(), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    assert_eq!(r.usage.context_tokens, 190_000);

    let reqs = p.requests();
    assert_eq!(reqs.len(), 3);
    assert!(is_compaction_ask(&reqs[1]));
    assert_eq!(
        reqs[1].messages.len(),
        3,
        "the whole conversation is summarized"
    );
    assert_eq!(
        reqs[1].messages[2].content.last(),
        Some(&Block::text(COMPACT_INSTRUCTIONS))
    );
    let after = &reqs[2].messages;
    assert_eq!(after.len(), 1, "the summary replaces the conversation");
    assert!(
        matches!(&after[0].content[..], [Block::Compaction { summary, .. }, Block::Text { text }]
        if summary.starts_with("Summary:") && text == AFTER_COMPACTION)
    );

    let recorded = host.messages.lock().clone();
    let (role, window) = recorded
        .iter()
        .find(|(role, _)| role == COMPACTION_RECORD)
        .expect("the window is stored");
    assert_eq!(role, COMPACTION_RECORD);
    assert_eq!(
        stored_messages(&recorded)[..1],
        after[..],
        "a resume starts from the window"
    );
    assert!(window.is_array());
}

#[tokio::test]
async fn a_compaction_without_summary_is_not_retried_every_turn() {
    let f = fixture();
    let p = ScriptedProvider::new();
    p.push(read_near_full(&f, 190_000));
    p.push(response(vec![], StopReason::MaxTokens));
    p.push(read_near_full(&f, 191_000));
    p.push_tool_use("submit_quick_answer", json!({"answer": "hello"}));
    let (exec, p) = executor(p);
    let r = exec
        .run(
            quick(&f),
            Arc::new(FakeHost::default()),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    let asks = p.requests().iter().filter(|r| is_compaction_ask(r)).count();
    assert_eq!(asks, 1);
}

#[test]
fn a_stored_compaction_replaces_the_messages_before_it() {
    let window = json!([{"role": "user", "content": [
        {"type": "compaction", "provider": "", "summary": "S", "value": null},
        {"type": "text", "text": "Continue."},
    ]}]);
    let transcript = vec![
        (
            "user".to_string(),
            json!([{"type": "text", "text": "Task."}]),
        ),
        (
            "assistant".to_string(),
            json!([{"type": "text", "text": "Working."}]),
        ),
        (COMPACTION_RECORD.to_string(), window),
        (
            "assistant".to_string(),
            json!([{"type": "text", "text": "After."}]),
        ),
    ];
    let m = stored_messages(&transcript);
    assert_eq!(m.len(), 2);
    assert!(matches!(&m[0].content[0], Block::Compaction { summary, .. } if summary == "S"));
    assert_eq!(m[1].content, vec![Block::text("After.")]);
}

#[tokio::test]
async fn each_response_reports_its_own_cost_before_its_tool_calls() {
    let f = fixture();
    let p = ScriptedProvider::new();
    let mut first = read_near_full(&f, 10);
    first.usage.cost_usd = 0.25;
    p.push(first);
    let mut second = response(
        vec![Block::ToolUse {
            id: "s1".into(),
            name: "submit_quick_answer".into(),
            input: json!({"answer": "hello"}),
        }],
        StopReason::ToolUse,
    );
    second.usage.cost_usd = 0.5;
    p.push(second);
    let (exec, _) = executor(p);
    let host = Arc::new(FakeHost::default());
    let r = exec
        .run(quick(&f), host.clone(), CancellationToken::new())
        .await;
    assert_eq!(r.status, ExecutionStatus::Ok, "{:?}", r.error);
    assert_eq!(r.usage.cost_usd, 0.75);

    let d = host.deltas.lock();
    let turns: Vec<(f64, Vec<String>)> = d
        .iter()
        .filter_map(|d| match d {
            ExecutionDelta::Turn { usage, call_ids } => Some((usage.cost_usd, call_ids.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns,
        vec![(0.25, vec!["r10".into()]), (0.5, vec!["s1".into()])]
    );
    let at = |pred: &dyn Fn(&ExecutionDelta) -> bool| d.iter().position(pred).unwrap();
    assert!(
        at(&|d| matches!(d, ExecutionDelta::Turn { .. }))
            < at(&|d| matches!(d, ExecutionDelta::ToolCall { call_id, .. } if call_id == "r10"))
    );
}
