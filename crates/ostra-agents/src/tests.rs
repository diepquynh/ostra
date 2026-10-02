use super::*;
use crate::brief::{
    ArtifactsBrief, BooksBrief, BriefInput, ProjectDoc, augment, build_brief, project_docs, stated_in,
};
use crate::spawn::*;
use ostra_core::HarnessKind;
use ostra_core::config::{Commands, ModuleRow, ProjectProfile, ReviewRule, SkillEntry, TestType};
use ostra_core::pipeline::QuestionAnswer;

fn all_executors() -> Vec<ExecutorKind> {
    ExecutorKind::all()
}

fn text_assets() -> Vec<(String, String)> {
    Assets::iter()
        .filter(|p| p.ends_with(".md") || p.ends_with(".toml"))
        .map(|p| (p.to_string(), asset_text(&p).unwrap()))
        .collect()
}

#[test]
fn every_agent_definition_loads_with_the_handover_tiers() {
    let expect = [
        (AgentName::Explore, Tier::Advanced),
        (AgentName::GenerateSpec, Tier::Advanced),
        (AgentName::FactCheck, Tier::Advanced),
        (AgentName::Plan, Tier::Advanced),
        (AgentName::Implementer, Tier::Balanced),
        (AgentName::CodeReviewer, Tier::Balanced),
        (AgentName::ExecutionPathAnalyzer, Tier::Balanced),
        (AgentName::WriteTest, Tier::Balanced),
        (AgentName::Documentation, Tier::Advanced),
        (AgentName::SystemArchitecture, Tier::Advanced),
        (AgentName::PromptGeneration, Tier::Advanced),
        (AgentName::Initializer, Tier::Balanced),
        (AgentName::QuickAnswer, Tier::Balanced),
    ];
    for (agent, tier) in expect {
        let def = agent_def(agent);
        assert_eq!(def.default_tier, tier, "{agent}");
        assert!(!def.description.is_empty());
        assert!(def.timeout_secs > 0);
        for e in all_executors() {
            assert!(
                def.effort.contains_key(e.tier_table()),
                "{agent} lacks effort for {e}"
            );
        }
    }
    assert_eq!(
        effort_for(
            AgentName::Initializer,
            ExecutorKind::Harness(HarnessKind::Codex)
        ),
        Effort::Xhigh
    );
    // Lowered from Ultracode's max after a self-init spent about $240 on Opus at max effort.
    assert_eq!(
        effort_for(AgentName::Initializer, ExecutorKind::Native),
        Effort::High
    );
}

#[test]
fn read_only_agents_have_no_edit_and_quick_answer_cannot_write() {
    let qa = agent_def(AgentName::QuickAnswer);
    assert!(
        !qa.capabilities
            .iter()
            .any(|c| c.writes() || *c == Capability::Shell)
    );
    for a in [
        AgentName::CodeReviewer,
        AgentName::ExecutionPathAnalyzer,
        AgentName::FactCheck,
    ] {
        assert!(
            !agent_def(a).capabilities.contains(&Capability::Edit),
            "{a}"
        );
    }
    // Explore, spec, and plan write their documents through the Document tool, never a file write.
    for a in [AgentName::Explore, AgentName::GenerateSpec, AgentName::Plan] {
        let caps = &agent_def(a).capabilities;
        assert!(
            caps.contains(&Capability::Document) && !caps.iter().any(|c| c.writes()),
            "{a}"
        );
    }
}

#[test]
fn every_prompt_renders_for_every_executor_with_no_unresolved_token() {
    for agent in AgentName::ALL {
        for e in all_executors() {
            let text =
                render_prompt(agent, e).unwrap_or_else(|err| panic!("{agent} on {e}: {err}"));
            assert!(
                !text.contains("{{") && !text.contains("{%"),
                "{agent} on {e} has an unresolved token"
            );
            let submit = tool_name("submit", agent, e).unwrap();
            assert!(
                text.contains(&submit),
                "{agent} on {e} never names its submit tool `{submit}`"
            );
        }
    }
}

#[test]
fn every_prompt_forbids_text_outside_ostra_tool_calls() {
    for agent in AgentName::ALL {
        for e in all_executors() {
            let text = render_prompt(agent, e).unwrap();
            assert!(
                text.contains("Write no text outside tool calls"),
                "{agent} on {e} lacks the output rule"
            );
        }
    }
}

