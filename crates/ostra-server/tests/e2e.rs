//! The whole stack with a scripted model: real listener, REST with cookie auth, engine, native
//! loop, policy, tools, and git staging. The model plays each agent: it writes its files with the
//! Write tool, then calls its submit tool.

use ostra_core::api::{
    ApiError, Artifact, CostReport, DiffFile, DiffLineKind, EnvironmentStatus, ExecutionView,
    FileDiff, FileIndex, FsBrowse, GitBranch, GitChange, GitMark, GitOpResult, GitRepoStatus,
    HarnessSetupTerminal, Heading, OnboardingState, ProjectChange, ProjectFile, ProjectSkills,
    ProjectTree, ProviderStatus, SearchKind, SearchResults, ServerMsg, SessionDetail,
    SessionStatus, SessionSummary, SkillDoc, SkillOrigin, UI_STATE_MAX_BYTES, UiTab,
    WorkspaceActivity, WorkspaceDetail, WorkspaceSummary, WorkspaceTree, WorkspaceUiState,
};
use ostra_core::code::{CodeDeps, CodeFile, CodeLocation, CodeSymbols, CodeUsages, SymbolKind};
use ostra_core::config::{GlobalConfig, PermissionMode, TierTable, ValidationIssue, save_toml};
use ostra_providers::mock::{response, tool_use_response};
use ostra_providers::{
    Block, ChatRequest, ChatResponse, ProviderError, Role, ScriptedProvider, StopReason,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static IDS: AtomicU64 = AtomicU64::new(1);

fn id() -> String {
    format!("toolu_e2e_{}", IDS.fetch_add(1, Ordering::SeqCst))
}

fn first_user_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| {
                    if let Block::Text { text } = b {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn label(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn assistant_turns(req: &ChatRequest) -> usize {
    req.messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count()
}

fn judge(req: &ChatRequest) -> Value {
    let schema = &req.tools[0].input_schema;
    let props = &schema["properties"];
    if props.get("category").is_some() {
        json!({"category": "IMPLEMENT", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "Research the greeting"}], "opts_in": {"tests": false, "docs": false}, "reason": "The request changes code.", "title": "Greeting file"})
    } else if props.get("stakes").is_some() {
        json!({"stakes": "high", "reason": "Plan it."})
    } else if props.get("report_markdown").is_some() {
        json!({"report_markdown": "# Greeting added\n\nEvery stage ran.", "reason": "done"})
    } else {
        match props["answer"]["properties"]["kind"]["const"].as_str() {
            Some("approval") => {
                json!({"answer": {"kind": "approval", "approved": true}, "reason": "The fact-check passed."})
            }
            _ => json!({"answer": {"kind": "choice", "option": "block"}, "reason": "r"}),
        }
    }
}

fn respond(req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
    let tools: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    if tools == ["decide"] {
        return Ok(tool_use_response(&id(), "decide", judge(req)));
    }
    let submit = tools
        .iter()
        .find(|t| t.starts_with("submit_"))
        .copied()
        .unwrap_or_default()
        .to_string();
    let text = first_user_text(req);
    let session = PathBuf::from(label(&text, "Session dir"));
    let repo = PathBuf::from(label(&text, "Repo root"));
    let report = label(&text, "Report file");
    let turn = assistant_turns(req);
    let write = |path: &Path, content: &str| {
        tool_use_response(
            &id(),
            "Write",
            json!({"file_path": path, "content": content}),
        )
    };
    let document = |path: &Path, doc: Value| {
        tool_use_response(&id(), "Document", json!({"path": path, "document": doc}))
    };
    let spec = session.join("ostra-spec-1.md");
    let plan = session.join("ostra-plan-1.md");
    let phase = session.join("ostra-plan-1-phase-1.md");
    let research = session.join("ostra-research-1.md");
    let out = match (submit.as_str(), turn) {
        ("submit_explore", 0) => document(
            &research,
            json!({"title": "Greeting", "date": "2026-07-28", "repo": "app", "scope": "The greeting.", "problem": "No greeting exists."}),
        ),
        ("submit_explore", _) => tool_use_response(
            &id(),
            &submit,
            json!({"research_path": research, "scope_covered": "s", "findings_summary": "No greeting exists.", "sources_retrieved": 0, "open_questions": 0, "not_covered": []}),
        ),
        ("submit_generate_spec", 0) => document(
            &spec,
            json!({
                "title": "Greeting", "date": "2026-07-28", "objective": "Print a greeting.", "current_behavior": "None.",
                "criteria": [{"id": "C1", "statement": "A greeting file exists.", "kind": "Functional", "repo": "app", "grounding": "new: no precedent found"}],
                "deliverables": [{"id": "D1", "title": "Greeting", "repo": "app", "outcome": "The file exists."}],
                "requirements": [{"id": "R1", "deliverable": "D1", "title": "Greeting file", "pattern": "ubiquitous", "statement": "THE SYSTEM SHALL contain greeting.txt.", "covers": ["C1"],
                    "acceptance": [{"id": "AC1.1", "given": "the repo", "when": "it is read", "then": "greeting.txt says hello"}]}]
            }),
        ),
        ("submit_generate_spec", _) => tool_use_response(
            &id(),
            &submit,
            json!({"spec_path": spec, "open_questions": [], "external_evidence_rows": 0, "deliverables": 1, "requirements": 1, "summary": "Print a greeting."}),
        ),
        ("submit_fact_check", _) => {
            let target = label(&text, "Target type");
            tool_use_response(
                &id(),
                &submit,
                json!({"verdict": "PASS", "target": target, "findings": []}),
            )
        }
        ("submit_plan", 0) => document(
            &plan,
            json!({
                "title": "Greeting", "date": "2026-07-28", "spec": spec, "stakes": "High", "stakes_rationale": "r", "summary": "One phase.",
                "phases": [{"id": 1, "name": "greeting", "deliverable": "D1", "repo": "app", "repo_root": repo, "complexity": "Low", "test_policy": "Required",
                    "test_rationale": "Step 1.1 writes the file.", "description": "d", "context": "This is the first phase. No prior phases.",
                    "requirements": [{"id": "R1", "statement": "THE SYSTEM SHALL contain greeting.txt."}],
                    "steps": [{"id": "1.1", "title": "Write the file", "file": "greeting.txt", "change": "Create", "delivers": ["R1"], "action": "Write hello.", "verify": "true", "size": "Small"}],
                    "verification": "true"}]
            }),
        ),
        ("submit_plan", _) => tool_use_response(
            &id(),
            &submit,
            json!({"spec_path": spec, "master_plan_path": plan, "phases": [{"id": 1, "deliverable": "D1", "project": "app", "title": "greeting", "complexity": "Low", "test_policy": "Required", "depends_on": [], "file": phase}], "stakes": "High", "summary": "One phase.", "step_count": 1, "requirement_coverage": "1 of 1"}),
        ),
        ("submit_implementer", 0) => write(&repo.join("greeting.txt"), "hello\n"),
        ("submit_implementer", 1) => write(Path::new(&report), "# Report\n\nAdded greeting.txt\n"),
        ("submit_implementer", _) => tool_use_response(
            &id(),
            &submit,
            json!({"status": "ok", "report_path": report, "changed_files": ["greeting.txt"], "summary": "Added the greeting."}),
        ),
        ("submit_code_reviewer", _) => tool_use_response(
            &id(),
            &submit,
            json!({"findings": [], "security_block": false, "ledger_path": session.join("ostra-review-ledger-phase-1.md"), "summary": "Code review passed"}),
        ),
        (other, _) => response(
            vec![Block::text(format!("unexpected agent {other}"))],
            StopReason::EndTurn,
        ),
    };
    Ok(out)
}

async fn wait_for<F: Fn(&SessionDetail) -> bool>(
    client: &reqwest::Client,
    base: &str,
    id: &str,
    f: F,
) -> SessionDetail {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let d: SessionDetail = client
            .get(format!("{base}/api/sessions/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if f(&d) {
            return d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out at {:?}: {}",
            d.summary.status,
            d.summary.stage_label
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The server reads its config and data paths from the process environment, so tests in this
/// binary hold this lock for as long as their server runs.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Server {
    app: Arc<ostra_server::app::App>,
    base: String,
    client: reqwest::Client,
}

/// Start a signed-in server whose config and data live under `root`, with every tier on the
/// scripted `mock` provider. Call it while holding [`SERIAL`].
async fn boot(root: &Path) -> Server {
    boot_with(root, |g| g.server.use_ip_host = true).await
}

/// [`boot`] with a change to the global config before the server starts.
async fn boot_with(root: &Path, change: impl FnOnce(&mut GlobalConfig)) -> Server {
    // SAFETY: the caller holds SERIAL, so no other test thread reads the environment meanwhile.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", root.join("config.toml"));
        std::env::set_var("OSTRA_DATA_DIR", root.join("data"));
        std::env::set_var("OSTRA_MASTER_KEY_FILE", root.join("master.key"));
        std::env::set_var("OSTRA_SANDBOX_CACHE", root.join("sandbox-cache"));
        std::env::set_var("OSTRA_MODELS_DEV_URL", "");
    }
    let mut global = GlobalConfig::default();
    global.tiers.insert(
        "native".into(),
        TierTable {
            fast: Some("mock:m".into()),
            balanced: Some("mock:m".into()),
            advanced: Some("mock:m".into()),
            frontier: Some("mock:m".into()),
        },
    );
    change(&mut global);
    save_toml(&root.join("config.toml"), &global).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let opts = ostra_server::app::ServeOptions {
        port: Some(port),
        open_browser: false,
        dev: false,
        exe: PathBuf::from("/nonexistent/ostra"),
        bind: None,
        allow_hosts: vec![],
    };
    let app = ostra_server::app::build(&opts, port).await.unwrap();
    app.shared.providers.register(
        "mock",
        Arc::new(ScriptedProvider::named("mock").with_responder(respond)),
    );
    let router = ostra_server::api::router(app.clone())
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let url = app.auth.sign_in_url().unwrap();
    let token = url.split("#token=").nth(1).unwrap();
    let r = client
        .post(format!("{base}/api/auth/exchange"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    Server { app, base, client }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn yolo_implement_session_end_to_end() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let Server { base, client, app } = boot(root).await;

    let app_dir = root.join("app");
    std::fs::create_dir_all(app_dir.join(".ostra")).unwrap();
    std::fs::write(app_dir.join(".ostra/INVENTORY.md"), "# app Inventory\n").unwrap();
    std::fs::write(
        app_dir.join(".ostra/project.toml"),
        "[commands]\nformat = \"true\"\nbuild = \"true\"\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&app_dir)
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);

    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "e2e", "root": root.join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces/{}/projects", ws.id))
        .json(&json!({"path": app_dir, "key": "app", "stack": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ws.projects.len(), 1);
    assert!(ws.validation.is_empty(), "{:?}", ws.validation);
    let head = std::fs::read_to_string(app_dir.join(".git/HEAD")).unwrap();
    let branch = head
        .trim()
        .strip_prefix("ref: refs/heads/")
        .unwrap()
        .to_string();
    assert_eq!(
        ws.projects[0].git_branch.as_ref(),
        Some(&branch),
        "read from .git/HEAD"
    );

    let mut pushed = app.push.subscribe();
    let s: SessionSummary = client
        .post(format!("{base}/api/workspaces/{}/sessions", ws.id))
        .json(&json!({"request": "Add a greeting file", "options": {"tests": false, "docs": false, "yolo": true}, "projects": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let d = wait_for(&client, &base, s.id.as_str(), |d| {
        matches!(
            d.summary.status,
            SessionStatus::Completed | SessionStatus::Failed
        )
    })
    .await;
    assert_eq!(d.summary.status, SessionStatus::Completed, "{:#?}", d.gates);
    assert!(
        d.completion
            .as_deref()
            .unwrap_or_default()
            .contains("Greeting added")
    );
    assert_eq!(
        std::fs::read_to_string(app_dir.join("greeting.txt")).unwrap(),
        "hello\n"
    );
    let staged = String::from_utf8(git(&["diff", "--cached", "--name-only"]).stdout).unwrap();
    assert!(staged.contains("greeting.txt"), "staged: {staged}");
    let agents: Vec<String> = d.executions.iter().map(|e| e.agent.to_string()).collect();
    assert_eq!(
        agents,
        [
            "explore",
            "generate-spec",
            "fact-check",
            "plan",
            "fact-check",
            "implementer",
            "code-reviewer"
        ]
    );
    let imp = d
        .executions
        .iter()
        .find(|e| e.agent == ostra_core::AgentName::Implementer)
        .unwrap();
    assert_eq!(d.summary.title.as_deref(), Some("Greeting file"));
    assert_eq!(imp.group, "implementer:app");
    assert_eq!(imp.run_label, "Phase 1");
    assert_eq!(imp.stream, ostra_core::executor::ExecStream::Activity);
    assert!(
        imp.summary
            .as_deref()
            .is_some_and(|s| s.starts_with("Write ")),
        "{:?}",
        imp.summary
    );
    assert!(!imp.has_transcript);
    assert_eq!(imp.repo_root.as_ref(), Some(&ws.projects[0].path));
    assert_eq!(
        imp.pending_gate, None,
        "nothing waits once the session completed"
    );
    let one: ExecutionView = client
        .get(format!("{base}/api/executions/{}", imp.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (one.repo_root, one.pending_gate),
        (Some(ws.projects[0].path.clone()), None)
    );
    let ledger: Vec<DiffFile> = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/sessions/{}/diff", s.id),
                &[("project", "app")],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        ledger,
        vec![DiffFile {
            path: "greeting.txt".into(),
            original: String::new(),
            modified: "hello\n".into()
        }]
    );
    // Typed documents come back with the artifact, and fact-check passes with the session.
    for (kind, doc_kind) in [
        ("research", "research"),
        ("spec", "spec"),
        ("plan", "plan"),
        ("phase", "phase"),
    ] {
        let r = d
            .artifacts
            .iter()
            .find(|a| a.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} artifact"));
        let art: Value = client
            .get(
                reqwest::Url::parse_with_params(
                    &format!("{base}/api/artifacts"),
                    &[("path", r.path.to_str().unwrap())],
                )
                .unwrap(),
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(art["document"]["document"]["kind"], doc_kind, "{art}");
        assert_eq!(art["document"]["issues"], json!([]), "{kind}");
    }
    let checks: Vec<(&str, bool)> = d
        .fact_checks
        .iter()
        .map(|c| (c.target.as_str(), c.current))
        .collect();
    assert_eq!(checks, [("spec", true), ("plan", true)]);
    let labels: Vec<&str> = d.executions.iter().map(|e| e.run_label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Research task 1",
            "Spec",
            "Spec check",
            "Plan",
            "Plan check",
            "Phase 1",
            "Phase 1 · review pass"
        ]
    );
    let groups: Vec<(&str, usize)> = d
        .execution_groups
        .iter()
        .map(|g| (g.group.as_str(), g.executions.len()))
        .collect();
    assert_eq!(
        groups,
        [
            ("explore:app", 1),
            ("generate-spec:app", 1),
            ("fact-check:app", 2),
            ("plan:app", 1),
            ("implementer:app", 1),
            ("code-reviewer:app", 1)
        ]
    );
    let listed: Vec<SessionSummary> = client
        .get(format!("{base}/api/workspaces/{}/sessions", ws.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0].title.as_deref(), Some("Greeting file"));
    // An ended run with a stored transcript replays it on `term:<execution>`.
    assert_eq!(ostra_server::ws::stored_transcript(&app, &imp.id), None);
    let transcript = ostra_core::paths::terminal_transcript(&d.session_root, imp.id.as_str());
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, b"\x1b[1mdone\r\n").unwrap();
    assert_eq!(
        ostra_server::ws::stored_transcript(&app, &imp.id).as_deref(),
        Some(&b"\x1b[1mdone\r\n"[..])
    );
    let activity: Vec<Value> = client
        .get(format!("{base}/api/executions/{}/activity", imp.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // The project write would have asked in default mode; under YOLO the policy allowed it and
    // recorded the rule.
    assert!(
        activity
            .iter()
            .any(|a| a["delta"]["kind"] == "policy"
                && a["delta"]["decision"]["rule"]["rule"] == "yolo"),
        "{activity:#?}"
    );
    let art = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/artifacts"),
                &[(
                    "path",
                    imp.report_path.clone().unwrap().display().to_string(),
                )],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(art.status(), 200);
    let outside = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/artifacts"),
                &[("path", app_dir.join("greeting.txt").display().to_string())],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(outside.status(), 403, "only session files are served");

    let art: Artifact = art.json().await.unwrap();
    assert_eq!(
        art.headings,
        vec![Heading {
            id: "report".into(),
            level: 1,
            title: "Report".into()
        }]
    );

    // The implementer's Write reached the workspace channel as a project_fs_changed message, and
    // the session's tree node and the workspace activity were pushed there as they changed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let (mut fs_changed, mut completed_node, mut activity_pushed) = (false, None, false);
    while !(fs_changed && completed_node.is_some() && activity_pushed) {
        let p = match tokio::time::timeout_at(deadline, pushed.recv())
            .await
            .expect("a workspace message is missing")
        {
            Ok(p) => p,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(p.channels, vec![format!("workspace:{}", ws.id)]);
        match p.msg {
            ServerMsg::ProjectFsChanged {
                workspace,
                key,
                paths,
            } => {
                assert_eq!((workspace, key.as_str()), (ws.id.clone(), "app"));
                fs_changed |= paths.iter().any(|p| p == "greeting.txt");
            }
            ServerMsg::TreePatch { workspace, session } => {
                assert_eq!(workspace, ws.id);
                if session.id == s.id && session.status == SessionStatus::Completed {
                    completed_node = Some(session);
                }
            }
            ServerMsg::Activity { workspace, .. } => {
                assert_eq!(workspace, ws.id);
                activity_pushed = true;
            }
            _ => {}
        }
    }

    // The Sessions tree: one node, grouped like the session detail, with its artifacts.
    let wsp = format!("{base}/api/workspaces/{}", ws.id);
    let tree: WorkspaceTree = client
        .get(format!("{wsp}/tree"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tree.sessions.len(), 1);
    let node = &tree.sessions[0];
    assert_eq!(
        Some(node),
        completed_node.as_ref(),
        "the last patch matches the tree"
    );
    assert_eq!(
        (
            node.id.clone(),
            node.title.as_deref(),
            node.request.as_str(),
            node.open_gates
        ),
        (
            s.id.clone(),
            Some("Greeting file"),
            "Add a greeting file",
            0
        )
    );
    let groups: Vec<(&str, Vec<&str>)> = node
        .groups
        .iter()
        .map(|g| {
            (
                g.group.as_str(),
                g.runs.iter().map(|r| r.run_label.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        [
            ("explore:app", vec!["Research task 1"]),
            ("generate-spec:app", vec!["Spec"]),
            ("fact-check:app", vec!["Spec check", "Plan check"]),
            ("plan:app", vec!["Plan"]),
            ("implementer:app", vec!["Phase 1"]),
            ("code-reviewer:app", vec!["Phase 1 · review pass"]),
        ]
    );
    assert_eq!(node.groups[4].runs[0].id, imp.id);
    assert_eq!(node.groups[4].runs[0].summary, imp.summary);
    assert_eq!(node.artifacts, d.artifacts);
    assert!(
        node.artifacts.iter().any(|a| a.kind == "spec"),
        "{:?}",
        node.artifacts
    );

    // The read-only file endpoints, on the project the session changed (a repo with no commits).
    let project = format!("{base}/api/workspaces/{}/projects/app", ws.id);
    let get = |path: &str, query: &[(&str, &str)]| {
        let url = reqwest::Url::parse_with_params(&format!("{project}/{path}"), query).unwrap();
        client.get(url).send()
    };
    let tree: ProjectTree = get("tree", &[]).await.unwrap().json().await.unwrap();
    assert!(tree.is_git);
    let names: Vec<&str> = tree.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["greeting.txt"],
        "dotfiles are hidden by default"
    );
    let g = &tree.entries[0];
    assert_eq!(
        (g.git, g.staged, g.has_changes, g.size),
        (Some(GitMark::Added), true, true, 6)
    );
    let by = g
        .changed_by
        .as_ref()
        .expect("attributed to the implementer");
    assert_eq!(
        (
            by.session.clone(),
            by.agent,
            by.phase,
            by.staged,
            by.running
        ),
        (
            s.id.clone(),
            ostra_core::AgentName::Implementer,
            Some(1),
            true,
            false
        )
    );
    assert_eq!(by.execution, imp.id);

    let tree: ProjectTree = get("tree", &[("hidden", "true"), ("depth", "2")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paths: Vec<&str> = tree.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            ".ostra",
            ".ostra/INVENTORY.md",
            ".ostra/project.toml",
            "greeting.txt"
        ]
    );
    assert!(
        tree.entries[0].has_changes,
        "a folder holding untracked files has changes"
    );

    let file: ProjectFile = get("file", &[("path", "greeting.txt")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (file.content.as_deref(), file.binary, file.truncated),
        (Some("hello\n"), false, false)
    );
    assert_eq!(file.changed_by.map(|c| c.execution), Some(imp.id.clone()));
    assert_eq!(
        get("file", &[("path", "missing.txt")])
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        get("file", &[("path", "../config.toml")])
            .await
            .unwrap()
            .status(),
        403
    );
    std::os::unix::fs::symlink(root, app_dir.join("escape")).unwrap();
    assert_eq!(
        get("file", &[("path", "escape/config.toml")])
            .await
            .unwrap()
            .status(),
        403,
        "a symlink cannot lead out of the project"
    );
    assert_eq!(
        get("tree", &[("path", "escape")]).await.unwrap().status(),
        403
    );

    let diff: FileDiff = get("diff", &[("path", "greeting.txt")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (diff.added, diff.removed, diff.binary, diff.base.as_str()),
        (1, 0, false, "HEAD")
    );
    assert_eq!(diff.hunks.len(), 1);
    let line = &diff.hunks[0].lines[0];
    assert_eq!(
        (line.kind, line.text.as_str(), line.old_no, line.new_no),
        (DiffLineKind::Add, "hello", None, Some(1))
    );
    assert_eq!(
        get(
            "diff",
            &[("path", "greeting.txt"), ("base", "--output=/tmp/x")]
        )
        .await
        .unwrap()
        .status(),
        400
    );

    let changes: Vec<ProjectChange> = get("changes", &[]).await.unwrap().json().await.unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].path.as_str(), changes[0].added, changes[0].git),
        ("greeting.txt", 1, Some(GitMark::Added))
    );

    let index: FileIndex = get("files", &[]).await.unwrap().json().await.unwrap();
    assert!(
        index.paths.contains(&"greeting.txt".to_string())
            && index.paths.contains(&".ostra/INVENTORY.md".to_string()),
        "{:?}",
        index.paths
    );
    assert!(!index.truncated);

    // Code navigation: tokens and outline, usages across files, dependencies, symbol search.
    std::fs::create_dir_all(app_dir.join("src")).unwrap();
    std::fs::write(app_dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::write(
        app_dir.join("src/lib.rs"),
        "mod greet;\npub use greet::hello;\n",
    )
    .unwrap();
    std::fs::write(
        app_dir.join("src/greet.rs"),
        "pub fn hello() -> &'static str {\n    \"hi\"\n}\n",
    )
    .unwrap();
    app.files.invalidate(&ws.id, "app");
    let file: CodeFile = get("code/file", &[("path", "src/greet.rs")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            file.provider.as_str(),
            file.language.as_deref(),
            file.tokens.len() % 4
        ),
        ("native", Some("rust"), 0)
    );
    assert_eq!(
        (
            file.symbols[0].name.as_str(),
            file.symbols[0].kind,
            file.symbols[0].end_line
        ),
        ("hello", SymbolKind::Function, Some(3))
    );
    let usages: CodeUsages = get(
        "code/usages",
        &[("symbol", "hello"), ("path", "src/lib.rs")],
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let at = |l: &[CodeLocation]| {
        l.iter()
            .map(|x| (x.path.clone(), x.line))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        at(&usages.definitions),
        vec![("src/greet.rs".to_string(), 1)]
    );
    assert_eq!(at(&usages.references), vec![("src/lib.rs".to_string(), 2)]);
    let deps: CodeDeps = get("code/deps", &[("path", "src/greet.rs")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let by: Vec<(&str, u32)> = deps
        .importers
        .iter()
        .map(|i| (i.path.as_str(), i.line))
        .collect();
    assert_eq!(by, vec![("src/lib.rs", 1), ("src/lib.rs", 2)]);
    let found: CodeSymbols = get("code/symbols", &[("q", "hel")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found.items[0].name, "hello");

    // Editing hints over the socket: without a language server, completions come from the index.
    {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let server = Server {
            app: app.clone(),
            base: base.clone(),
            client: client.clone(),
        };
        let mut sock = socket(&server).await;
        let mut ask = async |v: Value| -> Value {
            sock.send(Message::Text(v.to_string().into()))
                .await
                .unwrap();
            loop {
                let m = tokio::time::timeout(Duration::from_secs(10), sock.next())
                    .await
                    .expect("no answer")
                    .unwrap()
                    .unwrap();
                if let Message::Text(t) = m {
                    return serde_json::from_str(&t).unwrap();
                }
            }
        };
        let text = "mod greet;\npub use greet::hel";
        let hint = |kind: &str, id: u32, path: &str| {
            json!({"type": kind, "id": id, "workspace": ws.id, "key": "app", "path": path,
                   "text": text, "line": 2, "col": 18})
        };
        let c = ask(hint("code_complete", 1, "src/lib.rs")).await;
        assert_eq!(
            (c["type"].as_str(), c["id"].as_u64()),
            (Some("code_completion"), Some(1))
        );
        let item = &c["result"]["items"][0];
        assert_eq!(
            (item["label"].as_str(), item["kind"].as_str()),
            (Some("hello"), Some("function"))
        );
        assert_eq!(
            item["range"],
            json!({"line": 2, "col": 15, "end_line": 2, "end_col": 18})
        );
        assert_eq!(c["result"]["provider"], "native");
        let h = ask(hint("code_signature", 2, "src/lib.rs")).await;
        assert_eq!(
            (h["type"].as_str(), &h["result"], &h["error"]),
            (Some("code_signature_help"), &Value::Null, &Value::Null)
        );
        // Jumps come from the index; a name it does not define, with no language server, gets null.
        let jump = |id: u32, line: u32, col: u32, target: &str| {
            json!({"type": "code_navigate", "id": id, "workspace": ws.id, "key": "app",
                   "path": "src/lib.rs", "text": "mod greet;\npub use greet::hello;\nfn nothing() {}",
                   "line": line, "col": col, "target": target})
        };
        let n = ask(jump(4, 2, 17, "implementations")).await;
        assert_eq!(
            (n["type"].as_str(), n["id"].as_u64()),
            (Some("code_navigation"), Some(4))
        );
        assert_eq!(
            (
                &n["result"]["symbol"],
                &n["result"]["locations"],
                &n["result"]["provider"]
            ),
            (&json!("hello"), &json!([]), &json!("native")),
            "{n}"
        );
        let none = ask(jump(5, 3, 5, "supertypes")).await;
        assert_eq!(
            (&none["result"], &none["error"]),
            (&Value::Null, &Value::Null),
            "{none}"
        );
        let bad = ask(hint("code_complete", 3, "../outside.rs")).await;
        assert!(bad["result"].is_null() && bad["error"].is_string(), "{bad}");
    }
    assert_eq!(
        get("code/usages", &[("symbol", "two words")])
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        get("code/file", &[("path", "../outside.rs")])
            .await
            .unwrap()
            .status(),
        403
    );
    // Only files a language server pointed at are read outside the project.
    for q in [
        vec![("uri", "file:///etc/passwd")],
        vec![("uri", "jdt://contents/a.jar/b/C.class")],
    ] {
        assert_eq!(get("code/external", &q).await.unwrap().status(), 404);
    }
    assert_eq!(
        get(
            "code/usages",
            &[
                ("symbol", "x"),
                ("uri", "file:///etc/passwd"),
                ("line", "1")
            ]
        )
        .await
        .unwrap()
        .status(),
        404
    );

    // Saving from the browser: the base hash must match the disk, and the save supersedes attribution.
    let put = |body: serde_json::Value| client.put(format!("{project}/file")).json(&body).send();
    let read = |path: &'static str| async move {
        get("file", &[("path", path)])
            .await
            .unwrap()
            .json::<ProjectFile>()
            .await
            .unwrap()
    };
    let before = read("greeting.txt").await;
    let base_hash = before.hash.clone().expect("a small text file is editable");
    assert_eq!(before.read_only, None);
    assert!(before.changed_by.is_some());
    let r =
        put(json!({"path": "greeting.txt", "content": "hello\nworld\n", "base_hash": base_hash}))
            .await
            .unwrap();
    assert_eq!(r.status(), 200);
    let saved: ProjectFile = r.json().await.unwrap();
    assert_eq!(saved.content.as_deref(), Some("hello\nworld\n"));
    assert_ne!(saved.hash, Some(base_hash.clone()));
    assert_eq!(
        saved.changed_by, None,
        "the user's save supersedes the implementer"
    );
    assert_eq!(
        std::fs::read_to_string(app_dir.join("greeting.txt")).unwrap(),
        "hello\nworld\n"
    );

    let stale = put(json!({"path": "greeting.txt", "content": "x", "base_hash": base_hash}))
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    let err: ApiError = stale.json().await.unwrap();
    assert_eq!(err.issues[0].path, "base_hash");

    std::fs::write(app_dir.join("greeting.txt"), "edited in a shell\n").unwrap();
    let r = put(json!({"path": "greeting.txt", "content": "x", "base_hash": saved.hash}))
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        409,
        "an edit outside Ostra is caught by the hash"
    );
    assert_eq!(
        std::fs::read_to_string(app_dir.join("greeting.txt")).unwrap(),
        "edited in a shell\n"
    );

    let exists = put(json!({"path": "greeting.txt", "content": "x", "base_hash": null}))
        .await
        .unwrap();
    assert_eq!(exists.status(), 409);
    let created = put(
        json!({"path": "src/deep/new.rs", "content": "pub fn fresh() {}\n", "base_hash": null}),
    )
    .await
    .unwrap();
    assert_eq!(created.status(), 200);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(app_dir.join("src/deep/new.rs"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
    }
    let fresh: CodeUsages = get("code/usages", &[("symbol", "fresh")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        fresh.definitions.len(),
        1,
        "the code index sees the saved file"
    );

    // An edit outside Ostra reaches the index through the folder watch, well before the 30 s
    // disk check.
    std::fs::write(
        app_dir.join("src/deep/outside.rs"),
        "pub fn from_the_shell() {}\n",
    )
    .unwrap();
    let mut seen = false;
    for _ in 0..50 {
        let u: CodeUsages = get("code/usages", &[("symbol", "from_the_shell")])
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !u.definitions.is_empty() {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "the watch reports a file written outside Ostra");

    // The agents' code tools answer from the same index.
    use ostra_tools::CodeNav;
    let callers = app
        .code_tools
        .call(&app_dir, "CodeCallers", &json!({"symbol": "hello"}))
        .await
        .unwrap();
    assert!(callers.contains("src/greet.rs:1 fn hello"), "{callers}");
    let impls = app
        .code_tools
        .call(&app_dir, "CodeImplementations", &json!({"symbol": "hello"}))
        .await
        .unwrap();
    assert!(impls.starts_with("src/greet.rs:1 fn hello"), "{impls}");
    assert!(
        impls.contains("implemented or extended by: none"),
        "{impls}"
    );
    assert!(callers.contains("src/lib.rs"), "{callers}");
    let outline = app
        .code_tools
        .call(
            &app_dir,
            "CodeOutline",
            &json!({"path": app_dir.join("src/lib.rs")}),
        )
        .await
        .unwrap();
    assert!(outline.contains("greet -> src/greet.rs"), "{outline}");
    let map = app
        .code_tools
        .call(&app_dir, "CodeMap", &json!({}))
        .await
        .unwrap();
    assert!(map.contains("files indexed"), "{map}");
    let bad = app
        .code_tools
        .call(&app_dir, "CodeNeighbors", &json!({"path": "nope.rs"}))
        .await
        .unwrap_err();
    assert!(bad.contains("not in the code index"), "{bad}");
    // The dependency graph views the project screen draws, and a rebuild from disk.
    let graph = |q: &[(&str, &str)]| get("code/graph", q);
    let pk: ostra_core::code::CodeGraph = graph(&[]).await.unwrap().json().await.unwrap();
    assert_eq!(pk.view, ostra_core::code::CodeGraphView::Packages);
    assert!(pk.nodes.iter().any(|n| n.id == "package:"), "{pk:?}");
    let one: ostra_core::code::CodeGraph = graph(&[("package", ".")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(one.nodes.iter().any(|n| n.id == "src/greet.rs"), "{one:?}");
    let around: ostra_core::code::CodeGraph = graph(&[("path", "src/greet.rs")])
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lib = around
        .nodes
        .iter()
        .find(|n| n.id == "src/lib.rs")
        .expect("lib.rs uses greet.rs");
    assert_eq!(lib.column, -1);
    assert!(
        around
            .edges
            .iter()
            .any(|e| e.from == "src/lib.rs" && e.to == "src/greet.rs")
    );
    let sym: ostra_core::code::CodeGraph = graph(&[
        ("path", "src/greet.rs"),
        ("symbol", "hello"),
        ("depth", "2"),
    ])
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(sym.view, ostra_core::code::CodeGraphView::Symbol);
    assert_eq!(sym.focus, "symbol:src/greet.rs:1:hello");
    assert_eq!(
        graph(&[("path", "src/greet.rs"), ("symbol", "nope")])
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(graph(&[("path", "nope.rs")]).await.unwrap().status(), 404);
    assert_eq!(graph(&[("path", "../x.rs")]).await.unwrap().status(), 403);
    assert_eq!(
        graph(&[("path", "src/greet.rs"), ("package", ".")])
            .await
            .unwrap()
            .status(),
        400
    );
    let r = client
        .post(format!("{project}/code/reindex"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let re: ostra_core::code::CodeReindex = r.json().await.unwrap();
    assert!(re.indexed_files >= 3 && !re.truncated, "{re:?}");

    // Commands edited from the project screen land in `.ostra/project.toml`.
    let put_commands = |body: Value| client.put(format!("{project}/commands")).json(&body).send();
    let r = put_commands(
        json!({"build": "  cargo build  ", "test": "cargo test", "format": "true", "lint": ""}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let saved: ostra_core::config::Commands = r.json().await.unwrap();
    assert_eq!(
        (
            saved.build.as_deref(),
            saved.test.as_deref(),
            saved.lint.as_deref()
        ),
        (Some("cargo build"), Some("cargo test"), None)
    );
    let profile: ostra_core::config::ProjectProfile =
        ostra_core::config::load_toml(&app_dir.join(".ostra/project.toml")).unwrap();
    assert_eq!(profile.commands, saved);
    let bad = put_commands(json!({"build": "make\nmake install"}))
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    let err: ApiError = bad.json().await.unwrap();
    assert_eq!(issue_paths(&err.issues), vec!["commands.build"]);
    assert_eq!(
        ostra_core::config::load_toml::<ostra_core::config::ProjectProfile>(
            &app_dir.join(".ostra/project.toml")
        )
        .unwrap()
        .commands,
        saved,
        "a refused save changes nothing"
    );

    let outside = app
        .code_tools
        .call(Path::new("/"), "CodeMap", &json!({}))
        .await
        .unwrap_err();
    assert!(outside.contains("not inside a project"), "{outside}");

    let git_file = read(".git/HEAD").await;
    assert!(git_file.read_only.is_some());
    let r = put(json!({"path": ".git/HEAD", "content": "x", "base_hash": git_file.hash}))
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = put(json!({"path": "../escape.txt", "content": "x", "base_hash": null}))
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(!root.join("escape.txt").exists());

    let mkdir = |path: &str| {
        client
            .post(format!("{project}/mkdir"))
            .json(&json!({ "path": path }))
            .send()
    };
    let r = mkdir("docs/guides").await.unwrap();
    assert_eq!(r.status(), 200);
    let listing: ProjectTree = r.json().await.unwrap();
    assert_eq!(listing.path, "docs");
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect::<Vec<_>>(),
        vec![("guides", true)]
    );
    assert!(app_dir.join("docs/guides").is_dir());
    assert_eq!(mkdir("docs/guides").await.unwrap().status(), 409);
    assert_eq!(mkdir(".git/hooks2").await.unwrap().status(), 403);
    assert_eq!(mkdir("../outside").await.unwrap().status(), 403);
    assert!(!root.join("outside").exists());
    std::fs::create_dir_all(app_dir.join(".ostra/memory")).unwrap();
    std::fs::write(app_dir.join(".ostra/memory/notes.txt"), "state\n").unwrap();
    let memory = read(".ostra/memory/notes.txt").await;
    assert!(memory.read_only.is_some());
    let r =
        put(json!({"path": ".ostra/memory/notes.txt", "content": "x", "base_hash": memory.hash}))
            .await
            .unwrap();
    assert_eq!(r.status(), 403);

    let fs: FsBrowse = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/fs"),
                &[
                    ("path", root.display().to_string()),
                    ("prefix", "AP".into()),
                ],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entries: Vec<(&str, bool, bool)> = fs
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.is_git, e.is_ostra_project))
        .collect();
    assert_eq!(entries, vec![("app", true, true)]);
    assert!(!fs.is_git && !fs.is_ostra_project);
    let fs: FsBrowse = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/fs"),
                &[("path", app_dir.display().to_string())],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        fs.is_git && fs.is_ostra_project,
        "the browsed folder itself is described"
    );
    let fs: FsBrowse = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/fs"),
                &[("path", root.join("nope/deeper").display().to_string())],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!fs.exists && !fs.readable);
    assert_eq!(fs.nearest, std::fs::canonicalize(root).unwrap());

    let made: FsBrowse = client
        .post(format!("{base}/api/fs/mkdir"))
        .json(&json!({"path": root.join("nope/deeper").display().to_string()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        made.exists && made.readable,
        "mkdir creates the missing parents"
    );
    let refused = client
        .post(format!("{base}/api/fs/mkdir"))
        .json(&json!({"path": "relative/dir"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    // Search: one hit per kind the session produced, plus a lesson, a project, and a setting.
    client
        .patch(format!("{wsp}/projects/app/memory"))
        .json(&json!({"id": null, "area": "greeting", "lesson": "A greeting file ends with a newline."}))
        .send()
        .await
        .unwrap();
    let search = |q: &str| {
        let url = reqwest::Url::parse_with_params(&format!("{wsp}/search"), &[("q", q)]).unwrap();
        let client = client.clone();
        async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .json::<SearchResults>()
                .await
                .unwrap()
        }
    };
    let found = search("greeting").await;
    let first = |kind: SearchKind| {
        found
            .items
            .iter()
            .find(|h| h.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} hit in {:#?}", found.items))
    };
    assert_eq!(first(SearchKind::Session).id, format!("session:{}", s.id));
    assert_eq!(first(SearchKind::Session).label, "Greeting file");
    assert_eq!(first(SearchKind::File).id, "file:app:greeting.txt");
    assert!(
        first(SearchKind::Artifact)
            .id
            .starts_with(&format!("artifact:{}", d.session_root.display())),
        "{:?}",
        first(SearchKind::Artifact)
    );
    assert!(first(SearchKind::Lesson).id.starts_with("lesson:app:"));
    assert!(
        found.items.windows(2).all(|w| w[0].score >= w[1].score),
        "best first"
    );
    let exec = search("implementer").await;
    assert_eq!(exec.items[0].id, format!("exec:{}", imp.id));
    assert_eq!(exec.items[0].label, "implementer · Phase 1");
    assert_eq!(search("app").await.items[0].id, "project:app");
    assert_eq!(
        search("budget").await.items[0].id,
        "setting:limits.session_budget_usd"
    );
    let heading = search("report").await;
    assert!(heading.items.iter().any(|h| h.kind == SearchKind::Artifact
        && h.id == format!("artifact:{}", imp.report_path.clone().unwrap().display())));
    let recent = search("").await;
    assert_eq!(
        recent
            .items
            .iter()
            .map(|h| h.id.clone())
            .collect::<Vec<_>>(),
        [format!("session:{}", s.id)]
    );
    let url =
        reqwest::Url::parse_with_params(&format!("{wsp}/search"), &[("q", "a"), ("limit", "2")])
            .unwrap();
    assert_eq!(
        client
            .get(url)
            .send()
            .await
            .unwrap()
            .json::<SearchResults>()
            .await
            .unwrap()
            .items
            .len(),
        2
    );

    // Activity: nothing runs or waits once the session completed.
    let activity: WorkspaceActivity = client
        .get(format!("{wsp}/activity"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        activity.running.is_empty() && activity.open_gates.is_empty(),
        "{activity:?}"
    );
    assert!(activity.spend_week_usd >= activity.spend_today_usd);
    assert!(
        activity.week_since <= activity.today_since && activity.today_since <= chrono::Utc::now()
    );

    // Cost over all time, over the status bar's week, and over a window with nothing in it.
    let cost = |since: Option<String>| {
        let query: Vec<(&str, String)> = since.into_iter().map(|s| ("since", s)).collect();
        let url = reqwest::Url::parse_with_params(&format!("{wsp}/cost"), &query).unwrap();
        client.get(url).send()
    };
    let all: CostReport = cost(None).await.unwrap().json().await.unwrap();
    assert_eq!(
        (all.since, all.total.executions as usize),
        (None, d.executions.len())
    );
    let week: CostReport = cost(Some(activity.week_since.to_rfc3339()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(week.since, Some(activity.week_since));
    assert_eq!(week.total.executions, all.total.executions);
    assert!(
        (week.total.usage.cost_usd - activity.spend_week_usd).abs() < 1e-9,
        "the same week as the status bar"
    );
    let later: CostReport = cost(Some("2999-01-01T00:00:00Z".into()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(later.total.executions, 0);
    let bad = cost(Some("last week".into())).await.unwrap();
    assert_eq!(bad.status(), 400);
    assert!(
        bad.json::<ApiError>()
            .await
            .unwrap()
            .error
            .contains("RFC 3339")
    );
}

fn issue_paths(issues: &[ValidationIssue]) -> Vec<&str> {
    issues.iter().map(|i| i.path.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setup_wizard_creates_a_workspace_in_one_call() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let Server { app, base, client } = boot(root).await;
    let get = |path: &str| client.get(format!("{base}{path}")).send();
    let post = |path: &str, body: Value| client.post(format!("{base}{path}")).json(&body).send();

    let first: OnboardingState = get("/api/onboarding").await.unwrap().json().await.unwrap();
    assert_eq!(
        first,
        OnboardingState {
            onboarded_at: None,
            workspaces: 0
        }
    );

    let env: EnvironmentStatus = get("/api/environment").await.unwrap().json().await.unwrap();
    assert_eq!(env.harnesses.len(), 4);
    assert!(
        env.providers.iter().any(|p| p.name == "anthropic"),
        "{:?}",
        env.providers
    );
    assert_eq!(
        env.stacks,
        ["go", "java-spring", "python", "typescript-node"]
    );

    let (a, b) = (root.join("a"), root.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ws_root = root.join("shop");
    let good = json!({
        "name": "shop",
        "root": ws_root,
        "projects": [{"path": a, "key": "api", "stack": null}, {"path": b, "key": "web", "stack": "typescript-node"}],
        "permissions": {"mode": "plan"},
        "yolo": {"default": true},
        "routing_preset": "native",
        "notifications": {"push": false},
    });
    let issues: Vec<ValidationIssue> = post("/api/workspaces/validate", good.clone())
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issues, vec![]);
    assert!(!ws_root.exists(), "validation writes nothing");

    let bad = json!({
        "name": " ",
        "root": ws_root,
        "projects": [{"path": a, "key": "api"}, {"path": root.join("missing"), "key": "Web"}],
        "permissions": {"mode": "loose"},
        "routing_preset": "grok",
    });
    let r = post("/api/workspaces/validate", bad.clone()).await.unwrap();
    assert_eq!(r.status(), 200);
    let issues: Vec<ValidationIssue> = r.json().await.unwrap();
    let expected = [
        "name",
        "permissions.mode",
        "projects[1].key",
        "projects[1].path",
        "routing_preset",
    ];
    assert_eq!(issue_paths(&issues), expected);
    let r = post("/api/workspaces", bad).await.unwrap();
    assert_eq!(r.status(), 422);
    let err: ApiError = r.json().await.unwrap();
    assert_eq!(issue_paths(&err.issues), expected);
    assert!(!ws_root.exists(), "a refused create writes nothing");
    assert!(app.shared.registry.list_workspaces().unwrap().is_empty());
    assert_eq!(app.shared.registry.onboarded_at().unwrap(), None);

    let r = post("/api/workspaces", good.clone()).await.unwrap();
    assert_eq!(r.status(), 200);
    let ws: WorkspaceDetail = r.json().await.unwrap();
    assert!(ws.validation.is_empty(), "{:?}", ws.validation);
    assert_eq!(
        ws.projects
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>(),
        ["api", "web"]
    );
    assert_eq!(
        ws.settings.projects[1].stack.as_deref(),
        Some("typescript-node")
    );
    assert_eq!(ws.settings.permissions.mode, PermissionMode::Plan);
    assert!(ws.settings.yolo.default);
    assert!(!ws.settings.notifications.push);
    assert!(ws.settings.routing.executor.by_agent.is_empty());
    assert!(ws_root.join(".ostra/workspace.toml").is_file());
    assert_eq!(ws.stacks, env.stacks);
    assert_eq!(
        ws.global_permissions.deny,
        ["Bash(rm -rf /*)"],
        "the global config's rules, shown read-only"
    );
    assert_eq!(
        ws.projects
            .iter()
            .map(|p| p.git_branch.clone())
            .collect::<Vec<_>>(),
        [None, None],
        "neither folder is a repository"
    );
    assert_eq!(ws.agents.len(), 12);
    let plan = ws
        .agents
        .iter()
        .find(|a| a.name == ostra_core::AgentName::Plan)
        .unwrap();
    assert_eq!(
        (plan.label.as_str(), plan.default_tier),
        ("Plan", ostra_core::Tier::Advanced)
    );
    assert_eq!(
        plan.resolved.as_ref().map(|r| r.model.as_str()),
        Some("mock:m"),
        "resolved through the tier table"
    );
    assert_eq!(plan.default_route, plan.resolved);

    // Importing reports each problem on its request field.
    let projects = format!("/api/workspaces/{}/projects", ws.id);
    let r = post(
        &projects,
        json!({"path": root.join("missing"), "key": "Bad Key", "stack": "Type\u{7}Script"}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 422);
    let err: ApiError = r.json().await.unwrap();
    assert_eq!(issue_paths(&err.issues), ["key", "stack", "path"]);
    assert!(
        err.error
            .starts_with("Fix these problems and import the project again:"),
        "{}",
        err.error
    );
    let r = post(&projects, json!({"path": a, "key": "web", "stack": null}))
        .await
        .unwrap();
    let err: ApiError = r.json().await.unwrap();
    assert_eq!(issue_paths(&err.issues), ["key", "path"]);
    let c = root.join("c");
    std::fs::create_dir_all(&c).unwrap();
    let r = post(
        &projects,
        json!({"path": c, "key": "docs", "stack": "rust-axum"}),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let added: WorkspaceDetail = r.json().await.unwrap();
    assert_eq!(added.projects[2].stack.as_deref(), Some("rust-axum"));

    let again: ApiError = post("/api/workspaces", good)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue_paths(&again.issues), ["root"]);

    let after: OnboardingState = get("/api/onboarding").await.unwrap().json().await.unwrap();
    assert!(after.onboarded_at.is_some());
    assert_eq!(after.workspaces, 1);
    let list: Vec<WorkspaceSummary> = get("/api/workspaces").await.unwrap().json().await.unwrap();
    assert_eq!(
        (list[0].projects, list[0].available, list[0].root.clone()),
        (3, true, ws.root.clone())
    );

    // The per-workspace validate route still answers next to the new one.
    let settings = serde_json::to_value(&ws.settings).unwrap();
    let r = post(&format!("/api/workspaces/{}/validate", ws.id), settings)
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Vec<ValidationIssue>>().await.unwrap(), vec![]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn onboarding_flag_and_ui_state_round_trip() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;

    let done: OnboardingState = client
        .post(format!("{base}/api/onboarding/complete"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let at = done.onboarded_at.expect("marked");
    assert_eq!(done.workspaces, 0);
    let again: OnboardingState = client
        .post(format!("{base}/api/onboarding/complete"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again.onboarded_at, Some(at), "the first time stays");

    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "ui", "root": dir.path().join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ui = format!("{base}/api/workspaces/{}/ui", ws.id);
    let empty: WorkspaceUiState = client.get(&ui).send().await.unwrap().json().await.unwrap();
    assert_eq!(empty, WorkspaceUiState::default());

    let r = client
        .patch(&ui)
        .json(&json!({"tabs": [{"id": "ws:overview", "pinned": true}, {"id": "session:s1", "preview": true}], "active": "session:s1", "left_tab": "files", "files_project": "app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = client
        .patch(&ui)
        .json(&json!({"theme": "dark", "dock_open": true}))
        .send()
        .await
        .unwrap();
    let merged: WorkspaceUiState = r.json().await.unwrap();
    let stored: WorkspaceUiState = client.get(&ui).send().await.unwrap().json().await.unwrap();
    assert_eq!(stored, merged);
    assert_eq!(
        stored.tabs,
        vec![
            UiTab {
                id: "ws:overview".into(),
                pinned: true,
                preview: false
            },
            UiTab {
                id: "session:s1".into(),
                pinned: false,
                preview: true
            }
        ]
    );
    assert_eq!(
        (
            stored.active.as_deref(),
            stored.left_tab.as_deref(),
            stored.theme.as_deref(),
            stored.dock_open
        ),
        (Some("session:s1"), Some("files"), Some("dark"), true)
    );

    let big = json!({"active": "a".repeat(UI_STATE_MAX_BYTES)});
    assert_eq!(
        client.patch(&ui).json(&big).send().await.unwrap().status(),
        413
    );
    assert_eq!(
        client
            .patch(&ui)
            .json(&json!([1, 2]))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let unchanged: WorkspaceUiState = client.get(&ui).send().await.unwrap().json().await.unwrap();
    assert_eq!(unchanged, stored);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_credentials_saved_from_the_browser() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { app, base, client } = boot(dir.path()).await;
    // Point the variables somewhere unset, so this machine's own keys do not interfere.
    let mut global = GlobalConfig::default();
    let openai = global.providers.get_mut("openai").unwrap();
    openai.api_key_env = Some("OSTRA_E2E_UNSET_KEY".into());
    openai.base_url_env = Some("OSTRA_E2E_UNSET_URL".into());
    save_toml(&dir.path().join("config.toml"), &global).unwrap();

    let url = format!("{base}/api/providers/openai");
    let r = client
        .patch(&url)
        .json(&json!({"base_url": "https://gw.example/v1", "api_key": "sk-e2e-secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(!body.contains("sk-e2e-secret"), "{body}");
    let status: ProviderStatus = serde_json::from_str(&body).unwrap();
    assert_eq!((status.has_key, status.source.as_str()), (true, "saved"));
    assert_eq!(
        (status.base_url.as_deref(), status.base_url_source.as_str()),
        (Some("https://gw.example/v1"), "saved")
    );
    assert!(status.saved.has_api_key && !status.saved.has_auth_token);
    assert!(app.shared.providers.get("openai").is_some());
    assert!(
        app.shared.providers.get("mock").is_some(),
        "registered providers survive a reload"
    );

    let r = client
        .patch(&url)
        .json(&json!({"base_url": "gw.example", "api_key": "two words"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let err: serde_json::Value = r.json().await.unwrap();
    assert_eq!(err["issues"].as_array().unwrap().len(), 2);

    let status: ProviderStatus = client
        .patch(&url)
        .json(&json!({"api_key": ""}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!status.has_key && !status.saved.has_api_key);
    assert_eq!(
        status.saved.base_url.as_deref(),
        Some("https://gw.example/v1")
    );
    assert!(app.shared.providers.get("openai").is_none());

    assert_eq!(
        client
            .patch(format!("{base}/api/providers/nope"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skills_are_listed_edited_adopted_and_deleted() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;
    let repo = dir.path().join("app");
    std::fs::create_dir_all(repo.join(".claude/skills/deploy/references")).unwrap();
    std::fs::write(
        repo.join(".claude/skills/deploy/SKILL.md"),
        "---\ndescription: Deploy the app.\n---\n# Deploy\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(".claude/skills/deploy/references/steps.md"),
        "1. ship\n",
    )
    .unwrap();
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "sk", "root": dir.path().join("ws"), "projects": [{"path": repo, "key": "app"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wsp = format!("{base}/api/workspaces/{}", ws.id);
    let list = || async {
        client
            .get(format!("{wsp}/skills"))
            .send()
            .await
            .unwrap()
            .json::<Vec<ProjectSkills>>()
            .await
            .unwrap()
    };

    let before = list().await;
    assert_eq!(before[0].skills.len(), 1);
    assert_eq!(
        (
            before[0].skills[0].origin,
            before[0].skills[0].description.as_deref()
        ),
        (SkillOrigin::Harness, Some("Deploy the app."))
    );

    let skill = format!("{wsp}/projects/app/skills/entity");
    let r = client.put(&skill).json(&json!({"kind": "creation", "component_type": "Entity", "content": "---\ndescription: Entities.\n---\n"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let doc: SkillDoc = r.json().await.unwrap();
    assert_eq!(
        doc.skill
            .entry
            .as_ref()
            .map(|e| (e.path.as_str(), e.source.as_deref())),
        Some((".agents/skills/entity/SKILL.md", Some("user")))
    );
    assert!(repo.join(".agents/skills/entity/SKILL.md").is_file());

    let r = client
        .put(&skill)
        .json(&json!({"kind": "nope", "content": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = client
        .put(format!("{wsp}/projects/app/skills/a.b"))
        .json(&json!({"kind": "other", "content": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let r = client
        .post(format!("{wsp}/projects/app/skills/deploy/adopt"))
        .json(&json!({"from": ".claude/skills/deploy/SKILL.md", "kind": "other"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        repo.join(".agents/skills/deploy/references/steps.md")
            .is_file()
    );
    let r = client
        .post(format!("{wsp}/projects/app/skills/deploy/adopt"))
        .json(&json!({"from": ".claude/skills/deploy/SKILL.md", "kind": "other"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "the name is taken now");

    let names: Vec<(String, SkillOrigin, bool)> = list().await[0]
        .skills
        .iter()
        .map(|s| (s.name.clone(), s.origin, s.entry.is_some()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("entity".into(), SkillOrigin::Ostra, true),
            ("deploy".into(), SkillOrigin::Ostra, true),
            ("deploy".into(), SkillOrigin::Harness, false)
        ]
    );

    assert_eq!(client.delete(&skill).send().await.unwrap().status(), 204);
    assert!(!repo.join(".agents/skills/entity").exists());
    assert_eq!(client.delete(&skill).send().await.unwrap().status(), 404);
    assert_eq!(client.get(&skill).send().await.unwrap().status(), 404);
}

/// A signed-in WebSocket on `server`, with its own sign-in.
async fn socket(
    server: &Server,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = server.app.auth.sign_in_url().unwrap();
    let token = url.split("#token=").nth(1).unwrap();
    let r = reqwest::Client::new()
        .post(format!("{}/api/auth/exchange", server.base))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    let set = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(
        !set.contains("Secure"),
        "plain http gets no Secure flag: {set}"
    );
    let cookie = set.split(';').next().unwrap().to_string();
    let mut req = format!("{}/ws", server.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut()
        .insert("origin", server.base.parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap().0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_streams_over_the_socket() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let server = boot(dir.path()).await;
    let mut ws = socket(&server).await;
    let exec = ostra_core::ids::ExecutionId::new();
    let send = |v: Value| Message::Text(v.to_string().into());
    let next = async |ws: &mut tokio_tungstenite::WebSocketStream<_>| -> Message {
        tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no message")
            .unwrap()
            .unwrap()
    };

    // Subscribed before the run has a PTY: the stream starts once it does.
    ws.send(send(
        json!({"type": "subscribe", "channel": format!("term:{exec}")}),
    ))
    .await
    .unwrap();
    assert!(matches!(next(&mut ws).await, Message::Text(t) if t.contains("subscribed")));
    let plan = ostra_exec_harness::launch::LaunchPlan {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "echo hello-ws; read x; echo after-$x; sleep 5".into(),
        ],
        env: vec![],
        cwd: dir.path().to_path_buf(),
        files: vec![],
        links: vec![],
        session_id: None,
        env_remove: vec![],
        tty_param: None,
    };
    let pty = ostra_exec_harness::PtySession::spawn(&plan, 80, 24, Box::new(|_| {})).unwrap();
    let ptys = server.app.shared.harness.ptys();
    ptys.insert(exec.clone(), pty.clone());

    let mut seen = String::new();
    let mut first = true;
    let prefix = 1 + exec.as_str().len();
    while !seen.contains("hello-ws") {
        let Message::Binary(b) = next(&mut ws).await else {
            continue;
        };
        assert_eq!(&b[1..prefix], exec.as_str().as_bytes());
        if std::mem::take(&mut first) {
            assert!(
                b[prefix..].starts_with(b"\x1bc"),
                "the stream opens with a snapshot"
            );
        }
        seen.push_str(&String::from_utf8_lossy(&b[prefix..]));
    }

    // Limits come back as errors and do not reach the PTY.
    ws.send(send(
        json!({"type": "term_resize", "execution": exec, "cols": 0, "rows": 24}),
    ))
    .await
    .unwrap();
    assert!(matches!(next(&mut ws).await, Message::Text(t) if t.contains("out of range")));
    ws.send(send(
        json!({"type": "term_input", "execution": exec, "data": "x".repeat(70 * 1024)}),
    ))
    .await
    .unwrap();
    assert!(matches!(next(&mut ws).await, Message::Text(t) if t.contains("64 KiB")));

    ws.send(send(
        json!({"type": "term_input", "execution": exec, "data": "go\r"}),
    ))
    .await
    .unwrap();
    let mut after = String::new();
    while !after.contains("after-go") {
        if let Message::Binary(b) = next(&mut ws).await {
            after.push_str(&String::from_utf8_lossy(&b[prefix..]));
        }
    }

    // A second subscribe on the same socket resends the screen rather than doubling the stream.
    ws.send(send(
        json!({"type": "subscribe", "channel": format!("term:{exec}")}),
    ))
    .await
    .unwrap();
    let mut snapshot = None;
    while snapshot.is_none() {
        if let Message::Binary(b) = next(&mut ws).await {
            snapshot = Some(b[prefix..].to_vec());
        }
    }
    let snapshot = String::from_utf8_lossy(snapshot.as_deref().unwrap()).into_owned();
    assert!(
        snapshot.starts_with("\x1bc") && snapshot.contains("after-go"),
        "{snapshot:?}"
    );

    ptys.remove(&exec);
    pty.terminate(Duration::from_millis(200)).await;
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Serve a bare repository over git's dumb HTTP protocol, answering 401 without the expected
/// Basic credentials.
async fn serve_repo(bare: PathBuf, basic: String) -> String {
    use axum::http::{HeaderMap, StatusCode, Uri, header};
    let handler = move |uri: Uri, headers: HeaderMap| {
        let (bare, basic) = (bare.clone(), basic.clone());
        async move {
            let authorized = headers
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                == Some(basic.as_str());
            if !authorized {
                return (
                    StatusCode::UNAUTHORIZED,
                    [(header::WWW_AUTHENTICATE, "Basic realm=\"git\"")],
                    Vec::new(),
                );
            }
            let rel = uri.path().trim_start_matches("/repo.git/");
            match std::fs::read(bare.join(rel)) {
                Ok(bytes) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/octet-stream")],
                    bytes,
                ),
                Err(_) => (
                    StatusCode::NOT_FOUND,
                    [(header::CONTENT_TYPE, "text/plain")],
                    Vec::new(),
                ),
            }
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = axum::Router::new().fallback(handler);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://127.0.0.1:{port}/repo.git")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn projects_clone_and_pull_with_a_saved_git_credential() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "gitws", "root": dir.path().join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let src = dir.path().join("src");
    let bare = dir.path().join("repo.git");
    std::fs::create_dir(&src).unwrap();
    git(&src, &["init", "-q"]);
    std::fs::write(src.join("a.txt"), "1").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-qm", "one"]);
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            "--bare",
            &src.to_string_lossy(),
            &bare.to_string_lossy(),
        ],
    );
    git(&bare, &["update-server-info"]);
    // base64 of "x-access-token:tok-e2e-secret"
    let url = serve_repo(
        bare.clone(),
        "Basic eC1hY2Nlc3MtdG9rZW46dG9rLWUyZS1zZWNyZXQ=".into(),
    )
    .await;

    let clone = format!("{base}/api/workspaces/{}/clone", ws.id);
    let r = client
        .post(&clone)
        .json(&json!({"url": "file:///etc", "key": "Bad Key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let err: ApiError = r.json().await.unwrap();
    assert_eq!(issue_paths(&err.issues), ["url", "key"]);

    // No credential saved yet: git fails instead of prompting.
    let r = client
        .post(&clone)
        .json(&json!({"url": url, "key": "app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
    assert!(!dir.path().join("ws/app").exists());

    let creds = format!("{base}/api/git/credentials");
    let r = client
        .post(&creds)
        .json(&json!({"host": "127.0.0.1", "kind": "https", "secret": "tok-e2e-secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    assert!(!body.contains("tok-e2e-secret"), "{body}");

    let r = client
        .post(&clone)
        .json(&json!({"url": url, "key": "app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let detail: WorkspaceDetail = r.json().await.unwrap();
    let app = detail.projects.iter().find(|p| p.key == "app").unwrap();
    assert!(app.is_git);
    assert_eq!(app.git_branch.as_deref(), Some("main"));
    let checkout = dir.path().join("ws/app");
    assert!(checkout.join("a.txt").exists());
    let config = std::fs::read_to_string(checkout.join(".git/config")).unwrap();
    assert!(!config.contains("tok-e2e-secret"), "{config}");

    let pull = format!("{base}/api/workspaces/{}/projects/app/pull", ws.id);
    let out: Value = client
        .post(&pull)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(out["updated"], false, "{out}");

    std::fs::write(src.join("b.txt"), "2").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-qm", "two"]);
    git(&src, &["push", "-q", &bare.to_string_lossy(), "main"]);
    git(&bare, &["update-server-info"]);
    let r = client.post(&pull).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let out: Value = r.json().await.unwrap();
    assert_eq!(out["updated"], true, "{out}");
    assert!(checkout.join("b.txt").exists());

    let list: Vec<Value> = client
        .get(&creds)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = list[0]["id"].as_str().unwrap();
    let left: Vec<Value> = client
        .delete(format!("{creds}/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(left.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_git_dock_stages_commits_pushes_and_switches_branches() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "gitdock", "root": dir.path().join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let src = dir.path().join("src");
    let bare = dir.path().join("origin.git");
    let work = dir.path().join("ws/app");
    std::fs::create_dir(&src).unwrap();
    git(&src, &["init", "-q"]);
    std::fs::write(src.join("a.txt"), "1\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-qm", "one"]);
    let (bare_s, work_s) = (bare.to_string_lossy(), work.to_string_lossy());
    git(
        dir.path(),
        &["clone", "-q", "--bare", &src.to_string_lossy(), &bare_s],
    );
    git(dir.path(), &["clone", "-q", &bare_s, &work_s]);
    git(&work, &["config", "user.name", "Dock"]);
    git(&work, &["config", "user.email", "dock@example.com"]);
    let r = client
        .post(format!("{base}/api/workspaces/{}/projects", ws.id))
        .json(&json!({"path": work, "key": "app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());

    let g = format!("{base}/api/workspaces/{}/projects/app/git", ws.id);
    let post = |path: &str, body: Value| {
        let req = client.post(format!("{g}/{path}")).json(&body);
        async move { req.send().await.unwrap() }
    };
    let ok = |r: reqwest::Response| async move {
        assert_eq!(r.status(), 200);
        r.json::<GitOpResult>().await.unwrap().status
    };
    let paths = |l: &[GitChange]| l.iter().map(|c| c.path.clone()).collect::<Vec<_>>();

    std::fs::write(work.join("a.txt"), "2\n").unwrap();
    std::fs::write(work.join("new.txt"), "n\n").unwrap();
    let st: GitRepoStatus = client.get(&g).send().await.unwrap().json().await.unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert!(st.staged.is_empty());
    assert_eq!(paths(&st.unstaged), ["a.txt", "new.txt"]);
    assert_eq!(st.unstaged[1].mark, GitMark::Untracked);

    assert_eq!(
        post("stage", json!({"paths": ["../x"]})).await.status(),
        400
    );
    let st = ok(post("stage", json!({"paths": ["new.txt", "a.txt"]})).await).await;
    assert_eq!(paths(&st.staged), ["a.txt", "new.txt"]);
    assert_eq!(st.staged[1].mark, GitMark::Added);
    let st = ok(post("unstage", json!({"paths": ["a.txt"]})).await).await;
    assert_eq!(paths(&st.staged), ["new.txt"]);
    assert_eq!(paths(&st.unstaged), ["a.txt"]);

    assert_eq!(post("commit", json!({"message": "  "})).await.status(), 400);
    let st = ok(post("commit", json!({"message": "Add new.txt"})).await).await;
    assert!(st.staged.is_empty());
    assert_eq!((st.ahead, st.behind), (1, 0));
    let st = ok(post("push", json!({})).await).await;
    assert_eq!(st.ahead, 0);
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&bare)
        .args(["log", "-1", "--format=%s", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Add new.txt");

    let st = ok(post("checkout", json!({"branch": "feature", "create": true})).await).await;
    assert_eq!(st.branch.as_deref(), Some("feature"));
    assert_eq!(st.upstream, None);
    assert_eq!(
        paths(&st.unstaged),
        ["a.txt"],
        "local changes move to the new branch"
    );
    let st = ok(post("push", json!({})).await).await;
    assert_eq!(st.upstream.as_deref(), Some("origin/feature"));
    assert_eq!(
        post("checkout", json!({"branch": "--orphan"}))
            .await
            .status(),
        400
    );
    let st = ok(post("checkout", json!({"branch": "main"})).await).await;
    assert_eq!(st.branch.as_deref(), Some("main"));
    let st = ok(post("fetch", json!({})).await).await;
    assert_eq!(st.behind, 0);

    let branches: Vec<GitBranch> = client
        .get(format!("{g}/branches"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<_> = branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["main", "feature", "origin/feature", "origin/main"]);
    assert!(branches[0].current);
    assert_eq!(branches[0].subject, "Add new.txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_login_runs_in_a_setup_terminal() {
    use std::os::unix::fs::PermissionsExt;
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { app, base, client } = boot(dir.path()).await;
    let fake = dir.path().join("fake-claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho \"login $*\"\nread x\necho \"got $x\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut global = GlobalConfig::default();
    global.harness.get_mut("claude").unwrap().command = fake.display().to_string();
    save_toml(&dir.path().join("config.toml"), &global).unwrap();
    let mut pushed = app.push.subscribe();

    let start = || async {
        client
            .post(format!("{base}/api/harnesses/claude/setup"))
            .json(&json!({"action": "login"}))
            .send()
            .await
            .unwrap()
            .json::<HarnessSetupTerminal>()
            .await
            .unwrap()
    };
    let term = start().await;
    assert_eq!(term.terminal, "setup_claude_login");
    assert!(
        term.command.ends_with("fake-claude auth login"),
        "{}",
        term.command
    );
    let pty = app
        .shared
        .harness
        .ptys()
        .get(&term.terminal.as_str().into())
        .unwrap();
    let wait_for = |text: &'static str| {
        let pty = pty.clone();
        async move {
            for _ in 0..100 {
                if pty.screen_text().contains(text) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("no {text:?} in {}", pty.screen_text());
        }
    };
    wait_for("login auth login").await;
    start().await;
    assert!(
        Arc::ptr_eq(
            &pty,
            &app.shared
                .harness
                .ptys()
                .get(&term.terminal.as_str().into())
                .unwrap()
        ),
        "a running terminal is reattached, not started twice"
    );
    pty.write(b"code-123\r").unwrap();
    wait_for("got code-123").await;
    let msg = tokio::time::timeout(Duration::from_secs(10), pushed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.channels, vec!["home".to_string()]);
    assert!(matches!(msg.msg, ServerMsg::HarnessStatus { .. }));

    let r = client
        .post(format!("{base}/api/harnesses/nope/setup"))
        .json(&json!({"action": "login"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_workspace_keeps_projects_and_session_folders() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { app, base, client } = boot(dir.path()).await;
    let repo = dir.path().join("app");
    std::fs::create_dir_all(repo.join(".ostra")).unwrap();
    std::fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.join(".ostra/INVENTORY.md"), "# app\n").unwrap();
    let root = dir.path().join("ws");
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "del", "root": root, "projects": [{"path": repo, "key": "app"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wsp = format!("{base}/api/workspaces/{}", ws.id);
    let session_dir = ostra_core::paths::session_root(&ws.root, "s_old");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("spec.md"), "# spec\n").unwrap();
    assert!(ostra_core::paths::workspace_db(&ws.root).is_file());

    let rt = app.workspace(&ws.id).unwrap();
    rt.cloning.lock().push(("app".into(), repo.clone()));
    let r = client.delete(&wsp).send().await.unwrap();
    assert_eq!(r.status(), 409);
    assert!(app.workspace(&ws.id).is_some());
    rt.cloning.lock().clear();
    drop(rt);

    let r = client.delete(&wsp).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert!(app.workspace(&ws.id).is_none());
    let list: Vec<WorkspaceSummary> = client
        .get(format!("{base}/api/workspaces"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());
    assert_eq!(client.get(&wsp).send().await.unwrap().status(), 404);
    assert_eq!(client.delete(&wsp).send().await.unwrap().status(), 404);

    assert!(!ostra_core::paths::workspace_toml(&ws.root).exists());
    assert!(!ostra_core::paths::workspace_db(&ws.root).exists());
    assert!(session_dir.join("spec.md").is_file());
    assert!(repo.join("main.rs").is_file());
    assert!(repo.join(".ostra/INVENTORY.md").is_file());

    let again: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "del", "root": root, "projects": [{"path": repo, "key": "app"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(again.id, ws.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_are_kept_in_the_session_and_downloadable() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let Server { base, client, .. } = boot(root).await;
    let app_dir = root.join("app");
    std::fs::create_dir_all(app_dir.join(".ostra")).unwrap();
    std::fs::write(app_dir.join(".ostra/INVENTORY.md"), "# app Inventory\n").unwrap();
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "up", "root": root.join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let _: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces/{}/projects", ws.id))
        .json(&json!({"path": app_dir, "key": "app", "stack": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0, 0xff, 0xfe, 1, 2, 3];
    let up: ostra_core::api::UploadRef = client
        .post(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/workspaces/{}/uploads", ws.id),
                &[("name", "../My diagram.png")],
            )
            .unwrap(),
        )
        .header("content-type", "application/octet-stream")
        .body(bytes.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(up.name, "My diagram.png");
    assert_eq!(up.size, bytes.len() as u64);

    let s: SessionSummary = client
        .post(format!("{base}/api/workspaces/{}/sessions", ws.id))
        .json(&json!({"request": "Explain the diagram", "uploads": [up.id]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let d: SessionDetail = client
        .get(format!("{base}/api/sessions/{}", s.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(d.uploads.len(), 1);
    let art = d
        .artifacts
        .iter()
        .find(|a| a.kind == "upload")
        .expect("the upload is an artifact");
    let path = art.path.display().to_string();

    let a: ostra_core::api::Artifact = client
        .get(
            reqwest::Url::parse_with_params(&format!("{base}/api/artifacts"), &[("path", &path)])
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(a.binary);
    assert_eq!(a.size, bytes.len() as u64);

    let r = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/artifacts/download"),
                &[("path", &path)],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename*=UTF-8''My%20diagram.png"
    );
    assert_eq!(r.bytes().await.unwrap().to_vec(), bytes);

    std::fs::write(root.join("secret.txt"), "x").unwrap();
    let outside = client
        .get(
            reqwest::Url::parse_with_params(
                &format!("{base}/api/artifacts/download"),
                &[("path", root.join("secret.txt").display().to_string())],
            )
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(outside.status(), 403);
    let _ = client
        .post(format!("{base}/api/sessions/{}/stop", s.id))
        .send()
        .await;
}

/// Wait up to `secs` for `path` to exist.
async fn appears(path: &Path, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    path.exists()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn folder_commands_run_only_after_approval() {
    use ostra_core::api::PendingKind;
    use ostra_core::config::{
        LanguageServerConfig, McpServerConfig, ProjectEntry, WorkspaceSettings,
    };
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let Server { base, client, .. } = boot(root).await;
    // Inside the folders each program may write, because both run sandboxed.
    let (mcp_marker, lsp_marker) = (root.join("evil/mcp-ran"), root.join("app/lsp-ran"));
    let touch = |m: &Path| {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("touch {}", m.display()),
        ]
    };

    // A repository that ships its own workspace file, as a clone or a pull would bring it.
    let app_dir = root.join("app");
    std::fs::create_dir_all(app_dir.join("src")).unwrap();
    std::fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(app_dir.join(".ostra")).unwrap();
    std::fs::write(
        app_dir.join(".ostra/project.toml"),
        format!(
            "[commands]\nformat = \"touch {}\"\n",
            root.join("fmt-ran").display()
        ),
    )
    .unwrap();
    let evil = root.join("evil");
    let mut s = WorkspaceSettings::seeded("evil");
    let mut server = McpServerConfig::local("evil", &[]);
    server.command = touch(&mcp_marker);
    s.mcp_servers.push(server);
    s.projects.push(ProjectEntry {
        key: "app".into(),
        path: app_dir.clone(),
        stack: None,
        code_provider: None,
        language_servers: vec![LanguageServerConfig {
            command: touch(&lsp_marker),
            languages: vec!["rust".into()],
            timeout_secs: 2,
            initialization_options: None,
        }],
    });
    s.permissions.mode = PermissionMode::Bypass;
    s.permissions.allow.push("Bash(*)".into());
    s.yolo.default = true;
    save_toml(&ostra_core::paths::workspace_toml(&evil), &s).unwrap();

    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "evil", "root": evil}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ws.settings.permissions.mode, PermissionMode::Default);
    assert!(
        !ws.settings.yolo.default,
        "a folder file never turns YOLO on"
    );
    let wsp = format!("{base}/api/workspaces/{}", ws.id);
    let kinds: Vec<(Option<String>, Vec<PendingKind>)> = ws
        .pending_commands
        .iter()
        .map(|p| (p.project.clone(), p.items.iter().map(|i| i.kind).collect()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                None,
                vec![
                    // `app` lives outside the workspace folder, so agents could write there.
                    PendingKind::ProjectOutside,
                    PendingKind::McpServer,
                    PendingKind::LanguageServer,
                    PendingKind::AllowRule
                ]
            ),
            (Some("app".into()), vec![PendingKind::FormatCommand]),
        ]
    );
    let shown = serde_json::to_string(&ws.pending_commands).unwrap();
    assert!(
        shown.contains(&format!("touch {}", mcp_marker.display())),
        "{shown}"
    );

    let mcp: Value = client
        .get(format!("{wsp}/mcp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        mcp[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Approve"),
        "{mcp}"
    );
    let r = client
        .get(format!("{wsp}/projects/app/code/file?path=src/main.rs"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!mcp_marker.exists() && !lsp_marker.exists());

    // A save made in Ostra does not approve what waits for approval.
    let mut edited = ws.settings.clone();
    edited.instructions.all = Some("Keep it short.".into());
    let saved: WorkspaceDetail = client
        .patch(&wsp)
        .json(&edited)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved.pending_commands.len(), 2);

    let stale = client
        .post(format!("{wsp}/approve"))
        .json(&json!({"hash": "0000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    let hash = saved.pending_commands[0].hash.clone();
    let approved: WorkspaceDetail = client
        .post(format!("{wsp}/approve"))
        .json(&json!({"hash": hash}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        approved.pending_commands.len(),
        1,
        "the format command still waits"
    );

    let _ = client.get(format!("{wsp}/mcp")).send().await.unwrap();
    assert!(
        appears(&mcp_marker, 10).await,
        "the approved MCP server starts"
    );
    let _ = client
        .get(format!("{wsp}/projects/app/code/file?path=src/main.rs"))
        .send()
        .await
        .unwrap();
    assert!(
        appears(&lsp_marker, 10).await,
        "the approved language server starts"
    );

    // Once approved, the user's own edits stay approved, and the mode saves outside the file.
    let mut more = approved.settings.clone();
    let mut second = McpServerConfig::local("second", &[]);
    second.command = vec!["true".into()];
    more.mcp_servers.push(second);
    more.permissions.mode = PermissionMode::AcceptEdits;
    let saved: WorkspaceDetail = client
        .patch(&wsp)
        .json(&more)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        saved.pending_commands.len(),
        1,
        "{:?}",
        saved.pending_commands
    );
    assert_eq!(saved.settings.permissions.mode, PermissionMode::AcceptEdits);
    let text = std::fs::read_to_string(ostra_core::paths::workspace_toml(&evil)).unwrap();
    assert!(
        !text.contains("acceptEdits") && !text.contains("yolo"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_workspace_needs_no_approval_for_its_own_commands() {
    use ostra_core::config::McpServerConfig;
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "mine", "root": dir.path().join("ws"), "permissions": {"mode": "plan"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ws.pending_commands.is_empty());
    assert_eq!(ws.settings.permissions.mode, PermissionMode::Plan);
    let mut s = ws.settings.clone();
    s.mcp_servers
        .push(McpServerConfig::local("tool", &["true"]));
    s.permissions.allow.push("Bash(cargo test)".into());
    let saved: WorkspaceDetail = client
        .patch(format!("{base}/api/workspaces/{}", ws.id))
        .json(&s)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        saved.pending_commands.is_empty(),
        "{:?}",
        saved.pending_commands
    );
    assert_eq!(saved.settings.permissions.mode, PermissionMode::Plan);
}

/// Sign in with a fresh token, as a separate browser, and return its `Cookie` header value.
async fn sign_in_cookie(server: &Server) -> String {
    let url = server.app.auth.sign_in_url().unwrap();
    let token = url.split("#token=").nth(1).unwrap();
    let r = reqwest::Client::new()
        .post(format!("{}/api/auth/exchange", server.base))
        .header("user-agent", "SecondBrowser/2")
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let set = r.headers()["set-cookie"].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

async fn socket_with(
    server: &Server,
    cookie: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("{}/ws", server.base.replace("http://", "ws://"))
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut()
        .insert("origin", server.base.parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap().0
}

async fn status_with(server: &Server, cookie: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{}/api/auth/sessions", server.base))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[track_caller]
fn signed_out(
    msg: Option<
        Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    >,
) {
    use tokio_tungstenite::tungstenite::Message;
    match msg {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 4401),
        other => panic!("expected a sign-out close frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_in_sessions_are_listed_and_revoked() {
    use futures::StreamExt;
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let server = boot(dir.path()).await;
    let base = server.base.clone();
    let list = async || -> Vec<ostra_core::api::SignInSession> {
        server
            .client
            .get(format!("{base}/api/auth/sessions"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    };

    let second = sign_in_cookie(&server).await;
    let sessions = list().await;
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions.iter().filter(|s| s.current).count(), 1);
    let other = sessions.iter().find(|s| !s.current).unwrap();
    assert_eq!(other.user_agent.as_deref(), Some("SecondBrowser/2"));
    assert_eq!(other.ip.as_deref(), Some("127.0.0.1"));
    assert!(
        !second.contains(&other.id),
        "the listed id is not the cookie"
    );

    // Revoked from the browser: the next request is refused and the open socket closes at once.
    let mut ws = socket_with(&server, &second).await;
    assert_eq!(status_with(&server, &second).await, 200);
    let r = server
        .client
        .delete(format!("{base}/api/auth/sessions/{}", other.id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    signed_out(
        tokio::time::timeout(Duration::from_secs(1), ws.next())
            .await
            .expect("the socket closes within a second"),
    );
    assert_eq!(status_with(&server, &second).await, 401);

    // Revoked from the CLI, another process: refused within the recheck window.
    let third = sign_in_cookie(&server).await;
    let mut ws = socket_with(&server, &third).await;
    let id = list().await.into_iter().find(|s| !s.current).unwrap().id;
    assert!(ostra_server::auth::revoke_sign_in(&server.app.shared.registry, &id).unwrap());
    signed_out(
        tokio::time::timeout(
            ostra_server::auth::RECHECK + Duration::from_secs(2),
            ws.next(),
        )
        .await
        .expect("the socket closes within the recheck window"),
    );
    assert_eq!(status_with(&server, &third).await, 401);

    // Everyone else signed out; this browser stays.
    let fourth = sign_in_cookie(&server).await;
    let r: ostra_core::api::RevokedSignIns = server
        .client
        .post(format!("{base}/api/auth/sessions/revoke-others"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.revoked, 1);
    assert_eq!(status_with(&server, &fourth).await, 401);
    assert_eq!(list().await.len(), 1);

    // A guessing loop is cut off after a few failures from one address.
    let bad = async || {
        reqwest::Client::new()
            .post(format!("{base}/api/auth/exchange"))
            .json(&json!({"token": "0000"}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    };
    // Loopback is never rate limited, so a local process cannot lock the user out.
    for _ in 0..6 {
        assert_eq!(bad().await, 401);
    }

    // Signing out clears the cookie and revokes this sign-in.
    let r = server
        .client
        .post(format!("{base}/api/auth/signout"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert!(
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let r = server
        .client
        .get(format!("{base}/api/auth/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_browser_boundary_refuses_page_content_and_other_sites() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let Server { base, client, .. } = boot(dir.path()).await;
    let url = format!("{base}/api/environment");
    let get = |headers: &[(&str, &str)]| {
        let mut r = client.get(&url);
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        r.send()
    };

    let ok = get(&[("sec-fetch-site", "same-origin"), ("sec-fetch-dest", "empty")])
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let csp = ok.headers()["content-security-policy"].to_str().unwrap();
    for part in ["script-src 'self'", "img-src 'self' data:", "frame-ancestors 'none'", "object-src 'none'"] {
        assert!(csp.contains(part), "{csp}");
    }
    assert_eq!(ok.headers()["cache-control"], "no-store");

    for headers in [
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
        vec![("sec-fetch-site", "same-origin"), ("sec-fetch-dest", "image")],
        vec![("sec-fetch-site", "same-origin"), ("sec-fetch-dest", "script")],
        vec![("sec-fetch-site", "same-origin"), ("sec-fetch-dest", "iframe")],
        vec![("origin", "http://evil.example")],
        vec![("origin", "null")],
    ] {
        let r = get(&headers).await.unwrap();
        assert_eq!(r.status(), 403, "{headers:?}");
        // A refusal is framed or sniffed no more than an answer is.
        assert_eq!(r.headers()["x-frame-options"], "DENY", "{headers:?}");
        assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    }
    let misdirected = client
        .get(&url)
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(misdirected.status(), 421);
    assert_eq!(misdirected.headers()["x-frame-options"], "DENY");
    let csp = misdirected.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    assert!(
        csp.contains("connect-src 'self';"),
        "a refused host is not named: {csp}"
    );
    let unsigned = reqwest::Client::new().get(&url).send().await.unwrap();
    assert_eq!(unsigned.status(), 401);
    assert_eq!(unsigned.headers()["x-frame-options"], "DENY");
    assert_eq!(unsigned.headers()["cache-control"], "no-store");

    // The harness bridge refuses browser requests before reading the body or the bearer token.
    for path in ["/internal/policy", "/internal/mcp"] {
        for headers in [
            vec![("origin", "http://127.0.0.1:9")],
            vec![("sec-fetch-site", "same-site")],
            vec![("sec-fetch-site", "same-origin")],
        ] {
            let mut r = client
                .post(format!("{base}{path}"))
                .header("content-type", "application/json")
                .body("{}");
            for (k, v) in &headers {
                r = r.header(*k, *v);
            }
            let r = r.send().await.unwrap();
            assert_eq!(r.status(), 403, "{path} {headers:?}");
        }
        let r = client
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_ne!(r.status(), 403, "{path} without browser headers reaches the bridge");
    }

    let page = client.get(format!("{base}/")).send().await.unwrap();
    assert!(page.headers().contains_key("content-security-policy"));
    assert_eq!(page.headers()["x-frame-options"], "DENY");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_loopback_server_lives_at_its_private_name() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Boots at 127.0.0.1 with the private name on; the helper's own sign-in on 127.0.0.1 is
    // refused, so this test signs in again at the private name.
    let url = {
        unsafe {
            std::env::set_var("OSTRA_CONFIG", root.join("config.toml"));
            std::env::set_var("OSTRA_DATA_DIR", root.join("data"));
            std::env::set_var("OSTRA_MASTER_KEY_FILE", root.join("master.key"));
            std::env::set_var("OSTRA_SANDBOX_CACHE", root.join("sandbox-cache"));
            std::env::set_var("OSTRA_MODELS_DEV_URL", "");
        }
        save_toml(&root.join("config.toml"), &GlobalConfig::default()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let opts = ostra_server::app::ServeOptions {
            port: Some(port),
            open_browser: false,
            dev: false,
            exe: PathBuf::from("/nonexistent/ostra"),
            bind: None,
            allow_hosts: vec![],
        };
        let app = ostra_server::app::build(&opts, port).await.unwrap();
        let router = ostra_server::api::router(app.clone())
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        app.auth.sign_in_url().unwrap()
    };
    let (origin, token) = url.split_once("/#token=").unwrap();
    let host = origin.trim_start_matches("http://");
    let (name, port) = host.split_once(':').unwrap();
    assert!(name.starts_with("ostra-") && name.ends_with(".localhost"), "{host}");
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let plain = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let page = plain
        .get(format!("http://127.0.0.1:{port}/w/x?y=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 307);
    assert_eq!(page.headers()["location"], format!("{origin}/w/x?y=1"));
    let exchange = plain
        .post(format!("http://127.0.0.1:{port}/api/auth/exchange"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(exchange.status(), 421, "no cookie is ever set on 127.0.0.1");

    let private = reqwest::Client::builder()
        .cookie_store(true)
        .resolve(name, addr)
        .build()
        .unwrap();
    let r = private
        .post(format!("{origin}/api/auth/exchange"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let env = private.get(format!("{origin}/api/environment")).send().await.unwrap();
    assert_eq!(env.status(), 200);
}
