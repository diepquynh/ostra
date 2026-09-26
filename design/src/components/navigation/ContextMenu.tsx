import { Menu, type MenuItem } from "./Menu";

export interface ContextMenuProps {
  /** Viewport point the menu opens at, such as a right click's `clientX`/`clientY`. Null closes it. */
  at: { x: number; y: number } | null;
  items: MenuItem[];
  onClose: () => void;
  label?: string;
  width?: number;
}

const MARGIN = 8;

/** A {@link Menu} opened at a point instead of under a trigger, kept inside the viewport. */
export function ContextMenu({ at, items, onClose, label, width = 220 }: ContextMenuProps) {
  if (!at) return null;
  const left = Math.max(MARGIN, Math.min(at.x, window.innerWidth - width - MARGIN));
  const rows = items.length * 30 + 8;
  const top = Math.max(MARGIN, Math.min(at.y, window.innerHeight - rows - MARGIN) - 4);
  return (
    <div style={{ position: "fixed", left, top, width, height: 0, zIndex: 91 }}>
      <Menu open items={items} onClose={onClose} label={label} width={width} style={{ top: 0 }} />
    </div>
  );
}
