import { describe, expect, it } from "vitest";
import { cleanName, moveDestinations, parseProjectHash, projectHash } from "./projectNav";

describe("project hash", () => {
  it("reads the tab and the folder", () => {
    expect(parseProjectHash("")).toEqual({ tab: "overview", dir: "" });
    expect(parseProjectHash("#git")).toEqual({ tab: "git", dir: "" });
    expect(parseProjectHash("#files")).toEqual({ tab: "files", dir: "" });
    expect(parseProjectHash("#dir=src%2Forders%20v2")).toEqual({ tab: "files", dir: "src/orders v2" });
    expect(parseProjectHash("#dir=/src/")).toEqual({ tab: "files", dir: "src" });
    expect(parseProjectHash("#gate-1")).toEqual({ tab: "overview", dir: "" });
  });

  it("round-trips", () => {
    for (const v of [
      { tab: "overview", dir: "" },
      { tab: "git", dir: "" },
      { tab: "files", dir: "" },
      { tab: "files", dir: "a/b c#d" },
    ] as const)
      expect(parseProjectHash(projectHash(v))).toEqual(v);
  });

  it("keeps a malformed escape as typed", () => {
    expect(parseProjectHash("#dir=%E0%A4%A")).toEqual({ tab: "files", dir: "%E0%A4%A" });
  });
});

describe("cleanName", () => {
  it("trims spaces and slashes and folds repeated slashes", () => {
    expect(cleanName("  /docs//drafts/ ")).toBe("docs/drafts");
    expect(cleanName(" / ")).toBeNull();
  });
});

describe("moveDestinations", () => {
  const files = ["a.txt", "docs/x.md", "docs/deep/y.md", "samples/z.csv"];

  it("lists every folder but the current one, the item itself and its contents", () => {
    expect(moveDestinations("docs/x.md", files, [])).toEqual(["", "docs/deep", "samples"]);
    expect(moveDestinations("docs", files, ["empty"])).toEqual(["empty", "samples"]);
    expect(moveDestinations("a.txt", files, [])).toEqual(["docs", "docs/deep", "samples"]);
  });
});
