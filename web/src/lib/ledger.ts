/** A finding row from a review ledger's markdown tables. */
export type LedgerFinding = { id: string; severity: string; file: string; rule: string; description: string; fix: string; iteration: number };

/** Parse the findings tables of `ostra-review-ledger-*.md` (Ultracode's ledger shape). */
export function parseLedger(markdown: string): LedgerFinding[] {
  const out: LedgerFinding[] = [];
  let iteration = 0;
  let header: string[] | null = null;
  for (const raw of markdown.split("\n")) {
    const line = raw.trim();
    const it = /^#{1,4}\s+Iteration\s+(\d+)/i.exec(line);
    if (it) {
      iteration = Number(it[1]);
      header = null;
      continue;
    }
    if (!line.startsWith("|")) {
      header = null;
      continue;
    }
    const cells = line
      .replace(/^\|/, "")
      .replace(/\|$/, "")
      .split("|")
      .map((c) => c.trim());
    if (cells.every((c) => /^:?-+:?$/.test(c))) continue;
    if (!header) {
      header = cells.map((c) => c.toLowerCase());
      continue;
    }
    const col = (name: string) => {
      const i = header!.findIndex((h) => h.startsWith(name));
      return i >= 0 ? (cells[i] ?? "") : "";
    };
    if (!header.includes("severity")) continue;
    out.push({
      id: col("id"),
      severity: col("severity").toUpperCase(),
      file: col("file").replace(/`/g, ""),
      rule: col("rule"),
      description: col("description"),
      fix: col("fix"),
      iteration,
    });
  }
  return out;
}

/** The line a finding points at, from "line N" in its text. */
export function findingLine(f: { description: string; fix: string }): number | null {
  const m = /line (\d+)/i.exec(`${f.description} ${f.fix}`);
  return m ? Number(m[1]) : null;
}

/** Which review loop a ledger belongs to: `{ project, phase }` from its path. */
export function ledgerLoop(path: string): { project: string | null; phase: string } | null {
  const m = /\/([^/]+)\/ostra-review-ledger(?:-phase-([\w-]+))?\.md$/.exec(path);
  if (!m) return null;
  return { project: m[1] ?? null, phase: m[2] ?? "none" };
}
