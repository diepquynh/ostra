import { Banner, Chip, Decision, Icon, Spinner, StatusChip, StatusDot, Switch } from "@ostra/design";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../../api";
import type {
  ContextDelivery,
  ContextFile,
  DecisionView,
  GateView,
  Lane,
  SessionDetail,
  SessionSummary,
  StoredEvent,
} from "../../api/types";
import { Markdown } from "../../components/Markdown";
import { LANES } from "../../content/stages";
import { FileTagInput } from "../../features/context/FileTagInput";
import { TaggedText, UntaggedFiles } from "../../features/context/TaggedText";
import { UploadChip, useUploads } from "../../features/context/uploads";
import { BlockerNotice } from "../../features/gates/BlockerNotice";
import { GateCard } from "../../features/gates/GateCard";
import { activeSecurityBlocks, decisionChoice, splitDecidedForYou } from "../../lib/events";
import { formatCost, humanize } from "../../lib/format";
import { useAsync, useChannel, useThrottled } from "../../lib/hooks";
import { useNav } from "../../lib/nav";
import {
  answeredGates,
  defaultLane,
  eventLine,
  laneStates,
  mergeEvents,
  openGatesInOrder,
  stageKey,
  stageMeta,
} from "../../screens/session/board";
import { OverrideDialog } from "../../screens/session/SidePanels";
import { artifactIcon, EXEC_TONE, execGroups, laneCards, phaseWaves, STAGE_TONE, stageTarget } from "./sessionView";
import "./MSession.css";

type Tab = "phases" | "exec" | "art" | "dec" | "log";

const hhmm = (iso: string) => new Date(iso).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
const lowerFirst = (s: string) => s.replace(/^[A-Z](?![A-Z])/, (c) => c.toLowerCase());

/** `session:<id>`: the session board. A `#gate-<id>` URL hash scrolls to that gate. */
export function MSession({ ws, id }: { ws: string; id: string }) {
  const detail = useAsync(() => api.session(id), [id]);
  const events = useAsync(() => api.events(id), [id]);
  const [live, setLive] = useState<StoredEvent[]>([]);
  const refresh = useThrottled(() => detail.reload(), 400);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new snapshot replaces the live events it contains.
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
      <div className="m-page">
        <Banner tone="bad" title="Ostra could not load this session">
          {detail.error.message}
        </Banner>
        <button type="button" className="m-btn" onClick={detail.reload}>
          Try again
        </button>
      </div>
    );
  if (!detail.data)
    return (
      <div className="m-page">
        <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size={11} /> Reading the session…
        </span>
      </div>
    );
  return (
    <Board
      ws={ws}
      d={detail.data}
      events={mergeEvents(events.data ?? [], live)}
      onDetail={detail.set}
      reload={() => {
        detail.reload();
        events.reload();
      }}
    />
  );
}

