import { Docs, type NavGroup } from "@ostra/design/docs";
import type { Book } from "../../api/gen/Book";
import type { CodeRef } from "../../api/gen/CodeRef";
import type { DocSection } from "../../api/gen/DocSection";

/** A book's Markdown pages: one Overview, the Glossary, then each project's sections. */
export type BookPages = { nav: NavGroup[]; sources: Record<string, string> };

export const OVERVIEW = "overview";
export const GLOSSARY = "glossary";

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

function table(columns: string[], rows: string[][]): string {
  const line = (cells: string[]) => `| ${cells.join(" | ")} |`;
  return [line(columns.map(cell)), line(columns.map(() => "---")), ...rows.map((r) => line(r))].join("\n");
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

/** A writer's Markdown with each level-1 heading outside a code block escaped, because the page title is the only one. */
export function pageBody(s: string): string {
  let fence: string | null = null;
  return s
    .trim()
    .split("\n")
    .map((l) => {
      const marker = /^\s*(`{3,}|~{3,})/.exec(l)?.[1];
      if (fence) {
        if (marker?.startsWith(fence)) fence = null;
        return l;
      }
      if (marker) {
        fence = marker;
        return l;
      }
      return l.replace(/^(\s*)#(\s|$)/, "$1\\#$2");
    })
    .join("\n");
}

/** A section page: the title, the summary, the writer's own Markdown, and every code reference last. */
export function sectionMarkdown(s: DocSection, project: string): string {
  const out = [`# ${inlineText(s.title)}`];
  if (s.summary.trim()) out.push(prose(s.summary));
  if (s.body.trim()) out.push(pageBody(s.body));
  if (s.code_refs.length) out.push("## Code references", codeRefs(s.code_refs, project));
  return out.join("\n\n");
}

function overviewMarkdown(book: Book): string {
  const out = [`# ${inlineText(book.title || book.id)}`];
  out.push(`Projects: ${book.projects.map(code).join(", ")}. Updated ${new Date(book.updated_at).toLocaleString()}.`);
  const counts = book.parts.map((p) => [codeCell(p.project), String(p.sections.length)]);
  if (counts.length) out.push(table(["Project", "Sections"], counts));
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

/** The pages a book renders as, in reading order. Previous and Next follow this order. */
export function bookPages(book: Book): BookPages {
  const sources: Record<string, string> = { [OVERVIEW]: overviewMarkdown(book) };
  const head: NavGroup = { label: "Book", pages: [{ id: OVERVIEW, title: "Overview", file: OVERVIEW }] };
  if (book.glossary.length) {
    sources[GLOSSARY] = glossaryMarkdown(book);
    head.pages.push({ id: GLOSSARY, title: "Glossary", file: GLOSSARY });
  }
  // Pages group by their `group` in plan order, under the project's name when the book has more than one.
  const parts: NavGroup[] = [];
  for (const p of book.parts) {
    for (const s of p.sections) {
      const id = sectionPage(p.project, s.id);
      sources[id] = sectionMarkdown(s, p.project);
      const name = s.group || p.project;
      const label = book.parts.length > 1 && s.group ? `${p.project}: ${name}` : name;
      let group = parts.find((g) => g.label === label);
      if (!group) {
        group = { label, pages: [] };
        parts.push(group);
      }
      group.pages.push({ id, title: s.title, file: id });
    }
  }
  return { nav: [head, ...parts.filter((g) => g.pages.length)], sources };
}

/** The renderer's model of a book. Links to files outside the book render as plain text. */
export function bookDocs(book: Book): Docs {
  const { nav, sources } = bookPages(book);
  return new Docs(nav, sources);
}
