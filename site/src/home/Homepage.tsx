import {
  Chip,
  Icon,
  IconButton,
  type IconName,
  LiveMark,
  type PolicyInfo,
  REST,
  Terminal,
  type TerminalLine,
  ToolCall,
} from "@ostra/design";
import type { ReactNode } from "react";
import { GitHubMark } from "../shared/GitHubMark";
import { docsHref, REPO } from "../shared/links";
import { useSiteTheme } from "../shared/theme";
import { ConsoleShot } from "./ConsoleShot";
import { Install } from "./Install";

const Code = ({ children }: { children: ReactNode }) => <code className="site-code">{children}</code>;

const SECURITY: { icon: IconName; title: string; body: ReactNode }[] = [
  {
    icon: "box",
    title: "Agent commands run in a sandbox",
    body: (
      <>
        The kernel enforces it under every command: bubblewrap on Linux, Seatbelt on macOS. The host is read-only, only
        the execution's own folders are writable, and Ostra's data, your credentials and the keychain are not reachable.
        On Linux without bubblewrap, every execution is refused.
      </>
    ),
  },
  {
    icon: "lock",
    title: "Guards come before permissions",
    body: (
      <>
        Each agent writes only inside its own scope, the implementer cannot write tests, and Ostra's binary, config and
        databases are read-only to agents. No permission rule, answer or YOLO setting overrides a guard. Your allow, ask
        and deny rules, such as <Code>Bash(git push *)</Code>, apply after.
      </>
    ),
  },
  {
    icon: "scan-search",
    title: "Security findings cannot be waived",
    body: (
      <>
        The code reviewer scans every change for destructive operations, backdoors, secret exfiltration, obfuscated
        payloads and prompt injection. A <Code>SEC-BLOCK-*</Code> finding goes back to the fix agent until the code is
        removed, with no cap, and no gate answer waives it.
      </>
    ),
  },
  {
    icon: "key-round",
    title: "Keys stay on your machine",
    body: (
      <>
        Provider keys come from the environment or the keychain, or are sealed in Ostra's registry. They never reach the
        browser or a folder file. A harness starts with provider variables removed from its environment, and git
        credentials reach git one command at a time, never an agent.
      </>
    ),
  },
  {
    icon: "log-in",
    title: "Localhost, one-time sign-in",
    body: (
      <>
        Ostra listens on <Code>127.0.0.1</Code>. The sign-in link carries a token that works once, and a request with
        any other <Code>Host</Code> header is refused, which blocks DNS rebinding.
      </>
    ),
  },
  {
    icon: "file-check",
    title: "Repo files cannot start commands",
    body: (
      <>
        A <Code>workspace.toml</Code> or <Code>project.toml</Code> can arrive with a git pull. The MCP servers, code
        providers, allow rules and format command it names do not start until you approve that exact content.
      </>
    ),
  },
];

const DENY: PolicyInfo = {
  decision: "deny",
  layer: "permission",
  rule: "Bash(git push *)",
  reason: "This workspace denies pushes.",
  advice: "Leave the changes staged. Ostra stages each phase after review, and you push.",
};

const HARNESS_LINES: TerminalLine[] = [
  ["fn", "› Implement phase 1: refund model and migration"],
  ["muted", "  Read migrations/0012_orders.sql"],
  ["add", "  Write migrations/0013_refunds.sql"],
  ["warn", "  Permission asked: Bash(sqlx migrate run)"],
];

const FOOTER_LINKS: { label: string; links: [string, string][] }[] = [
  {
    label: "Start",
    links: [
      ["Build and run", "build-and-run"],
      ["Model access", "model-access"],
      ["Configuration", "configuration"],
    ],
  },
  {
    label: "Internals",
    links: [
      ["The engine", "engine"],
      ["Security", "security"],
      ["API", "api"],
    ],
  },
];

function SectionHead({ eyebrow, title, children }: { eyebrow: string; title: string; children: ReactNode }) {
  return (
    <div className="home-section__head">
      <div className="site-eyebrow">{eyebrow}</div>
      <h2 className="home-h2">{title}</h2>
      <p className="home-lede">{children}</p>
    </div>
  );
}

