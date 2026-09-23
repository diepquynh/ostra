import { useNavigate } from "react-router";
import type { SessionSummary } from "../../api/types";
import { Chip, SessionStatusChip } from "../../components/Status";
import { LANES } from "../../content/stages";
import { formatCost, humanize, relativeTime, truncate } from "../../lib/format";

export function SessionList({ ws, sessions }: { ws: string; sessions: SessionSummary[] }) {
  const navigate = useNavigate();
  if (sessions.length === 0) return <div className="card empty">No sessions yet. Start one with the New task form.</div>;
  return (
    <div className="card" style={{ padding: 0 }}>
      <table>
        <thead>
          <tr>
            <th>Request</th>
            <th>Stage</th>
            <th>Status</th>
            <th>Projects</th>
            <th className="num">Cost</th>
            <th>Updated</th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((s) => (
            <tr key={s.id} className="clickable" onClick={() => navigate(`/w/${ws}/s/${s.id}`)}>
              <td>
                <div>{truncate(s.request, 110)}</div>
                <div className="row small">
                  {s.kind.kind === "init" ? <Chip tone="info">Init</Chip> : s.category && <Chip>{humanize(s.category)}</Chip>}
                  {s.yolo && <Chip tone="warn">YOLO</Chip>}
                </div>
              </td>
              <td>
                <Chip tone="accent">{LANES[s.lane].title}</Chip> <span className="small">{s.stage_label}</span>
              </td>
              <td>
                <SessionStatusChip status={s.status} />
                {s.open_gates > 0 && (
                  <div className="small muted">
                    {s.open_gates} gate{s.open_gates === 1 ? "" : "s"} open
                  </div>
                )}
              </td>
              <td className="small">{s.projects.join(", ")}</td>
              <td className="num">{formatCost(s.cost_usd)}</td>
              <td className="small nowrap muted">{relativeTime(s.updated_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
