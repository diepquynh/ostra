import { useState, type ReactNode } from "react";
import { Link, useParams } from "react-router";
import { api } from "../../api";
import type { GateAnswer, GatePayload, GateView, PermissionAnswer } from "../../api/types";
import { Chip } from "../../components/Status";
import { humanize } from "../../lib/format";
import {
  DISPOSITIONS,
  OTHER,
  approval,
  changeRequest,
  choice,
  closingAnswer,
  defaultSelections,
  orderedOptions,
  permission,
  questionsAnswer,
  skillsAnswer,
  toggleSelection,
  type QuestionSelection,
} from "../../lib/gateAnswers";
import { FactFindings, ReviewFindings } from "./Findings";

type Submit = (answer: GateAnswer) => Promise<void>;

function ArtifactLink({ path, label }: { path: string; label: string }) {
  const { ws } = useParams();
  return <Link to={`/w/${ws}/artifact?path=${encodeURIComponent(path)}`}>{label}</Link>;
}

function OpenQuestions({ payload, submit }: { payload: Extract<GatePayload, { kind: "open_questions" }>; submit: Submit }) {
  const [sel, setSel] = useState<Record<string, QuestionSelection>>(() => defaultSelections(payload.questions));
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="stack">
      <p className="small">
        Answers are written back into the <ArtifactLink path={payload.artifact_path} label={payload.artifact} /> by its own agent, so
        every later stage reads them from the file.
      </p>
      {payload.questions.map((q) => {
        const s = sel[q.id];
        const set = (next: QuestionSelection) => setSel({ ...sel, [q.id]: next });
        return (
          <fieldset key={q.id} className="card">
            <div className="row">
              <Chip>{q.id}</Chip>
              <Chip tone="info">{q.tag}</Chip>
            </div>
            <p>
              <strong>{q.question}</strong>
            </p>
            {orderedOptions(q).map((o) => (
              <label key={o.label} className="check" style={{ display: "flex", alignItems: "flex-start", marginBottom: 4 }}>
                <input
                  type={q.multi_select ? "checkbox" : "radio"}
                  name={q.id}
                  checked={s.selected.includes(o.label)}
                  onChange={() => set(toggleSelection(s, o.label, q.multi_select))}
                />
                <span>
                  {o.label}
                  {o.recommended && <span className="chip ok" style={{ marginLeft: 6 }}>Recommended</span>}
                  <div className="small muted">{o.description}</div>
                </span>
              </label>
            ))}
            <label className="check">
              <input
                type={q.multi_select ? "checkbox" : "radio"}
                name={q.id}
                checked={s.selected.includes(OTHER)}
                onChange={() => set(toggleSelection(s, OTHER, q.multi_select))}
              />
              Other
            </label>
            {s.selected.includes(OTHER) && (
              <textarea rows={2} value={s.other} placeholder="Your answer" onChange={(e) => set({ ...s, other: e.target.value })} />
            )}
          </fieldset>
        );
      })}
      {error && <div className="form-error">{error}</div>}
      <div>
        <button
          className="primary"
          onClick={() => {
            const r = questionsAnswer(payload.questions, sel);
            if ("error" in r) setError(r.error);
            else void submit(r.answer);
          }}
        >
          Send answers
        </button>
      </div>
    </div>
  );
}

function Approval({
  what,
  path,
  summary,
  children,
  submit,
}: {
  what: "spec" | "plan";
  path: string;
  summary: string;
  children?: ReactNode;
  submit: Submit;
}) {
  const [feedback, setFeedback] = useState("");
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="stack">
      <div className="row">
        <Chip tone="ok">Fact-check PASS</Chip>
        <ArtifactLink path={path} label={`Read the ${what}`} />
      </div>
      <p>{summary}</p>
      {children}
      <p className="small muted">
        {what === "spec"
          ? "Approving lets the plan agent turn this spec into phases. A change request goes back to the spec agent, then through fact-check again."
          : "Approving starts implementation. A change to a requirement goes back into the spec first, then a new plan is written."}
      </p>
      <textarea
        rows={2}
        value={feedback}
        placeholder={`What should change in the ${what}? Leave empty to approve.`}
        onChange={(e) => setFeedback(e.target.value)}
      />
      {error && <div className="form-error">{error}</div>}
      <div className="row">
        <button className="primary" disabled={Boolean(feedback.trim())} onClick={() => void submit(approval(true))}>
          Approve the {what}
        </button>
        <button
          onClick={() => {
            const r = changeRequest(feedback);
            if ("error" in r) setError(r.error);
            else void submit(r.answer);
          }}
        >
          Request changes
        </button>
      </div>
    </div>
  );
}

