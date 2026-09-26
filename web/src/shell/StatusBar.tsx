import { Icon, Menu, type MenuItem, Spinner, StatusDot } from "@ostra/design";
import { useState } from "react";
import { isMock } from "../api";
import { formatCost, humanize, truncate } from "../lib/format";
import { useActivity, useSessionSummaries, useSocketState } from "../lib/live";
import type { OpenOptions, Theme } from "../lib/nav";

type StatusBarProps = {
  ws: string;
  /** Session of the active tab, for the YOLO state. */
  session: string | null;
  open: (id: string, opts?: OpenOptions) => void;
  theme: Theme;
  toggleTheme: () => void;
};

const ABOVE = { top: "auto", bottom: "calc(100% + 4px)" } as const;

function Connection() {
  const state = useSocketState();
  if (isMock)
    return (
      <span
        className="shell-status-item"
        style={{ cursor: "default" }}
        title="Mock mode: fixtures stand in for the server"
      >
        <StatusDot tone="info" size={6} /> Mock data
      </span>
    );
  const [tone, label] =
    state === "open"
      ? (["ok", `Live · ${location.host}`] as const)
      : state === "connecting"
        ? (["warn", "Connecting"] as const)
        : (["bad", "Offline, reconnecting"] as const);
  return (
    <span className="shell-status-item" style={{ cursor: "default" }} title="Live updates from the Ostra server">
      <StatusDot tone={tone} size={6} pulse={state === "connecting"} /> {label}
    </span>
  );
}

export function StatusBar({ ws, session, open, theme, toggleTheme }: StatusBarProps) {
  const { activity } = useActivity(ws);
  const { sessions } = useSessionSummaries(ws);
  const [menu, setMenu] = useState<"running" | "gates" | null>(null);
  const running = activity?.running ?? [];
  const gates = activity?.open_gates ?? [];
  const summary = session ? sessions.find((s) => s.id === session) : undefined;

  const openGate = (g: (typeof gates)[number]) =>
    open(`session:${g.session}`, g.id.startsWith("session:") ? {} : { anchor: `gate-${g.id}` });
  const runningItems: MenuItem[] = [
    { type: "heading", label: "Running executions" },
    ...running.map((x) => ({
      id: x.id,
      icon: x.stream === "terminal" ? ("square-terminal" as const) : ("activity" as const),
      label: `${humanize(x.agent)} · ${x.run_label}`,
      sub: [x.project, x.summary ? truncate(x.summary, 40) : null].filter(Boolean).join(" · "),
      onSelect: () => open(`exec:${x.id}`),
    })),
  ];
  const gateItems: MenuItem[] = [
    { type: "heading", label: "Waiting for you" },
    ...gates.map((g) => ({
      id: g.id,
      icon: "hand" as const,
      label: g.title,
      sub: g.session_title ?? undefined,
      onSelect: () => openGate(g),
    })),
  ];

  return (
    <footer
      style={{
        height: "var(--statusbar-h)",
        flex: "none",
        display: "flex",
        alignItems: "center",
        gap: 4,
        padding: "0 6px",
        background: "var(--surface-chrome)",
        borderTop: "1px solid var(--border-default)",
        fontSize: "var(--text-xs)",
        color: "var(--text-secondary)",
      }}
    >
      <Connection />
      {running.length > 0 && (
        <span style={{ position: "relative" }}>
          <button
            className="shell-status-item"
            aria-haspopup="menu"
            aria-expanded={menu === "running"}
            onClick={() => setMenu(menu === "running" ? null : "running")}
          >
            <Spinner size={10} style={{ color: "var(--accent)" }} /> {running.length} running
          </button>
          <Menu
            open={menu === "running"}
            onClose={() => setMenu(null)}
            items={runningItems}
            width={320}
            style={ABOVE}
            label="Running executions"
          />
        </span>
      )}
      {gates.length > 0 && (
        <span style={{ position: "relative" }}>
          <button
            className="shell-status-item"
            style={{ color: "var(--warn)" }}
            aria-haspopup={gates.length > 1 ? "menu" : undefined}
            onClick={() => (gates.length === 1 ? openGate(gates[0]) : setMenu(menu === "gates" ? null : "gates"))}
          >
            <Icon name="hand" size={12} /> {gates.length} waiting for you
          </button>
          <Menu
            open={menu === "gates"}
            onClose={() => setMenu(null)}
            items={gateItems}
            width={320}
            style={ABOVE}
            label="Gates waiting for you"
          />
        </span>
      )}
      <span style={{ flex: 1 }} />
      {summary && (
        <span
          className="shell-status-item"
          style={{ cursor: "default", color: summary.yolo ? "var(--warn)" : undefined }}
          title={
            summary.yolo
              ? "Ostra answers this session's gates and permission asks, and lists each decision at the end"
              : "You answer this session's gates"
          }
        >
          YOLO {summary.yolo ? "on" : "off"}
        </span>
      )}
      {activity && (
        <button
          className="shell-status-item"
          style={{ fontFamily: "var(--font-mono)", fontSize: "var(--text-2xs)" }}
          title={`${formatCost(activity.spend_today_usd)} today`}
          onClick={() => open("ws:cost")}
        >
          {formatCost(activity.spend_week_usd)} this week
        </button>
      )}
      <button
        className="shell-status-item"
        onClick={toggleTheme}
        title={theme === "dark" ? "Switch to the light theme" : "Switch to the dark theme"}
        aria-label="Toggle theme"
      >
        <Icon name={theme === "dark" ? "moon" : "sun"} size={12} />
      </button>
    </footer>
  );
}
