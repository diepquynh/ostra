import type { CSSProperties } from "react";
import type { DiffHunk } from "../../api/types";
import { diffRows } from "./diff";

const ln: CSSProperties = {
  width: 44,
  flex: "none",
  paddingRight: 10,
  textAlign: "right",
  color: "var(--text-disabled)",
  userSelect: "none",
};

/** Unified diff against HEAD with old and new line numbers, one gap row between hunks. */
export function DiffPane({ hunks }: { hunks: DiffHunk[] }) {
  const rows = diffRows(hunks);
  return (
    <div style={{ font: "var(--type-code)", fontVariantLigatures: "none", padding: "6px 0" }} data-testid="diff">
      {rows.map((r, i) =>
        r.type === "gap" ? (
          <div
            key={i}
            style={{
              padding: "3px 12px 3px 98px",
              color: "var(--text-muted)",
              background: "var(--surface-panel)",
              borderTop: "1px solid var(--border-subtle)",
              borderBottom: "1px solid var(--border-subtle)",
              fontSize: "var(--text-xs)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            ⋯{" "}
            {r.skipped === null
              ? "unchanged lines"
              : r.skipped === 1
                ? "1 unchanged line"
                : `${r.skipped} unchanged lines`}
            {r.header && <span style={{ marginLeft: 12, fontFamily: "var(--font-mono)" }}>{r.header}</span>}
          </div>
        ) : (
          <div
            key={i}
            className={`os-diff__line ${r.type === "add" ? "os-diff__line--add" : r.type === "del" ? "os-diff__line--del" : ""}`}
            style={{ padding: 0, color: r.type === "ctx" ? "var(--text-primary)" : undefined }}
          >
            <span style={ln}>{r.old ?? ""}</span>
            <span style={ln}>{r.new ?? ""}</span>
            <span className="os-diff__sign" style={{ width: 18 }}>
              {r.type === "add" ? "+" : r.type === "del" ? "-" : " "}
            </span>
            {r.text || " "}
          </div>
        ),
      )}
    </div>
  );
}
