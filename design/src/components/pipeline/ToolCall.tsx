import { type ReactNode, useState } from "react";
import { activateOnKey } from "../../keys";
import { Icon } from "../core/Icon";
import type { IconName } from "../core/icons";
import { Chip } from "../feedback/Chip";
import { Spinner } from "../feedback/Spinner";

export interface PolicyInfo {
  decision: "allow" | "ask" | "deny";
  /** "guard" (layer 1) or "permission" (layer 2). */
  layer: string;
  rule: string;
  reason?: string;
  /** "What to do instead" line shown on denials. */
  advice?: string;
}

export interface ToolCallProps {
  /** Canonical Claude Code tool name: Read, Write, Edit, Bash, Grep, Glob, WebFetch, Skill, Report, Memory… */
  tool: string;
  /** One-line mono summary: the path, the command, the pattern. */
  summary?: string;
  state?: "running" | "done" | "error";
  /** Preformatted duration, e.g. "1.2s". */
  duration?: string;
  policy?: PolicyInfo;
  /** Denied and asked calls open by default. */
  defaultOpen?: boolean;
  /** Body: a <pre> of the input/output, or a DiffView. */
  children?: ReactNode;
}

const TOOL_ICON: Record<string, IconName> = {
  Read: "file",
  Write: "file-plus",
  Edit: "file-diff",
  Bash: "square-terminal",
  Grep: "text-search",
  Glob: "folder-search",
  WebFetch: "globe",
  WebSearch: "search",
  Skill: "book-open",
  Report: "file-check",
  Memory: "brain",
  MemoryRecall: "brain",
};

/** One tool call in an execution's Activity stream, with its policy decision. */
export function ToolCall({ tool, summary, state = "done", duration, policy, defaultOpen, children }: ToolCallProps) {
  const denied = policy?.decision === "deny";
  const asked = policy?.decision === "ask";
  const [open, setOpen] = useState(defaultOpen ?? (denied || asked));
  const toggle = () => setOpen((o) => !o);
  return (
    <div className={`os-tool ${denied ? "os-tool--denied" : asked ? "os-tool--asked" : ""}`}>
      <div
        className="os-tool__head"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={toggle}
        onKeyDown={activateOnKey(toggle)}
      >
        <Icon
          name="chevron-right"
          size={12}
          style={{
            color: "var(--text-muted)",
            transform: open ? "rotate(90deg)" : "none",
            transition: "transform var(--dur-fast)",
          }}
        />
        <Icon name={TOOL_ICON[tool] ?? "wrench"} size={13} style={{ color: "var(--text-muted)" }} />
        <span className="os-tool__name">{tool}</span>
        <span className="os-tool__summary">{summary}</span>
        {denied && <Chip tone="bad">Denied</Chip>}
        {asked && state !== "done" && <Chip tone="warn">Asking you</Chip>}
        {state === "error" && !denied && <Chip tone="bad">Error</Chip>}
        {state === "running" && !asked && <Spinner size={11} style={{ color: "var(--accent)" }} />}
        {duration && <span className="os-tool__time">{duration}</span>}
      </div>
      {open && (
        <div className="os-tool__body">
          {policy && policy.decision !== "allow" && (
            <div className={`os-policy os-policy--${policy.decision}`}>
              <div>
                <strong>{denied ? "Denied" : "Needs permission"}</strong> by {policy.layer} rule{" "}
                <code>{policy.rule}</code>
              </div>
              {policy.reason && <div>{policy.reason}</div>}
              {policy.advice && (
                <div style={{ color: "var(--text-secondary)" }}>What to do instead: {policy.advice}</div>
              )}
            </div>
          )}
          {policy && policy.decision === "allow" && policy.rule && (
            <div className="os-policy os-policy--allow">
              Allowed by {policy.layer} rule <code>{policy.rule}</code>
            </div>
          )}
          {children}
        </div>
      )}
    </div>
  );
}
