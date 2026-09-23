import { useId, useRef, type CSSProperties, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useFocusTrap } from "../../focus";
import { IconButton } from "../core/IconButton";

export interface DialogProps {
  open?: boolean;
  title: ReactNode;
  subtitle?: ReactNode;
  /** Close button, Esc and scrim click call this. Omit for a dialog that must be answered. */
  onClose?: () => void;
  /** Footer row, usually right-aligned buttons after a flex spacer. */
  footer?: ReactNode;
  width?: number | string;
  /** Render without scrim or fixed positioning. */
  inline?: boolean;
  bodyStyle?: CSSProperties;
  /** Element to focus on open. Defaults to the first focusable element in the dialog. */
  initialFocus?: RefObject<HTMLElement | null>;
  children?: ReactNode;
}

/** Modal dialog. inline renders the panel without the scrim (for docs and cards). */
export function Dialog({ open = true, title, subtitle, onClose, footer, width = 560, inline, bodyStyle, initialFocus, children }: DialogProps) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const downOnScrim = useRef(false);
  const trapTab = useFocusTrap(panelRef, open && !inline, initialFocus);
  if (!open) return null;

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && onClose) {
      e.stopPropagation();
      onClose();
      return;
    }
    trapTab(e);
  };

  const panel = (
    <div
      ref={panelRef}
      className="os-dialog"
      role="dialog"
      aria-modal={!inline}
      aria-labelledby={titleId}
      tabIndex={-1}
      style={{ width, ...(inline ? { animation: "none" } : null) }}
      onClick={(e) => e.stopPropagation()}
    >
      <div className="os-dialog__head">
        <div className="os-dialog__title" id={titleId}>
          {title}
          {subtitle && <span className="os-dialog__sub">{subtitle}</span>}
        </div>
        {onClose && <IconButton size="sm" icon="x" label="Close" onClick={onClose} />}
      </div>
      <div className="os-dialog__body" style={bodyStyle}>
        {children}
      </div>
      {footer && <div className="os-dialog__foot">{footer}</div>}
    </div>
  );
  if (inline) return panel;
  // Close on a click that both starts and ends on the scrim, so a drag out of a field does not close it.
  return createPortal(
    <div
      className="os-dialog-scrim"
      onKeyDown={onKeyDown}
      onMouseDown={(e) => (downOnScrim.current = e.target === e.currentTarget)}
      onClick={(e) => {
        if (e.target === e.currentTarget && downOnScrim.current && onClose) onClose();
        downOnScrim.current = false;
      }}
    >
      {panel}
    </div>,
    document.body,
  );
}