#[test]
fn native_prompts_use_claude_tool_names_and_harness_prompts_open_with_a_vocabulary() {
    let native = render_prompt(AgentName::Implementer, ExecutorKind::Native).unwrap();
    assert!(native.contains("submit_implementer"));
    assert!(native.contains("Report"));
    assert!(!native.starts_with("## Tool vocabulary"));
    let codex = render_prompt(
        AgentName::Implementer,
        ExecutorKind::Harness(HarnessKind::Codex),
    )
    .unwrap();
    assert!(codex.starts_with("## Tool vocabulary"));
    assert!(codex.contains("apply_patch"));
    assert!(codex.contains(".agents/skills/{name}/SKILL.md"));
    let claude = render_prompt(
        AgentName::CodeReviewer,
        ExecutorKind::Harness(HarnessKind::Claude),
    )
    .unwrap();
    assert!(claude.contains("mcp__ostra__submit_code_reviewer"));
    assert!(claude.contains("Do not start this harness's own subagents"));
}

#[test]
fn coordination_guide_names_the_tools_per_executor() {
    let native = render_prompt(AgentName::GenerateSpec, ExecutorKind::Native).unwrap();
    assert!(native.contains("## Subagent coordination"));
    assert!(native.contains("`SubagentAsk`") && native.contains("`SubagentReply`"));
    let claude = render_prompt(AgentName::FactCheck, ExecutorKind::Harness(HarnessKind::Claude)).unwrap();
    assert!(claude.contains("`mcp__ostra__subagent_ask`"));
    assert!(claude.contains("| coordinate | mcp__ostra__subagent_list, mcp__ostra__subagent_ask, mcp__ostra__subagent_reply |"));
    let codex = render_prompt(AgentName::Implementer, ExecutorKind::Harness(HarnessKind::Codex)).unwrap();
    assert!(codex.contains("`subagent_reply`"));
    // Agents outside the pipeline's pairs get no coordination tools.
    let advisor = render_prompt(AgentName::Advisor, ExecutorKind::Native).unwrap();
    assert!(!advisor.contains("Subagent coordination"));
}

#[test]
fn code_tools_are_named_per_executor() {
    let native = render_prompt(AgentName::Explore, ExecutorKind::Native).unwrap();
    assert!(native.contains("## Code navigation"));
    for t in [
        "CodeMap",
        "CodeFind",
        "CodeOutline",
        "CodeCallers",
        "CodeCallees",
        "CodeImplementations",
        "CodeNeighbors",
        "CodeImpact",
    ] {
        assert!(native.contains(&format!("`{t}`")), "native lacks {t}");
    }
    let claude = render_prompt(
        AgentName::Explore,
        ExecutorKind::Harness(HarnessKind::Claude),
    )
    .unwrap();
    assert!(claude.contains("`mcp__ostra__code_callers`"));
    assert!(claude.contains("| code | mcp__ostra__code_outline, mcp__ostra__code_find,"));
    let codex = render_prompt(
        AgentName::Explore,
        ExecutorKind::Harness(HarnessKind::Codex),
    )
    .unwrap();
    assert!(codex.contains("`code_impact`"));
    assert!(claude.contains("`mcp__ostra__code_implementations`"));
    assert!(codex.contains("`code_implementations`"));
    for h in [HarnessKind::Grok, HarnessKind::Agy] {
        let p = render_prompt(AgentName::Explore, ExecutorKind::Harness(h)).unwrap();
        assert!(
            p.contains("`code_implementations`"),
            "{h:?} lacks code_implementations"
        );
    }
}

