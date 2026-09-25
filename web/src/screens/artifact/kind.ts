import type { SessionDetail } from "../../api/types";
import type { Tone } from "../../design";
import { basename } from "../../lib/format";

export type ArtifactKind = "research" | "spec" | "plan" | "phase" | "report" | "ledger" | "completion";

const KINDS: ArtifactKind[] = ["research", "spec", "plan", "phase", "report", "ledger", "completion"];

/** The artifact kind: the session's `ArtifactRef.kind` when known, else read from the file name. */
export function artifactKind(path: string, refKind?: string | null): ArtifactKind {
  if (refKind && (KINDS as string[]).includes(refKind)) return refKind as ArtifactKind;
  const name = basename(path);
  if (/^ostra-review-ledger/.test(name)) return "ledger";
  if (/^ostra-spec-/.test(name)) return "spec";
  if (/^ostra-plan-.*phase-\d+/.test(name) || /^phase-\d+\.md$/.test(name)) return "phase";
  if (/^ostra-plan-/.test(name)) return "plan";
  if (/^ostra-research-/.test(name)) return "research";
  if (/completion/.test(name)) return "completion";
  return "report";
}

export const KIND_LABEL: Record<ArtifactKind, string> = {
  research: "Research",
  spec: "Spec",
  plan: "Plan",
  phase: "Phase file",
  report: "Report",
  ledger: "Review ledger",
  completion: "Completion report",
};

/** Specs, plans and phase files state requirements in EARS, so they carry the reading note. */
export const hasEarsNote = (k: ArtifactKind) => k === "spec" || k === "plan" || k === "phase";

export type Badge = { tone: Tone; label: string; icon?: "circle-check" | "circle-x" | "circle-pause" };

/** Fact-check and approval chips for a spec or plan, from the session's stage cards and gates. */
export function approvalBadges(detail: SessionDetail | null, kind: ArtifactKind): Badge[] {
  if (!detail || (kind !== "spec" && kind !== "plan")) return [];
  const out: Badge[] = [];
  const fc = detail.stages.filter((s) => s.stage === (kind === "spec" ? "fact-check-spec" : "fact-check-plan")).pop();
  if (fc?.status === "done") {
    const pass = !fc.detail || /pass/i.test(fc.detail);
    out.push(
      pass
        ? { tone: "ok", label: "Fact-check PASS", icon: "circle-check" }
        : { tone: "bad", label: `Fact-check ${fc.detail}`, icon: "circle-x" },
    );
  } else if (fc?.status === "running") out.push({ tone: "accent", label: "Fact-check running" });
  const approval = detail.stages.filter((s) => s.stage === (kind === "spec" ? "spec-approval" : "plan-approval")).pop();
  const gate = approval?.gate ? detail.gates.find((g) => g.id === approval.gate) : null;
  if (gate?.answer?.kind === "approval") {
    const by = gate.source === "user" ? "you" : gate.source === "yolo" ? "YOLO" : "Ostra";
    out.push(
      gate.answer.approved
        ? { tone: "ok", label: `Approved by ${by}` }
        : { tone: "warn", label: `Changes requested by ${by}` },
    );
  } else if (gate && !gate.answer) out.push({ tone: "warn", label: "Waiting for your approval", icon: "circle-pause" });
  return out;
}
