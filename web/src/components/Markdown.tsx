import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Renders agent-written markdown. Raw HTML is not rendered, so artifacts cannot inject markup. */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