#[test]
fn no_ultracode_leftovers_in_any_asset() {
    // The initializer detects and migrates `.ultracode/` bootstraps, so it names them on purpose.
    let allowed = [
        ".ultracode",
        "ultracode_bootstrap",
        "ULTRACODE-BOOTSTRAP",
        "Ultracode bootstrap",
        "`ultracode-` or `ultracode:`",
        "Ultracode, the plugin Ostra replaces",
        "bootstrapped by Ultracode",
    ];
    for (path, text) in text_assets() {
        let mut t = text.clone();
        if path == "agents/initializer/prompt.md" || path == "agents/initializer/agent.toml" {
            for a in allowed {
                t = t.replace(a, "");
            }
        }
        assert!(
            !t.to_lowercase().contains("ultracode"),
            "{path} still names ultracode"
        );
        assert!(
            !text.contains("{{#") && !text.contains("{{/"),
            "{path} still has a harness block"
        );
        assert!(
            !text.contains("Primary repo root"),
            "{path} still says Primary repo root"
        );
        assert!(
            !text.contains("repo-profile.json") || path.starts_with("agents/initializer/"),
            "{path}"
        );
        assert!(
            !text.contains("code-graph MCP"),
            "{path} still mentions a code-graph MCP"
        );
    }
}

#[test]
fn no_em_dashes_in_any_asset() {
    for (path, text) in text_assets() {
        assert!(!text.contains('\u{2014}'), "{path} contains an em dash");
    }
}

#[test]
fn rule_id_sets_survive_the_port() {
    let spec = asset_text("agents/generate-spec/prompt.md").unwrap();
    for id in (1..=8)
        .map(|n| format!("K{n}"))
        .chain((1..=8).map(|n| format!("S{n}")))
    {
        assert!(
            spec.contains(&format!("**{id}")) || spec.contains(&format!("({id})")),
            "{id} missing"
        );
    }
    for id in [
        "R-a", "R-b", "R-c", "R-d", "R-e", "AC-a", "AC-b", "AC-c", "AC-d",
    ] {
        assert!(spec.contains(id), "{id} missing from generate-spec");
    }
    let plan = asset_text("agents/plan/prompt.md").unwrap();
    for n in 0..=13 {
        assert!(plan.contains(&format!("**P{n}")), "P{n} missing from plan");
    }
    let judges = ["classify", "sufficiency", "route-answer", "rescue"]
        .iter()
        .map(|j| judge_prompt(j).unwrap())
        .collect::<String>();
    for id in ["D1", "D2", "M1", "T3", "D3", "D10", "T5", "D9"] {
        assert!(judges.contains(id), "judges never cite {id}");
    }
}

#[test]
fn judges_and_references_resolve() {
    for j in [
        "classify",
        "sufficiency",
        "stakes",
        "track",
        "feedback",
        "route-answer",
        "rescue",
        "resolve-review",
        "yolo-answer",
        "completion",
    ] {
        let p = judge_prompt(j).unwrap_or_else(|| panic!("judge {j} missing"));
        assert!(p.contains("`decide`"), "{j} must name the decide tool");
    }
    assert!(judge_prompt("nope").is_none());
    for r in [
        "_generic",
        "go",
        "java-spring",
        "python",
        "typescript-node",
        "inventory-and-profile",
        "skill-archetypes",
    ] {
        let text = reference(r).unwrap_or_else(|| panic!("ref {r} missing"));
        assert!(!text.contains("{{"), "{r} has an unresolved token");
    }
    assert!(reference_names().contains(&"java-spring".to_string()));
    assert_eq!(
        stack_names(),
        ["go", "java-spring", "python", "typescript-node"]
    );
    let meta = embedded_skill("meta-author").unwrap();
    assert!(!meta.contains("{{"));
}

#[test]
fn materialize_writes_refs_and_skills_with_the_assets_dir_filled() {
    let dir = tempfile::tempdir().unwrap();
    let written = materialize_assets(dir.path()).unwrap();
    assert!(written.iter().any(|p| p.ends_with("refs/java-spring.md")));
    assert!(
        written
            .iter()
            .any(|p| p.ends_with("skills/meta-author/SKILL.md"))
    );
    let archetypes = std::fs::read_to_string(dir.path().join("refs/skill-archetypes.md")).unwrap();
    assert!(archetypes.contains(&dir.path().display().to_string()));
    // Rule O5: every agent's instructions, rendered with native tool names and this assets dir.
    for agent in AgentName::ALL {
        let text =
            std::fs::read_to_string(dir.path().join(format!("agents/{}.md", agent.as_str())))
                .unwrap();
        assert!(!text.contains("{{"), "{agent}: an unrendered token");
    }
    let initializer = std::fs::read_to_string(dir.path().join("agents/initializer.md")).unwrap();
    assert!(initializer.contains("## Mode: DETECT"));
    assert!(initializer.contains(&format!("{}/refs/", dir.path().display())));
    assert!(
        materialize_assets(dir.path()).unwrap().is_empty(),
        "second run rewrites nothing"
    );
}

