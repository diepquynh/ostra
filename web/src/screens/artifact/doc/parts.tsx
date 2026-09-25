import { createContext, Fragment, type ReactNode, useContext } from "react";
import type { Evidence, FactCheckView, Question, Source } from "../../../api/types";
import { Markdown } from "../../../components/Markdown";
import { Chip, cx, Icon, SectionLabel, type Tone } from "../../../design";
import { ColumnResizer } from "../ColumnResizer";
import { ScrollTable } from "../ScrollTable";
import { findingElement, type Mark, type MarkTone, normId, severityTone } from "./model";

export type DocCtx = {
  /** Go to an element id (`R3`, `step 2.1`) or a chapter id. */
  go: (ref: string) => void;
  /** Open another resource, for example `artifact:<path>`. */
  open: (id: string) => void;
  marks: Map<string, Mark[]>;
  focus: string | null;
  /** Element ids that exist in this document, so chips for unknown ids render inert. */
  known: (ref: string) => boolean;
};

export const DocContext = createContext<DocCtx>({
  go: () => {},
  open: () => {},
  marks: new Map(),
  focus: null,
  known: () => false,
});
export const useDoc = () => useContext(DocContext);

const MARK_CHIP: Record<MarkTone, Tone> = { bad: "bad", warn: "warn", info: "info" };

/** Prose an agent wrote, in markdown. */
export function Prose({ text }: { text: string | null | undefined }) {
  if (!text?.trim()) return null;
  return <Markdown text={text} className="doc-prose" />;
}

/** A clickable id that jumps to its element. Unknown ids render as plain chips. */
export function Ref({ id }: { id: string }) {
  const { go, known, marks } = useDoc();
  const key = normId(id);
  const tone = marks.get(key)?.length ? worstTone(marks.get(key)!) : undefined;
  if (!known(id)) return <span className="doc-ref doc-ref--inert">{id}</span>;
  return (
    <button
      type="button"
      className={cx("doc-ref", tone && `doc-ref--${tone}`)}
      onClick={() => go(id)}
      title={`Go to ${id}`}
    >
      {id}
    </button>
  );
}

export function Refs({ ids, none = "none" }: { ids: string[]; none?: string }) {
  if (!ids.length) return <span className="art-muted">{none}</span>;
  return (
    <span className="doc-refs">
      {ids.map((id) => (
        <Ref key={id} id={id} />
      ))}
    </span>
  );
}

const worstTone = (m: Mark[]): MarkTone =>
  m.some((x) => x.tone === "bad") ? "bad" : m.some((x) => x.tone === "warn") ? "warn" : "info";

/** The markers on one element, listed under it. */
export function Marks({ id }: { id: string }) {
  const marks = useDoc().marks.get(normId(id)) ?? [];
  if (!marks.length) return null;
  return (
    <ul className="doc-marks" aria-label={`Notes on ${id}`}>
      {marks.map((m, i) => (
        <li key={i} className={`doc-mark doc-mark--${m.tone}`}>
          <Chip tone={MARK_CHIP[m.tone]}>{m.source === "fact-check" ? "Fact-check" : "Check"}</Chip>
          <span>{m.text}</span>
        </li>
      ))}
    </ul>
  );
}

/** A block addressable by id: the jump target, highlighted when focused, with its markers. */
export function El({
  id,
  children,
  className,
  as = "section",
}: {
  id: string;
  children: ReactNode;
  className?: string;
  as?: "section" | "div" | "li";
}) {
  const { focus } = useDoc();
  const key = normId(id);
  const Tag = as;
  return (
    <Tag id={`el-${key}`} data-el={key} className={cx("doc-el", focus === key && "doc-el--focus", className)}>
      {children}
      <Marks id={id} />
    </Tag>
  );
}

