// Mock project files for the Files dock, modeled on the design kit's fsdata.js: two project trees
// with git marks, a few real file bodies, and diffs for the files the running session changed.

import type { DiffLine } from "../gen/DiffLine";
import type { ChangedBy, DiffHunk, FileDiff, FsBrowse, GitMark, ProjectChange, ProjectFile, ProjectTree, ProjectTreeEntry } from "../types";
import { SESSION } from "./fixtures";
import { extraDirs, modifiedFor } from "./fixtures.projects";

type Node = {
  name: string;
  dir?: boolean;
  children?: Node[];
  code?: string;
  git?: GitMark;
  /** New-file line numbers that the diff marks as added. */
  added?: number[];
  /** Old lines removed before the given new-file line number. */
  removed?: Record<number, string[]>;
  binary?: boolean;
  by?: { execution: string; phase: number | null; running: boolean; staged: boolean };
};

const serviceRs = `use crate::orders::{Actor, Error, Order, OrderId, OrderStatus, Result};
use crate::repo::OrderRepo;
use chrono::Utc;

pub struct OrderService {
    repo: OrderRepo,
}

impl OrderService {
    pub async fn get(&self, id: OrderId) -> Result<Order> {
        self.repo.find(id).await?.ok_or(Error::NotFound)
    }

    pub async fn pay(&self, id: OrderId) -> Result<Order> {
        let order = self.get(id).await?;
        // Rule: only Pending orders can be paid.
        if order.status != OrderStatus::Pending {
            return Err(Error::InvalidTransition);
        }
        self.repo.set_status(id, OrderStatus::Paid).await
    }

    pub async fn cancel(&self, id: OrderId, by: Actor) -> Result<Order> {
        let order = self.get(id).await?;
        match order.status {
            OrderStatus::Shipped | OrderStatus::Delivered => Err(Error::CannotCancel),
            _ => self.repo.mark_cancelled(id, by, Utc::now()).await,
        }
    }

    pub async fn ship(&self, id: OrderId, tracking: String) -> Result<Order> {
        let order = self.get(id).await?;
        if order.status != OrderStatus::Paid {
            return Err(Error::InvalidTransition);
        }
        self.repo.mark_shipped(id, tracking, Utc::now()).await
    }
}
`;

const migration = `-- Phase 1: cancellation state (ostra-plan-order-cancel/phase-1.md)
ALTER TYPE order_status ADD VALUE IF NOT EXISTS 'cancelled';

ALTER TABLE orders
    ADD COLUMN cancelled_at TIMESTAMPTZ,
    ADD COLUMN cancelled_by TEXT;

CREATE INDEX orders_cancelled_at_idx ON orders (cancelled_at) WHERE cancelled_at IS NOT NULL;
`;

const projectToml = `# Written by the initializer. Edit freely; agents re-read it on every execution.
stack = "rust-axum"
test_framework = "cargo test"

[commands]
build = "cargo check --workspace"
test = "cargo test --workspace"
format = "cargo fmt --all"

[[skills]]
name = "axum-handler"
path = ".ostra/skills/axum-handler/SKILL.md"

[review]
auto_fixable = ["unused-import", "missing-trailing-comma"]
`;

const orderActions = `import { Button } from "../ui/Button";
import { useCancelOrder } from "../hooks/useCancelOrder";
import type { Order } from "../api/types";

const CANCELLABLE = new Set(["pending", "paid"]);

export function canCancel(order: Order) {
  return CANCELLABLE.has(order.status);
}

export function OrderActions({ order }: { order: Order }) {
  const { cancel, pending, error } = useCancelOrder(order.id);
  return (
    <div className="order-actions">
      {canCancel(order) && (
        <Button variant="danger" onClick={cancel} disabled={pending}>Cancel order</Button>
      )}
      <Button onClick={() => track(order)}>Track</Button>
      {error && <p role="alert">{error.message}</p>}
    </div>
  );
}
`;

const inventory = `# Inventory: backend

## Commands
| Purpose | Command |
| --- | --- |
| Build | cargo check --workspace |
| Test | cargo test --workspace |
| Format | cargo fmt --all |
`;

