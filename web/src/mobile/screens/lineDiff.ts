import type { DiffLine } from "@ostra/design";
import { replaceDiff } from "../../screens/execution/model";

const splitLines = (s: string): string[] => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));

/** Above this many cells the LCS table costs too much memory, so the diff falls back to one replaced block. */
const MAX_CELLS = 4_000_000;

/**
 * A unified line diff of two versions of a file, with `context` unchanged lines around each change. Runs of
 * unchanged lines beyond that collapse into one context line that says how many were left out.
 */
export function unifiedDiff(original: string, modified: string, context = 3): DiffLine[] {
  const a = splitLines(original);
  const b = splitLines(modified);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const am = a.slice(head, a.length - tail);
  const bm = b.slice(head, b.length - tail);

  let middle: DiffLine[];
  if ((am.length + 1) * (bm.length + 1) > MAX_CELLS) {
    middle = replaceDiff(am.join("\n"), bm.join("\n"));
  } else {
    const n = am.length;
    const m = bm.length;
    // lcs[i][j]: the longest common subsequence of am[i..] and bm[j..].
    const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        lcs[i][j] = am[i] === bm[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    middle = [];
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && am[i] === bm[j]) {
        middle.push({ type: "ctx", text: am[i] });
        i++;
        j++;
      } else if (j < m && (i === n || lcs[i][j + 1] >= lcs[i + 1][j])) {
        middle.push({ type: "add", text: bm[j++] });
      } else {
        middle.push({ type: "del", text: am[i++] });
      }
    }
    // Deletions first within each changed run, as unified diffs read.
    for (let k = 0; k < middle.length; ) {
      if (middle[k].type === "ctx") {
        k++;
        continue;
      }
      let end = k;
      while (end < middle.length && middle[end].type !== "ctx") end++;
      const run = middle.slice(k, end);
      middle.splice(k, run.length, ...run.filter((l) => l.type === "del"), ...run.filter((l) => l.type === "add"));
      k = end;
    }
  }

  const all: DiffLine[] = [
    ...a.slice(0, head).map((text) => ({ type: "ctx" as const, text })),
    ...middle,
    ...a.slice(a.length - tail).map((text) => ({ type: "ctx" as const, text })),
  ];
  const keep = all.map(() => false);
  all.forEach((l, k) => {
    if (l.type === "ctx") return;
    for (let d = Math.max(0, k - context); d <= Math.min(all.length - 1, k + context); d++) keep[d] = true;
  });
  const out: DiffLine[] = [];
  for (let k = 0; k < all.length; ) {
    if (keep[k]) {
      out.push(all[k++]);
      continue;
    }
    let end = k;
    while (end < all.length && !keep[end]) end++;
    const skipped = end - k;
    out.push({ type: "ctx", text: `… ${skipped} unchanged ${skipped === 1 ? "line" : "lines"}` });
    k = end;
  }
  return out;
}
