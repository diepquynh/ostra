import type { Phase, PhaseDoc, PlanDoc, PlanStep } from "../../../api/types";
import { Chip, CodeView, PhaseDag, type Tone } from "../../../design";
import { layers, type Outline, OutlineBuilder } from "./model";
import {
  Bullets,
  El,
  Empty,
  EvidenceCard,
  Facts,
  Grid,
  IdTitle,
  inlineCode,
  Mono,
  Prose,
  Questions,
  Ref,
  Refs,
  Section,
  Stats,
  useDoc,
} from "./parts";
import { span } from "./spec";

const CHANGE_TONE: Record<PlanStep["change"], Tone> = { Create: "ok", Modify: "info", Delete: "bad" };
const LEVEL_TONE = { Low: "ok", Medium: "warn", High: "bad" } as const;

function StepCard({ s }: { s: PlanStep }) {
  return (
    <El id={`step ${s.id}`} className="doc-card doc-step">
      <div className="doc-card__head">
        <IdTitle id={s.id} title={s.title} />
        <span className="doc-card__badges">
          <Chip tone={CHANGE_TONE[s.change]}>{s.change}</Chip>
          <Chip>{s.size}</Chip>
        </span>
      </div>
      <div className="doc-row">
        <Mono>{s.file}</Mono>
      </div>
      <Prose text={s.action} />
      <Facts
        rows={[
          ["Delivers", <Refs key="d" ids={s.delivers} />],
          [
            "Read first",
            s.read_first.length ? (
              <span key="r" className="doc-symbols">
                {s.read_first.map((f) => (
                  <Mono key={f}>{f}</Mono>
                ))}
              </span>
            ) : (
              "none"
            ),
          ],
          [
            "Skills",
            s.skills.length ? (
              <span key="k" className="doc-symbols">
                {s.skills.map((k) => (
                  <Chip key={k} mono>
                    {k}
                  </Chip>
                ))}
              </span>
            ) : (
              "none"
            ),
          ],
          ["Verify", <Mono key="v">{s.verify}</Mono>],
        ]}
      />
      {s.binding_rules.length > 0 && (
        <div className="doc-rules">
          {s.binding_rules.map((r) => (
            <div key={r.id} className="doc-rule">
              <Ref id={r.id} /> <span>{inlineCode(r.rule)}</span>
            </div>
          ))}
        </div>
      )}
    </El>
  );
}

function PhaseHeader({ p, deliverableTitle }: { p: Phase; deliverableTitle?: string | null }) {
  return (
    <El id={`phase ${p.id}`} as="div" className="doc-stack">
      <Facts
        rows={[
          [
            "Deliverable",
            <span key="d" className="doc-row">
              <Ref id={p.deliverable} /> {deliverableTitle}
            </span>,
          ],
          [
            "Repo",
            <span key="r" className="doc-row">
              <Chip mono>{p.repo}</Chip> <span className="art-muted">{p.repo_root}</span>
            </span>,
          ],
          [
            "Complexity",
            <Chip key="c" tone={LEVEL_TONE[p.complexity]}>
              {p.complexity}
            </Chip>,
          ],
          [
            "Test policy",
            <span key="t" className="doc-row">
              <Chip tone={p.test_policy === "Skip" ? "neutral" : "accent"}>{p.test_policy}</Chip> {p.test_rationale}
            </span>,
          ],
          ["Depends on", <Refs key="p" ids={p.depends_on.map((d) => `phase ${d}`)} />],
          ["Areas", p.areas.join(", ") || "none"],
        ]}
      />
      <Section title="Context">
        <Prose text={p.context} />
      </Section>
      <Section title="Required skills">
        {p.skills.length ? (
          <span className="doc-symbols">
            {p.skills.map((k) => (
              <Chip key={k} mono>
                {k}
              </Chip>
            ))}
          </span>
        ) : (
          <Empty>None.</Empty>
        )}
      </Section>
    </El>
  );
}

function PhaseRequirements({ p }: { p: Phase }) {
  if (!p.requirements.length) return <Empty>No requirement quoted.</Empty>;
  return (
    <Grid
      head={["ID", "Statement"]}
      rows={p.requirements.map((r) => [<Ref key="i" id={r.id} />, inlineCode(r.statement)])}
    />
  );
}

