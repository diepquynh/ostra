import type { Requirement, SpecDoc } from "../../../api/types";
import { Chip } from "../../../design";
import { OutlineBuilder, type Outline } from "./model";
import {
  Bullets,
  Card,
  Ears,
  El,
  Empty,
  EvidenceCard,
  Facts,
  Grid,
  IdTitle,
  Mono,
  Prose,
  Questions,
  Ref,
  Refs,
  Section,
  Stats,
  inlineCode,
  useDoc,
} from "./parts";

const EARS_LABEL: Record<Requirement["pattern"], string> = {
  ubiquitous: "Ubiquitous",
  "event-driven": "Event-driven",
  "state-driven": "State-driven",
  unwanted: "Unwanted behavior",
  optional: "Optional feature",
  complex: "State and event",
};

/** `R1–R3` for a contiguous run, else the list. */
export function span(ids: string[]): string {
  if (ids.length === 0) return "none";
  if (ids.length === 1) return ids[0];
  const nums = ids.map((i) => Number(i.replace(/^\D+/, "")));
  const contiguous = nums.every((n, i) => i === 0 || n === nums[i - 1] + 1);
  return contiguous ? `${ids[0]}–${ids[ids.length - 1]}` : ids.join(", ");
}

function RequirementCard({ r }: { r: Requirement }) {
  return (
    <El id={r.id} className="doc-card doc-req">
      <div className="doc-card__head">
        <IdTitle id={r.id} title={r.title} />
        <span className="doc-card__badges">
          <Chip tone="info">{EARS_LABEL[r.pattern]}</Chip>
          <Ref id={r.deliverable} />
        </span>
      </div>
      <Ears text={r.statement} />
      <Facts
        rows={[
          ["Covers", <Refs key="c" ids={r.covers} />],
          ["Rests on", <Refs key="e" ids={r.rests_on} />],
        ]}
      />
      <div className="doc-acs">
        {r.acceptance.map((a) => (
          <El key={a.id} id={a.id} as="div" className="doc-ac">
            <div className="doc-ac__id">{a.id}</div>
            <div className="doc-gwt">
              <span className="doc-gwt__k">Given</span>
              <span>{inlineCode(a.given)}</span>
              <span className="doc-gwt__k">When</span>
              <span>{inlineCode(a.when)}</span>
              <span className="doc-gwt__k">Then</span>
              <span>{inlineCode(a.then)}</span>
            </div>
          </El>
        ))}
      </div>
    </El>
  );
}

function ResearchLinks({ paths }: { paths: string[] }) {
  const { open } = useDoc();
  if (!paths.length) return <span className="art-muted">none</span>;
  return (
    <span className="doc-symbols">
      {paths.map((p) => (
        <button key={p} type="button" className="doc-linkbtn" onClick={() => open(`artifact:${p}`)} title={p}>
          {p.split("/").pop()}
        </button>
      ))}
    </span>
  );
}