// ---------------------------------------------------------------------------------------------
// Spawn contract
// ---------------------------------------------------------------------------------------------

fn common() -> Common {
    Common {
        workspace_root: "/ws".into(),
        repo_root: "/ws/backend".into(),
        session_dir: "/ws/.ostra/sessions/s1/backend".into(),
        repo_key: "backend".into(),
    }
}

fn roundtrip(p: &dyn SpawnParams) -> std::collections::BTreeMap<String, String> {
    let block = p.render();
    parse_block(p.agent(), &block).unwrap_or_else(|e| panic!("{}: {e}\n{block}", p.agent()))
}

#[test]
fn every_struct_renders_a_block_its_own_contract_accepts() {
    let extras = Extras {
        research_docs: vec!["/ws/.ostra/sessions/s1/backend/ostra-research-1.md".into()],
        user_answers: vec![QuestionAnswer {
            id: "Q1".into(),
            question: "Scope?".into(),
            answer: "Only backend".into(),
        }],
        required_skills: vec!["entity".into(), "service".into()],
        ..Default::default()
    };
    let spec = GenerateSpecParams {
        common: common(),
        task: "Add cancel\nwith refunds".into(),
        spec_file: None,
        new_research_docs: vec![],
        requirement_changes: vec![],
        extra: extras,
    };
    let v = roundtrip(&spec);
    assert_eq!(v["task"], "Add cancel\nwith refunds");
    assert!(v["research_docs"].contains("ostra-research-1.md"));

    let fc = FactCheckParams {
        common: common(),
        target: "/ws/s/ostra-spec-1.md".into(),
        target_type: TargetType::Spec,
        prior_findings: "none".into(),
        spec_file: "/ws/s/ostra-spec-1.md".into(),
        source_check: SourceCheck::Refetch,
        research_docs: vec![],
    };
    assert_eq!(roundtrip(&fc)["source_check"], "refetch");

    let plan = PlanParams {
        common: common(),
        spec_file: "/ws/s/ostra-spec-1.md".into(),
        projects_in_scope: vec![("backend".into(), "/ws/backend".into())],
        findings: None,
        master_plan: None,
    };
    assert_eq!(
        roundtrip(&plan)["projects_in_scope"],
        "backend -> /ws/backend"
    );

    let imp = ImplementerParams {
        common: common(),
        report_file: "/ws/s/backend/ostra-implementer-phase-1.md".into(),
        work: WorkSource::PhaseFile("/ws/s/ostra-plan-1-phase-1-data.md".into()),
        unblock: None,
        extra: Extras::default(),
    };
    assert_eq!(
        imp.report_file(),
        Some(Path::new("/ws/s/backend/ostra-implementer-phase-1.md"))
    );
    assert!(roundtrip(&imp).contains_key("phase_file"));

    let cr = CodeReviewerParams {
        common: common(),
        phase: "1".into(),
        changed_files: vec!["src/a.rs".into(), "src/b.rs".into()],
        change_rationale: "Phase 1 intent".into(),
        work: WorkSource::NoPlan("inline fix".into()),
        review_scope: Some("unstaged".into()),
        context: Some(ReviewContext::Implementation),
        epa_report: None,
    };
    let v = roundtrip(&cr);
    assert_eq!(v["changed_files"], "src/a.rs\nsrc/b.rs");
    assert!(cr.render().contains("Review scope: unstaged"));

    let epa = EpaParams {
        common: common(),
        implementer_report: "/r/i.md".into(),
        report_file: "/r/e.md".into(),
        phase_file: None,
        extra: Extras::default(),
    };
    roundtrip(&epa);
    let wt = WriteTestParams {
        common: common(),
        implementer_report: "/r/i.md".into(),
        epa_report: "/r/e.md".into(),
        report_file: "/r/w.md".into(),
        work: WorkSource::PhaseFile("/r/p.md".into()),
        extra: Extras::default(),
    };
    roundtrip(&wt);
    let md = DocumentationParams {
        common: common(),
        implementer_reports: vec!["/r/i1.md".into(), "/r/i2.md".into()],
        existing_book: Some("/ws/.ostra/docs/api_web/book.json".into()),
        area: None,
        extra: Extras::default(),
    };
    roundtrip(&md);
    let area = DocumentationParams {
        area: Some(DocsAreaScope {
            id: "server-and-1-more".into(),
            title: "server, other files".into(),
            paths: vec!["server/**".into()],
            rest: true,
            others: vec![("engine".into(), vec!["engine/**".into()])],
        }),
        ..md.clone()
    };
    roundtrip(&area);
    let text = area.render();
    assert!(text.contains("Area: server, other files (server-and-1-more)"), "{text}");
    assert!(text.contains("Area paths: server/**, and every file no other area covers"), "{text}");
    assert!(text.contains("Other areas: engine (engine/**)"), "{text}");
    let arch = ArchitectureParams {
        common: common(),
        book_parts: "/r/ostra-docs-parts.json".into(),
        projects: vec![("api".into(), "/ws/api".into()), ("web".into(), "/ws/web".into())],
        existing_book: None,
        extra: Extras::default(),
    };
    roundtrip(&arch);
    let pg = PromptGenParams {
        common: common(),
        task: "Write a skill".into(),
        target_files: vec![".agents/skills/x/SKILL.md".into()],
        report_file: "/r/pg.md".into(),
        extra: Extras::default(),
    };
    roundtrip(&pg);
    let qa = QuickAnswerParams {
        common: common(),
        question: "Where is auth?".into(),
        projects_in_scope: vec![],
        session_artifacts: vec![],
    };
    roundtrip(&qa);
    let ex = ExploreParams {
        common: common(),
        task: "Research orders".into(),
        extra: Extras::default(),
    };
    roundtrip(&ex);
}

