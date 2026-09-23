import { useEffect, useRef, type CSSProperties, type KeyboardEvent, type ReactElement, type ReactNode } from "react";
import { arrowIndex } from "../../focus";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";
import { Kbd } from "../core/Kbd";

export interface MenuActionItem {
  type?: "item";
  id?: string;
  label: ReactNode;
  sub?: ReactNode;
  icon?: IconName | ReactElement;
  hint?: string;
  kbd?: string[];
  checked?: boolean;
  danger?: boolean;
  /** Nest under the previous item (e.g. pages of the active workspace). */
  indent?: number;
  /** Current page among indented items. */
  active?: boolean;
  onSelect?: (item: MenuActionItem) => void;
}

export type MenuItem = MenuActionItem | { type: "divider" } | { type: "heading"; label: string };

export interface MenuProps {
  open: boolean;
  onClose?: () => void;
  items: MenuItem[];
  /** Which edge of the trigger the menu aligns to. */
  align?: "left" | "right";
  width?: number | string;
  /** Accessible name for the menu. */
  label?: string;
  style?: CSSProperties;
}

const isAction = (it: MenuItem): it is MenuActionItem => it.type === undefined || it.type === "item";

/**
 * Dropdown menu. Place inside a position:relative wrapper next to its trigger. Focus moves to the first item on
 * open; arrows, Home and End move it, Enter or Space selects, Escape and Tab close, and focus returns to the trigger.
 */
export function Menu({ open, onClose, items, align = "left", width, label, style }: MenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    menuRef.current?.querySelector<HTMLElement>('[role^="menuitem"]')?.focus({ preventScroll: true });
    return () => {
      // Restore only when focus fell to the body with the removed menu, not when the user moved it elsewhere.
      const lost = !document.activeElement || document.activeElement === document.body;
      if (lost && previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [open]);

  if (!open) return null;

  const select = (it: MenuActionItem) => {
    it.onSelect?.(it);
    onClose?.();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose?.();
      return;
    }
    if (e.key === "Tab") {
      onClose?.();
      return;
    }
    const rows = Array.from(menuRef.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? []);
    const next = arrowIndex(e.key, rows.indexOf(document.activeElement as HTMLElement), rows.length, "vertical");
    if (next === null) return;
    e.preventDefault();
    rows[next]?.focus();
  };

  return (
    <>
      <div className="os-menu-scrim" onClick={onClose} />
      <div ref={menuRef} className="os-menu" role="menu" aria-label={label} style={{ [align]: 0, width, ...style }} onKeyDown={onKeyDown}>
        {items.map((it, i) => {
          if (it.type === "divider") return <div key={i} className="os-menu__divider" role="separator" />;
          if (it.type === "heading")
            return (
              <div key={i} className="os-menu__heading" role="presentation">
                {it.label}
              </div>
            );
          if (!isAction(it)) return null;
          return (
            <div
              key={it.id ?? i}
              role={it.checked !== undefined ? "menuitemradio" : "menuitem"}
              aria-checked={it.checked !== undefined ? it.checked : undefined}
              aria-current={it.active ? "page" : undefined}
              tabIndex={-1}
              className={`os-menu__item ${it.danger ? "os-menu__item--danger" : ""} ${it.sub ? "os-menu__item--tall" : ""}`}
              style={
                it.indent
                  ? {
                      paddingLeft: 8 + it.indent * 22,
                      height: 28,
                      color: it.active ? "var(--text-primary)" : "var(--text-secondary)",
                      background: it.active ? "var(--surface-active)" : undefined,
                    }
                  : undefined
              }
              onClick={() => select(it)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  select(it);
                }
              }}
            >
              {it.icon &&
                (typeof it.icon === "string" ? (
                  <Icon name={it.icon} size={14} className="os-menu__icon" style={it.sub ? { marginTop: 2 } : undefined} />
                ) : (
                  it.icon
                ))}
              <span className="os-menu__label">
                {it.label}
                {it.sub && <span className="os-menu__sub">{it.sub}</span>}
              </span>
              {it.checked && <Icon name="check" size={14} style={{ color: "var(--accent-fg)" }} />}
              {it.hint && <span className="os-menu__hint">{it.hint}</span>}
              {it.kbd && <Kbd keys={it.kbd} />}
            </div>
          );
        })}
      </div>
    </>
  );
}
