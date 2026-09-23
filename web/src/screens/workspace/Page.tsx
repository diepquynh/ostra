import { useEffect, useRef, type ReactNode } from "react";
import { useLocation } from "react-router";
import { Banner, Button, Spinner } from "../../design";
import "./workspace.css";

/** The column every workspace page uses: title, optional subtitle and actions, then the content. */
export function Page({ title, sub, actions, children }: { title: ReactNode; sub?: ReactNode; actions?: ReactNode; children?: ReactNode }) {
  return (
    <div className="wp-page">
      <div className="wp-head">
        <div className="wp-head__text">
          <h1 className="wp-title">{title}</h1>
          {sub && <div className="wp-sub">{sub}</div>}
        </div>
        {actions}
      </div>
      {children}
    </div>
  );
}

export function Stat({ label, value, title }: { label: string; value: ReactNode; title?: string }) {
  return (
    <div className="wp-stat" title={title}>
      <span className="wp-stat__label">{label}</span>
      <span className="wp-stat__value">{value}</span>
    </div>
  );
}

export function Loading({ children = "Loading…" }: { children?: ReactNode }) {
  return (
    <div className="wp-row wp-muted" style={{ padding: "8px 0" }}>
      <Spinner size={12} /> {children}
    </div>
  );
}

export function LoadError({ error, onRetry }: { error: Error; onRetry?: () => void }) {
  return (
    <Banner tone="bad" actions={onRetry && <Button size="sm" onClick={onRetry}>Try again</Button>}>
      {error.message}
    </Banner>
  );
}

/**
 * The URL hash the shell set with `open(id, { anchor })`, decoded, plus a counter that changes on every
 * navigation so opening the same anchor twice still fires.
 */
export function useAnchor(): { anchor: string | null; nonce: string } {
  const { hash, key } = useLocation();
  let anchor: string | null = null;
  if (hash.length > 1) {
    try {
      anchor = decodeURIComponent(hash.slice(1));
    } catch {
      anchor = hash.slice(1);
    }
  }
  return { anchor, nonce: key };
}

const FLASH_MS = 2400;

/** Scroll an element into view and ring it for a moment. */
export function flash(el: Element | null) {
  if (!el) return;
  el.scrollIntoView?.({ block: "center", behavior: "smooth" });
  el.classList.add("wp-flash");
  setTimeout(() => el.classList.remove("wp-flash"), FLASH_MS);
}

/** Run `fn` once after the next paint whenever `deps` change, for scrolling to content that just rendered. */
export function useAfterPaint(fn: () => void, deps: unknown[]) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => {
    const id = requestAnimationFrame(() => ref.current());
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
