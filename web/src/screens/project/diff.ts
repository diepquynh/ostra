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

export type ChangeKind = "add" | "mod" | "del";

/** One run of changed lines, as the file view's gutter marks it. */
export type ChangeBlock = {
  kind: ChangeKind;
  /** Current line numbers the mark covers; a deletion marks the line before the removed text. */
  lines: number[];
  /** The line the inline comparison opens under. */
  anchor: number;
  old: { no: number; text: string }[];
  current: { no: number; text: string }[];
};

/** Group each hunk's consecutive added and removed lines into blocks, VS Code style. */
export function changeBlocks(hunks: DiffHunk[]): ChangeBlock[] {
  const out: ChangeBlock[] = [];
  for (const h of hunks) {
    let open: ChangeBlock | null = null;
    // The current line the next removed line sits before.
    let next = h.new_lines === 0 ? h.new_start + 1 : h.new_start;
    const close = () => {
      if (!open) return;
      if (open.current.length) {
        open.kind = open.old.length ? "mod" : "add";
        open.lines = open.current.map((l) => l.no);
      } else {
        open.lines = [Math.max(1, next - 1)];
      }
      open.anchor = open.lines[open.lines.length - 1];
      out.push(open);
      open = null;
    };
    for (const l of h.lines) {
      if (l.type === "context") {
        close();
        next = (l.new_no ?? next) + 1;
        continue;
      }
      open ??= { kind: "del", lines: [], anchor: 0, old: [], current: [] };
      if (l.type === "del" && l.old_no !== null) open.old.push({ no: l.old_no, text: l.text });
      if (l.type === "add" && l.new_no !== null) {
        open.current.push({ no: l.new_no, text: l.text });
        next = l.new_no + 1;
      }
    }
    close();
  }
  return out;
}
