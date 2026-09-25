import { describe, expect, it } from "vitest";
import { findingLine, ledgerLoop, parseLedger } from "./ledger";

const LEDGER = `# Code Review Ledger

## Iteration 1 (context: implementation)

### Findings

| ID  | Severity | File | Rule | Description | Fix Suggestion |
| --- | -------- | ---- | ---- | ----------- | -------------- |
| F1  | HIGH | \`src/a.ts\` | C3 | Missing check on line 12 | Add guard |
| F2  | low | src/b.ts | C1 | Naming | Rename |

## Iteration 2 (context: implementation)

| ID | Severity | File | Rule | Description | Fix Suggestion |
| --- | --- | --- | --- | --- | --- |
| F3 | MEDIUM | src/a.ts | PHASE-REQ-2 | Wrong status | Change \`400\` to \`409\` on line 41. |
`;

describe("ledger", () => {
  it("parses findings with their iteration", () => {
    const f = parseLedger(LEDGER);
    expect(f.map((x) => [x.id, x.severity, x.file, x.iteration])).toEqual([
      ["F1", "HIGH", "src/a.ts", 1],
      ["F2", "LOW", "src/b.ts", 1],
      ["F3", "MEDIUM", "src/a.ts", 2],
    ]);
  });

  it("finds the line a finding names", () => {
    const f = parseLedger(LEDGER);
    expect(findingLine(f[0])).toBe(12);
    expect(findingLine(f[1])).toBeNull();
    expect(findingLine(f[2])).toBe(41);
  });

  it("reads the loop from the path", () => {
    expect(ledgerLoop("/w/.ostra/sessions/s1/backend/ostra-review-ledger-phase-3-tests.md")).toEqual({
      project: "backend",
      phase: "3-tests",
    });
    expect(ledgerLoop("/w/.ostra/sessions/s1/web/ostra-review-ledger.md")).toEqual({ project: "web", phase: "none" });
    expect(ledgerLoop("/w/notes.md")).toBeNull();
  });
});
