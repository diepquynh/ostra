/**
 * A local stand-in for the Anthropic Messages API (SSE streaming). It plays every agent of a YOLO
 * session the way `crates/ostra-server/tests/e2e.rs` does, with test strings in every field the UI
 * renders. It also records canary hits: any request to `/canary/<id>` means a test string loaded.
 */
import fs from "node:fs";
import http from "node:http";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { ROOT } from "./env";
import { inline, markdown } from "./payloads";

type Block =
  | { type: "text"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown };

interface Req {
  model: string;
  tools?: { name: string; input_schema: Record<string, any> }[];
  messages: { role: string; content: string | { type: string; text?: string }[] }[];
}

let ids = 1;
const id = () => `toolu_pw_${ids++}`;

function firstUserText(req: Req): string {
  const m = req.messages.find((x) => x.role === "user");
  if (!m) return "";
  if (typeof m.content === "string") return m.content;
  return m.content
    .filter((b) => b.type === "text")
    .map((b) => b.text ?? "")
    .join("\n");
}

const label = (text: string, name: string) =>
  text
    .split("\n")
    .find((l) => l.startsWith(`${name}: `))
    ?.slice(name.length + 2)
    .trim() ?? "";

function judge(req: Req, fake: string): Record<string, unknown> {
  const props = req.tools?.[0]?.input_schema?.properties ?? {};
  if (props.category)
    return {
      category: "IMPLEMENT",
      projects: ["app"],
      explore_tasks: [{ project: "app", task: `Research ${inline("explore-task")}` }],
      opts_in: { tests: false, docs: false },
      reason: `The request changes code. ${inline("classify-reason")}`,
      title: inline("title"),
    };
  if (props.items && !props.route) return { items: [], reason: `Research covers it. ${inline("sufficiency-reason")}` };
  // The full track, so the session writes the spec and plan the specs render.
  if (props.track) return { track: "full", reason: `Spec and plan it. ${inline("track-reason")}` };
  if (props.stakes) return { stakes: "high", reason: `Plan it. ${inline("stakes-reason")}` };
  if (props.report_markdown) return { report_markdown: markdown("completion", fake), reason: "done" };
  if (props.answer?.properties?.kind?.const === "approval")
    return { answer: { kind: "approval", approved: true }, reason: `The fact-check passed. ${inline("gate-reason")}` };
  return { answer: { kind: "choice", option: "block" }, reason: "r" };
}