const TREES: Record<string, Node[]> = {
  backend: [
    {
      name: ".ostra",
      dir: true,
      children: [
        { name: "INVENTORY.md", code: inventory },
        { name: "project.toml", code: projectToml },
        { name: "skills", dir: true, children: [{ name: "axum-handler", dir: true, children: [{ name: "SKILL.md" }] }, { name: "sqlx-repo", dir: true, children: [{ name: "SKILL.md" }] }] },
        { name: "memory", dir: true, children: [{ name: "knowledge.sqlite3", binary: true }] },
      ],
    },
    {
      name: "crates",
      dir: true,
      children: [
        {
          name: "orders",
          dir: true,
          children: [
            { name: "Cargo.toml" },
            {
              name: "src",
              dir: true,
              children: [
                { name: "lib.rs" },
                { name: "model.rs", git: "M" },
                { name: "repo.rs", git: "M" },
                { name: "router.rs" },
                {
                  name: "service.rs",
                  git: "M",
                  code: serviceRs,
                  added: [23, 24, 25, 26, 27, 28, 29],
                  removed: { 23: ["    pub async fn cancel(&self, _id: OrderId) -> Result<Order> {", "        todo!()", "    }"] },
                  by: { execution: "x_imp2", phase: 2, running: false, staged: false },
                },
              ],
            },
          ],
        },
        { name: "payments", dir: true, children: [{ name: "src", dir: true, children: [{ name: "webhooks.rs" }] }] },
      ],
    },
    {
      name: "migrations",
      dir: true,
      children: [
        { name: "20260611_payment_events.sql" },
        { name: "20260923_cancelled_at.sql", git: "A", code: migration, by: { execution: "x_imp1", phase: 1, running: false, staged: true } },
      ],
    },
    { name: "tests", dir: true, children: [{ name: "orders_test.rs" }] },
    { name: "target", dir: true, children: [] },
    { name: ".gitignore", code: "/target\n" },
    { name: "Cargo.toml" },
    { name: "README.md", code: "# shop-backend\n\nOrders, payments and refunds for the shop.\n" },
  ],
  web: [
    { name: ".ostra", dir: true, children: [{ name: "INVENTORY.md" }, { name: "project.toml" }] },
    {
      name: "src",
      dir: true,
      children: [
        { name: "api", dir: true, children: [{ name: "orders.ts" }, { name: "types.ts" }] },
        {
          name: "components",
          dir: true,
          children: [
            {
              name: "OrderActions.tsx",
              git: "M",
              code: orderActions,
              added: [13, 14, 15, 17],
              removed: { 13: ['      {order.status === "paid" && <Button variant="danger">Cancel</Button>}'] },
              by: { execution: "x_imp3", phase: 3, running: true, staged: false },
            },
            { name: "OrderHistory.tsx" },
          ],
        },
        { name: "hooks", dir: true, children: [{ name: "useOrder.ts" }, { name: "useCancelOrder.ts", git: "?", by: { execution: "x_imp3", phase: 3, running: true, staged: false } }] },
        { name: "pages", dir: true, children: [{ name: "OrderPage.tsx" }] },
      ],
    },
    { name: "package.json" },
    { name: "tsconfig.json" },
  ],
};

const MODIFIED = "2026-09-22T10:40:00Z";

function hash(s: string) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h;
}

function nodeAt(key: string, path: string): Node | null {
  let cur: Node = { name: "", dir: true, children: TREES[key] ?? [] };
  if (!TREES[key]) return null;
  for (const seg of path.split("/").filter(Boolean)) {
    const next = (cur.children ?? []).find((c) => c.name === seg);
    if (!next) return null;
    cur = next;
  }
  return cur;
}

const hasChanges = (n: Node): boolean => (n.dir ? (n.children ?? []).some(hasChanges) : !!n.git);
const sizeOf = (n: Node, path: string) => (n.dir ? 0 : n.code ? n.code.length : n.binary ? 98304 + (hash(path) % 40000) : 400 + (hash(path) % 9000));

