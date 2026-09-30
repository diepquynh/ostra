import { afterEach, describe, expect, it } from "vitest";
import {
  conflicts,
  defaults,
  keysHint,
  matchStroke,
  parseOverrides,
  reservedBy,
  resolve,
  SEQUENCE_MS,
  saveOverrides,
  strokeOf,
  typesText,
} from "./shortcuts";

type Mods = { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean };
const ev = (key: string, mods: Mods = {}, code = "") => ({
  key,
  code,
  metaKey: !!mods.meta,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
});

/** The action one key event runs with the default bindings. */
const action = (key: string, mods: Mods, mac: boolean, claim = false) => {
  const s = strokeOf(ev(key, mods));
  return s ? matchStroke(defaults(mac), null, s, 0, claim).action : null;
};

afterEach(() => localStorage.clear());

describe("strokes", () => {
  it("names a key with its modifiers in a fixed order", () => {
    expect(strokeOf(ev("K", { ctrl: true, shift: true }, "KeyK"))).toBe("ctrl+shift+k");
    expect(strokeOf(ev("k", { meta: true, alt: true, ctrl: true }))).toBe("ctrl+alt+meta+k");
    expect(strokeOf(ev(" ", { ctrl: true }))).toBe("ctrl+Space");
    expect(strokeOf(ev("F5"))).toBe("F5");
  });

  it("uses the physical key when Shift or Alt changes the character", () => {
    expect(strokeOf(ev("˚", { alt: true, meta: true }, "KeyK"))).toBe("alt+meta+k");
    expect(strokeOf(ev("?", { shift: true, ctrl: true }, "Slash"))).toBe("ctrl+shift+/");
    expect(strokeOf(ev("л", { ctrl: true }, "KeyK"))).toBe("ctrl+k");
  });

  it("ignores a lone modifier", () => {
    expect(strokeOf(ev("Shift", { shift: true }))).toBeNull();
    expect(strokeOf(ev("Meta", { meta: true }))).toBeNull();
  });

  it("formats hints per system", () => {
    expect(keysHint(["shift+meta+e"], true)).toBe("⇧⌘E");
    expect(keysHint(["ctrl+shift+e"], false)).toBe("Ctrl+Shift+E");
    expect(keysHint(["ctrl+k", "ctrl+ArrowLeft"], false)).toBe("Ctrl+K Ctrl+←");
  });

  it("tells a stroke that types text from a command", () => {
    expect(typesText("g")).toBe(true);
    expect(typesText("shift+g")).toBe(true);
    expect(typesText("F2")).toBe(false);
    expect(typesText("alt+g")).toBe(false);
  });
});

describe("default shortcuts", () => {
  it("moves between tabs with Ctrl+Tab on every system", () => {
    for (const mac of [true, false]) {
      expect(action("Tab", { ctrl: true }, mac)).toBe("next-tab");
      expect(action("Tab", { ctrl: true, shift: true }, mac)).toBe("prev-tab");
    }
    expect(action("Tab", { meta: true }, true)).toBeNull();
    expect(action("Tab", {}, false)).toBeNull();
  });

  it("uses ⌘ on a Mac", () => {
    expect(action("k", { meta: true }, true)).toBe("palette");
    expect(action("/", { meta: true }, true)).toBe("dock");
    expect(action("b", { meta: true }, true)).toBe("sidebar");
    expect(action("E", { meta: true, shift: true }, true)).toBe("files");
    expect(action("k", { ctrl: true }, true)).toBeNull();
  });

  it("uses Ctrl elsewhere", () => {
    expect(action("K", { ctrl: true }, false)).toBe("palette");
    expect(action("e", { ctrl: true, shift: true }, false)).toBe("files");
    expect(action("k", { meta: true }, false)).toBeNull();
  });

  it("ignores other keys and extra modifiers", () => {
    expect(action("k", {}, true)).toBeNull();
    expect(action("k", { meta: true, alt: true }, true)).toBeNull();
    expect(action("b", { meta: true, shift: true }, true)).toBeNull();
    expect(action("x", { meta: true }, true)).toBeNull();
  });

  it("claims ⌘W only while the browser passes it through", () => {
    expect(action("w", { meta: true }, true)).toBeNull();
    expect(action("w", { meta: true }, true, true)).toBe("close-tab");
    expect(action("W", { ctrl: true }, false, true)).toBe("close-tab");
    expect(action("w", { ctrl: true, shift: true }, false, true)).toBeNull();
  });

  it("claims a rebound close shortcut at all times", () => {
    const b = resolve({ "close-tab": ["alt+w"] }, false);
    expect(matchStroke(b, null, "alt+w", 0).action).toBe("close-tab");
  });
});

