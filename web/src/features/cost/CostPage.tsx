import { Link, useParams } from "react-router";
import { api } from "../../api";
import type { CostRow } from "../../api/types";
import { useCrumbs } from "../../components/Layout";
import { ErrorBox, Loading } from "../../components/Status";
import { formatCost, formatDuration, formatTokens } from "../../lib/format";
import { useAsync } from "../../lib/hooks";
import { WorkspaceTabs } from "../workspaces/WorkspaceTabs";

function CostTable({ title, rows, link }: { title: string; rows: CostRow[]; link?: (key: string) => string }) {
  const sorted = [...rows].sort((a, b) => b.usage.cost_usd - a.usage.cost_usd);
  return (
    <div className="card" style={{ padding: 0 }}>
      <h3 style={{ padding: "10px 12px 0" }}>{title}</h3>
      <table>
        <thead>
          <tr>
            <th>{title.replace(/^By /, "")}</th>
            <th className="num">Runs</th>
            <th className="num">Input</th>
            <th className="num">Output</th>
            <th className="num">Cache reads</th>
            <th className="num" title="Cache reads divided by tool calls. A rising value means each tool call re-reads more context.">
              Cache reads per tool call
            </th>
            <th className="num">Build time</th>
            <th className="num">Cost</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((r) => (
            <tr key={r.key}>
              <td className="mono small">{link ? <Link to={link(r.key)}>{r.key}</Link> : r.key}</td>
              <td className="num">{r.executions}</td>
              <td className="num">{formatTokens(r.usage.input_tokens)}</td>
              <td className="num">{formatTokens(r.usage.output_tokens)}</td>
              <td className="num">{formatTokens(r.usage.cache_read_tokens)}</td>
              <td className="num">{formatTokens(r.cache_reads_per_tool_call)}</td>
              <td className="num">{r.usage.build_ms ? formatDuration(r.usage.build_ms) : ""}</td>
              <td className="num">{formatCost(r.usage.cost_usd)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function CostPage() {
  const { ws = "" } = useParams();
  const report = useAsync(() => api.cost(ws), [ws]);
  useCrumbs([{ label: "Workspace", to: `/w/${ws}` }, { label: "Cost" }]);
  if (report.error) return <ErrorBox error={report.error} onRetry={report.reload} />;
  if (!report.data) return <Loading />;
  const r = report.data;
  return (
    <div className="page">
      <h1>Cost</h1>
      <WorkspaceTabs ws={ws} />
      <p className="muted small">
        Two numbers matter most. Cache reads per tool call show how much context each step re-reads, which grows when an agent loops.
        Build time shows how long agents spend in build and test commands; a run with many consecutive build failures is where spend
        goes without progress.
      </p>
      <div className="row mb">
        <div className="card">
          <div className="muted small">Total</div>
          <h2>{formatCost(r.total.usage.cost_usd)}</h2>
          <div className="small">
            {r.total.executions} runs · {formatTokens(r.total.usage.cache_read_tokens)} cache reads · {r.total.usage.tool_calls} tool calls
          </div>
        </div>
      </div>
      <div className="stack">
        <CostTable title="By session" rows={r.by_session} link={(k) => `/w/${ws}/s/${k}`} />
        <CostTable title="By stage" rows={r.by_stage} />
        <CostTable title="By agent" rows={r.by_agent} />
        <CostTable title="By executor" rows={r.by_executor} />
      </div>
    </div>
  );
}
