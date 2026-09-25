import type { CSSProperties, KeyboardEvent, MouseEvent, ReactElement, ReactNode, SyntheticEvent } from "react";
import { arrowIndex } from "../../focus";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface TreeItemProps {
  label: ReactNode;
  /** Lucide icon name, or a node (e.g. a StatusDot). */
  icon?: IconName | ReactElement | null;
  /** Nesting level; each level indents 14px. */
  depth?: number;
  /** Pass true/false to render a disclosure chevron. Omit for leaves. */
  expanded?: boolean;
  selected?: boolean;
  /** Right-aligned mono metadata: cost, count, relative time. */
  meta?: ReactNode;
  trailing?: ReactNode;
  /** Mouse click, or Enter/Space on the focused row. */
  onClick?: (e: SyntheticEvent) => void;
  /** Chevron click, ArrowRight on a collapsed row, or ArrowLeft on an expanded row. */
  onToggle?: (e: SyntheticEvent) => void;
  title?: string;
  style?: CSSProperties;
}

/** Arrow Up/Down move focus to the neighbouring row within the enclosing tree, or the enclosing sections' parent. */
function moveFocus(from: HTMLElement, key: string) {
  const scope = from.closest('[role="tree"]') ?? from.closest('[role="group"]')?.parentElement ?? from.parentElement;
  if (!scope) return false;
  const rows = Array.from(scope.querySelectorAll<HTMLElement>('[role="treeitem"]'));
  const next = arrowIndex(key, rows.indexOf(from), rows.length, "vertical", false);
  if (next === null) return false;
  rows[next]?.focus();
  return true;
}

/** One row of the resource sidebar. Indent by depth; pass expanded to show a chevron. */
export function TreeItem({
  label,
  icon,
  depth = 0,
  expanded,
  selected,
  meta,
  trailing,
  onClick,
  onToggle,
  title,
  style,
}: TreeItemProps) {
  const hasChev = expanded !== undefined;
  const activate = (e: SyntheticEvent) => {
    if (hasChev && onToggle && !onClick) onToggle(e);
    else onClick?.(e);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      activate(e);
    } else if (e.key === "ArrowRight" && hasChev && !expanded && onToggle) {
      e.preventDefault();
      onToggle(e);
    } else if (e.key === "ArrowLeft" && hasChev && expanded && onToggle) {
      e.preventDefault();
      onToggle(e);
    } else if (moveFocus(e.currentTarget, e.key)) {
      e.preventDefault();
    }
  };
  return (
    <div
      role="treeitem"
      aria-selected={!!selected}
      aria-expanded={hasChev ? !!expanded : undefined}
      aria-level={depth + 1}
      tabIndex={0}
      className={`os-tree-item ${selected ? "os-tree-item--selected" : ""}`}
      style={{ paddingLeft: 6 + depth * 14, ...style }}
      onClick={(e: MouseEvent) => activate(e)}
      onKeyDown={onKeyDown}
      title={title ?? (typeof label === "string" ? label : undefined)}
    >
      <span
        className={`os-tree-item__chev ${expanded ? "os-tree-item__chev--open" : ""}`}
        onClick={
          hasChev && onToggle && onClick
            ? (e) => {
                e.stopPropagation();
                onToggle(e);
              }
            : undefined
        }
      >
        {hasChev && <Icon name="chevron-right" size={12} />}
      </span>
      {icon && (typeof icon === "string" ? <Icon name={icon} size={14} className="os-tree-item__icon" /> : icon)}
      <span className="os-tree-item__label">{label}</span>
      {meta != null && <span className="os-tree-item__meta">{meta}</span>}
      {trailing}
    </div>
  );
}
