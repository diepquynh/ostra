import { Fragment } from "react";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface Crumb {
  label: string;
  icon?: IconName;
  to?: string;
}

export interface BreadcrumbsProps {
  items: Crumb[];
  onNavigate?: (crumb: Crumb, index: number) => void;
}

/** Path of the open resource, shown in the title bar. Every crumb but the last navigates. */
export function Breadcrumbs({ items, onNavigate }: BreadcrumbsProps) {
  return (
    <nav className="os-crumbs" aria-label="Breadcrumb">
      {items.map((c, i) => {
        const last = i === items.length - 1;
        const go = !last && onNavigate ? () => onNavigate(c, i) : undefined;
        return (
          <Fragment key={i}>
            {i > 0 && <Icon name="chevron-right" size={12} className="os-crumbs__sep" />}
            <span
              className={`os-crumbs__item ${last ? "os-crumbs__item--current" : ""}`}
              aria-current={last ? "page" : undefined}
              role={go ? "link" : undefined}
              tabIndex={go ? 0 : undefined}
              onClick={go}
              onKeyDown={
                go
                  ? (e) => {
                      if (e.key === "Enter") go();
                    }
                  : undefined
              }
            >
              {c.icon && <Icon name={c.icon} size={13} />}
              {c.label}
            </span>
          </Fragment>
        );
      })}
    </nav>
  );
}
