import { useEffect, useRef, useState } from "react";
import type { Theme } from "../shared/theme";

let seq = 0;

// Mermaid's defaults: config keys a diagram's directives and frontmatter cannot set.
const SECURE = ["secure", "securityLevel", "startOnLoad", "maxTextSize", "suppressErrorRendering", "maxEdges"];

/**
 * A fenced `mermaid` block drawn as SVG. Mermaid loads only on a page that has one. Strict mode ignores click callbacks,
 * the links a `click` line makes are unwrapped, and labels are SVG text, never HTML, so a diagram cannot carry an image
 * or leave the page; its own directives cannot change these settings. A block that does not parse shows its source.
 */
export function Diagram({ source, theme }: { source: string; theme: Theme }) {
  const box = useRef<HTMLDivElement>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let live = true;
    const id = `md-diagram-${++seq}`;
    import("mermaid")
      .then(async ({ default: mermaid }) => {
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          htmlLabels: false,
          flowchart: { htmlLabels: false },
          secure: [...SECURE, "htmlLabels"],
          suppressErrorRendering: true,
          theme: theme === "dark" ? "dark" : "neutral",
        });
        const { svg } = await mermaid.render(id, source);
        const el = box.current;
        if (!live || !el) return;
        el.innerHTML = svg;
        for (const a of el.querySelectorAll("a")) a.replaceWith(...a.childNodes);
      })
      .catch(() => {
        if (live) setFailed(true);
      });
    return () => {
      live = false;
    };
  }, [source, theme]);

  if (failed)
    return (
      <pre className="md-pre">
        <code>{source}</code>
      </pre>
    );
  return <div ref={box} className="md-diagram" />;
}