export function specOutline(d: SpecDoc): Outline {
  const b = new OutlineBuilder();
  const covered = new Set(d.requirements.flatMap((r) => r.covers));
  const uncovered = d.criteria.filter((c) => !covered.has(c.id));
  const reqsOf = (id: string) => d.requirements.filter((r) => r.deliverable === id);
  const coverers = (c: string) => d.requirements.filter((r) => r.covers.includes(c)).map((r) => r.id);

  b.chapter({
    id: "overview",
    title: "Overview",
    render: () => (
      <div className="doc-stack">
        <Stats
          items={[
            { label: "Deliverables", value: d.deliverables.length },
            { label: "Requirements", value: d.requirements.length },
            { label: "Criteria covered", value: `${d.criteria.length - uncovered.length} of ${d.criteria.length}`, tone: uncovered.length ? "bad" : "ok" },
            { label: "Evidence rows", value: d.evidence.length },
            { label: "Open questions", value: d.open_questions.length, tone: d.open_questions.length ? "warn" : undefined },
          ]}
        />
        <Section title="Objective">
          <Prose text={d.objective} />
        </Section>
        <Section title="Current behavior">
          <Prose text={d.current_behavior} />
        </Section>
        <Facts
          rows={[
            ["Date", d.date],
            ["Repos", <span key="r" className="doc-symbols">{d.repos.map((r) => <Chip key={r.key} mono title={r.root}>{r.key}</Chip>)}</span>],
            ["Research", <ResearchLinks key="d" paths={d.research} />],
          ]}
        />
      </div>
    ),
  });
  b.chapter({
    id: "scope",
    title: "Scope",
    count: d.in_scope.length + d.out_of_scope.length,
    render: () => (
      <div className="doc-stack">
        <Section title="In scope">
          {d.in_scope.length === 0 && <Empty>None.</Empty>}
          <ul className="doc-bullets">
            {d.in_scope.map((s, i) => (
              <li key={i}>
                {inlineCode(s.text)} <Refs ids={s.criteria} none="" />
              </li>
            ))}
          </ul>
        </Section>
        <Section title="Out of scope">
          {d.out_of_scope.length === 0 && <Empty>None.</Empty>}
          <ul className="doc-bullets">
            {d.out_of_scope.map((s, i) => (
              <li key={i}>
                <strong>{inlineCode(s.text)}</strong> <span className="art-muted">{inlineCode(s.reason)}</span>
              </li>
            ))}
          </ul>
        </Section>
      </div>
    ),
  });
  b.chapter(
    {
      id: "criteria",
      title: "Criteria",
      count: d.criteria.length,
      attention: uncovered.length > 0,
      render: () => (
        <Grid
          head={["Criterion", "Statement", "Type", "Repo", "Grounding", "Depends on", "Status", "Covered by"]}
          ids={d.criteria.map((c) => c.id)}
          rows={d.criteria.map((c) => [
            <span key="i" className="doc-id">{c.id}</span>,
            c.statement,
            <Chip key="k">{c.kind}</Chip>,
            <Mono key="r">{c.repo}</Mono>,
            <span key="g" className="doc-wrap">{inlineCode(c.grounding)}</span>,
            <Refs key="d" ids={c.depends_on} />,
            c.provisional ? <span key="s" className="doc-row">Provisional <Ref id={c.provisional} /></span> : "Confirmed",
            coverers(c.id).length ? <Refs key="v" ids={coverers(c.id)} /> : <Chip key="v" tone="bad">Not covered</Chip>,
          ])}
        />
      ),
    },
    d.criteria.map((c) => c.id),
  );
  b.chapter(
    {
      id: "deliverables",
      title: "Deliverables",
      count: d.deliverables.length,
      render: () => (
        <ol className="doc-flow">
          {d.deliverables.map((x, i) => (
            <li key={x.id} className="doc-flow__hop">
              <span className="doc-flow__n">{i + 1}</span>
              <El id={x.id} as="div" className="doc-card">
                <div className="doc-card__head">
                  <IdTitle id={x.id} title={x.title} />
                  <span className="doc-card__badges">
                    <Chip mono>{x.repo}</Chip>
                  </span>
                </div>
                <p>{inlineCode(x.outcome)}</p>
                <Facts
                  rows={[
                    ["Depends on", <Refs key="d" ids={x.depends_on} />],
                    ["Requirements", span(reqsOf(x.id).map((r) => r.id))],
                    ["Areas", x.areas.join(", ") || "none"],
                  ]}
                />
              </El>
            </li>
          ))}
        </ol>
      ),
    },
    d.deliverables.map((x) => x.id),
  );
  b.chapter({
    id: "requirements",
    title: "Requirements",
    count: d.requirements.length,
    render: () => (
      <div className="doc-stack">
        {d.deliverables.map((x) => (
          <Section key={x.id} title={`${x.id}: ${x.title}`}>
            <div className="doc-stack">
              {reqsOf(x.id).map((r) => (
                <RequirementCard key={r.id} r={r} />
              ))}
            </div>
          </Section>
        ))}
      </div>
    ),
  });
  for (const x of d.deliverables) {
    const reqs = reqsOf(x.id);
    b.chapter(
      {
        id: `req-${x.id.toLowerCase()}`,
        title: `${x.id}: ${x.title}`,
        count: reqs.length,
        depth: 1,
        render: () => (
          <div className="doc-stack">
            <p className="art-muted">{inlineCode(x.outcome)}</p>
            {reqs.map((r) => (
              <RequirementCard key={r.id} r={r} />
            ))}
          </div>
        ),
      },
      reqs.flatMap((r) => [r.id, ...r.acceptance.map((a) => a.id)]),
    );
  }
  b.chapter({
    id: "contracts",
    title: "Contracts",
    count: d.contracts_provided.length + d.contracts_consumed.length,
    render: () => (
      <div className="doc-stack">
        <Section title="Provided">
          {d.contracts_provided.length === 0 && <Empty>None: this spec provides no cross-boundary contract.</Empty>}
          {d.contracts_provided.map((c) => (
            <Card key={c.name} title={<strong>{c.name}</strong>} badges={<>by <Ref id={c.provided_by} /> for <Refs ids={c.consumed_by} none="no one yet" /></>}>
              <Prose text={c.shape} />
            </Card>
          ))}
        </Section>
        <Section title="Consumed">
          {d.contracts_consumed.length === 0 && <Empty>None: this spec consumes no existing contract.</Empty>}
          {d.contracts_consumed.map((c) => (
            <Card key={c.name} title={<strong>{c.name}</strong>} badges={<Mono>{c.source}</Mono>}>
              <Prose text={c.shape} />
            </Card>
          ))}
        </Section>
      </div>
    ),
  });
  b.chapter(
    {
      id: "evidence",
      title: "External evidence",
      count: d.evidence.length,
      render: () =>
        d.evidence.length === 0 ? (
          <Empty>None: this spec rests on no technology outside the repo.</Empty>
        ) : (
          <div className="doc-stack">
            {d.evidence.map((e) => (
              <EvidenceCard key={e.id} e={e} restedOnBy={d.requirements.filter((r) => r.rests_on.includes(e.id)).map((r) => r.id)} />
            ))}
          </div>
        ),
    },
    d.evidence.map((e) => e.id),
  );
  b.maybe(d.data_impact.length > 0, {
    id: "data",
    title: "Data impact",
    count: d.data_impact.length,
    render: () => (
      <ul className="doc-bullets">
        {d.data_impact.map((c, i) => (
          <li key={i}>
            {c.deliverable && <Ref id={c.deliverable} />} {inlineCode(c.change)}
          </li>
        ))}
      </ul>
    ),
  });
  b.maybe(d.assumptions.length > 0, {
    id: "assumptions",
    title: "Assumptions",
    count: d.assumptions.length,
    render: () => <Grid head={["Assumption", "Source"]} rows={d.assumptions.map((a) => [inlineCode(a.text), <span key="s" className="doc-wrap">{inlineCode(a.source)}</span>])} />,
  });
  b.chapter(
    {
      id: "questions",
      title: "Open questions",
      count: d.open_questions.length,
      attention: d.open_questions.length > 0,
      render: () => <Questions items={d.open_questions} empty="None: every requirement is resolved from the research or the code." />,
    },
    d.open_questions.map((q) => q.id),
  );
  b.chapter({
    id: "traceability",
    title: "Traceability",
    count: d.criteria.length,
    attention: uncovered.length > 0,
    render: () => (
      <Grid
        head={["Criterion", "Deliverable", "Requirements", "Acceptance criteria"]}
        rows={d.criteria.map((c) => {
          const reqs = d.requirements.filter((r) => r.covers.includes(c.id));
          const dels = [...new Set(reqs.map((r) => r.deliverable))];
          return [
            <Ref key="c" id={c.id} />,
            <Refs key="d" ids={dels} />,
            reqs.length ? <Refs key="r" ids={reqs.map((r) => r.id)} /> : <Chip key="r" tone="bad">Not covered</Chip>,
            <Refs key="a" ids={reqs.flatMap((r) => r.acceptance.map((a) => a.id))} />,
          ];
        })}
      />
    ),
  });
  b.maybe(d.notes.length > 0, { id: "notes", title: "Notes", count: d.notes.length, render: () => <Bullets items={d.notes} /> });
  return b.done();
}