/** One scripted turn: what the model "says" given the request. */
export function respond(req: Req, fake: string): Block[] {
  const tools = (req.tools ?? []).map((t) => t.name);
  const tool = (name: string, input: unknown): Block => ({ type: "tool_use", id: id(), name, input });
  if (tools.length === 1 && tools[0] === "decide") return [tool("decide", judge(req, fake))];
  const submit = tools.find((t) => t.startsWith("submit_")) ?? "";
  const text = firstUserText(req);
  const session = label(text, "Session dir");
  const repo = label(text, "Repo root");
  const report = label(text, "Report file");
  const turn = req.messages.filter((m) => m.role === "assistant").length;
  const say = (tag: string): Block => ({ type: "text", text: markdown(`${tag}-t${turn}`, fake) });
  const spec = path.join(session, "ostra-spec-1.md");
  const plan = path.join(session, "ostra-plan-1.md");
  const phase = path.join(session, "ostra-plan-1-phase-1.md");
  const research = path.join(session, "ostra-research-1.md");
  const md = (tag: string) => markdown(tag, fake);
  const one = (tag: string) => inline(tag);

  if (submit.startsWith("submit_quick")) {
    return [
      say("ask"),
      tool(submit, {
        answer: md("ask-answer"),
        sources: [`javascript:window.__pwned='ask-src'`, `${fake}/canary/ask-src`, one("ask-src")],
      }),
    ];
  }
  // Explore calls the workspace MCP tool first when it is offered, so its result reaches the UI.
  const mcp = tools.find((t) => /echo/.test(t));
  if (submit === "submit_explore" && mcp && turn === 0)
    return [say("explore-mcp"), tool(mcp, { text: one("mcp-arg") })];
  const t = submit === "submit_explore" && mcp ? turn - 1 : turn;
  switch (`${submit}:${t === 0 ? 0 : t === 1 ? 1 : "n"}`) {
    case "submit_explore:0":
      return [
        say("explore"),
        tool("Document", {
          path: research,
          document: {
            title: one("research-title"),
            date: "2026-07-28",
            repo: "app",
            scope: md("research-scope"),
            problem: md("research-problem"),
          },
        }),
      ];
    case "submit_explore:1":
    case "submit_explore:n":
      return [
        tool(submit, {
          research_path: research,
          scope_covered: one("scope"),
          findings_summary: md("findings"),
          sources_retrieved: 0,
          open_questions: 0,
          not_covered: [],
        }),
      ];
    case "submit_generate_spec:0":
      return [
        say("spec"),
        tool("Document", {
          path: spec,
          document: {
            title: one("spec-title"),
            date: "2026-07-28",
            objective: md("spec-objective"),
            current_behavior: md("spec-current"),
            criteria: [
              {
                id: "C1",
                statement: `A greeting file exists. ${one("criterion")}`,
                kind: "Functional",
                repo: "app",
                grounding: "new: no precedent found",
              },
            ],
            deliverables: [{ id: "D1", title: one("deliverable"), repo: "app", outcome: md("outcome") }],
            requirements: [
              {
                id: "R1",
                deliverable: "D1",
                title: one("requirement"),
                pattern: "ubiquitous",
                statement: "THE SYSTEM SHALL contain greeting.txt.",
                covers: ["C1"],
                acceptance: [{ id: "AC1.1", given: "the repo", when: "it is read", then: one("then") }],
              },
            ],
          },
        }),
      ];
    case "submit_generate_spec:1":
    case "submit_generate_spec:n":
      return [
        tool(submit, {
          spec_path: spec,
          open_questions: [],
          external_evidence_rows: 0,
          deliverables: 1,
          requirements: 1,
          summary: md("spec-summary"),
        }),
      ];
    case "submit_plan:0":
      return [
        say("plan"),
        tool("Document", {
          path: plan,
          document: {
            title: one("plan-title"),
            date: "2026-07-28",
            spec,
            stakes: "High",
            stakes_rationale: one("rationale"),
            summary: md("plan-summary"),
            phases: [
              {
                id: 1,
                name: "greeting",
                deliverable: "D1",
                repo: "app",
                repo_root: repo,
                complexity: "Low",
                test_policy: "Required",
                test_rationale: "Step 1.1 writes the file.",
                description: md("phase-description"),
                context: "This is the first phase. No prior phases.",
                skills: ["pw-skill"],
                requirements: [{ id: "R1", statement: "THE SYSTEM SHALL contain greeting.txt." }],
                steps: [
                  {
                    id: "1.1",
                    title: one("step"),
                    file: "greeting.txt",
                    change: "Create",
                    delivers: ["R1"],
                    skills: ["pw-skill"],
                    action: md("step-action"),
                    verify: "true",
                    size: "Small",
                  },
                ],
                verification: "true",
              },
            ],
          },
        }),
      ];
    case "submit_plan:1":
    case "submit_plan:n":
      return [
        tool(submit, {
          spec_path: spec,
          master_plan_path: plan,
          phases: [
            {
              id: 1,
              deliverable: "D1",
              project: "app",
              title: "greeting",
              complexity: "Low",
              test_policy: "Required",
              depends_on: [],
              file: phase,
            },
          ],
          stakes: "High",
          summary: md("plan-submit"),
          step_count: 1,
          requirement_coverage: "1 of 1",
        }),
      ];
    case "submit_fact_check:0":
    case "submit_fact_check:1":
    case "submit_fact_check:n":
      return [tool(submit, { verdict: "PASS", target: label(text, "Target type"), findings: [] })];
    case "submit_implementer:0":
      return [
        say("implementer"),
        tool("Write", { file_path: path.join(repo, "greeting.txt"), content: `hello ${one("greeting")}\n` }),
      ];
    case "submit_implementer:1":
      return [tool("Write", { file_path: report, content: md("impl-report") })];
    case "submit_implementer:n":
      if (turn === 2) return [tool("Bash", { command: `printf '%s\\n' ${shq(md("bash-output"))}` })];
      return [
        tool(submit, {
          status: "ok",
          report_path: report,
          changed_files: ["greeting.txt"],
          summary: md("impl-summary"),
        }),
      ];
    case "submit_code_reviewer:0":
    case "submit_code_reviewer:1":
    case "submit_code_reviewer:n":
      return [
        tool(submit, {
          findings: [],
          security_block: false,
          ledger_path: path.join(session, "ostra-review-ledger-phase-1.md"),
          summary: md("review-summary"),
        }),
      ];
    default:
      return [{ type: "text", text: `unexpected agent ${submit}\n\n${md("unexpected")}` }];
  }
}

