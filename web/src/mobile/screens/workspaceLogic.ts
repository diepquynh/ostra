import type { SkillView } from "../../api/types";

export type SkillTarget =
  | { creating: true; project: string }
  | { creating: false; project: string; origin: SkillView["origin"]; path: string };

/** The anchor that opens one skill's editor on the Skills screen. Project keys and origins never contain a colon. */
export const skillAnchor = (project: string, s: Pick<SkillView, "origin" | "path">) =>
  `skill:${project}:${s.origin}:${s.path}`;

/** `skill:<project>:<origin>:<path>` or `skill-new:<project>`, else null. */
export function parseSkillAnchor(anchor: string | null): SkillTarget | null {
  if (!anchor) return null;
  const fresh = /^skill-new:([^:]+)$/.exec(anchor);
  if (fresh) return { creating: true, project: fresh[1] };
  const m = /^skill:([^:]+):(ostra|harness):(.+)$/.exec(anchor);
  return m ? { creating: false, project: m[1], origin: m[2] as SkillView["origin"], path: m[3] } : null;
}
