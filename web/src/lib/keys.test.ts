import { describe, expect, it } from "vitest";
import { shortcutOf } from "./keys";

const key = (k: string, mods: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean } = {}) => ({
  key: k,
  metaKey: !!mods.meta,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
});

describe("console shortcuts", () => {
  it("moves between tabs with Ctrl+Tab on every system", () => {
    for (const mac of [true, false]) {
      expect(shortcutOf(key("Tab", { ctrl: true }), mac)).toBe("next-tab");
      expect(shortcutOf(key("Tab", { ctrl: true, shift: true }), mac)).toBe("prev-tab");
    }
    expect(shortcutOf(key("Tab", { meta: true }), true)).toBeNull();
    expect(shortcutOf(key("Tab"), false)).toBeNull();
  });

  it("uses ⌘ on a Mac", () => {
    expect(shortcutOf(key("k", { meta: true }), true)).toBe("palette");
    expect(shortcutOf(key("/", { meta: true }), true)).toBe("dock");
    expect(shortcutOf(key("b", { meta: true }), true)).toBe("sidebar");
    expect(shortcutOf(key("E", { meta: true, shift: true }), true)).toBe("files");
    expect(shortcutOf(key("k", { ctrl: true }), true)).toBeNull();
  });

  it("uses Ctrl elsewhere", () => {
    expect(shortcutOf(key("K", { ctrl: true }), false)).toBe("palette");
    expect(shortcutOf(key("e", { ctrl: true, shift: true }), false)).toBe("files");
    expect(shortcutOf(key("k", { meta: true }), false)).toBeNull();
  });

  it("ignores other keys and Alt combinations", () => {
    expect(shortcutOf(key("k"), true)).toBeNull();
    expect(shortcutOf(key("k", { meta: true, alt: true }), true)).toBeNull();
    expect(shortcutOf(key("b", { meta: true, shift: true }), true)).toBeNull();
    expect(shortcutOf(key("x", { meta: true }), true)).toBeNull();
  });

  it("claims ⌘W only while the keyboard is locked", () => {
    expect(shortcutOf(key("w", { meta: true }), true)).toBeNull();
    expect(shortcutOf(key("w", { meta: true }), true, true)).toBe("close-tab");
    expect(shortcutOf(key("W", { ctrl: true }), false, true)).toBe("close-tab");
    expect(shortcutOf(key("w", { ctrl: true, shift: true }), false, true)).toBeNull();
  });
});
