import type { GitBranch, GitChange, GitCheckoutRequest, GitOpResult, GitRepoStatus } from "../types";

type Repo = GitRepoStatus & { branchList: GitBranch[] };

const repos = new Map<string, Repo>();

function repo(key: string): Repo {
  let r = repos.get(key);
  if (!r) {
    r = {
      is_git: true,
      branch: "main",
      head: "1a2b3c4",
      upstream: "origin/main",
      ahead: 0,
      behind: 1,
      staged: [{ path: "src/lib.rs", mark: "M", orig_path: null }],
      unstaged: [
        { path: "README.md", mark: "M", orig_path: null },
        { path: "src/old_name.rs", mark: "D", orig_path: null },
        { path: "notes/todo.md", mark: "?", orig_path: null },
      ],
      conflicted: [],
      staged_elsewhere: 0,
      truncated: false,
      busy: null,
      branchList: [
        {
          name: "main",
          remote: false,
          current: true,
          upstream: "origin/main",
          commit: "1a2b3c4",
          subject: "Split the order service",
        },
        {
          name: "fix/cancel-race",
          remote: false,
          current: false,
          upstream: null,
          commit: "9f8e7d6",
          subject: "Lock the order row before cancelling",
        },
        {
          name: "origin/main",
          remote: true,
          current: false,
          upstream: null,
          commit: "0c0ffee",
          subject: "Bump dependencies",
        },
        {
          name: "origin/release",
          remote: true,
          current: false,
          upstream: null,
          commit: "5e5e5e5",
          subject: "Release 1.4",
        },
      ],
    };
    repos.set(key, r);
  }
  return r;
}

const view = ({ branchList: _, ...st }: Repo): GitRepoStatus => st;

const move = (from: GitChange[], to: GitChange[], paths: string[]) => {
  const take = paths.length ? from.filter((c) => paths.includes(c.path)) : [...from];
  const rest = from.filter((c) => !take.includes(c));
  const moved = take.map((c) => ({ ...c, mark: c.mark === "?" ? ("A" as const) : c.mark }));
  return [
    rest,
    [...to.filter((t) => !moved.some((m) => m.path === t.path)), ...moved].sort((a, b) => a.path.localeCompare(b.path)),
  ];
};

export const mockGit = {
  status: (key: string) => view(repo(key)),
  branches: (key: string) => repo(key).branchList,
  op: (key: string, output: string): GitOpResult => ({ output, status: view(repo(key)) }),
  stage(key: string, paths: string[], stage: boolean): GitOpResult {
    const r = repo(key);
    if (stage) [r.unstaged, r.staged] = move(r.unstaged, r.staged, paths);
    else [r.staged, r.unstaged] = move(r.staged, r.unstaged, paths);
    return mockGit.op(key, "");
  },
  commit(key: string, message: string): GitOpResult {
    const r = repo(key);
    if (!message.trim()) throw new Error("Write a commit message first.");
    if (!r.staged.length) throw new Error("Stage the changes to commit first. Nothing is staged.");
    r.staged = [];
    r.ahead += 1;
    r.head = Math.random().toString(16).slice(2, 9);
    return mockGit.op(key, `[${r.branch} ${r.head}] ${message.split("\n")[0]}`);
  },
  push(key: string): GitOpResult {
    const r = repo(key);
    r.ahead = 0;
    r.upstream ??= `origin/${r.branch}`;
    return mockGit.op(key, `To origin\n   ${r.head}  HEAD -> ${r.branch}`);
  },
  checkout(key: string, req: GitCheckoutRequest): GitOpResult {
    const r = repo(key);
    const name = req.branch.replace(/^origin\//, "");
    if (req.create && r.branchList.some((b) => !b.remote && b.name === name))
      throw new Error(`A branch named \`${name}\` already exists.`);
    if (!r.branchList.some((b) => !b.remote && b.name === name))
      r.branchList.push({
        name,
        remote: false,
        current: false,
        upstream: req.create ? null : `origin/${name}`,
        commit: r.head ?? "",
        subject: "",
      });
    r.branchList = r.branchList.map((b) => ({ ...b, current: !b.remote && b.name === name }));
    r.branch = name;
    r.upstream = r.branchList.find((b) => b.current)?.upstream ?? null;
    r.ahead = 0;
    r.behind = 0;
    return mockGit.op(key, `Switched to branch '${name}'`);
  },
};
