import { type CSSProperties, type KeyboardEvent, type MouseEvent, type ReactNode, useRef } from "react";
import { arrowIndex } from "../../focus";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface TabItem {
  id: string;
  label: string;
  /** Lucide icon name. */
  icon?: IconName | null;
  /** Count shown after the label (open gates, findings). */
  count?: number;
  /** Leading node, typically a StatusDot. */
  dot?: ReactNode;
  /** Italic label = preview tab (replaced on next open, like an editor preview tab). */
  italic?: boolean;
  /** Bar variant: a pin button replaces the close button and unpins on click. */
  pinned?: boolean;
  title?: string;
}

export interface TabsProps {
  tabs: TabItem[];
  value: string;
  onChange?: (id: string) => void;
  /** Bar variant only: shows a close affordance on hover/active. Delete also closes the focused tab. */
  onClose?: (id: string) => void;
  /** Bar variant only: the pin button on a pinned tab. */
  onUnpin?: (id: string) => void;
  /** Right-click or the context-menu key on a tab. */
  onContextMenu?: (id: string, e: MouseEvent) => void;
  variant?: "bar" | "underline" | "segmented";
  /** Accessible name for the tab list. */
  label?: string;
  className?: string;
  style?: CSSProperties;
}

/**
 * Tab strip. variant "bar" = editor tabs (closable, with icons); "underline" = in-page sections; "segmented" = compact toggles.
 * Controlled by value/onChange. Arrow keys, Home and End move between tabs and select them.
 */
export function Tabs({
  tabs,
  value,
  onChange,
  onClose,
  onUnpin,
  onContextMenu,
  variant = "underline",
  label,
  className = "",
  style,
}: TabsProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const selectedIndex = tabs.findIndex((t) => t.id === value);
  const focusIndex = selectedIndex >= 0 ? selectedIndex : 0;

  const onKeyDown = (e: KeyboardEvent, i: number) => {
    const t = tabs[i];
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onChange?.(t.id);
      return;
    }
    if (e.key === "Delete" && variant === "bar" && onClose) {
      e.preventDefault();
      onClose(t.id);
      return;
    }
    const next = arrowIndex(e.key, i, tabs.length, "horizontal");
    if (next === null) return;
    e.preventDefault();
    listRef.current?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
    onChange?.(tabs[next].id);
  };

  return (
    <div
      ref={listRef}
      className={`os-tabs os-tabs--${variant} ${className}`}
      role="tablist"
      aria-label={label}
      style={style}
    >
      {tabs.map((t, i) => {
        const active = t.id === value;
        return (
          <div
            key={t.id}
            role="tab"
            aria-selected={active}
            tabIndex={i === focusIndex ? 0 : -1}
            className={`os-tab ${active ? "os-tab--active" : ""}`}
            onClick={() => onChange?.(t.id)}
            onKeyDown={(e) => onKeyDown(e, i)}
            onContextMenu={
              onContextMenu
                ? (e) => {
                    e.preventDefault();
                    onContextMenu(t.id, e);
                  }
                : undefined
            }
            title={t.title}
          >
            {t.dot}
            {t.icon && <Icon name={t.icon} size={14} style={{ color: active ? "var(--text-secondary)" : undefined }} />}
            <span style={t.italic ? { fontStyle: "italic" } : undefined}>{t.label}</span>
            {t.count != null && <span className="os-tab__count">{t.count}</span>}
            {variant === "bar" && t.pinned && onUnpin ? (
              <span
                className="os-tab__close os-tab__pin"
                role="button"
                aria-label={`Unpin ${t.label}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onUnpin(t.id);
                }}
              >
                <Icon name="pin" size={12} />
              </span>
            ) : (
              variant === "bar" &&
              onClose && (
                <span
                  className="os-tab__close"
                  role="button"
                  aria-label={`Close ${t.label}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onClose(t.id);
                  }}
                >
                  <Icon name="x" size={12} />
                </span>
              )
            )}
          </div>
        );
      })}
    </div>
  );
}
