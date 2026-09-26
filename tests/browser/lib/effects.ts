/**
 * Everything a cross-origin request could change, read back as the signed-in user. Two equal
 * snapshots around an attack mean it had no server-side effect.
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import type { Api } from "./api";
import { ROOT, type SuiteState } from "./env";
import { STUB_RUNS } from "./scenario";

const lines = (f: string) => (fs.existsSync(f) ? fs.readFileSync(f, "utf8").split("\n").filter(Boolean).length : 0);

function tree(dir: string, depth: number): string[] {
  if (!fs.existsSync(dir) || depth < 0) return [];
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((e) => !/^workspace\.db|\.log$|^server\.(json|out|pid)$|^pty-|^canary-hits|^test-results/.test(e.name))
    .flatMap((e) => {
      const p = path.join(dir, e.name);
      return e.isDirectory() ? [`${p}/`, ...tree(p, depth - 1)] : [p];
    })
    .sort();
}

export async function snapshot(api: Api, state: SuiteState) {
  const signIns = (await api.get<{ id: string }[]>("/api/auth/sessions")).map((s) => s.id).sort();
  const workspaces = (await api.get<any[]>("/api/workspaces")).map((w) => `${w.id}:${w.name}:${w.projects}`);
  const ui = await api.get(`/api/workspaces/${state.ws}/ui`);
  const session = await api.get(`/api/sessions/${state.session}`);
  const onboarding = await api.get("/api/onboarding");
  const detail = await api.get(`/api/workspaces/${state.ws}`);
  const git = execFileSync("git", ["-C", state.repo, "status", "--porcelain=v1", "-b"], { encoding: "utf8" });
  const refs = execFileSync("git", ["-C", state.repo, "for-each-ref", "--format=%(refname) %(objectname)"], {
    encoding: "utf8",
  });
  return {
    signIns,
    workspaces,
    ui,
    session: {
      status: session.summary.status,
      yolo: session.summary.yolo,
      executions: session.executions.length,
      gates: session.gates?.length,
    },
    onboarded: onboarding.onboarded_at,
    settings: detail.settings,
    git,
    refs,
    files: tree(ROOT, 3).filter((f) => !/\/(data|home|attacker)\//.test(f)),
    data: tree(path.join(ROOT, "data"), 1),
    modelCalls: lines(path.join(ROOT, "fake-model.log")),
    harnessRuns: lines(STUB_RUNS),
  };
}