function PhaseConstraints({ p }: { p: Phase }) {
  if (!p.constraints.length)
    return <Empty>None: no step in this phase depends on a technology outside the repo.</Empty>;
  return (
    <div className="doc-stack">
      {p.constraints.map((e) => (
        <EvidenceCard key={e.id} e={e} />
      ))}
    </div>
  );
}

function PhaseBody({ p, deliverableTitle }: { p: Phase; deliverableTitle?: string | null }) {
  return (
    <div className="doc-stack">
      <PhaseHeader p={p} deliverableTitle={deliverableTitle} />
      <Section title="Requirements delivered">
        <PhaseRequirements p={p} />
      </Section>
      <Section title="External constraints">
        <PhaseConstraints p={p} />
      </Section>
      <Section title={`Steps (${p.steps.length})`}>
        <div className="doc-stack">
          {p.steps.map((s) => (
            <StepCard key={s.id} s={s} />
          ))}
        </div>
      </Section>
      <Section title="Phase verification">
        <CodeView language="bash" code={p.verification} />
      </Section>
    </div>
  );
}

function PhaseGraph({ d }: { d: PlanDoc }) {
  const { go } = useDoc();
  const dag = layers(d.phases).map((layer) =>
    layer.map((p) => ({
      id: p.id,
      title: p.name,
      project: p.repo,
      complexity: p.complexity.toLowerCase() as "low" | "medium" | "high",
      deliverable: p.deliverable,
    })),
  );
  return <PhaseDag layers={dag} onOpen={(p) => go(`phase ${p.id}`)} />;
}

export function planOutline(d: PlanDoc): Outline {
  const b = new OutlineBuilder();
  const steps = d.phases.flatMap((p) => p.steps);
  const title = (id: string) => d.deliverables.find((x) => x.id === id)?.title ?? null;
  const delivered = new Map<string, string[]>();
  for (const p of d.phases)
    for (const s of p.steps) for (const r of s.delivers) delivered.set(r, [...(delivered.get(r) ?? []), s.id]);
  const reqs = [...delivered.keys()].sort((a, b) => Number(a.slice(1)) - Number(b.slice(1)));

  b.chapter(
    {
      id: "overview",
      title: "Overview",
      render: () => (
        <div className="doc-stack">
          <Stats
            items={[
              { label: "Phases", value: d.phases.length },
              { label: "Steps", value: steps.length },
              { label: "Requirements", value: span(reqs) },
              {
                label: "Stakes",
                value: d.stakes,
                tone: d.stakes === "High" ? "bad" : d.stakes === "Medium" ? "warn" : "ok",
              },
            ]}
          />
          <Section title="Summary">
            <Prose text={d.summary} />
          </Section>
          <Facts
            rows={[
              ["Stakes", `${d.stakes}: ${d.stakes_rationale}`],
              ["Date", d.date],
              [
                "Repos",
                <span key="r" className="doc-symbols">
                  {d.repos.map((r) => (
                    <Chip key={r.key} mono title={r.root}>
                      {r.key}
                    </Chip>
                  ))}
                </span>,
              ],
            ]}
          />
          <Section title="Success criteria">
            <ul className="doc-checklist">
              {d.success_criteria.map((c, i) => (
                <li key={i} id={c.id ? `el-${c.id.toLowerCase()}` : undefined}>
                  <input type="checkbox" disabled aria-label={c.id ?? "Build"} />{" "}
                  {c.id && <span className="doc-id">{c.id}</span>} {inlineCode(c.text)}
                </li>
              ))}
            </ul>
          </Section>
        </div>
      ),
    },
    d.success_criteria.flatMap((c) => (c.id ? [c.id] : [])),
  );
  b.chapter({
    id: "phases",
    title: "Phases",
    count: d.phases.length,
    render: () => (
      <div className="doc-stack">
        <PhaseGraph d={d} />
        <Grid
          head={["Phase", "Name", "Deliverable", "Repo", "Complexity", "Test policy", "Depends on", "Steps"]}
          rows={d.phases.map((p) => [
            <Ref key="i" id={`phase ${p.id}`} />,
            p.name,
            <Ref key="d" id={p.deliverable} />,
            <Mono key="r">{p.repo}</Mono>,
            <Chip key="c" tone={LEVEL_TONE[p.complexity]}>
              {p.complexity}
            </Chip>,
            p.test_policy,
            <Refs key="p" ids={p.depends_on.map((x) => `phase ${x}`)} />,
            p.steps.length,
          ])}
        />
      </div>
    ),
  });
  for (const p of d.phases) {
    b.chapter(
      {
        id: `phase-${p.id}`,
        title: `Phase ${p.id}: ${p.name}`,
        count: p.steps.length,
        depth: 1,
        render: () => <PhaseBody p={p} deliverableTitle={title(p.deliverable)} />,
      },
      [`phase ${p.id}`, ...p.steps.map((s) => `step ${s.id}`), ...p.constraints.map((e) => e.id)],
    );
  }
  b.chapter(
    {
      id: "traceability",
      title: "Traceability",
      count: reqs.length,
      render: () => (
        <Grid
          head={["Requirement", "Delivered by", "Acceptance criteria"]}
          ids={reqs}
          rows={reqs.map((r) => [
            <span key="r" className="doc-id">
              {r}
            </span>,
            <Refs key="s" ids={delivered.get(r)!.map((s) => `step ${s}`)} />,
            <Refs
              key="a"
              ids={d.success_criteria.flatMap((c) => (c.id?.startsWith(`AC${r.slice(1)}.`) ? [c.id] : []))}
            />,
          ])}
        />
      ),
    },
    reqs,
  );
  b.maybe(d.risks.length > 0, {
    id: "risks",
    title: "Risks",
    count: d.risks.length,
    render: () => (
      <Grid
        head={["Risk", "Impact", "Likelihood", "Mitigation"]}
        rows={d.risks.map((r) => [
          r.risk,
          r.impact,
          <Chip key="l" tone={LEVEL_TONE[r.likelihood]}>
            {r.likelihood}
          </Chip>,
          r.mitigation,
        ])}
      />
    ),
  });
  b.chapter(
    {
      id: "questions",
      title: "Clarifying questions",
      count: d.clarifying_questions.length,
      attention: d.clarifying_questions.length > 0,
      render: () => <Questions items={d.clarifying_questions} empty="None: the spec resolves every category." />,
    },
    d.clarifying_questions.map((q) => q.id),
  );
  b.chapter({
    id: "checks",
    title: "Checks and verification",
    count: d.pre_checks.length,
    render: () => (
      <div className="doc-stack">
        <Section title="Mechanical pre-checks">
          {d.pre_checks.length ? (
            <Grid head={["Check", "Scope", "Result"]} rows={d.pre_checks.map((c) => [c.check, c.scope, c.result])} />
          ) : (
            <Empty>Not recorded.</Empty>
          )}
        </Section>
        <Section title="Verification strategy">
          <Bullets items={d.verification} empty="Not recorded." />
        </Section>
      </div>
    ),
  });
  return b.done();
}

