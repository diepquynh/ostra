import { Fragment } from "react";
import { Icon } from "../core/Icon";

export interface StepItem {
  id?: string;
  label: string;
  hint?: string;
}

export interface StepperProps {
  steps: StepItem[];
  /** Zero-based index of the current step. */
  current: number;
  /** vertical = wizard rail with hints; horizontal = compact bar that shows only the current label. */
  orientation?: "vertical" | "horizontal";
  /** Called with a done step's index, to go back. */
  onSelect?: (index: number) => void;
}

/** Wizard progress. Steps before current are done; clicking a done step calls onSelect. */
export function Stepper({ steps, current, orientation = "vertical", onSelect }: StepperProps) {
  const horiz = orientation === "horizontal";
  return (
    <div className={`os-stepper ${horiz ? "os-stepper--horizontal" : ""}`} role="group" aria-label="Progress">
      {steps.map((s, i) => {
        const st = i < current ? "done" : i === current ? "current" : "pending";
        const click = st === "done" && onSelect;
        return (
          <Fragment key={s.id ?? i}>
            {horiz && i > 0 && <span className={`os-stepper__line ${i <= current ? "os-stepper__line--done" : ""}`} />}
            <button
              type="button"
              aria-current={st === "current" ? "step" : undefined}
              aria-disabled={click ? undefined : true}
              aria-label={horiz && st !== "current" ? `Step ${i + 1}: ${s.label}` : undefined}
              tabIndex={click ? 0 : -1}
              className={`os-step os-step--${st} ${click ? "os-step--clickable" : ""}`}
              onClick={click ? () => onSelect(i) : undefined}
            >
              <span className="os-step__mark">{st === "done" ? <Icon name="check" size={12} /> : i + 1}</span>
              {(!horiz || st === "current") && (
                <span className="os-step__text">
                  <span>{s.label}</span>
                  {!horiz && s.hint && <span className="os-step__hint">{s.hint}</span>}
                </span>
              )}
            </button>
          </Fragment>
        );
      })}
    </div>
  );
}