function changedBy(n: Node): ChangedBy | null {
  if (!n.by) return null;
  return { session: SESSION, execution: n.by.execution, agent: "implementer", phase: n.by.phase, tests: false, staged: n.by.staged, running: n.by.running, at: MODIFIED };
}

function entry(key: string, n: Node, path: string): ProjectTreeEntry {
  return {
    name: n.name,
    path,
    is_dir: !!n.dir,
    is_symlink: false,
    size: sizeOf(n, path),
    modified: modifiedFor(path, hasChanges(n)),
    ignored: key === "backend" && path === "target",
    git: n.dir ? null : (n.git ?? null),
    staged: !!n.by?.staged,
    has_changes: hasChanges(n),
    changed_by: n.dir ? null : changedBy(n),
  };
}

export function mockTree(key: string, path = "", hidden = false): ProjectTree {
  const dir = nodeAt(key, path);
  if (!dir || !dir.dir) throw Object.assign(new Error(`${path} does not exist in project \`${key}\`.`), { status: 404 });
  const entries = (dir.children ?? [])
    .filter((c) => hidden || !c.name.startsWith("."))
    .sort((a, b) => (!!a.dir === !!b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1))
    .map((c) => entry(key, c, path ? `${path}/${c.name}` : c.name));
  return { project: key, path, entries, is_git: true, truncated: false };
}

function stub(path: string): string {
  const ext = path.split(".").pop();
  const comment = ext === "md" ? "" : ext === "toml" || ext === "sql" ? "# " : "// ";
  return `${comment}${path}\n${comment}The mock server has no body for this file; the real server returns its contents.\n`;
}

export function mockFile(key: string, path: string): ProjectFile {
  const n = nodeAt(key, path);
  if (!n || n.dir) throw new Error(`${path} is not a file in project \`${key}\`.`);
  const content = n.binary ? null : (n.code ?? stub(path));
  return { path, content, binary: !!n.binary, size: sizeOf(n, path), truncated: false, modified: modifiedFor(path, !!n.git), git: n.git ?? null, staged: !!n.by?.staged, changed_by: changedBy(n) };
}

function walk(nodes: Node[], base: string, out: { path: string; node: Node }[]) {
  for (const n of nodes) {
    const p = base ? `${base}/${n.name}` : n.name;
    if (n.dir) walk(n.children ?? [], p, out);
    else out.push({ path: p, node: n });
  }
  return out;
}

export const mockFileIndex = (key: string) => ({ paths: walk(TREES[key] ?? [], "", []).map((f) => f.path).sort(), truncated: false });

/** Unified diff of a mock file against HEAD, with three lines of context around each change. */
export function mockDiff(key: string, path: string): FileDiff {
  const n = nodeAt(key, path);
  if (!n || n.dir) throw new Error(`${path} is not a file in project \`${key}\`.`);
  const base = { path, base: "HEAD", binary: !!n.binary, truncated: false, git: n.git ?? null, changed_by: changedBy(n) };
  if (!n.git || !n.code) return { ...base, hunks: [], added: 0, removed: 0 };
  const lines = n.code.replace(/\n$/, "").split("\n");
  const added = new Set(n.git === "A" || n.git === "?" ? lines.map((_, i) => i + 1) : (n.added ?? []));
  const rows: DiffLine[] = [];
  let oldNo = 0;
  lines.forEach((text, i) => {
    const newNo = i + 1;
    for (const r of n.removed?.[newNo] ?? []) rows.push({ type: "del", text: r, old_no: ++oldNo, new_no: null });
    if (added.has(newNo)) rows.push({ type: "add", text, old_no: null, new_no: newNo });
    else rows.push({ type: "context", text, old_no: ++oldNo, new_no: newNo });
  });
  const keep = rows.map((_, i) => rows.slice(Math.max(0, i - 3), i + 4).some((x) => x.type !== "context"));
  const hunks: DiffHunk[] = [];
  let cur: DiffLine[] | null = null;
  rows.forEach((r, i) => {
    if (!keep[i]) {
      cur = null;
      return;
    }
    if (!cur) {
      cur = [];
      hunks.push({ old_start: 0, old_lines: 0, new_start: 0, new_lines: 0, header: "", lines: cur });
    }
    cur.push(r);
  });
  for (const h of hunks) {
    const firstOld = h.lines.find((l) => l.old_no !== null)?.old_no ?? 0;
    const firstNew = h.lines.find((l) => l.new_no !== null)?.new_no ?? 0;
    Object.assign(h, {
      old_start: firstOld,
      new_start: firstNew,
      old_lines: h.lines.filter((l) => l.type !== "add").length,
      new_lines: h.lines.filter((l) => l.type !== "del").length,
    });
  }
  return { ...base, hunks, added: rows.filter((r) => r.type === "add").length, removed: rows.filter((r) => r.type === "del").length };
}

