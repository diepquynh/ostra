import type { InputHTMLAttributes, ReactNode, Ref } from "react";

export interface SwitchProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: ReactNode;
  /** "warn" paints the on-state amber. Use it for YOLO. */
  tone?: "accent" | "warn";
  ref?: Ref<HTMLInputElement>;
}

/** On/off toggle. Controlled with checked/onChange, or uncontrolled with defaultChecked. */
export function Switch({ label, tone, className = "", style, ...rest }: SwitchProps) {
  return (
    <label className={`os-switch ${tone === "warn" ? "os-switch--warn" : ""} ${className}`} style={style}>
      <input type="checkbox" role="switch" {...rest} />
      <span className="os-switch__track" />
      {label && <span>{label}</span>}
    </label>
  );
}
