import { type ComponentProps, type ReactNode, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Heading } from "../../api/gen/Heading";
import { CodeView } from "../../design";
import { ColumnResizer } from "./ColumnResizer";
import { headingText, rehypeHeadingIds } from "./outline";
import { ScrollTable } from "./ScrollTable";

type HastLike = {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastLike[];
};

const ID_CELL = /^(R\d+[\w.-]*|AC[\w.-]*\d|E\d+|F\d+|P\d+)$/;

export interface ArtifactMarkdownProps {
  text: string;
  /** The server's outline; rendered headings get exactly these ids. */
  headings: Heading[];
  /** A `#id` link inside the document was followed. */
  onAnchor: (id: string) => void;
}

/** An artifact's markdown. Raw HTML is not rendered, so an agent-written file cannot inject markup. */
export function ArtifactMarkdown({ text, headings, onAnchor }: ArtifactMarkdownProps) {
  const rehype = useMemo(() => [rehypeHeadingIds(headings)], [headings]);
  const components = useMemo<Components>(
    () => ({
      a: ({ href, children, node: _node, ...rest }: ComponentProps<"a"> & { node?: unknown }) => {
        if (href?.startsWith("#")) {
          return (
            <a
              {...rest}
              href={href}
              onClick={(e) => {
                e.preventDefault();
                onAnchor(decodeURIComponent(href.slice(1)));
              }}
            >
              {children}
            </a>
          );
        }
        return (
          <a {...rest} href={href} target="_blank" rel="noreferrer">
            {children}
          </a>
        );
      },
      pre: ({ node }: { node?: unknown; children?: ReactNode }) => {
        const code = (node as HastLike | undefined)?.children?.find((c) => c.tagName === "code");
        const cls = (code?.properties?.className as string[] | undefined)?.find((c) => c.startsWith("language-"));
        const source = code ? collect(code) : "";
        return (
          <CodeView language={cls?.slice("language-".length) ?? "txt"} code={source} style={{ margin: "0 0 14px" }} />
        );
      },
      table: ({ node: _node, ...rest }: ComponentProps<"table"> & { node?: unknown }) => (
        <ScrollTable>
          <table {...rest} />
        </ScrollTable>
      ),
      th: ({ node: _node, children, ...rest }: ComponentProps<"th"> & { node?: unknown }) => (
        <th {...rest}>
          {children}
          <ColumnResizer />
        </th>
      ),
      td: ({ node, children, ...rest }: ComponentProps<"td"> & { node?: unknown }) => {
        const id = node ? headingText(node as HastLike) : "";
        return (
          <td {...rest} className={ID_CELL.test(id) ? "art-id" : undefined}>
            {children}
          </td>
        );
      },
    }),
    [onAnchor],
  );
  return (
    <div className="art-md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={rehype} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

const collect = (n: HastLike): string =>
  n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(collect).join("");
