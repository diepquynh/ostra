import { Fragment, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { useFocusTrap } from "../../focus";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";
import { Kbd } from "../core/Kbd";

export interface PaletteItem {
  id: string;
  label: string;
  /** Group header, e.g. "Sessions", "Executions", "Artifacts", "Actions". Items should be pre-sorted by group. */
  group?: string;
  /** Lucide icon name. */
  icon?: IconName;
  /** Right-aligned mono hint: a path, a status, a shortcut. */
  hint?: string;
}

export interface CommandPaletteProps {
  open: boolean;
  items: PaletteItem[];
  onSelect?: (item: PaletteItem) => void;
  onClose?: () => void;
  placeholder?: string;
  /** Called with the query on every keystroke, for callers that search a server. */
  onQueryChange?: (query: string) => void;
  /** Replaces the built-in label/hint filter, for callers that pass already-matched items. */
  filterItems?: (items: PaletteItem[], query: string) => PaletteItem[];
}

/** Items whose label, hint or group contains the query, case-insensitively, in their original order. */
export function filterPaletteItems(items: PaletteItem[], query: string): PaletteItem[] {
  const n = query.trim().toLowerCase();
  if (!n) return items;
  return items.filter((it) => `${it.label} ${it.hint ?? ""} ${it.group ?? ""}`.toLowerCase().includes(n));
}

/**
 * Resource switcher (⌘K). Controlled by open/onClose; items are grouped by item.group. Arrow keys move the
 * highlight, Enter opens the highlighted item, Escape or a scrim click closes, and focus returns to where it was.
 */
export function CommandPalette(props: CommandPaletteProps) {
  if (!props.open) return null;
  return createPortal(<PaletteBody {...props} />, document.body);
}

function PaletteBody({ items, onSelect, onClose, onQueryChange, filterItems = filterPaletteItems, placeholder = "Go to a session, execution, artifact or setting…" }: CommandPaletteProps) {
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const panelRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const trapTab = useFocusTrap(panelRef, true, inputRef);
  const list = useMemo(() => filterItems(items, q), [q, items, filterItems]);
  const active = Math.min(i, list.length - 1);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  const pick = (it: PaletteItem) => {
    onSelect?.(it);
    onClose?.();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setI(Math.min(active + 1, list.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setI(Math.max(active - 1, 0));
    } else if (e.key === "Enter" && list[active]) {
      e.preventDefault();
      pick(list[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose?.();
    } else {
      trapTab(e);
    }
  };

  let lastGroup: string | undefined;
  return (
    <div
      onClick={onClose}
      style={{ position: "fixed", inset: 0, background: "var(--surface-scrim)", display: "flex", justifyContent: "center", alignItems: "flex-start", paddingTop: "12vh", zIndex: 100 }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal
        aria-label="Command palette"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKey}
        style={{ width: 560, maxWidth: "calc(100vw - 32px)", background: "var(--surface-overlay)", borderRadius: "var(--radius-lg)", boxShadow: "var(--shadow-dialog)", overflow: "hidden", animation: "os-fade-in var(--dur-base) var(--ease-out)" }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, height: 44, padding: "0 14px", borderBottom: "1px solid var(--border-default)" }}>
          <Icon name="search" size={15} style={{ color: "var(--text-muted)" }} />
          <input
            ref={inputRef}
            value={q}
            onChange={(e) => {
              setQ(e.target.value);
              setI(0);
              onQueryChange?.(e.target.value);
            }}
            placeholder={placeholder}
            role="combobox"
            aria-expanded
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={list[active] ? `${listId}-${active}` : undefined}
            spellCheck={false}
            autoComplete="off"
            style={{ flex: 1, background: "none", border: 0, outline: 0, boxShadow: "none", color: "var(--text-primary)", font: "var(--weight-regular) var(--text-md)/1 var(--font-sans)" }}
          />
          <Kbd>Esc</Kbd>
        </div>
        <div ref={listRef} id={listId} role="listbox" aria-label="Results" style={{ maxHeight: 360, overflowY: "auto", overflowX: "hidden", padding: 4 }}>
          {list.length === 0 && <div style={{ padding: "18px 12px", color: "var(--text-muted)", textAlign: "center" }}>Nothing matches “{q}”.</div>}
          {list.map((it, idx) => {
            const head = it.group && it.group !== lastGroup ? it.group : null;
            lastGroup = it.group;
            const on = idx === active;
            return (
              <Fragment key={it.id}>
                {head && (
                  <div className="os-tree-section" role="presentation" style={{ paddingTop: 6 }}>
                    {head}
                  </div>
                )}
                <div
                  id={`${listId}-${idx}`}
                  data-index={idx}
                  role="option"
                  aria-selected={on}
                  onMouseEnter={() => setI(idx)}
                  onClick={() => pick(it)}
                  style={{ display: "flex", alignItems: "center", gap: 10, height: 32, padding: "0 10px", borderRadius: "var(--radius-sm)", cursor: "pointer", background: on ? "var(--surface-selected)" : "transparent" }}
                >
                  {it.icon && <Icon name={it.icon} size={14} style={{ color: on ? "var(--accent-fg)" : "var(--text-muted)" }} />}
                  <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--text-primary)" }}>{it.label}</span>
                  {it.hint && <span style={{ font: "var(--text-2xs)/1 var(--font-mono)", color: "var(--text-muted)" }}>{it.hint}</span>}
                </div>
              </Fragment>
            );
          })}
        </div>
        <div style={{ display: "flex", gap: 14, alignItems: "center", height: 30, padding: "0 12px", borderTop: "1px solid var(--border-subtle)", color: "var(--text-muted)", fontSize: "var(--text-xs)" }}>
          <span style={{ display: "flex", gap: 5, alignItems: "center" }}>
            <Kbd keys={["↑", "↓"]} /> move
          </span>
          <span style={{ display: "flex", gap: 5, alignItems: "center" }}>
            <Kbd>↵</Kbd> open
          </span>
        </div>
      </div>
    </div>
  );
}
