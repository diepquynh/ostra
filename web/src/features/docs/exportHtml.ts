import standaloneCss from "@ostra/design/docs-standalone.css?inline";

/** The exported file loads nothing and runs nothing: only its own inline styles and inline images apply. */
export const EXPORT_CSP = "default-src 'none'; style-src 'unsafe-inline'; img-src data:";

export type ExportPage = { id: string; title: string; group: string };

const EXPORT_CSS = `
body { margin: 0; background: var(--surface-editor); color: var(--text-primary); font-family: var(--font-sans, sans-serif); font-size: 13px; }
a { color: var(--text-link); text-decoration: none; }
.bk-export { display: flex; align-items: flex-start; }
.bk-export__toc { position: sticky; top: 0; width: 260px; flex: none; max-height: 100vh; overflow-y: auto; box-sizing: border-box; padding: 20px 14px; border-right: 1px solid var(--border-default); background: var(--surface-panel); }
.bk-export__toc h2 { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); margin: 14px 0 4px; }
.bk-export__toc a { display: block; padding: 3px 6px; color: var(--text-secondary); }
.bk-export__main { flex: 1; min-width: 0; }
.bk-export__page { border-bottom: 1px solid var(--border-subtle); }
@media (max-width: 800px) { .bk-export { display: block; } .bk-export__toc { position: static; width: auto; max-height: none; border-right: 0; } }
@media print { .bk-export__toc { display: none; } }
`;

const DROP =
  "script, iframe, object, embed, frame, frameset, link, meta, base, form, input, button, foreignObject, audio, video, source";

/**
 * Strip what the renderer never makes but a file must not carry: scripts, frames, event handlers, and links or images
 * that would reach another origin. Ids are prefixed with the page id, because every page has its own heading ids.
 */
function clean(root: Element, pageId: string) {
  for (const el of root.querySelectorAll(DROP)) el.remove();
  for (const el of [root, ...root.querySelectorAll("*")]) {
    for (const a of [...el.attributes]) {
      const name = a.name.toLowerCase();
      if (name.startsWith("on")) el.removeAttribute(a.name);
      else if (name === "src" && !/^data:image\//i.test(a.value)) el.removeAttribute(a.name);
      else if ((name === "href" || name === "xlink:href") && !/^(#|https?:|mailto:)/i.test(a.value.trim()))
        el.removeAttribute(a.name);
    }
    if (el.id && !el.closest("svg")) el.id = `${pageId}--${el.id}`;
  }
  for (const a of root.querySelectorAll<HTMLAnchorElement>("a[href^='#']")) {
    if (a.closest("svg")) continue;
    const [page, anchor] = decodeURIComponent(a.getAttribute("href")!.slice(1)).split("/");
    a.setAttribute("href", `#${anchor ? `${page}--${anchor}` : page}`);
  }
  for (const a of root.querySelectorAll("a[href^='http'], a[href^='mailto']")) a.setAttribute("rel", "noreferrer");
  for (const img of root.querySelectorAll("img:not([src])")) img.replaceWith(img.getAttribute("alt") ?? "");
}

/**
 * One self-contained HTML file of the book: every page in reading order with a table of contents, the diagrams as the
 * SVG already drawn in `rendered`, and the styles inline. It is built with DOM calls and serialized, so no book text is
 * ever parsed as markup. `rendered` holds one element per page, `[data-page="<id>"]`.
 */
export function buildExportHtml(opts: {
  title: string;
  pages: ExportPage[];
  rendered: HTMLElement;
  theme: "light" | "dark";
}): string {
  const doc = document.implementation.createHTMLDocument(opts.title);
  doc.documentElement.lang = "en";
  doc.documentElement.dataset.theme = opts.theme;
  const meta = (attrs: Record<string, string>) => {
    const m = doc.createElement("meta");
    for (const [k, v] of Object.entries(attrs)) m.setAttribute(k, v);
    doc.head.prepend(m);
  };
  meta({ name: "viewport", content: "width=device-width, initial-scale=1" });
  meta({ "http-equiv": "Content-Security-Policy", content: EXPORT_CSP });
  meta({ charset: "utf-8" });
  const style = doc.createElement("style");
  style.textContent = standaloneCss + EXPORT_CSS;
  doc.head.append(style);

  const shell = doc.createElement("div");
  shell.className = "bk-export";
  const toc = doc.createElement("nav");
  toc.className = "bk-export__toc";
  const main = doc.createElement("main");
  main.className = "bk-export__main";
  let group = "";
  for (const p of opts.pages) {
    if (p.group !== group) {
      group = p.group;
      const h = doc.createElement("h2");
      h.textContent = group;
      toc.append(h);
    }
    const link = doc.createElement("a");
    link.href = `#${p.id}`;
    link.textContent = p.title;
    toc.append(link);

    const source = opts.rendered.querySelector(`[data-page="${CSS.escape(p.id)}"]`);
    if (!source) continue;
    const page = doc.importNode(source, true) as HTMLElement;
    page.removeAttribute("data-page");
    clean(page, p.id);
    page.id = p.id;
    page.classList.add("bk-export__page");
    main.append(page);
  }
  shell.append(toc, main);
  doc.body.append(shell);
  return `<!doctype html>\n${doc.documentElement.outerHTML}`;
}

/** Wait until every diagram in `root` is drawn or has fallen back to its source, or `timeoutMs` passes. */
export async function diagramsDrawn(root: HTMLElement, timeoutMs = 15000): Promise<void> {
  const start = Date.now();
  const pending = () => [...root.querySelectorAll(".md-diagram")].some((d) => !d.querySelector("svg"));
  while (pending() && Date.now() - start < timeoutMs) await new Promise((r) => setTimeout(r, 100));
}

export function download(name: string, html: string) {
  const url = URL.createObjectURL(new Blob([html], { type: "text/html" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
