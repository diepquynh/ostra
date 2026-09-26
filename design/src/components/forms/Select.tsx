import { type Ref, type SelectHTMLAttributes, useId } from "react";
import { cx } from "../../cx";
import { Icon } from "../core/Icon";

export interface SelectOption {
  value: string;
  label: string;
}

export interface SelectProps extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "size"> {
  label?: string;
  hint?: string;
  options: (string | SelectOption)[];
  size?: "sm" | "md";
  mono?: boolean;
  ref?: Ref<HTMLSelectElement>;
}

export function Select({ label, hint, options, size = "md", mono, style, className = "", ...rest }: SelectProps) {
  const hintId = useId();
  const cls = cx("os-input", "os-select", size === "sm" && "os-input--sm", mono && "os-input--mono", className);
  const control = (
    <div className={cls} style={style}>
      <select
        aria-describedby={hint ? hintId : undefined}
        {...rest}
        style={mono ? { fontFamily: "var(--font-mono)", fontSize: "var(--text-code)" } : undefined}
      >
        {options.map((o) => {
          const v = typeof o === "string" ? { value: o, label: o } : o;
          return (
            <option key={v.value} value={v.value}>
              {v.label}
            </option>
          );
        })}
      </select>
      <Icon name="chevron-down" size={13} className="os-select__chev" />
    </div>
  );
  if (!label && !hint) return control;
  return (
    <label className="os-field">
      {label && <span className="os-field__label">{label}</span>}
      {control}
      {hint && (
        <span id={hintId} className="os-field__hint">
          {hint}
        </span>
      )}
    </label>
  );
}
