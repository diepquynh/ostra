import { Fragment, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { cx } from "../../cx";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";
import { Spinner } from "../feedback/Spinner";

export interface ComboItem {
  id: string;
  label: ReactNode;
  /** Second line, e.g. the file a symbol is defined in. */
  sub?: ReactNode;
  icon?: IconName;
  /** Color of the icon, e.g. a kind color. */
  iconColor?: string;
  /** Right-aligned mono hint. */
  hint?: string;
  /** Group header. Items should be pre-sorted by group. */
  group?: string;
}

export interface ComboboxProps {
  value: string;
  onChange: (query: string) => void;
  items: ComboItem[];
  onSelect: (item: ComboItem) => void;
  /** Enter with no highlighted item. */
  onSubmit?: (query: string) => void;
  placeholder?: string;
  label?: string;
  icon?: IconName;
  mono?: boolean;
  size?: "sm" | "md";
  loading?: boolean;
  /** Shown in the list when a non-empty query matches nothing. */
  empty?: string;
  width?: number | string;
  /** Width of the list, when wider than the field. */
  listWidth?: number | string;
  style?: CSSProperties;
}

/**
 * Text field with a list of suggestions below it. Arrow keys move the highlight, Enter picks the
 * highlighted item, Escape closes the list. The caller filters `items` for the query.
 */
export function Combobox({ value, onChange, items, onSelect, onSubmit, placeholder, label, icon = "search", mono, size = "md", loading, empty = "No matches", width, listWidth, style }: ComboboxProps) {
  const [open, setOpen] = useState(false);
  const [i, setI] = useState(0);
  const listId = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const active = items.length ? Math.min(i, items.length - 1) : -1;
  const show = open && value.trim().length > 0 && (items.length > 0 || !loading);

  useEffect(() => setI(0), [value]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  const pick = (it: ComboItem) => {
    onSelect(it);
    setOpen(false);
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setI(Math.min(active + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setI(Math.max(active - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (show && items[active]) pick(items[active]);
      else onSubmit?.(value);
    } else if (e.key === "Escape" && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <div className="os-combobox" style={{ width, ...style }}>
      <div className={cx("os-input", mono && "os-input--mono", size === "sm" && "os-input--sm")}>
        <Icon name={icon} size={14} className="os-input__icon" />
        <input
          role="combobox"
          aria-label={label ?? placeholder}
          aria-expanded={show}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={show && active >= 0 ? `${listId}-${active}` : undefined}
          autoComplete="off"
          spellCheck={false}
          placeholder={placeholder}
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={onKey}
        />
        {loading && <Spinner size={11} />}
      </div>
      {show && (
        <div ref={listRef} id={listId} role="listbox" className="os-menu os-combobox__list" style={{ width: listWidth ?? "100%" }} onMouseDown={(e) => e.preventDefault()}>
          {items.length === 0 && <div className="os-combobox__empty">{empty}</div>}
          {items.map((it, n) => (
            <Fragment key={it.id}>
              {it.group && it.group !== items[n - 1]?.group && (
                <div className="os-menu__heading" role="presentation">
                  {it.group}
                </div>
              )}
              <div
                id={`${listId}-${n}`}
                data-index={n}
                role="option"
                aria-selected={n === active}
                className={cx("os-menu__item", it.sub != null && "os-menu__item--tall", n === active && "os-menu__item--active")}
                onMouseMove={() => n !== active && setI(n)}
                onClick={() => pick(it)}
              >
                {it.icon && <Icon name={it.icon} size={14} className="os-menu__icon" style={{ color: it.iconColor, marginTop: it.sub ? 2 : undefined }} />}
                <span className="os-menu__label">
                  {it.label}
                  {it.sub && <span className="os-menu__sub">{it.sub}</span>}
                </span>
                {it.hint && <span className="os-menu__hint">{it.hint}</span>}
              </div>
            </Fragment>
          ))}
        </div>
      )}
    </div>
  );
}
