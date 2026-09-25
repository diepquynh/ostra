import type { Severity } from "../../api/gen/Severity";
import type { FactCheckFinding, ReviewFinding } from "../../api/types";
import { Chip, Table, type Tone } from "../../design";

const tone = (s: Severity): Tone => (s === "BLOCKER" || s === "HIGH" ? "bad" : s === "MEDIUM" ? "warn" : "neutral");

const muted = { fontSize: "var(--text-sm)", color: "var(--text-muted)" } as const;
const mono = { fontFamily: "var(--font-mono)", fontSize: "var(--text-sm)" } as const;

/** Reviewer findings: severity and rule, file, then what is wrong, the exact fix, and any Guidance. */
export function ReviewFindings({ findings }: { findings: ReviewFinding[] }) {
  const rows = findings.map((f, i) => ({ ...f, id: i }));
  return (
    <Table
      dense
      rows={rows}
      empty="No findings."
      columns={[
        {
          key: "severity",
          label: "Severity",
          width: 110,
          render: (f) => (
            <div style={{ display: "flex", flexDirection: "column", gap: 3, alignItems: "flex-start" }}>
              <Chip tone={tone(f.severity)}>{f.severity}</Chip>
              <span style={mono}>{f.rule}</span>
            </div>
          ),
        },
        {
          key: "file",
          label: "File",
          render: (f) => <span style={{ ...mono, overflowWrap: "anywhere" }}>{f.file}</span>,
        },
        {
          key: "description",
          label: "Finding",
          render: (f) => (
            <div style={{ display: "flex", flexDirection: "column", gap: 3, whiteSpace: "normal" }}>
              <span>{f.description}</span>
              <span style={muted}>
                Fix: <code>{f.fix}</code>
              </span>
              {f.guidance && <span style={{ fontSize: "var(--text-sm)" }}>Guidance: {f.guidance}</span>}
            </div>
          ),
        },
      ]}
    />
  );
}

/** Fact-check findings: severity, where in the artifact, the claim, and what is wrong with it. */
export function FactFindings({ findings }: { findings: FactCheckFinding[] }) {
  if (findings.length === 0) return null;
  const rows = findings.map((f, i) => ({ ...f, id: i }));
  return (
    <Table
      dense
      rows={rows}
      columns={[
        {
          key: "severity",
          label: "Severity",
          width: 90,
          render: (f) => <Chip tone={tone(f.severity)}>{f.severity}</Chip>,
        },
        { key: "location", label: "Location", width: 150 },
        {
          key: "claim",
          label: "Claim and issue",
          render: (f) => (
            <div style={{ display: "flex", flexDirection: "column", gap: 3, whiteSpace: "normal" }}>
              <span>{f.claim}</span>
              <span style={muted}>{f.issue}</span>
            </div>
          ),
        },
      ]}
    />
  );
}
