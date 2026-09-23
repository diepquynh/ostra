import type { DiffHunk } from "../../api/types";

export type DiffRow =
  | { type: "add" | "del" | "ctx"; text: string; old: number | null; new: number | null }
  | { type: "gap"; skipped: number | null; header: string };

/**
 * Flatten FileDiff hunks into rows with old and new line numbers. A gap row stands for the unchanged
 * lines before a hunk; its count is known before the first hunk and between hunks.
 */
export function diffRows(hunks: DiffHunk[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let prevOldEnd = 1;
  hunks.forEach((h, i) => {
    const before = h.old_lines === 0 ? h.old_start + 1 : h.old_start;
    const skipped = before - prevOldEnd;
    if (i > 0 || skipped > 0) rows.push({ type: "gap", skipped: skipped > 0 ? skipped : null, header: h.header });
    for (const l of h.lines) {
      rows.push({ type: l.type === "context" ? "ctx" : l.type, text: l.text, old: l.old_no, new: l.new_no });
    }
    prevOldEnd = before + h.old_lines;
  });
  return rows;
}
