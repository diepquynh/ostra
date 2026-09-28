import { Icon, IconButton, type IconName, LiveMark, type MarkState, REST } from "@ostra/design";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { GitHubMark } from "../shared/GitHubMark";
import { docsHref, REPO } from "../shared/links";
import { useSiteTheme } from "../shared/theme";
import { ConsoleShot } from "./ConsoleShot";
import { Executors } from "./Executors";
import { Install } from "./Install";
import { Intro } from "./Intro";
import { Pipeline } from "./Pipeline";

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
    <div data-reveal className="home-section__head">
      <div className="site-eyebrow">{eyebrow}</div>
      <h2 className="home-h2">{title}</h2>
      <p className="home-lede">{children}</p>
    </div>
  );
}

function useScrolled() {
  const [scrolled, setScrolled] = useState(false);
  useEffect(() => {
    const on = () => setScrolled(window.scrollY > 4);
    on();
    window.addEventListener("scroll", on, { passive: true });
    return () => window.removeEventListener("scroll", on);
  }, []);
  return scrolled;
}

export function Homepage() {
  const [pipeMark, setPipeMark] = useState<MarkState | null>(null);
  const mark = pipeMark ?? REST;
  const [theme, setTheme] = useSiteTheme(mark);
  const [landed, setLanded] = useState(false);
  const [typing, setTyping] = useState(false);
  const navMark = useRef<HTMLDivElement>(null);
  const scrolled = useScrolled();
  return (
    <div className="home">
      <Intro
        navMark={navMark}
        onHandoff={() => setTimeout(() => setTyping(true), 250)}
        onLanded={() => setLanded(true)}
      />
      <div data-reveal="fade" className="home-nav-bar" data-scrolled={scrolled || undefined}>
        <nav className="home-nav">
          <div ref={navMark} className="home-nav__mark" style={{ opacity: landed ? 1 : 0 }}>
            <LiveMark state={mark} size={22} />
          </div>
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
      </div>

      <header className="home-hero">
        <h1 className="home-h1">
          <span data-reveal="line">
            <span>Every stage shown.</span>
          </span>
          <span data-reveal="line">
            <span>Every gate yours.</span>
          </span>
        </h1>
        <p data-reveal className="home-hero__lede">
          Ostra runs a full engineering pipeline on your machine: research, spec, fact-check, plan, build, review, test
          and docs. Code drives the pipeline and holds every gate. Models do the work inside each stage, and you approve
          the spec and the plan before anything is built.
        </p>
        <Install run={typing} />
      </header>

      <ConsoleShot theme={theme} setTheme={setTheme} />

      <Pipeline onMark={setPipeMark} />

      <section className="home-section">
        <SectionHead eyebrow="Security" title="Secure by design">
          Code holds every boundary, not the model. Each tool call passes guards and your permissions first, and the
          command it runs sits in a kernel sandbox underneath.
        </SectionHead>
        <div className="home-security">
          {SECURITY.map((s) => (
            <div data-reveal key={s.title} className="home-security__item">
              <div data-line className="home-security__line" />
              <div className="home-security__title">
                <Icon name={s.icon} size={16} />
                <span>{s.title}</span>
              </div>
              <p>{s.body}</p>
            </div>
          ))}
        </div>
        <p data-reveal className="home-footnote">
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
        <Executors />
      </section>

      <footer className="home-footer">
        <div className="home-footer__cols">
          <div data-reveal className="home-footer__about">
            <p className="home-footer__tagline">
              A local workspace that runs the whole engineering pipeline, one gate at a time.
            </p>
            <p className="home-muted" style={{ fontSize: 13 }}>
              Open source under the <a href={`${REPO}/blob/master/LICENSE`}>MIT License</a>.
            </p>
          </div>
          {FOOTER_LINKS.map((col) => (
            <div data-reveal key={col.label} className="home-footer__col">
              <div className="site-eyebrow">{col.label}</div>
              {col.links.map(([label, page]) => (
                <a key={page} href={docsHref(page)}>
                  {label}
                </a>
              ))}
            </div>
          ))}
        </div>
        <div data-reveal="letters" aria-hidden="true" className="home-footer__word">
          {[..."ostra"].map((c, i) => (
            <span key={c} style={{ transitionDelay: `${i * 0.07}s` }}>
              {c}
            </span>
          ))}
        </div>
      </footer>
    </div>
  );
}
