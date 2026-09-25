import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { Breadcrumbs, Icon, IconButton, Kbd, Menu, type MenuItem } from "../design";
import { formatCost } from "../lib/format";
import { modKeys } from "../lib/keys";
import { useActivity, useWorkspaces } from "../lib/live";
import { resourcePath } from "../lib/resource";
import type { ResourceCrumb } from "./meta";
import { lastActive } from "./uiState";

/** Where opening a workspace lands: the tab it last showed, else its overview. */
export const workspaceEntry = (ws: string) => resourcePath(ws, lastActive(ws) ?? "ws:overview");

type SwitcherProps = {
  ws: string;
  wsName: string;
  activeId: string | null;
  go: (id: string) => void;
  onNewWorkspace: () => void;
  onAddProject: () => void;
  onSetup: () => void;
};

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function WorkspaceSwitcher({ ws, wsName, activeId, go, onNewWorkspace, onAddProject, onSetup }: SwitcherProps) {
  const [open, setOpen] = useState(false);
  const { workspaces } = useWorkspaces();
  const { activity } = useActivity(ws);
  const navigate = useNavigate();
  const list = workspaces.some((w) => w.id === ws)
    ? workspaces
    : [{ id: ws, name: wsName, root: "", projects: 0, active_sessions: 0, available: true }, ...workspaces];
  const page = (
    id: string,
    label: string,
    icon: "layout-dashboard" | "coins" | "settings" | "brain" | "book-open",
    hint?: string,
  ): MenuItem => ({
    id,
    label,
    icon,
    indent: 1,
    hint,
    active: activeId === id,
    onSelect: () => go(id),
  });
  const items: MenuItem[] = [
    { type: "heading", label: "Workspaces" },
    ...list.flatMap((w): MenuItem[] => {
      const current = w.id === ws;
      const row: MenuItem = {
        id: `w:${w.id}`,
        icon: w.available ? "box" : "folder-x",
        label: current ? wsName : w.name,
        checked: current,
        sub: !w.available
          ? "Folder missing"
          : [plural(w.projects, "project"), w.active_sessions ? `${w.active_sessions} active` : null, w.root || null]
              .filter(Boolean)
              .join(" · "),
        onSelect: current || !w.available ? undefined : () => navigate(workspaceEntry(w.id)),
      };
      if (!current) return [row];
      const week = activity?.spend_week_usd;
      return [
        row,
        page("ws:overview", "Overview", "layout-dashboard"),
        page("ws:cost", "Cost", "coins", week != null ? formatCost(week) : undefined),
        page("ws:settings", "Settings", "settings"),
        page("ws:memory", "Memory", "brain"),
        page("ws:skills", "Skills", "book-open"),
      ];
    }),
    { type: "divider" },
    { id: "new", icon: "plus", label: "New workspace…", onSelect: onNewWorkspace },
    { id: "add", icon: "folder-plus", label: `Add project to ${wsName}…`, onSelect: onAddProject },
    { type: "divider" },
    { id: "setup", icon: "graduation-cap", label: "Run the setup guide again", onSelect: onSetup },
  ];
  return (
    <div style={{ position: "relative" }}>
      <button
        className="os-btn os-btn--ghost os-btn--sm"
        style={{ color: "var(--text-primary)", fontWeight: 600 }}
        onClick={() => setOpen(!open)}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {wsName}
        <Icon name="chevrons-up-down" size={12} />
      </button>
      <Menu open={open} onClose={() => setOpen(false)} items={items} width={300} label="Workspace" />
    </div>
  );
}

export type TitleBarProps = SwitcherProps & {
  crumbs: ResourceCrumb[];
  onPalette: () => void;
  sidebar: boolean;
  toggleSidebar: () => void;
  dock: boolean;
  toggleDock: () => void;
};

export function TitleBar({ crumbs, onPalette, sidebar, toggleSidebar, dock, toggleDock, ...switcher }: TitleBarProps) {
  return (
    <header
      style={{
        height: "var(--titlebar-h)",
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "0 8px 0 10px",
        background: "var(--surface-chrome)",
        borderBottom: "1px solid var(--border-default)",
        flex: "none",
      }}
    >
      <Link to="/" title="All workspaces" style={{ display: "inline-flex" }}>
        <img src="/favicon.svg" width={16} height={16} alt="Ostra" />
      </Link>
      <WorkspaceSwitcher {...switcher} />
      <span style={{ width: 1, height: 16, background: "var(--border-default)" }} />
      <div style={{ flex: 1, minWidth: 0, overflow: "hidden" }}>
        <Breadcrumbs
          items={crumbs}
          onNavigate={(c) => (c as ResourceCrumb).to && switcher.go((c as ResourceCrumb).to!)}
        />
      </div>
      <button
        onClick={onPalette}
        className="os-input os-input--sm"
        style={{
          width: 280,
          maxWidth: "30vw",
          cursor: "pointer",
          gap: 8,
          color: "var(--text-muted)",
          fontSize: "var(--text-sm)",
        }}
      >
        <Icon name="search" size={13} />
        <span style={{ flex: 1, textAlign: "left" }}>Go to anything</span>
        <Kbd keys={modKeys("K")} />
      </button>
      <div style={{ display: "flex", gap: 2 }}>
        <IconButton size="sm" icon="panel-left" label="Toggle sidebar" active={sidebar} onClick={toggleSidebar} />
        <IconButton size="sm" icon="message-square" label="Quick question" active={dock} onClick={toggleDock} />
      </div>
    </header>
  );
}
