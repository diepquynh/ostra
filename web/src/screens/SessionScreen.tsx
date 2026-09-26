import { Banner, Button, LaneStepper, Panel, PhaseDag, SectionLabel, Spinner, Tabs } from "@ostra/design";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../api";
import type { GateView, Lane, SessionDetail, StoredEvent } from "../api/types";
import { Markdown } from "../components/Markdown";
import { BlockerNotice } from "../features/gates/BlockerNotice";
import { GateCard } from "../features/gates/GateCard";
import { activeSecurityBlocks, splitDecidedForYou } from "../lib/events";
import { useAsync, useChannel, useThrottled } from "../lib/hooks";
import { useNav } from "../lib/nav";
import {
  answeredGates,
  defaultLane,
  eventLine,
  laneStates,
  mergeEvents,
  openGatesInOrder,
  phaseNodes,
} from "./session/board";
import { ContextComposer } from "./session/ContextComposer";
import { SessionHeader } from "./session/Header";
import { LanePanel } from "./session/LanePanel";
import { DecisionsPanel } from "./session/SidePanels";

export type SessionScreenProps = {
  ws: string;
  /** Session id. */
  id: string;
};

const PHASE_LANES: Lane[] = ["design", "build", "review", "test"];
const column = { display: "flex", flexDirection: "column", gap: 16, minWidth: 0 } as const;
type BoardTab = "overview" | "decisions";

