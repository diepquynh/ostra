import { Docs, type NavGroup } from "@ostra/design/docs";
import type { Book } from "../../api/gen/Book";
import type { CodeRef } from "../../api/gen/CodeRef";
import type { DocSection } from "../../api/gen/DocSection";
import type { DocSubsection } from "../../api/gen/DocSubsection";

/** A book's Markdown pages: one Overview, the Glossary, the Architecture when present, then each project's sections. */
export type BookPages = { nav: NavGroup[]; sources: Record<string, string> };

export const OVERVIEW = "overview";
export const GLOSSARY = "glossary";
export const ARCHITECTURE = "architecture";

/** The page id of a project's section. Ids never hold `/`, which separates a page from its anchor. */
export const sectionPage = (project: string, id: string) => `${project}.${id}`;

/** Text in a table cell: one line, with `|` escaped so it cannot split the cell. */
export const cell = (s: string) => inlineText(s);

/** A code span in a table cell. GFM splits cells before code spans, so `|` is escaped there too. */
export const codeCell = (s: string) => code(s).replace(/\|/g, "\\|");

/** Text on one line with Markdown punctuation escaped, so a title or term renders as the characters it holds. */
export function inlineText(s: string): string {
  return s
    .replace(/\s*\n\s*/g, " ")
    .replace(/[\\`*_[\]<>#!~|]/g, (c) => `\\${c}`)
    .trim();
}

/**
 * Prose written by an agent. Inline Markdown (code spans, emphasis) stays, but a line cannot start a heading, a fence,
 * or a thematic break, because each would change the page's structure and its table of contents.
 */
export function prose(s: string): string {
  return s
    .trim()
    .split("\n")
    .map((l) => l.replace(/^(\s*)(#|```|~~~|---|___|\*\*\*|=+\s*$)/, "$1\\$2"))
    .join("\n");
}

/** A code span that holds `s` as it is, whatever backticks it contains. */
export function code(s: string): string {
  const text = s.replace(/\s*\n\s*/g, " ");
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((r) => r.length));
  const fence = "`".repeat(longest + 1);
  const pad = text.startsWith("`") || text.endsWith("`") ? " " : "";
  return `${fence}${pad}${text}${pad}${fence}`;
}

