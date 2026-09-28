import {
  type CSSProperties,
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
  type WheelEvent,
} from "react";
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
  /**
   * Bar variant only: reorder. Tabs become draggable, and Ctrl+Shift+Left or Right moves the focused tab. `to` is the
   * index the tab should end up at.
   */
  onMove?: (id: string, to: number) => void;
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
/**
 * Scrolls a sideways strip so its selected tab is in view. It moves the strip only: scrollIntoView would also scroll
 * every ancestor, including a page that frames the console, such as the homepage.
 */
export function revealSelectedTab(strip: HTMLElement) {
  const tab = strip.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
  if (!tab) return;
  const s = strip.getBoundingClientRect();
  const t = tab.getBoundingClientRect();
  if (t.left < s.left) strip.scrollLeft -= s.left - t.left;
  else if (t.right > s.right) strip.scrollLeft += t.right - s.right;
}

export function Tabs({
  tabs,
  value,
  onChange,
  onClose,
  onUnpin,
  onContextMenu,
  onMove,
  variant = "underline",
  label,
  className = "",
  style,
}: TabsProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<{ index: number; after: boolean } | null>(null);
  const movable = variant === "bar" && !!onMove;
  const selectedIndex = tabs.findIndex((t) => t.id === value);

  const focusTab = (id: string) =>
    requestAnimationFrame(() =>
      Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])
        .find((el) => el.dataset.id === id)
        ?.focus(),
    );

  const dropAt = (e: DragEvent, i: number) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return { index: i, after: e.clientX > r.left + r.width / 2 };
  };

  const endDrag = () => {
    setDragging(null);
    setDrop(null);
  };
  const focusIndex = selectedIndex >= 0 ? selectedIndex : 0;

  // A strip wider than its box scrolls sideways; keep the selected tab in view.
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the selection changes.
  useEffect(() => {
    if (listRef.current) revealSelectedTab(listRef.current);
  }, [value]);

  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    if (el.scrollWidth <= el.clientWidth || Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
    el.scrollLeft += e.deltaY;
  };

  const onKeyDown = (e: KeyboardEvent, i: number) => {
    const t = tabs[i];
    if (movable && (e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      onMove?.(t.id, i + (e.key === "ArrowLeft" ? -1 : 1));
      focusTab(t.id);
      return;
    }
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
      onWheel={variant === "segmented" ? undefined : onWheel}
    >
      {tabs.map((t, i) => {
        const active = t.id === value;
        return (
          <div
            key={t.id}
            role="tab"
            data-id={t.id}
            aria-selected={active}
            tabIndex={i === focusIndex ? 0 : -1}
            className={[
              "os-tab",
              active && "os-tab--active",
              dragging === t.id && "os-tab--dragging",
              drop?.index === i && dragging !== t.id && (drop.after ? "os-tab--drop-after" : "os-tab--drop-before"),
            ]
              .filter(Boolean)
              .join(" ")}
            draggable={movable || undefined}
            onDragStart={
              movable
                ? (e) => {
                    e.dataTransfer.effectAllowed = "move";
                    e.dataTransfer.setData("text/plain", t.id);
                    setDragging(t.id);
                  }
                : undefined
            }
            onDragOver={
              movable && dragging
                ? (e) => {
                    e.preventDefault();
                    e.dataTransfer.dropEffect = "move";
                    const next = dropAt(e, i);
                    if (next.index !== drop?.index || next.after !== drop?.after) setDrop(next);
                  }
                : undefined
            }
            onDrop={
              movable && dragging
                ? (e) => {
                    e.preventDefault();
                    const from = tabs.findIndex((x) => x.id === dragging);
                    const { after } = dropAt(e, i);
                    const slot = after ? i + 1 : i;
                    if (from >= 0) onMove?.(dragging, slot > from ? slot - 1 : slot);
                    endDrag();
                  }
                : undefined
            }
            onDragEnd={movable ? endDrag : undefined}
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
