import type { ResearchDoc } from "../../../api/types";
import { Chip, CodeView, cx } from "../../../design";
import { OutlineBuilder, type Outline } from "./model";
import { Bullets, Card, Empty, Facts, Grid, Link, Mono, Prose, Questions, Section, SourcesTable, Stats, inlineCode } from "./parts";

export function researchOutline(d: ResearchDoc): Outline {
  const b = new OutlineBuilder();
  b.chapter({
    id: "overview",
    title: "Overview",
    render: () => (
      <div className="doc-stack">
        <Facts
          rows={[
            ["Repo", <Chip key="r" mono>{d.repo}</Chip>],
            ["Areas", d.areas.length ? d.areas.join(", ") : "none"],
            ["Date", d.date],
          ]}
        />
        <Stats
          items={[
            { label: "Files", value: d.files.length },
            { label: "Patterns", value: d.patterns.length },
            { label: "Sources", value: d.sources.length },
            { label: "Open questions", value: d.open_questions.length, tone: d.open_questions.length ? "warn" : undefined },
            { label: "Not covered", value: d.not_covered.length, tone: d.not_covered.length ? "warn" : undefined },
          ]}
        />
        <Section title="Scope of this document">
          <Prose text={d.scope} />
        </Section>
        <Section title="Problem">
          <Prose text={d.problem} />
        </Section>
        <Section title="What the request asks">
          <Bullets items={d.asks} empty="None stated." />
        </Section>
      </div>
    ),
  });
  b.maybe(d.files.length > 0, {
    id: "files",
    title: "Files",
    count: d.files.length,
    render: () => (
      <Grid
        head={["File", "Purpose", "Symbols"]}
        rows={d.files.map((f) => [
          <Mono key="p">{f.path}</Mono>,
          f.purpose,
          <div key="s" className="doc-symbols">
            {f.symbols.map((s) => (
              <Mono key={s}>{s}</Mono>
            ))}
          </div>,
        ])}
      />
    ),
  });
  b.maybe(d.patterns.length > 0, {
    id: "patterns",
    title: "Patterns",
    count: d.patterns.length,
    render: () => (
      <div className="doc-stack">
        {d.patterns.map((p) => (
          <Card key={p.name} title={<strong>{p.name}</strong>}>
            <Prose text={p.description} />
            {p.files.length > 0 && (
              <div className="doc-row">
                <span className="art-muted">Used in</span>
                {p.files.map((f) => (
                  <Mono key={f}>{f}</Mono>
                ))}
              </div>
            )}
            {p.snippet && (
              <div className="doc-snippet">
                {p.snippet.source && <div className="art-muted doc-snippet__src">From {p.snippet.source}</div>}
                <CodeView language={p.snippet.language} code={p.snippet.code} maxHeight={360} />
              </div>
            )}
          </Card>
        ))}
      </div>
    ),
  });
  b.maybe(d.data_flow.length > 0, {
    id: "flow",
    title: "Data flow",
    count: d.data_flow.length,
    render: () => (
      <ol className="doc-flow">
        {d.data_flow.map((s, i) => (
          <li key={i} className="doc-flow__hop">
            <span className="doc-flow__n">{i + 1}</span>
            <div>
              <Prose text={s.step} />
              {s.location && <Mono>{s.location}</Mono>}
            </div>
          </li>
        ))}
      </ol>
    ),
  });
  b.maybe(d.dependencies.length > 0, {
    id: "dependencies",
    title: "Dependencies",
    count: d.dependencies.length,
    render: () => (
      <Grid
        head={["Dependency", "Kind", "Version", "Role"]}
        rows={d.dependencies.map((x) => [<Mono key="n">{x.name}</Mono>, <Chip key="k">{x.kind}</Chip>, x.version ?? "", x.role])}
      />
    ),
  });
  b.chapter({
    id: "external",
    title: "External technology",
    count: d.external.length,
    render: () =>
      d.external.length === 0 ? (
        <Empty>None: the request touches nothing the repo does not already do.</Empty>
      ) : (
        <div className="doc-stack">
          {d.external.map((f, i) => (
            <Card key={i} title={<strong>{f.technology}</strong>} badges={<Chip>{f.version}</Chip>}>
              <div className="doc-consequence">
                <div>
                  <div className="doc-label">The page states</div>
                  <Prose text={f.fact} />
                </div>
                <div>
                  <div className="doc-label">What it forces here</div>
                  <Prose text={f.consequence} />
                </div>
              </div>
              <Link url={f.source} />
            </Card>
          ))}
        </div>
      ),
  });
  b.chapter({
    id: "approaches",
    title: "Approaches",
    count: d.approaches.length,
    render: () => (
      <div className="doc-stack">
        {d.approaches.length === 0 && <Empty>Not applicable: the task is investigative only.</Empty>}
        <div className="doc-columns">
          {d.approaches.map((a) => (
            <section key={a.name} className={cx("doc-card", a.recommended && "doc-card--accent")}>
              <div className="doc-card__head">
                <strong>{a.name}</strong>
                {a.recommended && <Chip tone="ok">Recommended</Chip>}
              </div>
              <div className="doc-card__body">
                <Prose text={a.concept} />
                <div className="doc-label">Pros</div>
                <Bullets items={a.pros} empty="None." />
                <div className="doc-label">Cons</div>
                <Bullets items={a.cons} empty="None." />
                <Facts rows={[["Precedent", <span key="p">{inlineCode(a.precedent)}</span>], ["Best for", a.best_for]]} />
              </div>
            </section>
          ))}
        </div>
        {d.recommendation && (
          <Section title="Recommendation">
            <Prose text={d.recommendation} />
          </Section>
        )}
      </div>
    ),
  });
  b.chapter(
    {
      id: "questions",
      title: "Open questions",
      count: d.open_questions.length,
      attention: d.open_questions.length > 0,
      render: () => <Questions items={d.open_questions} empty="None: every ambiguity was resolved from the code or a retrieved page." />,
    },
    d.open_questions.map((q) => q.id),
  );
  b.chapter({
    id: "sources",
    title: "Sources",
    count: d.sources.length,
    render: () => (d.sources.length ? <SourcesTable sources={d.sources} /> : <Empty>None: no page was needed.</Empty>),
  });
  b.maybe(d.lessons.length > 0, {
    id: "lessons",
    title: "Lessons used",
    count: d.lessons.length,
    render: () => (
      <Grid
        head={["Area", "Lesson", "Verified"]}
        rows={d.lessons.map((l) => [<Mono key="a">{l.area}</Mono>, l.lesson, l.verified ?? <span className="art-muted">not stated</span>])}
      />
    ),
  });
  b.maybe(d.not_covered.length + d.next_steps.length > 0, {
    id: "gaps",
    title: "Not covered",
    count: d.not_covered.length,
    attention: d.not_covered.length > 0,
    render: () => (
      <div className="doc-stack">
        <Section title="Not covered by this document">
          <Bullets items={d.not_covered} empty="None." />
        </Section>
        <Section title="Next steps">
          <Bullets items={d.next_steps} empty="None." />
        </Section>
      </div>
    ),
  });
  return b.done();
}
