import { Button, Panel, Table, type TableColumn, Tabs } from "@ostra/design";
import { useMemo, useState } from "react";
import { api } from "../api";
import { formatCost } from "../lib/format";
import { useAsync, useChannel, useThrottled } from "../lib/hooks";
import { useActivity, useSessionSummaries } from "../lib/live";
import { useNav } from "../lib/nav";
import { COST_GROUPS, type CostLine, costLines, costTotals } from "./workspace/costTable";
import { LoadError, Loading, Page, Stat } from "./workspace/Page";

export type CostScreenProps = { ws: string };

const MONO = { fontFamily: "var(--font-mono)", fontSize: "var(--text-sm)" } as const;

function columns(first: string): TableColumn<CostLine>[] {
  return [
    { key: "label", label: first, render: (r) => <span style={r.mono ? MONO : undefined}>{r.label}</span> },
    { key: "runs", label: "Runs", num: true },
    { key: "input", label: "Input", num: true },
    { key: "output", label: "Output", num: true },
    { key: "cacheReads", label: "Cache reads", num: true },
    { key: "cacheWrites5m", label: <span title="Cache writes with the 5-minute TTL">Cache writes 5m</span>, num: true },
    {
      key: "cacheWrites1h",
      label: (
        <span title="Cache writes with the 1-hour TTL, which cost more than 5-minute writes">Cache writes 1h</span>
      ),
      num: true,
    },
    {
      key: "perCall",
      label: (
        <span title="Cache reads divided by tool calls. A rising value means each tool call re-reads more context.">
          Cache / tool call
        </span>
      ),
      num: true,
    },
    { key: "build", label: "Build time", num: true },
    { key: "cost", label: "Cost", num: true },
    {
      key: "share",
      label: "Share",
      num: true,
      render: (r) => <span style={{ color: "var(--text-muted)" }}>{r.share}</span>,
    },
  ];
}

export type CostRange = "week" | "all";

const RANGES = [
  { id: "week", label: "This week", title: "Today and the six days before it, the same week as the status bar" },
  { id: "all", label: "All time" },
];

/**
 * Resource `ws:cost`: spend per session, stage, agent and executor over this week or all time. The week starts at the
 * server's `WorkspaceActivity.week_since`, so the totals match the status bar.
 */
export function CostScreen({ ws }: CostScreenProps) {
  const [range, setRange] = useState<CostRange>("week");
  const { activity } = useActivity(ws);
  const since = range === "week" ? (activity?.week_since ?? null) : null;
  const waiting = range === "week" && since === null;
  const report = useAsync(() => (waiting ? Promise.resolve(null) : api.cost(ws, since)), [ws, since, waiting]);
  const { sessions } = useSessionSummaries(ws);
  const { open } = useNav();
  // Spend changes as executions run and finish; activity and session updates are the cue.
  const refresh = useThrottled(report.reload, 2000);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "activity" || m.type === "session_updated") refresh();
  });

  const r = report.data;
  const tables = useMemo(
    () =>
      r ? COST_GROUPS.map((g) => ({ ...g, lines: costLines(r[g.field], g.id, r.total.usage.cost_usd, sessions) })) : [],
    [r, sessions],
  );

  const lead = (
    <p className="wp-lead">
      Two numbers matter most. Cache reads per tool call show how much context each step re-reads, which grows when an
      agent loops. Build time shows how long agents spend in build and test commands, because a run with many
      consecutive build failures spends without progress.
    </p>
  );

  const picker = (
    <Tabs
      variant="segmented"
      label="Time range"
      value={range}
      onChange={(id) => setRange(id as CostRange)}
      tabs={RANGES}
    />
  );
  if (report.error && !r) {
    return (
      <Page title="Cost" actions={picker}>
        <LoadError error={report.error} onRetry={report.reload} />
      </Page>
    );
  }
  if (!r) {
    return (
      <Page title="Cost" actions={picker}>
        <Loading>Reading the spend…</Loading>
      </Page>
    );
  }
  const t = costTotals(r);
  const week = range === "week";

  return (
    <Page title="Cost" actions={picker}>
      {lead}
      <div className="wp-stats">
        <Stat
          label={week ? "This week" : "All time"}
          value={t.total}
          title={
            week && r.since
              ? `Executions that started since ${new Date(r.since).toLocaleString()}`
              : "Every execution in this workspace"
          }
        />
        <Stat
          label="Today"
          value={activity ? formatCost(activity.spend_today_usd) : "…"}
          title="Execution spend since local midnight on the server"
        />
        {!week && (
          <Stat
            label="This week"
            value={activity ? formatCost(activity.spend_week_usd) : "…"}
            title="Today and the six days before it"
          />
        )}
        <Stat label="Runs" value={t.runs} />
        <Stat label="Cache reads" value={t.cacheReads} />
        <Stat label="Tool calls" value={t.toolCalls} />
        <Stat label="Cache / tool call" value={t.perCall} />
        <Stat label="Build time" value={t.build} />
      </div>
      {r.total.executions === 0 ? (
        <Panel>
          <div className="wp-row">
            <span style={{ color: "var(--text-secondary)" }}>
              {week
                ? "No spend this week. Switch to All time for earlier runs."
                : "No spend yet. Costs appear here after a session runs its first execution."}
            </span>
            <span className="wp-spacer" />
            <Button size="sm" icon="plus" onClick={() => open("ws:overview")}>
              New task
            </Button>
          </div>
        </Panel>
      ) : (
        tables.map((g) => (
          <Panel key={g.id} title={g.title} bodyFlush>
            <Table<CostLine>
              columns={columns(g.column)}
              rows={g.lines}
              onRowClick={g.id === "session" ? (l) => l.open && open(l.open, { preview: true }) : undefined}
              empty="Nothing recorded for this grouping."
            />
          </Panel>
        ))
      )}
    </Page>
  );
}
