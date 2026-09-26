import type { InputHTMLAttributes, ReactNode, Ref } from "react";

export interface CheckboxProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: ReactNode;
  /** Secondary line under the label (an option's description). */
  description?: ReactNode;
  /** Render as a radio button (single-select open questions). */
  radio?: boolean;
  ref?: Ref<HTMLInputElement>;
}

/** Checkbox or radio. Controlled with checked/onChange, or uncontrolled with defaultChecked. */
export function Checkbox({ label, description, radio, className = "", style, ...rest }: CheckboxProps) {
  return (
    <label className={`os-check ${radio ? "os-check--radio" : ""} ${className}`} style={style}>
      <input type={radio ? "radio" : "checkbox"} {...rest} />
      <span className="os-check__box" />
      {(label || description) && (
        <span className="os-check__text">
          {label && <span>{label}</span>}
          {description && <span className="os-check__desc">{description}</span>}
        </span>
      )}
    </label>
  );
}