/** Resource `session:<id>`: the session board. A `#gate-<id>` URL hash scrolls to that gate. */
export function SessionScreen({ ws, id }: SessionScreenProps) {
  const detail = useAsync(() => api.session(id), [id]);
  const events = useAsync(() => api.events(id), [id]);
  const [live, setLive] = useState<StoredEvent[]>([]);
  const refresh = useThrottled(() => detail.reload(), 400);

  useEffect(() => setLive([]), [events.data]);
  useChannel(`session:${id}`, (m) => {
    if (m.type === "session_event") {
      setLive((l) => [...l, { seq: m.seq, at: m.at, event: m.event }]);
      refresh();
    }
    if (m.type === "session_updated" && detail.data) detail.set({ ...detail.data, summary: m.summary });
  });

  if (detail.error && !detail.data)
    return (
      <Board>
        <Banner
          tone="bad"
          title="Ostra could not load this session"
          actions={<Button onClick={detail.reload}>Try again</Button>}
        >
          {detail.error.message}
        </Banner>
      </Board>
    );
  if (!detail.data)
    return (
      <Board>
        <div style={{ display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
          <Spinner size={11} /> Reading the session…
        </div>
      </Board>
    );
  return (
    <SessionBoard
      ws={ws}
      detail={detail.data}
      events={mergeEvents(events.data ?? [], live)}
      onDetail={detail.set}
      reload={() => {
        detail.reload();
        events.reload();
      }}
    />
  );
}

function Board({ children }: { children: ReactNode }) {
  return <div style={{ padding: "20px 28px 40px", display: "flex", flexDirection: "column", gap: 16 }}>{children}</div>;
}

function SessionBoard({
  ws,
  detail: d,
  events,
  onDetail,
  reload,
}: {
  ws: string;
  detail: SessionDetail;
  events: StoredEvent[];
  onDetail: (d: SessionDetail) => void;
  reload: () => void;
}) {
  const nav = useNav();
  const location = useLocation();
  const [lane, setLane] = useState<Lane>(() => defaultLane(d));
  const [showLog, setShowLog] = useState(false);
  const [showAnswered, setShowAnswered] = useState(false);
  const [tab, setTab] = useState<BoardTab>("overview");
  const s = d.summary;
  const ended = s.status === "completed" || s.status === "failed";
  const running = d.executions.filter((x) => x.status === "running").length;
  const sessionProjects = useMemo(() => {
    const created = events.find((e) => e.event.type === "session_created")?.event;
    return created?.type === "session_created" ? created.projects.map((p) => p.key) : s.projects;
  }, [events, s.projects]);
  const lanes = useMemo(() => laneStates(d), [d]);
  const open = openGatesInOrder(d);
  const answered = answeredGates(d);
  const blocks = activeSecurityBlocks(events.map((e) => e.event));
  const completion = d.completion ? splitDecidedForYou(d.completion) : null;
  const phases = phaseNodes(d.phases);

  useEffect(() => {
    if (!location.hash.startsWith("#gate-")) return;
    const gate = d.gates.find((g) => `#gate-${g.id}` === location.hash);
    if (gate?.answer) setShowAnswered(true);
    setTab("overview");
    requestAnimationFrame(() =>
      document.getElementById(location.hash.slice(1))?.scrollIntoView({ behavior: "smooth", block: "start" }),
    );
    // Only on a new hash, not on every live refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [location.hash]);

  const goToGate = (gateId: string) => {
    if (d.gates.find((g) => g.id === gateId)?.answer) setShowAnswered(true);
    setTab("overview");
    requestAnimationFrame(() =>
      document.getElementById(`gate-${gateId}`)?.scrollIntoView({ behavior: "smooth", block: "start" }),
    );
  };

  const answeredGate = (g: GateView) => {
    onDetail({ ...d, gates: d.gates.map((x) => (x.id === g.id ? g : x)) });
    reload();
  };

  return (
    <Board>
      <SessionHeader
        summary={s}
        files={d.files}
        uploads={d.uploads}
        additions={d.additions}
        onSummary={(summary) => onDetail({ ...d, summary })}
        onChanged={reload}
      />

      {s.status === "paused" && (
        <Banner tone="info" title="The session is paused">
          No agent runs and nothing new starts. Continue it from the header, and each paused agent picks up where it
          stopped. You can still answer gates and add context while it is paused.
        </Banner>
      )}

      {s.yolo && s.status !== "completed" && s.status !== "failed" && (
        <Banner tone="warn" title="YOLO is on">
          Ostra answers gates and permission asks itself and records each decision. Guards, deny rules, the fact-check
          PASS requirement, the session budget, and security blocks still apply.
        </Banner>
      )}

      {blocks.map((b) => (
        <BlockerNotice key={`${b.project}:${b.phase}:${b.tests}`} block={b} />
      ))}

      <LaneStepper lanes={lanes} selected={lane} onSelect={(l) => setLane(l)} />

      <Tabs
        label="Session views"
        value={tab}
        onChange={(t) => setTab(t as BoardTab)}
        tabs={[
          { id: "overview", label: "Overview", icon: "layout-dashboard", count: open.length || undefined },
          { id: "decisions", label: "Decisions", icon: "scale", count: d.decisions.length || undefined },
        ]}
      />

      {tab === "decisions" ? (
        <DecisionsPanel decisions={d.decisions} onChanged={reload} />
      ) : (
        <div style={column}>
          {open.length > 0 && (
            <section aria-label="Waiting for you" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              <SectionLabel>Waiting for you</SectionLabel>
              {open.map((g) => (
                <GateCard key={g.id} gate={g} onAnswered={answeredGate} />
              ))}
            </section>
          )}

          {completion && (
            <Panel title="Completion report" icon="flag">
              <Markdown className="os-prose" text={completion.body} />
            </Panel>
          )}
          {completion?.decided && (
            <Panel
              title="Decided for you"
              subtitle="each decision Ostra made under YOLO, with its reason"
              icon="scale"
              tone="warn"
            >
              <Markdown className="os-prose" text={completion.decided} />
            </Panel>
          )}

          {!ended && s.kind.kind === "pipeline" && (
            <ContextComposer
              ws={ws}
              summary={s}
              projects={sessionProjects}
              running={running}
              onSent={(summary) => {
                onDetail({ ...d, summary });
                reload();
              }}
            />
          )}

          <LanePanel detail={d} lane={lane} onGate={goToGate} />

          {phases.length > 0 && PHASE_LANES.includes(lane) && (
            <Panel
              title="Phase graph"
              subtitle={`${d.phases.length} phase${d.phases.length === 1 ? "" : "s"}`}
              icon="git-fork"
            >
              <div
                style={{
                  fontSize: "var(--text-sm)",
                  color: "var(--text-muted)",
                  marginBottom: 10,
                  lineHeight: "var(--leading-normal)",
                }}
              >
                A phase starts when every phase it depends on has passed review. Phases of one project run one at a
                time; phases in different projects run in parallel. If a phase fails, the phases that depend on it are
                removed from the queue.
              </div>
              <PhaseDag
                layers={phases}
                onOpen={(p) => {
                  const file = d.phases.find((x) => x.info.id === p.id)?.info.file;
                  if (file) nav.open(`artifact:${file}`, { preview: true });
                }}
              />
            </Panel>
          )}

          {answered.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              <div>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={showAnswered ? "chevron-down" : "chevron-right"}
                  onClick={() => setShowAnswered(!showAnswered)}
                >
                  {showAnswered ? "Hide" : "Show"} answered gates ({answered.length})
                </Button>
              </div>
              {showAnswered && answered.map((g) => <GateCard key={g.id} gate={g} />)}
            </div>
          )}

          <div>
            <Button
              size="sm"
              variant="ghost"
              icon={showLog ? "chevron-down" : "chevron-right"}
              onClick={() => setShowLog(!showLog)}
            >
              {showLog ? "Hide" : "Show"} event log ({events.length})
            </Button>
            {showLog && (
              <div style={{ marginTop: 8, overflowX: "auto" }}>
                <pre style={{ margin: 0, fontSize: "var(--text-sm)", color: "var(--text-secondary)" }}>
                  {events.map(eventLine).join("\n") || "No events yet."}
                </pre>
              </div>
            )}
          </div>
        </div>
      )}
    </Board>
  );
}
