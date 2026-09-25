import { useMemo, useState } from "react";
import type { SessionSummary } from "../../api/types";
import { LANES } from "../../content/stages";
import { Button, Chip, Input, Panel, Select, StatusChip, Table, type TableColumn, Tabs } from "../../design";
import { formatCost, humanize, relativeTime, truncate } from "../../lib/format";
import { useNav } from "../../lib/nav";
import { SessionDot } from "../../shell/Sidebar";
import {
  filterSessions,
  isFiltered,
  type KindFilter,
  NO_FILTERS,
  type SessionFilters,
  STATUS_TABS,
  type StatusFilter,
  sessionProjects,
  sessionTitle,
  statusCounts,
} from "./sessionFilter";

const columns: TableColumn<SessionSummary>[] = [
  {
    key: "request",
    label: "Request",
    render: (s) => (
      <div className="wp-request" title={s.title ? s.request : undefined}>
        <span className="wp-request__title">
          <SessionDot status={s.status} />
          {truncate(sessionTitle(s), 140)}
        </span>
        <span className="wp-request__chips">
          {s.kind.kind === "init" ? <Chip tone="info">Init</Chip> : s.category && <Chip>{humanize(s.category)}</Chip>}
          {s.yolo && <Chip tone="warn">YOLO</Chip>}
          {s.projects.map((p) => (
            <Chip key={p} mono outline>
              {p}
            </Chip>
          ))}
        </span>
      </div>
    ),
  },
  {
    key: "stage",
    label: "Stage",
    render: (s) => (
      <span className="wp-stage">
        <span style={{ color: "var(--text-muted)", fontSize: "var(--text-xs)" }}>
          {LANES[s.lane]?.title ?? humanize(s.lane)}
        </span>
        {s.stage_label}
      </span>
    ),
  },
  {
    key: "status",
    label: "Status",
    render: (s) => (
      <span className="wp-stage" style={{ minWidth: 0 }}>
        <span>
          <StatusChip status={s.status} />
        </span>
        {s.open_gates > 0 && (
          <span className="wp-muted" style={{ fontSize: "var(--text-xs)" }}>
            {s.open_gates} gate{s.open_gates === 1 ? "" : "s"} open
          </span>
        )}
      </span>
    ),
  },
  { key: "cost", label: "Cost", num: true, render: (s) => formatCost(s.cost_usd) },
  {
    key: "updated",
    label: "Updated",
    render: (s) => (
      <span
        className="wp-nowrap"
        style={{ color: "var(--text-muted)", fontSize: "var(--text-sm)" }}
        title={s.updated_at}
      >
        {relativeTime(s.updated_at)}
      </span>
    ),
  },
];

/** The sessions of a workspace with status, kind, project and text filters. Rows open a preview tab. */
export function SessionsTable({
  sessions,
  projects,
  loading,
}: {
  sessions: SessionSummary[];
  projects: string[];
  loading: boolean;
}) {
  const { open } = useNav();
  const [filters, setFilters] = useState<SessionFilters>(NO_FILTERS);
  const set = <K extends keyof SessionFilters>(k: K, v: SessionFilters[K]) => setFilters((f) => ({ ...f, [k]: v }));
  const rows = useMemo(() => filterSessions(sessions, filters), [sessions, filters]);
  const counts = useMemo(() => statusCounts(sessions, filters), [sessions, filters]);
  const keys = useMemo(() => sessionProjects(sessions, projects), [sessions, projects]);
  const hasInit = sessions.some((s) => s.kind.kind === "init");

  const empty =
    sessions.length === 0 ? (
      loading ? (
        "Loading sessions…"
      ) : (
        "No sessions yet. Start one with the New task form."
      )
    ) : (
      <span className="wp-row" style={{ justifyContent: "center" }}>
        No sessions match these filters.
        <Button size="sm" variant="ghost" icon="x" onClick={() => setFilters(NO_FILTERS)}>
          Clear filters
        </Button>
      </span>
    );

  return (
    <>
      <div className="wp-row" style={{ marginTop: 6 }}>
        <div className="os-section-label" style={{ flex: 1 }}>
          Sessions
        </div>
        <Input
          size="sm"
          icon="search"
          aria-label="Filter sessions"
          placeholder="Filter sessions"
          value={filters.text}
          onChange={(e) => set("text", e.target.value)}
          style={{ width: 200 }}
        />
        {hasInit && (
          <Select
            size="sm"
            aria-label="Kind"
            value={filters.kind}
            onChange={(e) => set("kind", e.target.value as KindFilter)}
            options={[
              { value: "all", label: "All kinds" },
              { value: "pipeline", label: "Pipeline" },
              { value: "init", label: "Init" },
            ]}
          />
        )}
        {keys.length > 1 && (
          <Select
            size="sm"
            aria-label="Project"
            value={filters.project}
            onChange={(e) => set("project", e.target.value)}
            options={[{ value: "all", label: "All projects" }, ...keys.map((k) => ({ value: k, label: k }))]}
          />
        )}
        <Tabs
          variant="segmented"
          label="Status"
          value={filters.status}
          onChange={(id) => set("status", id as StatusFilter)}
          tabs={STATUS_TABS.map((t) => ({ ...t, count: counts[t.id] || undefined }))}
        />
      </div>
      <Panel bodyFlush>
        <Table<SessionSummary>
          onRowClick={(s) => open(`session:${s.id}`, { preview: true })}
          columns={columns}
          rows={rows}
          empty={empty}
        />
      </Panel>
      {isFiltered(filters) && rows.length > 0 && rows.length < sessions.length && (
        <div className="wp-muted">
          Showing {rows.length} of {sessions.length} sessions.
        </div>
      )}
    </>
  );
}