#[test]
fn initializer_modes_render_their_mode_and_required_lines() {
    let detect = InitDetectParams {
        common: common(),
        advisor_guidance: None,
        user_focus: Some("orders".into()),
    };
    let v = roundtrip(&detect);
    assert_eq!(v["mode"], "detect");
    assert_eq!(detect.to_json()["mode"], "detect");
    let scout = InitScoutParams {
        common: common(),
        advisor_guidance: None,
        slice: "orders".into(),
        slice_paths: vec!["src/orders".into()],
        stack_reference: "/data/assets/refs/java-spring.md".into(),
        scout_plan: "/s/ostra-scout-plan.md".into(),
    };
    assert_eq!(roundtrip(&scout)["slice_paths"], "src/orders");
    let propose = InitProposeParams {
        common: common(),
        advisor_guidance: None,
        scout_findings: vec![
            "/s/ostra-findings-a.md".into(),
            "/s/ostra-findings-b.md".into(),
        ],
        scout_plan: "/s/ostra-scout-plan.md".into(),
    };
    roundtrip(&propose);
    let gs = InitGenerateSkillParams {
        common: common(),
        advisor_guidance: None,
        skill_name: "entity".into(),
        skill_kind: "creation".into(),
        disposition: "generate".into(),
        proposal: "/s/ostra-proposal.json".into(),
        scout_findings: vec!["/s/ostra-findings-a.md".into()],
    };
    roundtrip(&gs);
    let gi = InitGenerateInventoryParams {
        common: common(),
        advisor_guidance: None,
        generated_skills: r#"[{"name":"entity","kind":"creation","component_type":"entity","path":".agents/skills/entity/SKILL.md"}]"#.into(),
        reused_skills: "[]".into(),
        proposal: "/s/ostra-proposal.json".into(),
        scout_findings: vec!["/s/ostra-findings-a.md".into()],
    };
    assert!(roundtrip(&gi)["generated_skills"].starts_with('['));
    let adopt = InitAdoptParams {
        common: common(),
        advisor_guidance: None,
        source_harness: "claude".into(),
        source_runtime_dir: ".ultracode".into(),
        source_skills_dir: ".claude/skills".into(),
    };
    roundtrip(&adopt);
}

