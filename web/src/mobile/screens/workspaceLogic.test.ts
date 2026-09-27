import { describe, expect, it } from "vitest";
import { parseSkillAnchor, skillAnchor } from "./workspaceLogic";

describe("skill anchors", () => {
  it("round-trips a skill, keeping colons in the path", () => {
    const a = skillAnchor("backend", { origin: "harness", path: ".claude/skills/a:b/SKILL.md" });
    expect(parseSkillAnchor(a)).toEqual({
      creating: false,
      project: "backend",
      origin: "harness",
      path: ".claude/skills/a:b/SKILL.md",
    });
  });

  it("reads a new-skill anchor and rejects others", () => {
    expect(parseSkillAnchor("skill-new:web")).toEqual({ creating: true, project: "web" });
    expect(parseSkillAnchor("lesson:web:3")).toBeNull();
    expect(parseSkillAnchor(null)).toBeNull();
  });
});
