import type { PhrasingContent, RootContent } from "mdast";
import type { ReactNode } from "react";
import type { Theme } from "../shared/theme";
import { Diagram } from "./Diagram";
import { type Docs, type Page, resolvePath, type Target } from "./model";
import { IMAGES } from "./sources";

type Ctx = { docs: Docs; page: Page; go: (t: Target) => void; theme: Theme };

/**
 * The page's blocks as React elements. Raw HTML is never rendered, except `<br>` and the SVG Mermaid draws in strict mode. Only images bundled from docs/images
 * render; any other image is a link to the file, because the site loads no image from another origin (the same rule
 * as the console's CSP).
 */
export function Markdown({ docs, page, go, theme }: Ctx) {
  const ctx = { docs, page, go, theme };
  return <>{page.blocks.map((b, i) => block(b, `b${i}`, ctx))}</>;
}

function block(n: RootContent, key: string, ctx: Ctx): ReactNode {
  switch (n.type) {
    case "heading": {
      const rel = n.depth - ctx.page.titleLevel;
      const Tag = `h${Math.min(6, rel + 1)}` as "h2";
      const cls = rel <= 1 ? "md-h2" : rel === 2 ? "md-h3" : "md-h4";
      return (
        <Tag key={key} id={ctx.page.ids.get(n)} className={cls}>
          {inline(n.children, key, ctx)}
        </Tag>
      );
    }
    case "paragraph":
      return (
        <p key={key} className="md-p">
          {inline(n.children, key, ctx)}
        </p>
      );
    case "code":
      if (n.lang === "mermaid") return <Diagram key={key} source={n.value} theme={ctx.theme} />;
      return (
        <pre key={key} className="md-pre">
          <code>{n.value}</code>
        </pre>
      );
    case "thematicBreak":
      return <hr key={key} className="md-hr" />;
    case "blockquote":
      return (
        <blockquote key={key} className="md-quote">
          {n.children.map((c, i) => block(c, `${key}.${i}`, ctx))}
        </blockquote>
      );
    case "list": {
      const items = n.children.map((item, i) => (
        <li key={i} className="md-li">
          {item.checked != null && (
            <input type="checkbox" checked={item.checked} disabled readOnly className="md-task" />
          )}
          {item.children.map((c, j) => {
            const k = `${key}.${i}.${j}`;
            if (c.type !== "paragraph") return block(c, k, ctx);
            const text = inline(c.children, k, ctx);
            return j === 0 ? (
              <span key={k}>{text}</span>
            ) : (
              <div key={k} style={{ marginTop: 6 }}>
                {text}
              </div>
            );
          })}
        </li>
      ));
      return n.ordered ? (
        <ol key={key} className="md-list" start={n.start != null && n.start !== 1 ? n.start : undefined}>
          {items}
        </ol>
      ) : (
        <ul key={key} className="md-list">
          {items}
        </ul>
      );
    }
    case "table": {
      const [head, ...rows] = n.children;
      const align = (i: number) => n.align?.[i] ?? undefined;
      return (
        <div key={key} className="md-table-wrap">
          <table className="md-table">
            <thead>
              <tr>
                {head?.children.map((c, i) => (
                  <th key={i} style={{ textAlign: align(i) }}>
                    {inline(c.children, `${key}h${i}`, ctx)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r, ri) => (
                <tr key={ri}>
                  {head?.children.map((_, ci) => (
                    <td key={ci} style={{ textAlign: align(ci) }}>
                      {r.children[ci] && inline(r.children[ci].children, `${key}r${ri}c${ci}`, ctx)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    case "footnoteDefinition":
      return (
        <div key={key} className="md-footnote">
          <sup>{n.label ?? n.identifier}</sup>
          {n.children.map((c, i) => block(c, `${key}.${i}`, ctx))}
        </div>
      );
    default:
      return null;
  }
}

function inline(nodes: PhrasingContent[], key: string, ctx: Ctx): ReactNode[] {
  return nodes.map((n, i) => {
    const k = `${key}.${i}`;
    switch (n.type) {
      case "text":
        return n.value;
      case "inlineCode":
        return (
          <code key={k} className="md-code">
            {n.value}
          </code>
        );
      case "strong":
        return (
          <strong key={k} className="md-strong">
            {inline(n.children, k, ctx)}
          </strong>
        );
      case "emphasis":
        return <em key={k}>{inline(n.children, k, ctx)}</em>;
      case "delete":
        return <del key={k}>{inline(n.children, k, ctx)}</del>;
      case "break":
        return <br key={k} />;
      case "html":
        return /^<br\s*\/?>$/i.test(n.value.trim()) ? <br key={k} /> : null;
      case "link":
        return anchor(n.url, inline(n.children, k, ctx), k, ctx);
      case "linkReference": {
        const def = ctx.page.definitions.get(n.identifier);
        const text = inline(n.children, k, ctx);
        return def ? anchor(def.url, text, k, ctx) : <span key={k}>{text}</span>;
      }
      case "image":
      case "imageReference": {
        const url = n.type === "image" ? n.url : ctx.page.definitions.get(n.identifier)?.url;
        const src = url && !/^[a-z][a-z0-9+.-]*:/i.test(url) ? IMAGES[resolvePath(ctx.page.file, url)] : undefined;
        if (src)
          return (
            <a key={k} href={src} target="_blank" rel="noreferrer" className="md-img-link">
              <img src={src} alt={n.alt ?? ""} className="md-img" loading="lazy" />
            </a>
          );
        const text = `Image: ${n.alt || url || ""}`;
        return url ? anchor(url, text, k, ctx) : text;
      }
      case "footnoteReference":
        return <sup key={k}>{n.label ?? n.identifier}</sup>;
      default:
        return null;
    }
  });
}

function anchor(url: string, children: ReactNode, key: string, ctx: Ctx): ReactNode {
  const l = ctx.docs.link(url, ctx.page);
  if (!l) return <span key={key}>{children}</span>;
  const nav = l.nav;
  return (
    <a
      key={key}
      href={l.href}
      className="md-a"
      target={l.external ? "_blank" : undefined}
      rel={l.external ? "noreferrer" : undefined}
      onClick={
        nav
          ? (e) => {
              if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
              e.preventDefault();
              ctx.go(nav);
            }
          : undefined
      }
    >
      {children}
    </a>
  );
}