export function mockChanges(key: string): ProjectChange[] {
  return walk(TREES[key] ?? [], "", [])
    .filter((f) => f.node.git && f.node.by)
    .map((f) => {
      const d = mockDiff(key, f.path);
      return { path: f.path, git: f.node.git ?? null, staged: !!f.node.by?.staged, added: d.added, removed: d.removed, changed_by: changedBy(f.node)! };
    });
}

// Type-to-browse folders for the folder picker (`GET /api/fs`).
const DIRS: Record<string, { name: string; is_git?: boolean; is_ostra_project?: boolean }[]> = {
  ...extraDirs,
  "/": [{ name: "etc" }, { name: "home" }, { name: "opt" }, { name: "srv" }, { name: "tmp" }, { name: "usr" }, { name: "var" }],
  "/home": [{ name: "me" }],
  "/home/me": [{ name: "code" }, { name: "Downloads" }, { name: "notes" }],
  "/home/me/code": [
    { name: "billing-service", is_git: true },
    { name: "dotfiles", is_git: true },
    { name: "shop" },
    { name: "shop-admin", is_git: true },
    { name: "shop-backend", is_git: true, is_ostra_project: true },
    { name: "shop-web", is_git: true, is_ostra_project: true },
  ],
  "/home/me/code/shop": [],
  "/home/me/code/shop-backend": [{ name: ".ostra" }, { name: "crates" }, { name: "migrations" }, { name: "tests" }],
  "/home/me/code/shop-web": [{ name: ".ostra" }, { name: "src" }, { name: "public" }],
  "/srv": [{ name: "repos" }],
  "/srv/repos": [{ name: "legacy-erp", is_git: true }, { name: "reporting", is_git: true }],
};

export function mockBrowse(path = "~", prefix = "", limit = 200): FsBrowse {
  const home = "/home/me";
  const full = (path.startsWith("~") ? home + path.slice(1) : path).replace(/\/+$/, "") || "/";
  const parentOf = (p: string) => (p === "/" ? null : p.split("/").slice(0, -1).join("/") || "/");
  let nearest = full;
  while (!(nearest in DIRS) && nearest !== "/") nearest = parentOf(nearest)!;
  const exists = full in DIRS;
  const p = prefix.toLowerCase();
  const names = exists ? DIRS[full].filter((e) => (p ? e.name.toLowerCase().includes(p) : !e.name.startsWith("."))) : [];
  names.sort((a, b) => Number(!a.name.toLowerCase().startsWith(p)) - Number(!b.name.toLowerCase().startsWith(p)) || a.name.localeCompare(b.name));
  const parent = parentOf(full);
  const self = exists && parent !== null ? DIRS[parent]?.find((e) => e.name === full.slice(full.lastIndexOf("/") + 1)) : undefined;
  return {
    path: full,
    parent: parentOf(full),
    home,
    exists,
    readable: exists,
    nearest,
    entries: names.slice(0, limit).map((e) => ({ name: e.name, is_dir: true, is_git: !!e.is_git, is_ostra_project: !!e.is_ostra_project })),
    truncated: names.length > limit,
    is_git: !!self?.is_git,
    is_ostra_project: !!self?.is_ostra_project,
  };
}
