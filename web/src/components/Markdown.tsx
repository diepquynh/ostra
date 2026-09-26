import type { ComponentProps } from "react";
import ReactMarkdown, { type Components, defaultUrlTransform, type UrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Links keep react-markdown's safe protocols. Images load only from `data:image/` URLs, because a remote or
 * protocol-relative image in agent-written text would request a URL the agent chose, and the CSP is the only
 * other thing that stops it.
 */
export const markdownUrl: UrlTransform = (url, key) =>
  key === "src" ? (/^data:image\//i.test(url.trim()) ? url : "") : defaultUrlTransform(url);

/** An image whose source was dropped shows its alt text instead. */
export const MarkdownImage = ({ src, alt, node: _node, ...rest }: ComponentProps<"img"> & { node?: unknown }) =>
  src ? <img {...rest} src={src} alt={alt} /> : <span className="md-img-alt">{alt}</span>;

const components: Components = { img: MarkdownImage };

/**
 * Renders agent-written markdown. Raw HTML is not rendered, so artifacts cannot inject markup.
 * `className` defaults to the old pages' `markdown`; new screens pass `os-prose`.
 */
export function Markdown({ text, className = "markdown" }: { text: string; className?: string }) {
  return (
    <div className={className}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} urlTransform={markdownUrl} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