function fenced(lang: string, body: string): string {
  const longest = Math.max(2, ...(body.match(/`+/g) ?? []).map((r) => r.length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${lang}\n${body.trimEnd()}\n${fence}`;
}

function table(columns: string[], rows: string[][]): string {
  const line = (cells: string[]) => `| ${cells.join(" | ")} |`;
  return [line(columns.map(cell)), line(columns.map(() => "---")), ...rows.map((r) => line(r))].join("\n");
}

function list(items: string[]): string {
  return items.map((i) => `- ${prose(i).replace(/\n/g, "\n  ")}`).join("\n");
}

function codeRefs(refs: CodeRef[], project: string): string {
  return table(
    ["Path", "Symbol", "Lines", "What is there"],
    refs.map((r) => [
      codeCell(`${project}/${r.path}`),
      r.symbol ? codeCell(r.symbol) : "",
      r.lines ? cell(r.lines) : "",
      cell(r.note),
    ]),
  );
}

/**
 * One unit of work in the order the book reads: purpose, boundaries, assumptions, business flow, diagrams, tables,
 * separation of concerns. Code references come separately, because they close the page.
 */
function unitBody(u: DocSection | DocSubsection, h: string): string[] {
  const out: string[] = [];
  if (u.purpose.trim()) out.push(prose(u.purpose));
  const { owns, does_not_own } = u.boundaries;
  if (owns.length || does_not_own.length) {
    out.push(`${h} Boundaries`);
    if (owns.length) out.push("**Owns**", list(owns));
    if (does_not_own.length) out.push("**Does not own**", list(does_not_own));
  }
  if (u.assumptions.length) out.push(`${h} Assumptions`, list(u.assumptions));
  if (u.business_flow.length)
    out.push(
      `${h} Business flow`,
      table(
        ["Step", "Actor", "Action", "Outcome"],
        u.business_flow.map((s, i) => [String(i + 1), cell(s.actor), cell(s.action), cell(s.outcome)]),
      ),
    );
  for (const d of u.diagrams) out.push(`${h}# ${inlineText(d.title)}`, fenced("mermaid", d.source));
  for (const t of u.tables)
    out.push(
      `${h}# ${inlineText(t.title)}`,
      table(
        t.columns,
        t.rows.map((r) => t.columns.map((_, i) => cell(r[i] ?? ""))),
      ),
    );
  if (u.concerns.length)
    out.push(
      `${h} Separation of concerns`,
      table(
        ["Component", "Responsibility"],
        u.concerns.map((c) => [cell(c.component), cell(c.responsibility)]),
      ),
    );
  return out;
}

/** A section page: the section's own body, each sub-section, and every code reference last. */
export function sectionMarkdown(s: DocSection, project: string): string {
  const out = [`# ${inlineText(s.title)}`, ...unitBody(s, "##")];
  for (const sub of s.subsections) {
    out.push(`## ${inlineText(sub.title)}`, ...unitBody(sub, "###"));
    if (sub.code_refs.length) out.push("### Code references", codeRefs(sub.code_refs, project));
  }
  if (s.code_refs.length) out.push("## Code references", codeRefs(s.code_refs, project));
  return out.join("\n\n");
}

function overviewMarkdown(book: Book): string {
  const out = [`# ${inlineText(book.title || book.id)}`];
  out.push(`Projects: ${book.projects.map(code).join(", ")}. Updated ${new Date(book.updated_at).toLocaleString()}.`);
  const counts = book.parts.map((p) => [
    codeCell(p.project),
    String(p.sections.length),
    String(p.sections.reduce((n, s) => n + s.subsections.length, 0)),
  ]);
  if (counts.length) out.push(table(["Project", "Sections", "Sub-sections"], counts));
  for (const p of book.parts) if (p.overview.trim()) out.push(`## ${inlineText(p.project)}`, prose(p.overview));
  return out.join("\n\n");
}

function glossaryMarkdown(book: Book): string {
  return [
    "# Glossary",
    table(
      ["Term", "Definition", "In code"],
      book.glossary.map((g) => [cell(g.term), cell(g.definition), g.code_ref ? codeCell(g.code_ref) : ""]),
    ),
  ].join("\n\n");
}

function architectureMarkdown(book: Book): string {
  const a = book.architecture!;
  const out = ["# System architecture"];
  if (a.overview.trim()) out.push(prose(a.overview));
  out.push(`## ${inlineText(a.diagram.title || "Components")}`, fenced("mermaid", a.diagram.source));
  if (a.components.length)
    out.push(
      "## Components",
      table(
        ["Component", "Project", "Role", "Owns"],
        a.components.map((c) => [
          cell(c.name),
          c.project ? codeCell(c.project) : "external",
          cell(c.role),
          cell(c.owns.join("; ")),
        ]),
      ),
    );
  if (a.links.length)
    out.push(
      "## Communication",
      table(
        ["From", "To", "Protocol", "Mode", "Payload"],
        a.links.map((l) => [cell(l.from), cell(l.to), cell(l.protocol), l.mode, cell(l.payload)]),
      ),
    );
  if (a.failure_recovery.length)
    out.push(
      "## Failure and recovery",
      table(
        ["Failure", "Detection", "Recovery"],
        a.failure_recovery.map((f) => [cell(f.failure), cell(f.detection), cell(f.recovery)]),
      ),
    );
  if (a.scalability.length)
    out.push(
      "## Scalability",
      table(
        ["Component", "Scales by", "First limit"],
        a.scalability.map((s) => [cell(s.component), cell(s.scales_by), cell(s.limit)]),
      ),
    );
  return out.join("\n\n");
}

/** The pages a book renders as, in reading order. Previous and Next follow this order. */
export function bookPages(book: Book): BookPages {
  const sources: Record<string, string> = { [OVERVIEW]: overviewMarkdown(book) };
  const head: NavGroup = { label: "Book", pages: [{ id: OVERVIEW, title: "Overview", file: OVERVIEW }] };
  if (book.glossary.length) {
    sources[GLOSSARY] = glossaryMarkdown(book);
    head.pages.push({ id: GLOSSARY, title: "Glossary", file: GLOSSARY });
  }
  if (book.architecture) {
    sources[ARCHITECTURE] = architectureMarkdown(book);
    head.pages.push({ id: ARCHITECTURE, title: "System architecture", file: ARCHITECTURE });
  }
  const parts: NavGroup[] = book.parts.map((p) => ({
    label: p.project,
    pages: p.sections.map((s) => {
      const id = sectionPage(p.project, s.id);
      sources[id] = sectionMarkdown(s, p.project);
      return { id, title: s.title, file: id };
    }),
  }));
  return { nav: [head, ...parts.filter((g) => g.pages.length)], sources };
}

/** The renderer's model of a book. Links to files outside the book render as plain text. */
export function bookDocs(book: Book): Docs {
  const { nav, sources } = bookPages(book);
  return new Docs(nav, sources);
}
