import { Banner, Button, Chip, Icon, type PolicyInfo, Terminal, type TerminalLine, ToolCall } from "@ostra/design";
import { useEffect, useRef, useState } from "react";
import { reducedMotion } from "./reveal";

const DENY: PolicyInfo = {
  decision: "deny",
  layer: "permission",
  rule: "Bash(git push *)",
  reason: "This workspace denies pushes.",
  advice: "Leave the changes staged. Ostra stages each phase after review, and you push.",
};

const HARNESS: TerminalLine[] = [
  ["fn", "› Implement phase 1: refund model and migration"],
  ["muted", "  Read migrations/0012_orders.sql"],
  ["add", "  Write migrations/0013_refunds.sql"],
  ["warn", "  Permission asked: Bash(sqlx migrate run)"],
];

const ALLOWED: TerminalLine[] = [
  ["ok", "  Allowed once by you: Bash(sqlx migrate run)"],
  ["muted", "  Applied 1 migration: 0013_refunds"],
];

/**
 * Both executor cards play their run once, when the section is in view: the native loop's tool calls end on a denied
 * push, and the harness terminal ends on a permission ask that Allow once answers.
 */
export function Executors() {
  const grid = useRef<HTMLDivElement>(null);
  const [calls, setCalls] = useState(0);
  const [lines, setLines] = useState(0);
  const [ask, setAsk] = useState(false);
  const [allowed, setAllowed] = useState(false);

  useEffect(() => {
    const timers: ReturnType<typeof setTimeout>[] = [];
    const play = () => {
      if (reducedMotion()) {
        setCalls(4);
        setLines(4);
        setAsk(true);
        return;
      }
      const at = (ms: number, fn: () => void) => timers.push(setTimeout(fn, ms));
      [200, 900, 1600, 2600].forEach((ms, i) => at(ms, () => setCalls(i + 1)));
      [400, 1100, 1800, 2500].forEach((ms, i) => at(ms, () => setLines(i + 1)));
      at(3000, () => setAsk(true));
    };
    const el = grid.current;
    if (!el || !("IntersectionObserver" in window)) {
      play();
      return () => timers.forEach(clearTimeout);
    }
    const io = new IntersectionObserver(
      (es) => {
        if (!es.some((e) => e.isIntersecting)) return;
        io.disconnect();
        play();
      },
      { threshold: 0.3 },
    );
    io.observe(el);
    return () => {
      io.disconnect();
      timers.forEach(clearTimeout);
    };
  }, []);

  return (
    <div ref={grid} className="home-executors">
      <div data-reveal className="home-card">
        <div className="home-card__head">
          <Icon name="activity" size={14} />
          <span>Ostra's agent loop</span>
          <span style={{ marginLeft: "auto" }}>
            <Chip mono tone="accent">
              native
            </Chip>
          </span>
        </div>
        <p className="home-card__body">
          Streams as Activity: a thinking summary, text, and each tool call with its diff, output and policy decision. A
          denied call shows the rule and what to do instead.
        </p>
        <div className="home-card__demo">
          <div className="home-card__calls">
            {calls >= 1 && (
              <div className="home-stream-in">
                <ToolCall
                  tool="Read"
                  summary="src/orders/refund.rs"
                  state={calls === 1 ? "running" : "done"}
                  duration={calls > 1 ? "0.1s" : undefined}
                />
              </div>
            )}
            {calls >= 2 && (
              <div className="home-stream-in">
                <ToolCall
                  tool="Edit"
                  summary="src/orders/refund.rs"
                  state={calls === 2 ? "running" : "done"}
                  duration={calls > 2 ? "0.2s" : undefined}
                />
              </div>
            )}
            {calls === 3 && (
              <div className="home-stream-in">
                <ToolCall tool="Bash" summary="git push origin refunds" state="running" />
              </div>
            )}
            {calls >= 4 && (
              <ToolCall tool="Bash" summary="git push origin refunds" state="error" policy={DENY} defaultOpen />
            )}
          </div>
        </div>
      </div>
      <div data-reveal className="home-card">
        <div className="home-card__head">
          <Icon name="square-terminal" size={14} />
          <span>CLI harnesses</span>
        </div>
        <div className="home-card__body" style={{ display: "grid", gap: 14 }}>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
            {["claude-code", "codex", "grok-build", "antigravity"].map((h) => (
              <Chip key={h} mono>{`harness:${h}`}</Chip>
            ))}
          </div>
          <p style={{ margin: 0 }}>
            Claude Code, Codex, Grok Build and Antigravity stream their own terminal, and you can type into it while the
            execution runs. Hook decisions are recorded in a Tool calls tab. A permission ask pauses the run and shows a
            banner above the terminal.
          </p>
        </div>
        <div className="home-card__term">
          {ask && (
            <div className="home-stream-in" style={{ flex: "none" }}>
              <Banner
                tone="warn"
                title="The harness is paused on a permission ask"
                actions={
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => {
                      setAsk(false);
                      setAllowed(true);
                    }}
                  >
                    Allow once
                  </Button>
                }
              >
                <code>sqlx migrate run</code> needs your answer. The hook bridge holds the call until you decide.
              </Banner>
            </div>
          )}
          <div style={{ flex: 1, minHeight: 0 }}>
            <Terminal
              title="codex"
              meta="~/code/acme/backend"
              lines={[...HARNESS.slice(0, lines), ...(allowed ? ALLOWED : [])]}
              live
              height="100%"
            />
          </div>
        </div>
      </div>
    </div>
  );
}