const shq = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

function sse(res: http.ServerResponse, model: string, blocks: Block[]) {
  const send = (event: string, data: unknown) => res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  send("message_start", {
    type: "message_start",
    message: {
      id: `msg_pw_${ids++}`,
      type: "message",
      role: "assistant",
      model,
      content: [],
      stop_reason: null,
      usage: { input_tokens: 10, cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: 1 },
    },
  });
  blocks.forEach((b, index) => {
    if (b.type === "text") {
      send("content_block_start", { type: "content_block_start", index, content_block: { type: "text", text: "" } });
      send("content_block_delta", { type: "content_block_delta", index, delta: { type: "text_delta", text: b.text } });
    } else {
      send("content_block_start", {
        type: "content_block_start",
        index,
        content_block: { type: "tool_use", id: b.id, name: b.name, input: {} },
      });
      send("content_block_delta", {
        type: "content_block_delta",
        index,
        delta: { type: "input_json_delta", partial_json: JSON.stringify(b.input) },
      });
    }
    send("content_block_stop", { type: "content_block_stop", index });
  });
  const stop = blocks.some((b) => b.type === "tool_use") ? "tool_use" : "end_turn";
  send("message_delta", { type: "message_delta", delta: { stop_reason: stop, stop_sequence: null }, usage: { output_tokens: 20 } });
  send("message_stop", { type: "message_stop" });
  res.end();
}

export interface FakeModel {
  port: number;
  origin: string;
  close: () => Promise<void>;
}

const HITS = path.join(ROOT, "canary-hits.jsonl");

/** Canary hits recorded so far, across every process of the run. */
export function canaryHits(): { id: string; at: number; referer?: string }[] {
  try {
    return fs
      .readFileSync(HITS, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((l) => JSON.parse(l));
  } catch {
    return [];
  }
}

export async function startFakeModel(): Promise<FakeModel> {
  let origin = "";
  const log = fs.createWriteStream(path.join(ROOT, "fake-model.log"), { flags: "a" });
  const server = http.createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://x");
    if (url.pathname.startsWith("/canary/")) {
      fs.appendFileSync(
        HITS,
        `${JSON.stringify({ id: url.pathname.slice("/canary/".length), at: Date.now(), referer: req.headers.referer })}\n`,
      );
      res.writeHead(200, { "content-type": "image/gif", "access-control-allow-origin": "*" });
      res.end();
      return;
    }
    if (req.method === "POST" && url.pathname === "/v1/messages") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        try {
          const parsed = JSON.parse(body) as Req;
          const blocks = respond(parsed, origin);
          const last = parsed.messages.at(-1)?.content;
          const result = Array.isArray(last)
            ? JSON.stringify(last.filter((b: any) => b.type === "tool_result").map((b: any) => b.content)).slice(0, 400)
            : "";
          const submit = parsed.tools?.find((t) => t.name.startsWith("submit_") || t.name === "decide")?.name;
          log.write(`${JSON.stringify({ submit, out: blocks.map((b) => (b.type === "text" ? "text" : b.name)), result })}\n`);
          sse(res, parsed.model, blocks);
        } catch (e) {
          log.write(`error ${String(e)}\n`);
          res.writeHead(400, { "content-type": "application/json" });
          res.end(JSON.stringify({ type: "error", error: { type: "invalid_request_error", message: String(e) } }));
        }
      });
      return;
    }
    res.writeHead(404);
    res.end();
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const port = (server.address() as AddressInfo).port;
  origin = `http://127.0.0.1:${port}`;
  return { port, origin, close: () => new Promise((r) => server.close(() => r())) };
}