export function phaseOutline(d: PhaseDoc): Outline {
  const p = d.phase;
  const b = new OutlineBuilder();
  b.chapter(
    {
      id: "overview",
      title: "Overview",
      render: () => (
        <div className="doc-stack">
          <Stats
            items={[
              { label: "Steps", value: p.steps.length },
              { label: "Requirements", value: p.requirements.length },
              { label: "Constraints", value: p.constraints.length },
            ]}
          />
          <p className="art-muted">
            Phase {p.id} of the plan {d.plan}.
          </p>
          <PhaseHeader p={p} deliverableTitle={d.deliverable_title} />
        </div>
      ),
    },
    [`phase ${p.id}`],
  );
  b.chapter(
    {
      id: "steps",
      title: "Steps",
      count: p.steps.length,
      render: () => (
        <div className="doc-stack">
          {p.steps.map((s) => (
            <StepCard key={s.id} s={s} />
          ))}
          <Section title="Phase verification">
            <CodeView language="bash" code={p.verification} />
          </Section>
        </div>
      ),
    },
    p.steps.map((s) => `step ${s.id}`),
  );
  b.chapter(
    {
      id: "requirements",
      title: "Requirements delivered",
      count: p.requirements.length,
      render: () => <PhaseRequirements p={p} />,
    },
    p.requirements.map((r) => r.id),
  );
  b.chapter(
    {
      id: "constraints",
      title: "External constraints",
      count: p.constraints.length,
      render: () => <PhaseConstraints p={p} />,
    },
    p.constraints.map((e) => e.id),
  );
  return b.done();
}
