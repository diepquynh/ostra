import type { FactCheckFinding, ReviewFinding } from "../../api/types";
import { Chip } from "../../components/Status";

const tone = (s: string) => (s === "BLOCKER" || s === "HIGH" ? "bad" : s === "MEDIUM" ? "warn" : "");

export function ReviewFindings({ findings }: { findings: ReviewFinding[] }) {
  if (findings.length === 0) return <div className="muted small">No findings.</div>;
  return (
    <table>
      <thead>
        <tr>
          <th>Severity</th>
          <th>Where</th>
          <th>Finding</th>
        </tr>
      </thead>
      <tbody>
        {findings.map((f, i) => (
          <tr key={i}>
            <td>
              <Chip tone={tone(f.severity)}>{f.severity}</Chip>
              <div className="mono small">{f.rule}</div>
            </td>
            <td className="mono small">{f.file}</td>
            <td>
              <div>{f.description}</div>
              <div className="small muted">
                Fix: <span className="mono">{f.fix}</span>
              </div>
              {f.guidance && <div className="small">Guidance: {f.guidance}</div>}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function FactFindings({ findings }: { findings: FactCheckFinding[] }) {
  if (findings.length === 0) return null;
  return (
    <table>
      <thead>
        <tr>
          <th>Severity</th>
          <th>Location</th>
          <th>Claim and issue</th>
        </tr>
      </thead>
      <tbody>
        {findings.map((f, i) => (
          <tr key={i}>
            <td>
              <Chip tone={tone(f.severity)}>{f.severity}</Chip>
            </td>
            <td className="small">{f.location}</td>
            <td>
              <div>{f.claim}</div>
              <div className="small muted">{f.issue}</div>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