describe("custom shortcuts", () => {
  it("replaces or unbinds a default and binds an action that has none", () => {
    const b = resolve({ palette: ["ctrl+p"], sidebar: null, theme: ["F9"] }, false);
    expect(matchStroke(b, null, "ctrl+p", 0).action).toBe("palette");
    expect(matchStroke(b, null, "ctrl+k", 0).action).toBeNull();
    expect(matchStroke(b, null, "ctrl+b", 0)).toEqual({ action: null, pending: null, consume: false });
    expect(matchStroke(b, null, "F9", 0).action).toBe("theme");
  });

  it("runs a two-stroke sequence and drops a stale first stroke", () => {
    const b = resolve({ settings: ["ctrl+k", "ctrl+s"], palette: ["ctrl+p"] }, false);
    const first = matchStroke(b, null, "ctrl+k", 1000);
    expect(first).toEqual({ action: null, pending: { stroke: "ctrl+k", at: 1000 }, consume: true });
    expect(matchStroke(b, first.pending, "ctrl+s", 1200).action).toBe("settings");
    expect(matchStroke(b, first.pending, "ctrl+s", 1000 + SEQUENCE_MS + 1).action).toBeNull();
    expect(matchStroke(b, first.pending, "ctrl+p", 1200).action).toBe("palette");
  });

  it("finds bindings that collide by keys or by a sequence's first stroke", () => {
    const b = resolve({ settings: ["ctrl+j", "ctrl+s"] }, false);
    expect(conflicts(b, "theme", ["ctrl+k"])).toEqual(["palette"]);
    expect(conflicts(b, "theme", ["ctrl+j"])).toEqual(["settings"]);
    expect(conflicts(b, "theme", ["ctrl+k", "ctrl+t"])).toEqual(["palette"]);
    expect(conflicts(b, "palette", ["ctrl+k"])).toEqual([]);
    expect(conflicts(b, "theme", ["ctrl+j", "ctrl+x"])).toEqual([]);
  });

  it("warns about combos the browser or the system keeps", () => {
    expect(reservedBy("ctrl+t", false)).toBe("browser");
    expect(reservedBy("meta+t", true)).toBe("browser");
    expect(reservedBy("ctrl+Tab", true)).toBe("browser");
    expect(reservedBy("meta+Space", true)).toBe("system");
    expect(reservedBy("meta+e", false)).toBe("system");
    expect(reservedBy("alt+F4", false)).toBe("system");
    expect(reservedBy("ctrl+k", false)).toBeNull();
  });

  it("stores overrides per workspace and drops anything malformed", () => {
    saveOverrides("ws_a", { palette: ["ctrl+p"], dock: null });
    expect(parseOverrides(localStorage.getItem("ostra.shortcuts.ws_a"))).toEqual({ palette: ["ctrl+p"], dock: null });
    expect(localStorage.getItem("ostra.shortcuts.ws_b")).toBeNull();
    saveOverrides("ws_a", {});
    expect(localStorage.getItem("ostra.shortcuts.ws_a")).toBeNull();

    const bad = JSON.stringify({
      bindings: { palette: "ctrl+p", nope: ["ctrl+x"], dock: ["a", "b", "c"], sidebar: [], files: [7], theme: ["F9"] },
    });
    expect(parseOverrides(bad)).toEqual({ theme: ["F9"] });
    expect(parseOverrides("{not json")).toEqual({});
    expect(parseOverrides(JSON.stringify({ bindings: { palette: ["meta+shift+k"] } }))).toEqual({});
  });
});
