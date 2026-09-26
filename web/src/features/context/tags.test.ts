import { describe, expect, it } from "vitest";
import {
  ARTIFACTS_ROOT,
  activeQuery,
  baseName,
  filesInText,
  rankFiles,
  removeTag,
  splitTags,
  withFolders,
} from "./tags";

const f = (project: string, path: string) => ({ project, path });

describe("file tags", () => {
  it("finds tags of known projects and drops sentence punctuation", () => {
    const text = "See @api/src/a.ts, and @web/b.tsx. Ignore @other/c.rs and mail@api/x.";
    expect(filesInText(text, ["api", "web"])).toEqual([f("api", "src/a.ts"), f("web", "b.tsx")]);
  });

  it("keeps punctuation that is part of a known path", () => {
    const known = new Set(["api/v1.", "api/a.ts"]);
    expect(filesInText("@api/v1. and @api/a.ts)", ["api"], known)).toEqual([f("api", "v1."), f("api", "a.ts")]);
    expect(filesInText("@api/missing.ts", ["api"], known)).toEqual([]);
  });

  it("splits text into runs and chips", () => {
    expect(splitTags("fix @api/a.ts, now", ["api"])).toEqual([
      { text: "fix " },
      { file: f("api", "a.ts"), raw: "@api/a.ts" },
      { text: "," },
      { text: " now" },
    ]);
  });

  it("removes every mention of a file", () => {
    expect(removeTag("@api/a.ts see @api/a.ts, ok", f("api", "a.ts"))).toBe("see , ok");
  });

  it("reads the query being typed at the caret", () => {
    expect(activeQuery("look at @src/or", 15)).toEqual({ start: 8, query: "src/or" });
    expect(activeQuery("mail@x", 6)).toBeNull();
    expect(activeQuery("@a b", 4)).toBeNull();
  });

  it("ranks basename prefixes first", () => {
    const files = [f("api", "src/order/list.ts"), f("api", "src/orders.ts"), f("web", "src/reorder.ts")];
    expect(rankFiles(files, "order").map((x) => x.path)).toEqual([
      "src/orders.ts",
      "src/reorder.ts",
      "src/order/list.ts",
    ]);
  });

  it("lists each folder once before the files, and names a folder with its slash", () => {
    const out = withFolders([f("api", "src/a/x.ts"), f("api", "src/b.ts"), f("web", "main.ts")]);
    expect(out.map((x) => `${x.project}/${x.path}`)).toEqual([
      "api/src/",
      "api/src/a/",
      "api/src/a/x.ts",
      "api/src/b.ts",
      "web/main.ts",
    ]);
    expect(baseName("src/a/")).toBe("a/");
    expect(baseName("src/a/x.ts")).toBe("x.ts");
    expect(filesInText("look in @api/src/a/ first", ["api"], new Set(["api/src/a/"]))).toEqual([f("api", "src/a/")]);
  });

  it("reads workspace artifact tags beside project tags", () => {
    const known = new Set(["_artifacts/guides/style.md", "web/src/a.ts"]);
    expect(filesInText("Use @_artifacts/guides/style.md, then @web/src/a.ts.", ["web", ARTIFACTS_ROOT], known)).toEqual(
      [f(ARTIFACTS_ROOT, "guides/style.md"), f("web", "src/a.ts")],
    );
  });
});
