import { describe, expect, it } from "vitest";
import { decodeTokens, lineFromHash, segments, splitLines } from "./tokens";

describe("decodeTokens", () => {
  it("groups runs by line", () => {
    const spans = decodeTokens(
      {
        classes: ["keyword", "name"],
        tokens: [1, 0, 2, 0, 1, 3, 4, 1, 3, 2, 1, 1],
      },
      3,
    );
    expect(spans).toEqual([
      [
        { col: 0, len: 2, cls: "keyword" },
        { col: 3, len: 4, cls: "name" },
      ],
      [],
      [{ col: 2, len: 1, cls: "name" }],
    ]);
  });

  it("drops runs a faulty provider sends", () => {
    const spans = decodeTokens(
      {
        classes: ["name"],
        tokens: [1, 0, 4, 0, 1, 2, 2, 0, 9, 0, 1, 0, 1, 0, 1, 7, 2, 0, 5, 0, 0, 1, 1, 0],
      },
      1,
    );
    expect(spans).toEqual([[{ col: 0, len: 4, cls: "name" }]]);
  });
});

describe("segments", () => {
  it("fills the text between tokens", () => {
    expect(
      segments("fn main() {}", [
        { col: 0, len: 2, cls: "keyword" },
        { col: 3, len: 4, cls: "function" },
      ]),
    ).toEqual([
      { text: "fn", col: 0, cls: "keyword" },
      { text: " ", col: 2, cls: null },
      { text: "main", col: 3, cls: "function" },
      { text: "() {}", col: 7, cls: null },
    ]);
  });

  it("clips tokens past the end of the line", () => {
    expect(
      segments("ab", [
        { col: 1, len: 5, cls: "name" },
        { col: 4, len: 1, cls: "name" },
      ]),
    ).toEqual([
      { text: "a", col: 0, cls: null },
      { text: "b", col: 1, cls: "name" },
    ]);
  });

  it("returns plain text without tokens", () => {
    expect(segments("x = 1", undefined)).toEqual([{ text: "x = 1", col: 0, cls: null }]);
    expect(segments("", [])).toEqual([]);
  });
});

describe("helpers", () => {
  it("splits lines like CodeView", () => {
    expect(splitLines("a\nb\n")).toEqual(["a", "b"]);
  });

  it("reads line anchors", () => {
    expect(lineFromHash("#L12")).toBe(12);
    expect(lineFromHash("L3")).toBe(3);
    expect(lineFromHash("#L0")).toBeNull();
    expect(lineFromHash("#gate-1")).toBeNull();
  });
});