export function Homepage() {
  const [theme, setTheme] = useSiteTheme();
  return (
    <div className="home">
      <nav className="home-nav">
        <LiveMark state={REST} size={22} />
        <span className="home-wordmark">Ostra</span>
        <div className="home-nav__links">
          <a className="site-quiet-link" href={docsHref()}>
            Docs
          </a>
          <a className="site-quiet-link" href={REPO}>
            <GitHubMark />
            GitHub
          </a>
          <IconButton
            icon={theme === "dark" ? "sun" : "moon"}
            label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
            onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
          />
        </div>
      </nav>

      <header className="home-hero">
        <h1 className="home-h1">
          Every stage shown.
          <br />
          Every gate yours.
        </h1>
        <p className="home-hero__lede">
          Ostra runs a full engineering pipeline on your machine: research, spec, fact-check, plan, build, review, test
          and docs. Code drives the pipeline and holds every gate. Models do the work inside each stage, and you approve
          the spec and the plan before anything is built.
        </p>
        <Install />
      </header>

      <ConsoleShot theme={theme} setTheme={setTheme} />

      <section className="home-section">
        <SectionHead eyebrow="Security" title="Secure by design">
          Code holds every boundary, not the model. Each tool call passes guards and your permissions first, and the
          command it runs sits in a kernel sandbox underneath.
        </SectionHead>
        <div className="home-security">
          {SECURITY.map((s) => (
            <div key={s.title} className="home-security__item">
              <div className="home-security__title">
                <Icon name={s.icon} size={16} />
                <span>{s.title}</span>
              </div>
              <p>{s.body}</p>
            </div>
          ))}
        </div>
        <p className="home-footnote">
          A browser security suite tests these defenses in a real Chromium: agent and repo text in every render path,
          terminal escape sequences, cross-origin requests, the sign-in token, uploads and browser storage.{" "}
          <a href={docsHref("browser-security-suite")}>tests/browser</a>
        </p>
      </section>

      <section className="home-section">
        <SectionHead eyebrow="Executors" title="Runs the agent you already use">
          Each stage runs on an executor you pick per agent. The pipeline and its gates stay the same whichever one does
          the work.
        </SectionHead>
        <div className="home-executors">
          <div className="home-card">
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
              Streams as Activity: a thinking summary, text, and each tool call with its diff, output and policy
              decision. A denied call shows the rule and what to do instead.
            </p>
            <div className="home-card__demo">
              <ToolCall tool="Read" summary="src/orders/refund.rs" state="done" duration="0.1s" />
              <ToolCall tool="Edit" summary="src/orders/refund.rs" state="done" duration="0.2s" />
              <ToolCall tool="Bash" summary="git push origin refunds" state="error" policy={DENY} defaultOpen />
            </div>
          </div>
          <div className="home-card">
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
                Claude Code, Codex, Grok Build and Antigravity stream their own terminal, and you can type into it while
                the execution runs. Hook decisions are recorded in a Tool calls tab. A permission ask pauses the run and
                shows a banner above the terminal.
              </p>
            </div>
            <div className="home-card__demo">
              <Terminal title="codex" meta="~/code/acme/backend" lines={HARNESS_LINES} height={150} />
            </div>
          </div>
        </div>
      </section>

      <footer className="home-footer">
        <div className="home-footer__cols">
          <div className="home-footer__about">
            <p className="home-footer__tagline">
              A local workspace that runs the whole engineering pipeline, one gate at a time.
            </p>
            <p className="home-muted" style={{ fontSize: 13 }}>
              Open source under the <a href={`${REPO}/blob/master/LICENSE`}>MIT License</a>.
            </p>
          </div>
          {FOOTER_LINKS.map((col) => (
            <div key={col.label} className="home-footer__col">
              <div className="site-eyebrow">{col.label}</div>
              {col.links.map(([label, page]) => (
                <a key={page} href={docsHref(page)}>
                  {label}
                </a>
              ))}
            </div>
          ))}
        </div>
        <div aria-hidden="true" className="home-footer__word">
          ostra
        </div>
      </footer>
    </div>
  );
}