export function Card({
  id,
  title,
  badges,
  children,
  tone,
}: {
  id?: string;
  title: ReactNode;
  badges?: ReactNode;
  children?: ReactNode;
  tone?: "accent";
}) {
  const body = (
    <>
      <div className="doc-card__head">
        {title}
        {badges && <span className="doc-card__badges">{badges}</span>}
      </div>
      {children && <div className="doc-card__body">{children}</div>}
    </>
  );
  return id ? (
    <El id={id} className={cx("doc-card", tone && `doc-card--${tone}`)}>
      {body}
    </El>
  ) : (
    <section className={cx("doc-card", tone && `doc-card--${tone}`)}>{body}</section>
  );
}

export function IdTitle({ id, title }: { id: string; title: string }) {
  return (
    <span className="doc-idtitle">
      <span className="doc-id">{id}</span>
      <span>{title}</span>
    </span>
  );
}

/** Label and value pairs. */
export function Facts({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="doc-facts">
      {rows.map(([k, v]) => (
        <Fragment key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

export function Stats({ items }: { items: { label: string; value: ReactNode; tone?: "warn" | "bad" | "ok" }[] }) {
  return (
    <div className="doc-stats">
      {items.map((s) => (
        <div key={s.label} className={cx("doc-stat", s.tone && `doc-stat--${s.tone}`)}>
          <div className="doc-stat__value">{s.value}</div>
          <div className="doc-stat__label">{s.label}</div>
        </div>
      ))}
    </div>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="doc-section">
      <SectionLabel>{title}</SectionLabel>
      {children}
    </div>
  );
}

export function Bullets({ items, empty }: { items: string[]; empty?: string }) {
  if (!items.length) return empty ? <p className="art-muted">{empty}</p> : null;
  return (
    <ul className="doc-bullets">
      {items.map((t, i) => (
        <li key={i}>
          <Markdown text={t} className="doc-inline" />
        </li>
      ))}
    </ul>
  );
}

export function Mono({ children }: { children: ReactNode }) {
  return <code className="doc-mono">{children}</code>;
}

export function Link({ url }: { url: string }) {
  const ok = /^https?:\/\//.test(url);
  return ok ? (
    <a className="doc-link" href={url} target="_blank" rel="noreferrer">
      {url}
    </a>
  ) : (
    <Mono>{url}</Mono>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="art-muted doc-empty">{children}</p>;
}

/** An open question in the card form the question gate uses. */
export function QuestionCard({ q }: { q: Question }) {
  return (
    <El id={q.id} className="doc-card doc-question">
      <div className="doc-card__head">
        <span className="doc-idtitle">
          <span className="doc-id">{q.id}</span>
          <Chip tone="accent">{q.tag}</Chip>
        </span>
        {q.multi_select && <Chip>Several answers</Chip>}
      </div>
      <p className="doc-question__text">{q.question}</p>
      <ol className="doc-options">
        {q.options.map((o, i) => (
          <li key={i} className={cx("doc-option", i === q.recommended && "doc-option--rec")}>
            <div className="doc-option__label">
              {o.label}
              {i === q.recommended && <Chip tone="ok">Recommended</Chip>}
            </div>
            <div className="art-muted">{o.description}</div>
          </li>
        ))}
      </ol>
    </El>
  );
}

export function Questions({ items, empty }: { items: Question[]; empty: string }) {
  if (!items.length) return <Empty>{empty}</Empty>;
  return (
    <div className="doc-stack">
      {items.map((q) => (
        <QuestionCard key={q.id} q={q} />
      ))}
    </div>
  );
}

/** An external fact with the rule it forces. */
export function EvidenceCard({ e, restedOnBy }: { e: Evidence; restedOnBy?: string[] }) {
  return (
    <Card
      id={e.id}
      title={<IdTitle id={e.id} title="External evidence" />}
      badges={e.note ? <Chip tone="warn">{e.note}</Chip> : undefined}
    >
      <Facts
        rows={[
          ["Fact", <Prose key="f" text={e.fact} />],
          ["Binding rule", <strong key="r">{e.rule}</strong>],
          ["Source", <Link key="s" url={e.source} />],
          ["Version", e.version],
          ...(restedOnBy
            ? ([["Rested on by", <Refs key="b" ids={restedOnBy} none="no requirement" />]] as [string, ReactNode][])
            : []),
        ]}
      />
    </Card>
  );
}

export function SourcesTable({ sources }: { sources: Source[] }) {
  return (
    <ScrollTable>
      <table>
        <thead>
          <tr>
            <th>
              Source
              <ColumnResizer />
            </th>
            <th>
              Version or date
              <ColumnResizer />
            </th>
            <th>
              What it established
              <ColumnResizer />
            </th>
          </tr>
        </thead>
        <tbody>
          {sources.map((s) => (
            <tr key={s.url}>
              <td>
                <Link url={s.url} />
              </td>
              <td className="art-cell">{s.version}</td>
              <td className="art-cell">{s.established}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </ScrollTable>
  );
}

/** A plain table from header labels and cell nodes. */
export function Grid({ head, rows, ids }: { head: string[]; rows: ReactNode[][]; ids?: string[] }) {
  const { focus } = useDoc();
  return (
    <ScrollTable>
      <table>
        <thead>
          <tr>
            {head.map((h) => (
              <th key={h}>
                {h}
                <ColumnResizer />
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const id = ids?.[i];
            const key = id ? normId(id) : undefined;
            return (
              <tr
                key={i}
                id={key ? `el-${key}` : undefined}
                data-el={key}
                className={cx(key && focus === key && "doc-row--focus")}
              >
                {r.map((c, j) => (
                  <td key={j} className="art-cell">
                    {c}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </ScrollTable>
  );
}

const VERDICT_TONE = { PASS: "ok", FAIL: "bad", ERROR: "warn" } as const;

/** The fact-check pass over this document, with each finding linked to its element. */
export function FactCheckChapter({ check }: { check: FactCheckView }) {
  return (
    <div className="doc-stack">
      <div className="doc-row">
        {check.verdict ? (
          <Chip tone={VERDICT_TONE[check.verdict]}>{check.verdict}</Chip>
        ) : (
          <Chip tone="accent">Running</Chip>
        )}
        <span className="art-muted">
          Pass over version {check.version}
          {check.current ? ", the version shown here" : ", an earlier version"}.
        </span>
      </div>
      {check.findings.length === 0 && <Empty>No findings.</Empty>}
      {check.findings.map((f, i) => {
        const el = findingElement(f);
        return (
          <section key={i} className={`doc-card doc-finding doc-finding--${severityTone(f.severity)}`}>
            <div className="doc-card__head">
              <span className="doc-idtitle">
                <Chip tone={VERDICT_SEVERITY[severityTone(f.severity)]}>{f.severity}</Chip>
                <span className="art-muted">{f.location}</span>
              </span>
              {el && <Ref id={f.element?.split(",")[0].trim() ?? el} />}
            </div>
            <p className="doc-finding__claim">
              <Icon name="text-search" size={12} /> {f.claim}
            </p>
            <p className="doc-finding__issue">{f.issue}</p>
          </section>
        );
      })}
    </div>
  );
}

const VERDICT_SEVERITY: Record<MarkTone, Tone> = { bad: "bad", warn: "warn", info: "info" };

const EARS = /\b(THE SYSTEM SHALL|WHEN|WHILE|IF|THEN|WHERE)\b/g;

/** An EARS statement with its keywords set apart. */
export function Ears({ text }: { text: string }) {
  const parts = text.split(EARS);
  return (
    <p className="doc-ears">
      {parts.map((p, i) =>
        i % 2 === 1 ? (
          <strong key={i} className="doc-ears__kw">
            {p}
          </strong>
        ) : (
          <Fragment key={i}>{inlineCode(p)}</Fragment>
        ),
      )}
    </p>
  );
}

/** Backticked spans as code, the rest as text. */
export function inlineCode(text: string): ReactNode {
  return text
    .split(/(`[^`]+`)/g)
    .map((p, i) => (p.startsWith("`") && p.endsWith("`") && p.length > 1 ? <code key={i}>{p.slice(1, -1)}</code> : p));
}