function Board({
  ws,
  d,
  events,
  onDetail,
  reload,
}: {
  ws: string;
  d: SessionDetail;
  events: StoredEvent[];
  onDetail: (d: SessionDetail) => void;
  reload: () => void;
}) {
  const nav = useNav();
  const location = useLocation();
  const s = d.summary;
  const ended = s.status === "completed" || s.status === "failed";
  const running = d.executions.filter((x) => x.status === "running").length;
  const [lane, setLane] = useState<Lane>(() => defaultLane(d));
  const [tab, setTab] = useState<Tab>(() => (d.phases.length ? "phases" : "exec"));
  const [showAnswered, setShowAnswered] = useState(false);
  const [editing, setEditing] = useState<DecisionView | null>(null);

  const states = useMemo(() => laneStates(d), [d]);
  const cards = laneCards(states);
  const open = openGatesInOrder(d);
  const answered = answeredGates(d);
  const blocks = activeSecurityBlocks(events.map((e) => e.event));
  const completion = d.completion ? splitDecidedForYou(d.completion) : null;
  const waves = phaseWaves(d.phases);
  const groups = execGroups(d);
  const sessionProjects = useMemo(() => {
    const created = events.find((e) => e.event.type === "session_created")?.event;
    return created?.type === "session_created" ? created.projects.map((p) => p.key) : s.projects;
  }, [events, s.projects]);

  const laneStrip = useRef<HTMLDivElement>(null);
  // Keep the selected lane card in view, since the strip is wider than a phone.
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the selected lane changes.
  useEffect(() => {
    const strip = laneStrip.current;
    const card = strip?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (strip && card) strip.scrollLeft += card.getBoundingClientRect().left - strip.getBoundingClientRect().left - 16;
  }, [lane]);

  const scrollToGate = (gateId: string) => {
    if (d.gates.find((g) => g.id === gateId)?.answer) setShowAnswered(true);
    requestAnimationFrame(() =>
      document.getElementById(`gate-${gateId}`)?.scrollIntoView({ behavior: "smooth", block: "start" }),
    );
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: only on a new hash, not on every live refresh.
  useEffect(() => {
    if (location.hash.startsWith("#gate-")) scrollToGate(location.hash.slice(6));
  }, [location.hash]);

  const answeredGate = (g: GateView) => {
    onDetail({ ...d, gates: d.gates.map((x) => (x.id === g.id ? g : x)) });
    reload();
  };
  const onSummary = (summary: SessionSummary) => onDetail({ ...d, summary });

  const counts: Record<Tab, number> = {
    phases: d.phases.length,
    exec: d.executions.length,
    art: d.artifacts.length,
    dec: d.decisions.length,
    log: events.length,
  };
  const TABS: [Tab, string][] = [
    ["phases", "Phases"],
    ["exec", "Executions"],
    ["art", "Artifacts"],
    ["dec", "Decisions"],
    ["log", "Event log"],
  ];
  const EMPTY: Record<Tab, string> = {
    phases: "No plan yet. Phases appear here after you approve the plan.",
    exec: "No executions yet.",
    art: "No artifacts yet. Research documents appear here first.",
    dec: "No decisions yet. Every judge decision shows here with its reason.",
    log: "No events yet.",
  };

  return (
    <div className="ms-board">
      <div className="ms-pad ms-head">
        {s.request && (
          <p className="ms-request">
            <TaggedText text={s.request} files={d.files} /> <UntaggedFiles text={s.request} files={d.files} />
          </p>
        )}
        {d.uploads.length > 0 && (
          <div className="ms-chips">
            {d.uploads.map((u) => (
              <UploadChip key={u.path} upload={u} />
            ))}
          </div>
        )}
        {d.additions.length > 0 && (
          <ul className="ms-additions" aria-label="Context you added">
            {d.additions.map((a) => (
              <li key={a.at}>
                <span className="ms-additions__meta">
                  {a.delivery === "now" ? "Sent now" : "Queued"} · {hhmm(a.at)}
                </span>
                <span>
                  <TaggedText text={a.text} files={a.files} /> <UntaggedFiles text={a.text} files={a.files} />
                </span>
                {a.uploads.length > 0 && (
                  <span className="ms-chips">
                    {a.uploads.map((u) => (
                      <UploadChip key={u.path} upload={u} />
                    ))}
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
        <div className="ms-chips">
          <StatusChip kind="session" status={s.status} />
          {s.kind.kind === "init" && <Chip tone="info">Init {s.kind.project}</Chip>}
          {s.projects.map((p) => (
            <Chip key={p} mono icon="folder-git-2">
              {p}
            </Chip>
          ))}
          {s.yolo && <Chip tone="warn">YOLO</Chip>}
          <span className="ms-cost">{formatCost(s.cost_usd)}</span>
        </div>
        {!ended && <Controls summary={s} onSummary={onSummary} onChanged={reload} />}
      </div>

      {s.status === "paused" && (
        <div className="ms-pad">
          <Banner tone="info" title="The session is paused">
            No agent runs and nothing new starts. Continue it above, and each paused agent picks up where it stopped.
            You can still answer gates and add context while it is paused.
          </Banner>
        </div>
      )}
      {s.yolo && !ended && (
        <div className="ms-pad">
          <Banner tone="warn" title="YOLO is on">
            Ostra answers gates and permission asks itself and records each decision. Guards, deny rules, the fact-check
            PASS requirement, the session budget, and security blocks still apply.
          </Banner>
        </div>
      )}
      {blocks.map((b) => (
        <div className="ms-pad" key={`${b.project}:${b.phase}:${b.tests}`}>
          <BlockerNotice block={b} />
        </div>
      ))}

      {open.length > 0 && (
        <section className="ms-pad ms-gates" aria-label="Waiting for you">
          {open.map((g) => (
            <GateCard key={g.id} gate={g} onAnswered={answeredGate} />
          ))}
        </section>
      )}

      {completion && (
        <section className="ms-pad">
          <div className="m-card ms-report">
            <span className="ms-card-title">
              <Icon name="flag" size={15} /> Completion report
            </span>
            <Markdown className="os-prose" text={completion.body} />
          </div>
        </section>
      )}
      {completion?.decided && (
        <section className="ms-pad">
          <div className="m-card ms-report ms-report--warn">
            <span className="ms-card-title">
              <Icon name="scale" size={15} /> Decided for you
            </span>
            <span className="m-help">Each decision Ostra made under YOLO, with its reason.</span>
            <Markdown className="os-prose" text={completion.decided} />
          </div>
        </section>
      )}

      {!ended && s.kind.kind === "pipeline" && (
        <div className="ms-pad">
          <AddContext
            ws={ws}
            summary={s}
            projects={sessionProjects}
            running={running}
            onSent={(summary) => {
              onSummary(summary);
              reload();
            }}
          />
        </div>
      )}

      <section className="m-section">
        <span className="m-label ms-pad">Pipeline</span>
        <div className="ms-lanes" role="tablist" aria-label="Lanes" ref={laneStrip}>
          {cards.map((c) => (
            <button
              type="button"
              role="tab"
              aria-selected={c.lane === lane}
              key={c.lane}
              className={`ms-lane${c.lane === lane ? " ms-lane--on" : ""}`}
              onClick={() => setLane(c.lane)}
            >
              <span className={`ms-lane__bar ms-lane__bar--${c.status}`} />
              <span className="ms-lane__name">{LANES[c.lane].title}</span>
              <span className="ms-lane__detail">{c.detail}</span>
            </button>
          ))}
        </div>
        <div className="ms-pad">
          <LaneCard d={d} lane={lane} detail={cards.find((c) => c.lane === lane)?.detail ?? ""} onGate={scrollToGate} />
        </div>
      </section>

      <section className="m-section">
        <div className="ms-tabs" role="tablist" aria-label="Session views">
          {TABS.map(([k, label]) => (
            <button
              type="button"
              role="tab"
              aria-selected={tab === k}
              key={k}
              className={`ms-tab${tab === k ? " ms-tab--on" : ""}`}
              onClick={() => setTab(k)}
            >
              {label}
              {counts[k] > 0 && <span className="ms-tab__count">{counts[k]}</span>}
            </button>
          ))}
        </div>
        <div className="ms-pad ms-tabbody">
          {counts[tab] === 0 && <span className="m-muted">{EMPTY[tab]}</span>}

          {tab === "phases" &&
            waves.map((w) => (
              <div key={w.label} className="ms-wave">
                <span className="ms-wave__label">{w.label}</span>
                {w.phases.map((p) => (
                  <button
                    type="button"
                    key={p.id}
                    className="ms-phase"
                    disabled={!p.file}
                    onClick={() => p.file && nav.open(`artifact:${p.file}`)}
                  >
                    <span className="ms-phase__top">
                      <span className="ms-phase__num">P{p.id}</span>
                      <span className="ms-phase__title">{p.title}</span>
                    </span>
                    <span className="ms-chips">
                      <StatusChip kind="phase" status={p.status} />
                      <Chip mono>{p.project}</Chip>
                      <span className="ms-meta">{p.meta}</span>
                    </span>
                  </button>
                ))}
              </div>
            ))}

          {tab === "exec" &&
            groups.map((g) => (
              <div key={g.key} className="ms-group">
                <div className="ms-group__head">
                  <StatusDot tone={EXEC_TONE[g.status]} pulse={g.status === "running"} />
                  <span className="ms-group__title">
                    {g.agent} <span className="ms-mono-muted">{g.project}</span>
                  </span>
                  <span className="ms-cost">{g.cost}</span>
                </div>
                {g.rows.map((x) => (
                  <button type="button" key={x.id} className="ms-group__row" onClick={() => nav.open(`exec:${x.id}`)}>
                    <Icon name={x.icon} size={14} style={{ color: "var(--text-muted)", flex: "none" }} />
                    <span className="m-row-body">
                      <span style={{ fontSize: 13.5 }}>{x.run}</span>
                      <span className="m-mono-sub" style={{ fontSize: 11 }}>
                        {x.meta}
                      </span>
                    </span>
                    {x.running && <StatusChip kind="execution" status="running" />}
                    <span className="ms-cost">{x.cost}</span>
                  </button>
                ))}
              </div>
            ))}

          {tab === "art" && (
            <div>
              {d.artifacts.map((a) => (
                <button type="button" key={a.path} className="m-row" onClick={() => nav.open(`artifact:${a.path}`)}>
                  <Icon name={artifactIcon(a.kind)} size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
                  <span className="m-row-body">
                    <span style={{ fontSize: 14 }}>{a.label}</span>
                    <span className="m-mono-sub" style={{ fontSize: 11 }}>
                      {a.path}
                    </span>
                  </span>
                  <Icon name="chevron-right" size={16} style={{ color: "var(--text-muted)", flex: "none" }} />
                </button>
              ))}
            </div>
          )}

          {tab === "dec" && d.decisions.length > 0 && (
            <div className="ms-decisions">
              {d.decisions.map((x) => (
                <Decision
                  key={x.id}
                  judge={humanize(x.judge)}
                  choice={decisionChoice(x)}
                  reason={lowerFirst(x.reason)}
                  basis={x.input_summary}
                  at={hhmm(x.at)}
                  overridden={x.overridden}
                  canOverride={x.can_override}
                  onOverride={() => setEditing(x)}
                />
              ))}
            </div>
          )}

          {tab === "log" && events.length > 0 && <pre className="ms-log">{events.map(eventLine).join("\n")}</pre>}
        </div>
      </section>

      {answered.length > 0 && (
        <section className="ms-pad m-section">
          <button type="button" className="ms-toggle" onClick={() => setShowAnswered(!showAnswered)}>
            <Icon name={showAnswered ? "chevron-down" : "chevron-right"} size={14} />
            {showAnswered ? "Hide" : "Show"} answered gates ({answered.length})
          </button>
          {showAnswered && (
            <div className="ms-gates">
              {answered.map((g) => (
                <GateCard key={g.id} gate={g} />
              ))}
            </div>
          )}
        </section>
      )}

      {editing && (
        <OverrideDialog
          decision={editing}
          onClose={() => setEditing(null)}
          onDone={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
    </div>
  );
}

/** YOLO, pause or continue, and stop, with an inline confirmation before stopping. */
function Controls({
  summary: s,
  onSummary,
  onChanged,
}: {
  summary: SessionSummary;
  onSummary: (s: SessionSummary) => void;
  onChanged: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const paused = s.status === "paused";
  const run = (p: Promise<SessionSummary>, after?: () => void) => {
    setError(null);
    setBusy(true);
    p.then(
      (next) => {
        onSummary(next);
        onChanged();
        after?.();
      },
      (e: Error) => setError(e.message),
    ).finally(() => setBusy(false));
  };
  return (
    <div className="ms-controls">
      <div className="ms-controls__row">
        <Switch
          label="YOLO"
          tone="warn"
          checked={s.yolo}
          onChange={() => {
            setError(null);
            api.setYolo(s.id, !s.yolo).then(onSummary, (e: Error) => setError(e.message));
          }}
        />
        <span style={{ flex: 1 }} />
        <button
          type="button"
          className={`m-btn m-btn-sm${paused ? " m-btn-primary" : ""}`}
          disabled={busy}
          onClick={() => run(paused ? api.resumeSession(s.id) : api.pauseSession(s.id))}
        >
          <Icon name={paused ? "play" : "pause"} size={14} />
          {paused ? "Continue" : "Pause"}
        </button>
        <button
          type="button"
          className="m-btn m-btn-sm"
          aria-label="Stop the session"
          disabled={busy}
          onClick={() => setConfirmStop(true)}
        >
          <Icon name="square" size={14} />
          Stop
        </button>
      </div>
      {confirmStop && (
        <Confirm
          title="Stop the session?"
          keep="Keep it running"
          go="Stop the session"
          busy={busy}
          onKeep={() => setConfirmStop(false)}
          onGo={() => run(api.stopSession(s.id), () => setConfirmStop(false))}
        >
          Ostra cancels every running execution, denies any waiting permission ask, and marks the session failed. Files
          the agents already wrote stay in the project.
        </Confirm>
      )}
      {error && (
        <span className="ms-error" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

function Confirm({
  title,
  keep,
  go,
  busy,
  onKeep,
  onGo,
  children,
}: {
  title: string;
  keep: string;
  go: string;
  busy: boolean;
  onKeep: () => void;
  onGo: () => void;
  children: ReactNode;
}) {
  return (
    <div className="ms-confirm" role="alertdialog" aria-label={title}>
      <span style={{ fontSize: 14, fontWeight: 600 }}>{title}</span>
      <span style={{ fontSize: 13, lineHeight: 1.55, textWrap: "pretty" }}>{children}</span>
      <div style={{ display: "flex", gap: 8 }}>
        <button
          type="button"
          className="m-btn"
          style={{ flex: 1, background: "var(--surface-raised)" }}
          onClick={onKeep}
        >
          {keep}
        </button>
        <button type="button" className="m-btn ms-btn-danger" style={{ flex: 1 }} disabled={busy} onClick={onGo}>
          {go}
        </button>
      </div>
    </div>
  );
}

/** Add text, tagged files and uploads to a running session: queued for the next step, or sent now. */
function AddContext({
  ws,
  summary,
  projects,
  running,
  onSent,
}: {
  ws: string;
  summary: SessionSummary;
  projects: string[];
  running: number;
  onSent: (s: SessionSummary) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [text, setText] = useState("");
  const [files, setFiles] = useState<ContextFile[]>([]);
  const uploads = useUploads(ws);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const empty = !text.trim() && files.length === 0 && uploads.ids.length === 0;

  const send = (delivery: ContextDelivery) => {
    if (empty) return setError("Write the context first, tag a file with @, or upload one.");
    setBusy(true);
    setError(null);
    api
      .amend(summary.id, { text: text.trim(), files, uploads: uploads.ids, delivery })
      .then((s) => {
        setText("");
        uploads.clear();
        setConfirm(false);
        setExpanded(false);
        onSent(s);
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  if (!expanded)
    return (
      <div className="ms-ctx">
        <button type="button" className="ms-ctx__open" onClick={() => setExpanded(true)}>
          <Icon name="message-square-plus" size={16} style={{ color: "var(--text-muted)" }} />
          <span className="m-row-body">
            <span style={{ fontSize: 14, fontWeight: 500 }}>Add context</span>
            <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
              Text, @file tags and uploads for the agents
            </span>
          </span>
          <Icon name="chevron-down" size={15} style={{ color: "var(--text-muted)" }} />
        </button>
      </div>
    );

  return (
    <div className="ms-ctx ms-ctx--open">
      <div style={{ display: "flex", alignItems: "center" }}>
        <span style={{ flex: 1, fontSize: 14, fontWeight: 600 }}>Add context</span>
        <span style={{ fontSize: 12, color: "var(--text-muted)" }}>type @ to tag a file</span>
      </div>
      <FileTagInput
        ws={ws}
        projects={projects}
        label="Context"
        rows={3}
        value={text}
        onChange={setText}
        onFiles={setFiles}
        uploads={uploads.items}
        onUploadFiles={uploads.add}
        onRemoveUpload={uploads.remove}
        disabled={busy}
        placeholder="Refunds follow the same rule. See @backend/src/payments/refund.ts"
      />
      <span className="m-help">
        Queued context reaches the agents at the next step, and running agents finish first. A requirement change goes
        through research, then the spec is rewritten and approved again.
      </span>
      {confirm ? (
        <Confirm
          title="Interrupt running work?"
          keep="Keep them running"
          go="Interrupt and send"
          busy={busy}
          onKeep={() => setConfirm(false)}
          onGo={() => send("now")}
        >
          Ostra stops the {running} running execution{running === 1 ? "" : "s"} now. Each one starts again from the
          beginning with your context, so the progress it made in this run is lost. Files it already wrote stay in the
          project, and what it spent stays on the bill.
        </Confirm>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <button
            type="button"
            className="m-btn m-btn-primary"
            disabled={busy || uploads.busy || empty}
            onClick={() => send("queue")}
          >
            <Icon name="plus" size={15} />
            Queue for the next step
          </button>
          <div style={{ display: "flex", gap: 8 }}>
            <button type="button" className="m-btn m-btn-quiet" onClick={() => setExpanded(false)}>
              Cancel
            </button>
            <button
              type="button"
              className="m-btn"
              style={{ flex: 1 }}
              disabled={busy || uploads.busy || empty || running === 0}
              onClick={() => setConfirm(true)}
            >
              <Icon name="octagon-alert" size={15} />
              Send now
            </button>
          </div>
          {running === 0 && !empty && (
            <span className="m-help">Nothing is running, so queued context is used right away.</span>
          )}
        </div>
      )}
      {error && (
        <span className="ms-error" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/** The selected lane: its stages, each opening its gate or execution, and why the lane exists. */
function LaneCard({
  d,
  lane,
  detail,
  onGate,
}: {
  d: SessionDetail;
  lane: Lane;
  detail: string;
  onGate: (id: string) => void;
}) {
  const nav = useNav();
  const [why, setWhy] = useState(false);
  const execs = new Map(d.executions.map((x) => [x.id, x]));
  const stages = d.stages.map((s, i) => ({ s, key: stageKey(s, i) })).filter(({ s }) => s.lane === lane);
  return (
    <div className="ms-lanecard">
      <div className="ms-lanecard__head">
        <span style={{ fontSize: 14, fontWeight: 600, flex: 1 }}>{LANES[lane].title}</span>
        <span style={{ fontSize: 12, color: "var(--text-muted)" }}>{detail}</span>
      </div>
      {stages.map(({ s, key }) => {
        const target = stageTarget(s, execs);
        const meta = stageMeta(s, execs);
        const body = (
          <>
            <StatusDot tone={STAGE_TONE[s.status]} pulse={s.status === "running"} hollow={s.status === "pending"} />
            <span className="ms-stage__label">{s.label}</span>
            {meta && <span className="ms-stage__meta">{meta}</span>}
          </>
        );
        return target ? (
          <button
            type="button"
            key={key}
            className="ms-stage ms-stage--tap"
            onClick={() => ("gate" in target ? onGate(target.gate) : nav.open(`exec:${target.exec}`))}
          >
            {body}
            <Icon name="chevron-right" size={14} style={{ color: "var(--text-muted)", flex: "none" }} />
          </button>
        ) : (
          <div key={key} className="ms-stage">
            {body}
          </div>
        );
      })}
      {stages.length === 0 && <span className="ms-lanecard__empty">This lane has not started.</span>}
      <button type="button" className="ms-why" aria-expanded={why} onClick={() => setWhy(!why)}>
        <Icon name={why ? "chevron-down" : "chevron-right"} size={14} />
        Why this step exists
      </button>
      {why && <p className="ms-why__text">{LANES[lane].why}</p>}
    </div>
  );
}
