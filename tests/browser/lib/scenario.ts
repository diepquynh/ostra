/**
 * The scratch world a run renders: a git repo whose names and contents carry test strings, a stub
 * harness CLI that prints hostile escape sequences, a stub MCP server, and the Ostra config.
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { ROOT } from "./env";
import { htmlFile, inline, js, markdown, svgFile } from "./payloads";

export const REPO = path.join(ROOT, "repo");
export const BRANCH = `pw/<img/src=x/onerror=${js("branch")}>`;
export const PTY_INPUT_LOG = path.join(ROOT, "pty-input.log");
export const PTY_GO = path.join(ROOT, "pty-go");
/** One line per launch of the stub harness, so a spec can tell whether anything started it. */
export const STUB_RUNS = path.join(ROOT, "stub-runs.log");
export const TREE_NAMES = [
  `<img src=x onerror=${js("tree-img")}>.md`,
  `"><svg onload=${js("tree-svg")}>.txt`,
  `javascript:${js("tree-js")}.md`,
];
export const TREE_DIR = `<script>${js("tree-dir")}`;

function git(args: string[], cwd = REPO) {
  execFileSync("git", args, {
    cwd,
    env: { ...process.env, HOME: path.join(ROOT, "home"), GIT_CONFIG_NOSYSTEM: "1" },
    stdio: "pipe",
  });
}

export function makeRepo(fake: string) {
  fs.mkdirSync(path.join(REPO, ".ostra"), { recursive: true });
  fs.writeFileSync(path.join(REPO, ".ostra/INVENTORY.md"), "# app Inventory\n");
  fs.writeFileSync(path.join(REPO, ".ostra/project.toml"), '[commands]\nformat = "true"\nbuild = "true"\n');
  fs.writeFileSync(path.join(REPO, "README.md"), markdown("readme", fake));
  fs.writeFileSync(path.join(REPO, "notes.md"), markdown("file-md", fake));
  fs.writeFileSync(path.join(REPO, "payload.html"), htmlFile("repo", fake));
  fs.writeFileSync(path.join(REPO, "payload.svg"), svgFile("repo", fake));
  fs.writeFileSync(
    path.join(REPO, "script.js"),
    `// </script><script>${js("monaco-js")}</script>\nconst s = "<img src=x onerror=${js("monaco-str")}>";\n`,
  );
  for (const n of TREE_NAMES) fs.writeFileSync(path.join(REPO, n), `${inline("tree-content")}\n`);
  fs.mkdirSync(path.join(REPO, TREE_DIR), { recursive: true });
  fs.writeFileSync(path.join(REPO, TREE_DIR, "inside.txt"), "inside\n");
  git(["init", "-q", "-b", "main"]);
  git(["config", "user.name", `PW <img src=x onerror=${js("author")}>`]);
  git(["config", "user.email", "pw@example.invalid"]);
  git(["config", "commit.gpgsign", "false"]);
  git(["add", "-A"]);
  git(["commit", "-q", "-m", `Initial ${inline("commit")}`, "-m", markdown("commit-body", fake)]);
  git(["branch", BRANCH]);
  git(["remote", "add", "origin", `https://127.0.0.1:9/<img src=x onerror=${js("remote")}>.git`]);
  // Uncommitted work, so the Git dock lists changes with test-string names.
  fs.appendFileSync(path.join(REPO, "README.md"), `\n${inline("dirty")}\n`);
  fs.writeFileSync(path.join(REPO, `untracked <img src=x onerror=${js("untracked")}>.md`), "new\n");
}

/**
 * The stub harness CLI. `--version` answers the environment check; any other launch prints a
 * banner, waits for one typed byte (so a browser is attached), then prints hostile sequences and
 * logs every byte that reaches it afterwards.
 */
