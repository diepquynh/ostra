import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router";
import { api } from "../../api";
import type { ExecutionView, SessionEvent, StageCard } from "../../api/types";
import { useCrumbs, useLayout } from "../../components/Layout";
import { Markdown } from "../../components/Markdown";
import { Chip, ErrorBox, ExecStatusChip, Loading, SessionStatusChip, StageStatusChip } from "../../components/Status";
import { LANES, STAGES } from "../../content/stages";
import { LANE_ORDER, activeSecurityBlocks, currentGate, describeEvent, openGates, splitDecidedForYou, stagesByLane } from "../../lib/events";
import { elapsed, formatCost, formatTime, humanize, truncate } from "../../lib/format";
import { useAsync, useChannel, useThrottled } from "../../lib/hooks";
import { BlockerNotice } from "../gates/BlockerNotice";
import { GateCard } from "../gates/GateCard";
import { Decisions } from "./Decisions";
import { PhaseDag } from "./PhaseDag";

function StageDetail({ stage, executions, ws }: { stage: StageCard; executions: ExecutionView[]; ws: string }) {
  const info = STAGES[stage.stage];
  const execs = executions.filter((e) => stage.executions.includes(e.id));
  return (
    <div className="card highlight">
      <div className="row between">
        <h3>{stage.label}</h3>
        <StageStatusChip status={stage.status} />
      </div>
      <p>
        <strong>Produces:</strong> {info.produces}
      </p>
      <p>
        <strong>Protects against:</strong> {info.protects}
      </p>
      {stage.detail && <p className="small muted">{stage.detail}</p>}
      {execs.length > 0 && (
        <table>
          <tbody>
            {execs.map((e) => (
              <tr key={e.id}>
                <td>
                  <Link to={`/w/${ws}/x/${e.id}`}>{humanize(e.agent)}</Link>
                </td>
                <td className="small mono">{e.executor}</td>
                <td>
                  <ExecStatusChip status={e.status} />
                </td>
                <td className="small muted">{elapsed(e.started_at, e.ended_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Lanes({ stages, onSelect, selected }: { stages: StageCard[]; onSelect: (s: StageCard) => void; selected: StageCard | null }) {
  const byLane = stagesByLane(stages);
  const active = stages.find((s) => s.status === "waiting") ?? stages.find((s) => s.status === "running");
  return (
    <div className="board">
      {LANE_ORDER.map((lane) => {
        const items = byLane.get(lane) ?? [];
        return (
          <div className={`lane ${active?.lane === lane ? "current" : ""}`} key={lane}>
            <div className="lane-head">
              <span>{LANES[lane].title}</span>
              <span className="muted small">{items.length || ""}</span>
            </div>
            <div className="lane-why">{LANES[lane].why}</div>
            {items.map((s, i) => (
              <div
                key={`${s.stage}-${s.project}-${s.phase}-${i}`}
                className={`stage ${s.status} ${selected === s ? "selected" : ""}`}
                onClick={() => onSelect(s)}
                title={STAGES[s.stage].produces}
              >
                <div>{s.label}</div>
                <div className="row small muted">
                  {s.project && <span>{s.project}</span>}
                  {s.detail && <span>{truncate(s.detail, 40)}</span>}
                </div>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}

function AmendBox({ session, onDone }: { session: string; onDone: () => void }) {
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!open)
    return (
      <button className="small" onClick={() => setOpen(true)}>
        Change the request
      </button>
    );
  return (
    <div className="card">
      <h3>Change the request</h3>
      <p className="small muted">
        A requirement change goes through research for the new part, then the spec is rewritten and approved again. If a plan exists, a
        new plan is written from the updated spec.
      </p>
      <textarea rows={3} value={text} onChange={(e) => setText(e.target.value)} />
      {error && <div className="form-error">{error}</div>}
      <div className="row mt">
        <button
          className="primary"
          disabled={!text.trim()}
          onClick={async () => {
            try {
              await api.amend(session, text.trim());
              setText("");
              setOpen(false);
              onDone();
            } catch (e) {
              setError((e as Error).message);
            }
          }}
        >
          Send change
        </button>
        <button onClick={() => setOpen(false)}>Cancel</button>
      </div>
    </div>
  );
}

export function BoardPage() {
  const { ws = "", id = "" } = useParams();
  const detail = useAsync(() => api.session(id), [id]);
  const events = useAsync(() => api.events(id), [id]);
  const [live, setLive] = useState<SessionEvent[]>([]);
  const [selected, setSelected] = useState<StageCard | null>(null);
  const [showTimeline, setShowTimeline] = useState(false);
  const { setAskSession } = useLayout();
  const location = useLocation();
  const navigate = useNavigate();
  const ws_ = detail.data?.summary.workspace ?? ws;

  useEffect(() => {
    setAskSession(id);
    return () => setAskSession(null);
  }, [id, setAskSession]);
  useEffect(() => setLive([]), [events.data]);

  const refresh = useThrottled(() => detail.reload(), 400);
  useChannel(`session:${id}`, (m) => {
    if (m.type === "session_event") {
      setLive((l) => [...l, m.event]);
      refresh();
    }
    if (m.type === "session_updated" && detail.data) detail.set({ ...detail.data, summary: m.summary });
  });

  useCrumbs([
    { label: "Workspace", to: `/w/${ws_}` },
    { label: detail.data ? truncate(detail.data.summary.request, 50) : "Session" },
  ]);

  useEffect(() => {
    if (location.hash.startsWith("#gate-")) document.getElementById(location.hash.slice(1))?.scrollIntoView({ behavior: "smooth" });
  }, [location.hash, detail.data]);

  const allEvents = useMemo(() => [...(events.data ?? []).map((e) => e.event), ...live], [events.data, live]);
  const blocks = activeSecurityBlocks(allEvents);

  if (detail.error) return <ErrorBox error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return <Loading />;
  const d = detail.data;
  const s = d.summary;
  const open = openGates(d);
  const current = currentGate(d);
  const answered = d.gates.filter((g) => g.answer !== null);
  const completion = d.completion ? splitDecidedForYou(d.completion) : null;

  const toggleYolo = async () => {
    const summary = await api.setYolo(id, !s.yolo);
    detail.set({ ...d, summary });
  };

  return (
    <div className="page">
      <div className="row between">
        <div style={{ minWidth: 0, flex: 1 }}>
          <h1>{s.request}</h1>
          <div className="row small">
            {s.kind.kind === "init" ? <Chip tone="info">Init {s.kind.project}</Chip> : s.category && <Chip>{humanize(s.category)}</Chip>}
            <SessionStatusChip status={s.status} />
            <Chip tone="accent">
              {LANES[s.lane].title}: {s.stage_label}
            </Chip>
            <span className="muted">{s.projects.join(", ")}</span>
            <span className="muted">{formatCost(s.cost_usd)}</span>
            <span className="muted">started {formatTime(s.created_at)}</span>
          </div>
        </div>
        <div className="row">
          <label className="check" title="Ostra answers every gate and permission ask itself, from the next gate or tool call on">
            <input type="checkbox" checked={s.yolo} onChange={() => void toggleYolo()} /> YOLO
          </label>
          {s.status !== "completed" && s.kind.kind === "pipeline" && <AmendBox session={id} onDone={detail.reload} />}
        </div>
      </div>

      {s.yolo && s.status !== "completed" && (
        <div className="banner mt">
          YOLO is on. Ostra answers gates and permission asks itself and records each decision. Guards, deny rules, the fact-check PASS
          requirement, and security blocks still apply.
        </div>
      )}

      {blocks.map((b, i) => (
        <BlockerNotice key={i} block={b} />
      ))}

      {open.length > 0 && (
        <>
          <div className="section-title">Waiting for you</div>
          {[current!, ...open.filter((g) => g !== current)].map((g) => (
            <GateCard key={g.id} gate={g} highlight={g === current} onAnswered={() => detail.reload()} />
          ))}
        </>
      )}

      {completion && (
        <>
          <div className="section-title">Completion report</div>
          <div className="card">
            <Markdown text={completion.body} />
          </div>
          {completion.decided && (
            <div className="card highlight">
              <h2>Decided for you</h2>
              <p className="small muted">Each decision Ostra made under YOLO, with its reason.</p>
              <Markdown text={completion.decided} />
            </div>
          )}
        </>
      )}

      <div className="section-title">Pipeline</div>
      <Lanes stages={d.stages} selected={selected} onSelect={(st) => setSelected(selected === st ? null : st)} />
      {selected && <StageDetail stage={selected} executions={d.executions} ws={ws_} />}

      {(d.phases.length > 0 || d.stages.some((st) => st.lane === "build")) && (
        <>
          <div className="section-title">Build lane: phase graph</div>
          <div className="card">
            <p className="small muted">
              A phase starts when every phase it depends on has passed review. Phases of one project run one at a time; phases in
              different projects run in parallel. If a phase fails, the phases that depend on it are removed from the queue.
            </p>
            <PhaseDag
              phases={d.phases}
              onOpen={(p) => p.info.file && navigate(`/w/${ws_}/artifact?path=${encodeURIComponent(p.info.file)}`)}
            />
          </div>
        </>
      )}

      <div className="split">
        <div>
          <div className="section-title">Decisions Ostra made</div>
          <p className="small muted">
            These are the only points where a model decides what the pipeline does next. You can override any decision until work that
            depends on it starts.
          </p>
          <Decisions decisions={d.decisions} onChange={detail.reload} />
        </div>
        <div>
          <div className="section-title">Artifacts</div>
          <div className="card">
            {d.artifacts.length === 0 && <div className="muted small">No artifacts yet.</div>}
            <ul style={{ margin: 0, paddingLeft: 18 }}>
              {d.artifacts.map((a) => (
                <li key={a.path}>
                  <Link to={`/w/${ws_}/artifact?path=${encodeURIComponent(a.path)}`}>{a.label}</Link>{" "}
                  <span className="muted small">
                    {a.kind}
                    {a.project ? `, ${a.project}` : ""}
                  </span>
                </li>
              ))}
            </ul>
          </div>
          <div className="section-title">Executions</div>
          <div className="card" style={{ padding: 0 }}>
            <table>
              <tbody>
                {d.executions.map((e) => (
                  <tr key={e.id} className="clickable" onClick={() => navigate(`/w/${ws_}/x/${e.id}`)}>
                    <td>{humanize(e.agent)}</td>
                    <td className="small">{e.project}</td>
                    <td className="small mono">{e.executor}</td>
                    <td>
                      <ExecStatusChip status={e.status} />
                    </td>
                    <td className="num small">{formatCost(e.usage.cost_usd)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>

      {answered.length > 0 && (
        <>
          <div className="section-title">Answered gates</div>
          {answered.map((g) => (
            <GateCard key={g.id} gate={g} />
          ))}
        </>
      )}

      <div className="section-title">
        <button className="ghost small" onClick={() => setShowTimeline((v) => !v)}>
          {showTimeline ? "Hide" : "Show"} event log ({allEvents.length})
        </button>
      </div>
      {showTimeline && (
        <div className="card">
          <ol className="small" style={{ margin: 0, paddingLeft: 20 }}>
            {allEvents.map((e, i) => (
              <li key={i}>{describeEvent(e)}</li>
            ))}
          </ol>
        </div>
      )}
    </div>
  );
}