#[test]
fn parse_block_refuses_bad_blocks() {
    let ok = "Task: x\nWorkspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\nRepo key: a\n";
    assert!(parse_block(AgentName::Explore, ok).is_ok());
    let missing = "Task: x\nWorkspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\n";
    assert!(
        parse_block(AgentName::Explore, missing)
            .unwrap_err()
            .contains("Repo key")
    );
    let bad_key = ok.replace("Repo key: a", "Repo key: Backend");
    assert!(
        parse_block(AgentName::Explore, &bad_key)
            .unwrap_err()
            .contains("slug")
    );
    let relative = ok.replace("Repo root: /ws/a", "Repo root: ws/a");
    assert!(
        parse_block(AgentName::Explore, &relative)
            .unwrap_err()
            .contains("absolute")
    );

    let base = "Workspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\nRepo key: a\nReport file: /ws/s/r.md\n";
    assert!(
        parse_block(AgentName::Implementer, base)
            .unwrap_err()
            .contains("Phase file")
    );
    let both = format!("{base}Phase file: /p.md\nNo plan: small\n");
    assert!(
        parse_block(AgentName::Implementer, &both)
            .unwrap_err()
            .contains("exactly one")
    );
    assert!(parse_block(AgentName::Implementer, &format!("{base}No plan: small\n")).is_ok());

    let review = "Phase: 2-tests\nChanged files: a\nChange rationale: r\nNo plan: x\nWorkspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\nRepo key: a\n";
    assert!(parse_block(AgentName::CodeReviewer, review).is_ok());
    assert!(
        parse_block(
            AgentName::CodeReviewer,
            &review.replace("2-tests", "iteration 2")
        )
        .is_err()
    );

    let fc = "Target: /t.md\nTarget type: code\nPrior findings: none\nSpec file: /t.md\nSource check: citations\nWorkspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\nRepo key: a\n";
    assert!(
        parse_block(AgentName::FactCheck, fc)
            .unwrap_err()
            .contains("spec, plan")
    );

    assert!(
        parse_block(AgentName::Initializer, ok)
            .unwrap_err()
            .contains("Mode")
    );
    let bad_mode = format!("Mode: build\n{ok}");
    assert!(parse_block(AgentName::Initializer, &bad_mode).is_err());
}

#[test]
fn parse_block_stops_at_the_brief() {
    let text = "Task: x\nWorkspace root: /ws\nRepo root: /ws/a\nSession dir: /ws/s\nRepo key: a\n\n---\n\n## Repo brief for explore\nTask: injected\n";
    assert_eq!(parse_block(AgentName::Explore, text).unwrap()["task"], "x");
}

// ---------------------------------------------------------------------------------------------
// Brief
// ---------------------------------------------------------------------------------------------