export function makeStubHarness(fake: string): string {
  const bin = path.join(ROOT, "bin");
  fs.mkdirSync(bin, { recursive: true });
  const file = path.join(bin, "claude");
  const osc52 = Buffer.from("PWNED-BY-OSC52").toString("base64");
  fs.writeFileSync(
    file,
    `#!/bin/sh
if [ "$1" = "--version" ]; then echo "9.9.9 (pw stub)"; exit 0; fi
echo "$$" >> ${STUB_RUNS}
stty raw -echo min 0 time 2 2>/dev/null
printf 'PW-TERM-READY press any key\\r\\n'
: > ${PTY_INPUT_LOG}
# Go on one typed byte (a browser is attached) or the go file (the no-browser baseline).
while :; do
  [ -e ${PTY_GO} ] && break
  c=$(dd bs=1 count=1 2>/dev/null | od -An -tx1)
  [ -n "$c" ] && break
done
rm -f ${PTY_GO}
stty min 1 time 0 2>/dev/null
printf '\\033]52;c;${osc52}\\007'
printf '\\033]52;p;${osc52}\\033\\\\'
printf '\\033]52;c;?\\007'
printf '\\033]8;;javascript:${js("osc8").replace(/'/g, "")}\\007PW-JS-LINK\\033]8;;\\007\\r\\n'
printf '\\033]8;;${fake}/canary/osc8\\007PW-HTTP-LINK\\033]8;;\\007\\r\\n'
printf '\\033]8;;file:///etc/passwd\\007PW-FILE-LINK\\033]8;;\\007\\r\\n'
printf '\\033]0;PWNED-TITLE-0\\007\\033]2;PWNED-TITLE-2\\007\\033]1;PWNED-ICON\\007'
printf '\\033[6n\\033[c\\033[>c\\033[=c\\033[5n\\033[?6n'
printf '\\033]10;?\\007\\033]11;?\\007\\033]4;1;?\\007'
printf '\\033[?2026$p\\033[4$p\\033[?u\\033[>q'
printf '\\033[14t\\033[18t\\033[19t\\033[21t'
printf '\\033P$qm\\033\\\\\\033P+q544e\\033\\\\'
printf '\\033]1337;File=inline=1:PHN2Zz48L3N2Zz4=\\007'
printf 'PW-TERM-DONE\\r\\n'
timeout --foreground 4 cat >> ${PTY_INPUT_LOG}
printf 'PW-TERM-EXIT\\r\\n'
`,
    { mode: 0o755 },
  );
  return file;
}

/** A stdio MCP server whose tool metadata and results carry test strings. */
export function makeStubMcp(fake: string): string {
  const file = path.join(ROOT, "bin", "mcp-stub.mjs");
  const md = markdown("mcp-result", fake);
  fs.writeFileSync(
    file,
    `import readline from "node:readline";
const out = (m) => process.stdout.write(JSON.stringify(m) + "\\n");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let m; try { m = JSON.parse(line); } catch { return; }
  if (m.id === undefined) return;
  if (m.method === "initialize") return out({ jsonrpc: "2.0", id: m.id, result: { protocolVersion: m.params?.protocolVersion ?? "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: ${JSON.stringify(inline("mcp-server"))}, version: "1" }, instructions: ${JSON.stringify(md)} } });
  if (m.method === "tools/list") return out({ jsonrpc: "2.0", id: m.id, result: { tools: [{ name: "echo", title: ${JSON.stringify(inline("mcp-title"))}, description: ${JSON.stringify(markdown("mcp-desc", fake))}, inputSchema: { type: "object", properties: { text: { type: "string", description: ${JSON.stringify(inline("mcp-param"))} } } } }] } });
  if (m.method === "tools/call") return out({ jsonrpc: "2.0", id: m.id, result: { content: [{ type: "text", text: ${JSON.stringify(md)} }] } });
  out({ jsonrpc: "2.0", id: m.id, result: {} });
});
`,
  );
  return file;
}

export function writeConfig(harness: string) {
  const toml = `[providers.anthropic]
api_key_env = "PW_FAKE_ANTHROPIC_KEY"
base_url_env = "PW_FAKE_ANTHROPIC_URL"

[tiers.native]
fast = "anthropic:pw-fake"
balanced = "anthropic:pw-fake"
advanced = "anthropic:pw-fake"
frontier = "anthropic:pw-fake"

[harness.claude]
command = "${harness}"
args = []

[harness.codex]
command = "/nonexistent/pw-codex"
args = []

[harness.grok]
command = "/nonexistent/pw-grok"
args = []

[harness.agy]
command = "/nonexistent/pw-agy"
args = []

# The stub harness and MCP server live in the scratch root under /tmp, which the sandbox's
# private /tmp would hide.
[sandbox]
extra_writable = ["${path.join(ROOT, "bin")}"]
`;
  fs.writeFileSync(path.join(ROOT, "config.toml"), toml);
}
