export interface DiffLine {
  type: "add" | "del" | "ctx";
  text: string;
}

export interface DiffViewProps {
  lines: DiffLine[];
}

/** Unified diff lines. */
export function DiffView({ lines }: DiffViewProps) {
  return (
    <div className="os-diff">
      {lines.map((l, i) => (
        <div
          key={i}
          className={`os-diff__line ${l.type === "add" ? "os-diff__line--add" : l.type === "del" ? "os-diff__line--del" : ""}`}
        >
          <span className="os-diff__sign">{l.type === "add" ? "+" : l.type === "del" ? "-" : " "}</span>
          {l.text}
        </div>
      ))}
    </div>
  );
}
