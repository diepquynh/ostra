// Supertype and implementation jumps and Ctrl/Cmd+click for the file editor, kept free of Monaco so they test
// without it.

import type { MenuItem } from "@ostra/design";
import { socket } from "../../../api/socket";
import type { CodeLocation, CodeNavigation, CodeUsages, NavigateTarget } from "../../../api/types";

export type JumpWhere = { workspace: string; key: string; path: string };
export type JumpReply = { result: CodeNavigation | null; error: string | null };

/**
 * Ask for the supertypes or implementations of the name at `line` (1-based) and `col` (0-based UTF-16) of `text`.
 * `result` is null when neither the code index nor a language server knows the name; the reply is null when the
 * socket is closed or a newer jump replaced this one.
 */
export async function askJump(
  where: JumpWhere,
  text: string,
  line: number,
  col: number,
  target: NavigateTarget,
): Promise<JumpReply | null> {
  const reply = await socket().hint("code_navigate", { ...where, text, line, col, target });
  return reply ? { result: reply.result, error: reply.error } : null;
}

export const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

export const JUMP_LABEL: Record<NavigateTarget, string> = {
  supertypes: "Show base class",
  implementations: "Show implementation",
};

export const jumpHeading = (nav: CodeNavigation, target: NavigateTarget) =>
  `${target === "supertypes" ? `What ${nav.symbol} extends` : `What implements ${nav.symbol}`}${nav.truncated ? ` (first ${nav.locations.length})` : ""}`;

export const emptyJumpNote = (nav: CodeNavigation, target: NavigateTarget) =>
  target === "supertypes"
    ? `${nav.symbol} extends or implements nothing that the project defines.`
    : `Nothing in the project extends or implements ${nav.symbol}.`;

/** The picker's rows: a heading, then one row per location. */
export function locationItems(title: string, locs: CodeLocation[], pick: (l: CodeLocation) => void): MenuItem[] {
  return [
    { type: "heading", label: title },
    ...locs.map((l) => ({
      id: `${l.path}:${l.line}:${l.col}`,
      label: `${l.container ? `${l.container}.` : ""}${l.name} · ${l.path}:${l.line}`,
      sub: l.preview || undefined,
      icon: "braces" as const,
      onSelect: () => pick(l),
    })),
  ];
}

/** Where a jump leads: one place, a list to pick from, a note to show, or nowhere. */
export type Landing =
  | { go: CodeLocation }
  | { pick: { title: string; locs: CodeLocation[] } }
  | { note: string }
  | null;

export function jumpLanding(nav: CodeNavigation | null, target: NavigateTarget): Landing {
  if (!nav) return null;
  const [first, ...rest] = nav.locations;
  if (!first) return { note: emptyJumpNote(nav, target) };
  return rest.length ? { pick: { title: jumpHeading(nav, target), locs: nav.locations } } : { go: first };
}

/** The Cmd key on macOS, Ctrl elsewhere, held during a click. */
export const linkModifier = (e: Pick<MouseEvent, "ctrlKey" | "metaKey">) => (IS_MAC ? e.metaKey : e.ctrlKey);

/**
 * Ctrl/Cmd+click on the name at `line`/`col` of `path`: on a use it goes to the definition (a list when the name has
 * several); on a definition it lists the usages, or goes to the only one. A name nothing defines leads nowhere.
 */
export function clickLanding(u: CodeUsages, path: string, line: number, col: number, uri?: string | null): Landing {
  // A click in a dependency file (`uri`) is on a definition only when the definition is in that same file.
  const here = (d: CodeLocation) => (uri ? d.uri === uri : !d.uri && d.path === path);
  const onDef = u.definitions.some((d) => here(d) && d.line === line && col >= d.col && col <= d.col + d.len);
  if (onDef) {
    const [first, ...rest] = u.references;
    if (!first) return { note: `Nothing in the project uses ${u.symbol}.` };
    if (!rest.length) return { go: first };
    const n = `${u.references.length}${u.truncated ? "+" : ""}`;
    return { pick: { title: `Usages of ${u.symbol} (${n})`, locs: u.references } };
  }
  const [first, ...rest] = u.definitions;
  if (!first) return null;
  return rest.length ? { pick: { title: `Definitions of ${u.symbol}`, locs: u.definitions } } : { go: first };
}
