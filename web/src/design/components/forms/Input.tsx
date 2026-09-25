import { type InputHTMLAttributes, type ReactNode, type Ref, type TextareaHTMLAttributes, useId } from "react";
import { cx } from "../../cx";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";

export interface InputProps extends Omit<InputHTMLAttributes<HTMLInputElement | HTMLTextAreaElement>, "size"> {
  label?: string;
  hint?: string;
  /** Error text; also turns the border red. */
  error?: string | null;
  /** Leading Lucide icon, e.g. "search". */
  icon?: IconName;
  /** Render a textarea (request text, feedback, facts for a rescue). */
  multiline?: boolean;
  rows?: number;
  /** Monospace text, for paths, patterns, JSON. */
  mono?: boolean;
  size?: "sm" | "md";
  /** Node rendered at the right edge inside the field (a Kbd, a button). */
  trailing?: ReactNode;
  ref?: Ref<HTMLInputElement & HTMLTextAreaElement>;
}

export function Input({
  label,
  hint,
  error,
  icon,
  multiline,
  mono,
  size = "md",
  rows = 3,
  trailing,
  className = "",
  style,
  id,
  ref,
  ...rest
}: InputProps) {
  const autoId = useId();
  const fieldId = id ?? (label ? autoId : undefined);
  const noteId = error || hint ? `${autoId}-note` : undefined;
  const cls = cx(
    "os-input",
    multiline && "os-input--multiline",
    mono && "os-input--mono",
    error && "os-input--invalid",
    size === "sm" && "os-input--sm",
    className,
  );
  const aria = { "aria-invalid": error ? true : undefined, "aria-describedby": noteId };
  const control = (
    <div className={cls} style={style}>
      {icon && <Icon name={icon} size={14} className="os-input__icon" />}
      {multiline ? (
        <textarea
          id={fieldId}
          rows={rows}
          ref={ref}
          {...aria}
          {...(rest as TextareaHTMLAttributes<HTMLTextAreaElement>)}
        />
      ) : (
        <input id={fieldId} ref={ref} {...aria} {...(rest as InputHTMLAttributes<HTMLInputElement>)} />
      )}
      {trailing}
    </div>
  );
  if (!label && !hint && !error) return control;
  return (
    <label className="os-field" htmlFor={fieldId}>
      {label && <span className="os-field__label">{label}</span>}
      {control}
      {error ? (
        <span id={noteId} className="os-field__error">
          {error}
        </span>
      ) : (
        hint && (
          <span id={noteId} className="os-field__hint">
            {hint}
          </span>
        )
      )}
    </label>
  );
}