function Choices({
  options,
  submit,
  textLabel,
  textFor,
}: {
  options: { value: string; label: string; primary?: boolean; needsText?: boolean }[];
  submit: Submit;
  textLabel?: string;
  textFor?: string[];
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="stack">
      {textLabel && (
        <label className="field">
          <span className="label">{textLabel}</span>
          <textarea rows={3} value={text} onChange={(e) => setText(e.target.value)} />
        </label>
      )}
      {error && <div className="form-error">{error}</div>}
      <div className="row">
        {options.map((o) => (
          <button
            key={o.value}
            className={o.primary ? "primary" : ""}
            onClick={() => {
              if (o.needsText && !text.trim()) {
                setError(`${o.label} needs the text above.`);
                return;
              }
              void submit(choice(o.value, textFor?.includes(o.value) ? text : undefined));
            }}
          >
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function Closing({ payload, submit }: { payload: Extract<GatePayload, { kind: "closing_gate" }>; submit: Submit }) {
  const [picks, setPicks] = useState<Record<string, { tests: boolean; docs: boolean }>>({});
  const pick = (p: string) => picks[p] ?? { tests: false, docs: false };
  return (
    <div className="stack">
      {payload.items.map((item) => (
        <div key={item.project} className="card">
          <strong>{item.project}</strong>{" "}
          <span className="muted small">
            All {item.phases} phase{item.phases === 1 ? " is" : "s are"} implemented and reviewed.
          </span>
          <div className="stack mt">
            {item.ask_tests && (
              <label className="check">
                <input
                  type="checkbox"
                  checked={pick(item.project).tests}
                  onChange={(e) => setPicks({ ...picks, [item.project]: { ...pick(item.project), tests: e.target.checked } })}
                />
                Write tests for them (covers phases tagged Required)
              </label>
            )}
            {item.ask_docs && (
              <label className="check">
                <input
                  type="checkbox"
                  checked={pick(item.project).docs}
                  onChange={(e) => setPicks({ ...picks, [item.project]: { ...pick(item.project), docs: e.target.checked } })}
                />
                Update the module documentation for the changed areas
              </label>
            )}
          </div>
        </div>
      ))}
      <p className="small muted">
        Leaving both unchecked is the recommended default. The completion report says how to run either stage later. This choice never
        changes the spec.
      </p>
      <div>
        <button className="primary" onClick={() => void submit(closingAnswer(payload.items, picks))}>
          Continue
        </button>
      </div>
    </div>
  );
}

function Permission({ payload, submit }: { payload: Extract<GatePayload, { kind: "permission" }>; submit: Submit }) {
  const p = (a: PermissionAnswer) => void submit(permission(a));
  return (
    <div className="stack">
      <div className="row">
        <Chip>{humanize(payload.agent)}</Chip>
        <span className="mono">{payload.call.tool}</span>
        <Link to={`../x/${payload.execution}`} relative="path" className="small">
          Open execution
        </Link>
      </div>
      <pre>{JSON.stringify(payload.call.input, null, 2)}</pre>
      <p>{payload.reason}</p>
      <p className="small muted">
        Rule: {payload.rule.layer} {payload.rule.rule}
      </p>
      <div className="row">
        <button className="primary" onClick={() => p("allow-once")}>
          Allow once
        </button>
        <button onClick={() => p("always-in-workspace")} disabled={!payload.suggestion} title={payload.suggestion ?? ""}>
          Always in this workspace{payload.suggestion ? `: ${payload.suggestion}` : ""}
        </button>
        <button className="danger" onClick={() => p("deny")}>
          Deny
        </button>
      </div>
    </div>
  );
}

function Skills({ payload, submit }: { payload: Extract<GatePayload, { kind: "skill_approval" }>; submit: Submit }) {
  const [picks, setPicks] = useState<Record<string, string>>(() => Object.fromEntries(payload.skills.map((s) => [s.name, s.disposition])));
  return (
    <div className="stack">
      <p className="small">
        Each skill is grounded in exemplar files from this project. <em>Generate</em> writes a new skill, <em>regenerate</em> rewrites
        an existing one, <em>reuse</em> keeps an existing skill as it is, and <em>drop</em> leaves it out.
      </p>
      <table>
        <thead>
          <tr>
            <th>Skill</th>
            <th>Kind</th>
            <th>What it covers</th>
            <th>Decision</th>
          </tr>
        </thead>
        <tbody>
          {payload.skills.map((s) => (
            <tr key={s.name}>
              <td className="mono">{s.name}</td>
              <td>{s.kind}</td>
              <td>
                {s.description}
                {s.exemplars.length > 0 && <div className="small muted mono">{s.exemplars.join(", ")}</div>}
              </td>
              <td>
                <select value={picks[s.name]} onChange={(e) => setPicks({ ...picks, [s.name]: e.target.value })}>
                  {DISPOSITIONS.map((d) => (
                    <option key={d} value={d}>
                      {d}
                    </option>
                  ))}
                </select>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div>
        <button className="primary" onClick={() => void submit(skillsAnswer(payload.skills, picks))}>
          Approve skills
        </button>
      </div>
    </div>
  );
}

function GateBody({ gate, submit }: { gate: GateView; submit: Submit }) {
  const p = gate.payload;
  switch (p.kind) {
    case "open_questions":
      return <OpenQuestions payload={p} submit={submit} />;
    case "spec_approval":
      return (
        <Approval what="spec" path={p.spec_path} summary={p.summary} submit={submit}>
          <FactFindings findings={p.findings} />
        </Approval>
      );
    case "plan_approval":
      return (
        <Approval what="plan" path={p.plan_path} summary={p.summary} submit={submit}>
          <table>
            <thead>
              <tr>
                <th>Phase</th>
                <th>Project</th>
                <th>Complexity</th>
                <th>Tests</th>
                <th>Depends on</th>
              </tr>
            </thead>
            <tbody>
              {p.phases.map((ph) => (
                <tr key={ph.id}>
                  <td>
                    {ph.id}. {ph.title} {ph.deliverable && <Chip>{ph.deliverable}</Chip>}
                  </td>
                  <td>{ph.project}</td>
                  <td>{ph.complexity}</td>
                  <td title={ph.test_rationale ?? ""}>{ph.test_policy}</td>
                  <td>{ph.depends_on === null ? "unclear, runs after earlier phases" : ph.depends_on.join(", ") || "none"}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <FactFindings findings={p.findings} />
        </Approval>
      );
    case "fact_check_recurring":
      return (
        <div className="stack">
          <p>
            The {p.target} failed fact-check {p.passes} times and the same findings keep coming back. Another round may not converge
            without your input.
          </p>
          <FactFindings findings={p.findings} />
          <Choices
            submit={submit}
            textLabel={`Optional: what the ${p.target} agent should know to resolve these findings`}
            textFor={["another-round"]}
            options={[
              { value: "another-round", label: "Run another round", primary: true },
              { value: "stop", label: "Stop here" },
            ]}
          />
        </div>
      );
    case "review_cap":
      return (
        <div className="stack">
          <p>
            Phase {p.phase}
            {p.tests ? " tests" : ""} in {p.project} had {p.iterations} review passes and findings are still open. A fourth pass runs
            only if you choose it. Stopping blocks this phase and every phase that depends on it; independent phases continue.
          </p>
          <ReviewFindings findings={p.findings} />
          <ArtifactLink path={p.ledger_path} label="Open the review ledger" />
          <Choices
            submit={submit}
            options={[
              { value: "another-pass", label: "Run another fix and review pass", primary: true },
              { value: "stop", label: "Stop and block the phase" },
            ]}
          />
        </div>
      );
    case "stuck":
      return (
        <div className="stack">
          <p>
            {humanize(p.agent)} in {p.project}
            {p.phase !== null ? `, phase ${p.phase},` : ""} hit its retry limit on the same failure. Retrying with the same prompt
            would reproduce it, so it needs a fact.
          </p>
          <div className="small">Diagnostic</div>
          <pre>{p.diagnostic}</pre>
          <p>
            <strong>Needs:</strong> {p.need}
          </p>
          <Choices
            submit={submit}
            textLabel="The missing fact, stated plainly. It is quoted to the agent verbatim."
            textFor={["fact"]}
            options={[
              { value: "fact", label: "Re-run with this fact", primary: true, needsText: true },
              { value: "block", label: "Block this work" },
            ]}
          />
        </div>
      );
    case "phase_blocked":
      return (
        <div className="stack">
          <p>
            Phase {p.phase} in {p.project} cannot complete: {p.reason}
          </p>
          <p className="small muted">Phases that depend on it are held. Independent phases keep running.</p>
          <Choices
            submit={submit}
            textLabel="Optional instructions for a retry"
            textFor={["retry"]}
            options={[
              { value: "retry", label: "Retry the phase", primary: true },
              { value: "leave", label: "Leave it blocked" },
            ]}
          />
        </div>
      );
    case "closing_gate":
      return <Closing payload={p} submit={submit} />;
    case "permission":
      return <Permission payload={p} submit={submit} />;
    case "harness_failure":
      return (
        <div className="stack">
          <p>
            The {p.harness} harness failed to start or is not logged in: {p.error}
          </p>
          <p className="small muted">
            Log in from the execution's Terminal tab, then retry. Or run this execution on Ostra's native agent loop instead.
          </p>
          <Link to={`../x/${p.execution}`} relative="path">
            Open the terminal
          </Link>
          <Choices
            submit={submit}
            options={[
              { value: "retry", label: "Retry after login", primary: true },
              { value: "native", label: "Run on native" },
            ]}
          />
        </div>
      );
    case "skill_approval":
      return <Skills payload={p} submit={submit} />;
    case "execution_failed":
      return (
        <div className="stack">
          <p>
            {humanize(p.agent)} in {p.project} stopped without a result.
          </p>
          <pre>{p.error}</pre>
          <Link to={`../x/${p.execution}`} relative="path">
            Open the execution
          </Link>
          <Choices
            submit={submit}
            options={[
              { value: "retry", label: "Retry", primary: true },
              { value: "abandon", label: "Abandon this work" },
            ]}
          />
        </div>
      );
    case "budget_reached":
      return (
        <div className="stack">
          <p>
            This session spent ${p.spent_usd.toFixed(2)} of its ${p.budget_usd.toFixed(2)} budget. No new execution starts until you
            decide. Raising adds the same amount again.
          </p>
          <Choices
            submit={submit}
            options={[
              { value: "raise", label: "Raise the budget", primary: true },
              { value: "stop", label: "Stop the session" },
            ]}
          />
        </div>
      );
  }
}

function answerSummary(a: GateAnswer): string {
  switch (a.kind) {
    case "questions":
      return a.answers.map((x) => `${x.id}: ${x.answer}`).join("; ");
    case "approval":
      return a.approved ? "Approved" : `Changes requested: ${a.feedback ?? ""}`;
    case "choice":
      return `${humanize(a.option)}${a.text ? `: ${a.text}` : ""}`;
    case "closing":
      return a.items.map((i) => `${i.project}: tests ${i.tests ? "yes" : "no"}, docs ${i.docs ? "yes" : "no"}`).join("; ");
    case "permission":
      return humanize(a.answer);
    case "skills":
      return a.decisions.map((d) => `${d.name} ${d.disposition}`).join(", ");
  }
}

export function GateCard({ gate, highlight, onAnswered }: { gate: GateView; highlight?: boolean; onAnswered?: (g: GateView) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit: Submit = async (answer) => {
    setBusy(true);
    setError(null);
    try {
      const g = await api.answerGate(gate.id, { answer });
      onAnswered?.(g);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const answered = gate.answer !== null;
  return (
    <div id={`gate-${gate.id}`} className={`card ${highlight && !answered ? "highlight" : ""}`} style={busy ? { opacity: 0.6 } : undefined}>
      <div className="row between">
        <h3>{gate.title}</h3>
        {answered ? (
          <Chip tone={gate.source === "yolo" ? "warn" : "ok"}>Answered by {gate.source === "yolo" ? "Ostra (YOLO)" : gate.source}</Chip>
        ) : (
          <Chip tone="warn">Waiting for you</Chip>
        )}
      </div>
      <p className="muted small">{gate.explanation}</p>
      {answered ? (
        <div className="stack">
          <div>{answerSummary(gate.answer!)}</div>
          {gate.reason && <div className="small muted">Reason: {gate.reason}</div>}
        </div>
      ) : (
        <GateBody gate={gate} submit={submit} />
      )}
      {error && <div className="form-error mt">{error}</div>}
    </div>
  );
}
