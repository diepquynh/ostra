import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Renders agent-written markdown. Raw HTML is not rendered, so artifacts cannot inject markup.
 * `className` defaults to the old pages' `markdown`; new screens pass `os-prose`.
 */
export function Markdown({ text, className = "markdown" }: { text: string; className?: string }) {
  return (
    <div className={className}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
