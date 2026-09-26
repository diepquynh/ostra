import type { ReactNode } from "react";
import { cx } from "../../cx";

export interface TableColumn<T = object> {
  key: string;
  label: ReactNode;
  /** Right-aligned tabular mono numbers (cost, tokens, durations). */
  num?: boolean;
  width?: number | string;
  render?: (row: T) => ReactNode;
}

export interface TableProps<T = object> {
  columns: TableColumn<T>[];
  rows: T[];
  /** Makes rows clickable; Enter on a focused row also opens it. */
  onRowClick?: (row: T) => void;
  selectedKey?: string | number;
  rowKey?: string;
  dense?: boolean;
  empty?: ReactNode;
}

const cell = (row: object, key: string): ReactNode => (row as Record<string, unknown>)[key] as ReactNode;

/** Dense data table. columns: [{ key, label, num?, width?, render? }]. */
export function Table<T extends object>({
  columns,
  rows,
  onRowClick,
  selectedKey,
  rowKey = "id",
  dense,
  empty = "Nothing yet.",
}: TableProps<T>) {
  return (
    <div className="os-table-wrap">
      <table className={`os-table ${dense ? "os-table--dense" : ""}`}>
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c.key} className={c.num ? "os-num" : undefined} style={{ width: c.width }}>
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td colSpan={columns.length} style={{ textAlign: "center", color: "var(--text-muted)", padding: 20 }}>
                {empty}
              </td>
            </tr>
          )}
          {rows.map((r, i) => {
            const k = (cell(r, rowKey) as string | number | undefined) ?? i;
            const selected = selectedKey === k;
            const cls = cx(onRowClick && "os-row--click", selected && "os-row--selected");
            return (
              <tr
                key={k}
                className={cls || undefined}
                tabIndex={onRowClick ? 0 : undefined}
                onClick={onRowClick ? () => onRowClick(r) : undefined}
                onKeyDown={
                  onRowClick
                    ? (e) => {
                        if (e.key === "Enter" && e.target === e.currentTarget) onRowClick(r);
                      }
                    : undefined
                }
              >
                {columns.map((c) => (
                  <td key={c.key} className={c.num ? "os-num" : undefined}>
                    {c.render ? c.render(r) : cell(r, c.key)}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