fn profile() -> ProjectProfile {
    ProjectProfile {
        commands: Commands {
            build: Some("./mvnw -q compile".into()),
            test: Some("./mvnw test".into()),
            ..Default::default()
        },
        module_map: vec![
            ModuleRow {
                glob: "src/orders/**".into(),
                area: "orders".into(),
            },
            ModuleRow {
                glob: "src/billing/**".into(),
                area: "billing".into(),
            },
        ],
        skills: vec![
            SkillEntry {
                name: "convention".into(),
                kind: "convention".into(),
                path: ".agents/skills/convention/SKILL.md".into(),
                ..Default::default()
            },
            SkillEntry {
                name: "entity".into(),
                kind: "creation".into(),
                path: ".agents/skills/entity/SKILL.md".into(),
                component_type: Some("JPA entity".into()),
                ..Default::default()
            },
            SkillEntry {
                name: "service-test".into(),
                kind: "test".into(),
                path: ".agents/skills/service-test/SKILL.md".into(),
                ..Default::default()
            },
        ],
        review_rules: vec![ReviewRule {
            id: "C1".into(),
            rule: "Parameters are final".into(),
            severity: "M".into(),
            auto_fixable: true,
        }],
        conventions: ostra_core::config::Conventions {
            notes: vec![
                "single timestamp per method".into(),
                "already in the table".into(),
            ],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn brief_selects_sections_per_agent_and_skips_what_the_inventory_states() {
    let p = profile();
    let inv = "| notes | already in the table |";
    let instructions = vec!["Write British English.".to_string()];
    let input = BriefInput {
        agent: AgentName::Implementer,
        prompt: "Phase file: /s/p.md\nTouch src/orders/OrderService.java",
        repo_root: Path::new("/ws/backend"),
        profile: Some(&p),
        inventory: Some(inv),
        instructions: &instructions,
        project_docs: &[],
        artifacts: None,
        books: None,
        new_projects: &[],
    };
    let brief = build_brief(&input).unwrap();
    assert!(brief.starts_with("## Repo brief for implementer"));
    assert!(brief.contains("`./mvnw -q compile`"));
    // The brief renders paths with the OS separator, so compare with slashes normalized.
    assert!(
        brief.replace('\\', "/").contains("/ws/backend/.agents/skills/entity/SKILL.md"),
        "{brief}"
    );
    assert!(brief.contains("use for JPA entity"));
    assert!(brief.contains("single timestamp per method"));
    assert!(!brief.contains("already in the table"));
    assert!(brief.contains("src/orders/**"));
    assert!(!brief.contains("src/billing/**"));
    assert!(!brief.contains("Review Rule Set"));
    assert!(brief.contains("## Workspace instructions"));
    assert!(brief.contains("Write British English."));
    assert!(!brief.contains('\u{2014}'));

    let reviewer = BriefInput {
        agent: AgentName::CodeReviewer,
        ..input
    };
    let rb = build_brief(&reviewer).unwrap();
    assert!(rb.contains("**C1** (M, auto-fixable)"));
    assert!(!rb.contains("entity/SKILL.md"));

    let wt = BriefInput {
        agent: AgentName::WriteTest,
        ..reviewer
    };
    let wb = build_brief(&wt).unwrap();
    assert!(wb.contains("service-test"));
    assert!(!wb.contains("`entity`"));
}

#[test]
fn brief_gives_each_test_type_its_level_command_and_its_one_test_command() {
    let mut p = profile();
    p.test_types.insert(
        "unit".into(),
        TestType {
            command: Some("pytest tests/unit".into()),
            command_one: Some("pytest {PATH}".into()),
            matches: vec!["tests/unit/**".into()],
            note: Some("Needs nothing.".into()),
            reports: None,
        },
    );
    p.test_types.insert(
        "e2e".into(),
        TestType {
            command: Some("npx playwright test".into()),
            ..Default::default()
        },
    );
    for agent in [AgentName::WriteTest, AgentName::ExecutionPathAnalyzer] {
        let input = BriefInput {
            agent,
            prompt: "",
            repo_root: Path::new("/ws/backend"),
            profile: Some(&p),
            inventory: None,
            instructions: &[],
            project_docs: &[],
            artifacts: None,
        books: None,
            new_projects: &[],
        };
        let brief = build_brief(&input).unwrap();
        assert!(
            brief.contains("- **unit**: `pytest tests/unit`; one test: `pytest {PATH}`"),
            "{brief}"
        );
        assert!(brief.contains("- **e2e**: `npx playwright test`\n"), "{brief}");
        assert!(brief.contains("### Commands"), "{agent}: regression suites need exact commands");
    }
}

#[test]
fn brief_is_idempotent_and_handles_a_missing_profile() {
    let p = profile();
    let input = BriefInput {
        agent: AgentName::Explore,
        prompt: "Task: x",
        repo_root: Path::new("/r"),
        profile: Some(&p),
        inventory: None,
        instructions: &[],
        project_docs: &[],
        artifacts: None,
        books: None,
        new_projects: &[],
    };
    let once = augment("Task: x", &input);
    assert_eq!(augment(&once, &input), once);
    let none = BriefInput {
        profile: None,
        ..input
    };
    assert_eq!(augment("Task: x", &none), "Task: x");
    let init = BriefInput {
        agent: AgentName::Initializer,
        profile: Some(&p),
        ..none
    };
    assert!(build_brief(&init).is_none());
    let docs = vec![ProjectDoc {
        path: "/r/AGENTS.md".into(),
        content: "Run make check before you finish.\n".into(),
    }];
    let init_docs = BriefInput {
        project_docs: &docs,
        ..init
    };
    let brief = build_brief(&init_docs).unwrap();
    assert!(brief.starts_with("## Project instructions"));
    assert!(brief.contains("### `/r/AGENTS.md`\n\nRun make check before you finish."));
    let once = augment("Task: x", &init_docs);
    assert_eq!(augment(&once, &init_docs), once);
    let artifacts = ArtifactsBrief {
        dir: "/ws/.ostra/artifacts".into(),
        entries: vec![("guides/style.md".into(), 12)],
        total: 3,
    };
    let with_artifacts = BriefInput {
        artifacts: Some(&artifacts),
        ..init_docs
    };
    let created = [ostra_core::manage::CreatedProject {
        key: "mcp".into(),
        path: "/ws/mcp".into(),
        stack: "rust".into(),
        purpose: "An MCP server.".into(),
        requirements: vec!["Rust 2024".into()],
        execution: "x_1".into(),
        agent: AgentName::GenerateSpec,
    }];
    let with_new = BriefInput {
        new_projects: &created,
        ..init_docs
    };
    let brief = build_brief(&with_new).unwrap();
    assert!(brief.contains("## Projects created in this session"));
    assert!(brief.contains(
        "- `mcp` at `/ws/mcp`, stack rust: An MCP server.\n  Base requirements:\n  - Rust 2024"
    ));
    let brief = build_brief(&with_artifacts).unwrap();
    assert!(brief.contains("## Workspace artifacts"));
    assert!(brief.contains("- `/ws/.ostra/artifacts/guides/style.md` (12 bytes)"));
    assert!(brief.contains("- and 2 more; find them with Glob in `/ws/.ostra/artifacts`"));
    let once = augment("Task: x", &with_artifacts);
    assert_eq!(augment(&once, &with_artifacts), once);
    let books = BooksBrief {
        dir: "/ws/.ostra/docs".into(),
        books: vec![ostra_core::book::BookSummary {
            id: "api_web".into(),
            title: "Documentation for api, web".into(),
            projects: vec!["api".into(), "web".into()],
            updated_at: chrono::DateTime::UNIX_EPOCH,
            sections: 7,
            has_architecture: true,
        }],
        search_tool: None,
    };
    let with_books = BriefInput {
        books: Some(&books),
        ..init_docs
    };
    let brief = build_brief(&with_books).unwrap();
    assert!(brief.contains("## Workspace documentation"));
    assert!(brief.contains(
        "- `/ws/.ostra/docs/api_web/index.md`: api, web (7 sections, with the system architecture)"
    ));
    assert!(brief.contains("Read the sections that bear on your task."));
    let searchable = BooksBrief {
        search_tool: Some("mcp__ostra__docs_search".into()),
        ..books.clone()
    };
    let brief = build_brief(&BriefInput {
        books: Some(&searchable),
        ..with_books
    })
    .unwrap();
    assert!(brief.contains("Search them with mcp__ostra__docs_search before you read code"));
    assert!(stated_in("a  b   c", "a\nb c"));
    assert!(stated_in("", "ab"));
}

#[test]
fn project_docs_skip_links_and_copies() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("CLAUDE.md"), "# Rules\n").unwrap();
    std::fs::write(d.path().join("agent.md"), "# Rules\n").unwrap();
    std::fs::write(d.path().join("Agents.MD"), "# Other\n").unwrap();
    let docs = project_docs(d.path());
    let names: Vec<String> = docs
        .iter()
        .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["CLAUDE.md", "Agents.MD"]);
}

#[test]
fn harness_prompts_ask_for_only_done_after_submitting() {
    for h in [
        HarnessKind::Claude,
        HarnessKind::Codex,
        HarnessKind::Grok,
        HarnessKind::Agy,
    ] {
        let p = render_prompt(AgentName::Implementer, ExecutorKind::Harness(h)).unwrap();
        let line = p
            .lines()
            .find(|l| l.contains("reply with only `Done!`"))
            .unwrap_or_else(|| panic!("{h:?} lacks the Done! instruction"));
        assert!(line.contains("submit_implementer"), "{h:?}: {line}");
        println!("{h:?}: {line}");
    }
    let native = render_prompt(AgentName::Implementer, ExecutorKind::Native).unwrap();
    assert!(!native.contains("`Done!`"));
}

#[test]
fn classify_judge_names_attached_files_in_research_tasks() {
    let p = crate::judge_prompt("classify").unwrap();
    assert!(p.contains("every attached file, attached folder, and upload"));
    assert!(p.contains("Rules C1, C3"));
}
