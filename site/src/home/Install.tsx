import { IconButton, Tabs } from "@ostra/design";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { docsHref } from "../shared/links";
import { reducedMotion } from "./reveal";

type Line = { cmd: string; note?: string };

const CLONE: Line = { cmd: "git clone https://github.com/diepquynh/ostra && cd ostra" };

const NEEDS = "Needs rustup, Node.js 24, a C compiler and git. On Linux, also bubblewrap.";

type Method = { label: string; note: string; lines: Line[]; prompt?: string; needs?: string; anchor?: string };

const METHODS: Record<string, Method> = {
  quick: {
    label: "Quick start",
    note: "Builds the UI and the binary, then opens a sign-in URL in your browser.",
    lines: [
      CLONE,
      { cmd: "export ANTHROPIC_API_KEY=sk-ant-...", note: "# or OPENAI_API_KEY, or add a key in setup" },
      { cmd: "./run.sh", note: "# builds, then starts Ostra on 127.0.0.1" },
    ],
  },
  docker: {
    label: "Docker",
    note: "Set the projects mount in docker-compose.yml to one host folder first.",
    lines: [
      CLONE,
      { cmd: "mkdir -p config && sudo chown 1000:1000 config" },
      { cmd: "docker compose up -d", note: "# publishes on 127.0.0.1:7878" },
      { cmd: "docker compose exec ostra ostra url", note: "# open it on localhost:7878" },
    ],
  },
  windows: {
    label: "Windows",
    note: "Runs Ostra as a logon task, with no administrator rights. Agent commands run without a sandbox on Windows.",
    prompt: "PS>",
    needs: "Needs rustup, Node.js 24, Visual Studio Build Tools with the C++ workload, and Git for Windows.",
    anchor: "windows",
    lines: [
      // Windows PowerShell 5.1 has no `&&`.
      { cmd: "git clone https://github.com/diepquynh/ostra; cd ostra" },
      { cmd: ".\\install.ps1", note: "# builds, starts the task, prints a sign-in URL" },
      { cmd: ".\\install.ps1 url", note: "# a fresh URL; add a model key in setup" },
    ],
  },
};

// One character per step, in milliseconds.
const CHAR_MS = 14;

/** A command line that types in: a clip steps across the monospace text, one character per step. */
function TypedLine({ line, delay, run }: { line: Line; delay: number; run: boolean }) {
  const el = useRef<HTMLSpanElement>(null);
  const text = line.note ? `${line.cmd}  ${line.note}` : line.cmd;
  // biome-ignore lint/correctness/useExhaustiveDependencies: the line types once, when `run` turns on.
  useLayoutEffect(() => {
    if (!run || !el.current?.animate || reducedMotion()) return;
    const a = el.current.animate([{ clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0 0 0)" }], {
      duration: text.length * CHAR_MS,
      delay,
      easing: `steps(${text.length}, end)`,
      fill: "backwards",
    });
    return () => a.cancel();
  }, [run]);
  return (
    <span ref={el} className="home-install__typed" style={run ? undefined : { clipPath: "inset(0 100% 0 0)" }}>
      <span>{line.cmd}</span>
      {line.note && <span className="home-install__note">{`  ${line.note}`}</span>}
    </span>
  );
}

/** The install box. Its commands type in once `run` turns on, and again after each change of method. */
export function Install({ run }: { run: boolean }) {
  const [method, setMethod] = useState("quick");
  const [changed, setChanged] = useState(false);
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const m = METHODS[method];
  const delays: number[] = [];
  let acc = changed ? 0 : 650;
  for (const l of m.lines) {
    delays.push(acc);
    acc += (l.note ? l.cmd.length + 2 + l.note.length : l.cmd.length) * CHAR_MS + 140;
  }

  const copy = () => {
    void navigator.clipboard?.writeText(m.lines.map((l) => l.cmd).join("\n")).catch(() => {});
    clearTimeout(timer.current);
    setCopied(true);
    timer.current = setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div data-reveal className="home-install">
      <div className="home-install__method">
        <Tabs
          variant="segmented"
          label="Install method"
          tabs={Object.entries(METHODS).map(([id, x]) => ({ id, label: x.label }))}
          value={method}
          onChange={(id) => {
            setMethod(id);
            setChanged(true);
            setCopied(false);
          }}
        />
        <span className="home-muted">{m.note}</span>
      </div>
      <div className="home-install__box">
        <div className="home-install__lines">
          {m.lines.map((l, i) => (
            <div key={`${method}:${l.cmd}`} className="home-install__line">
              <span className="home-install__prompt">{m.prompt ?? "$"}</span>
              <TypedLine line={l} delay={delays[i]} run={run} />
            </div>
          ))}
        </div>
        <div style={{ padding: 6 }}>
          <IconButton icon={copied ? "check" : "copy"} label={copied ? "Copied" : "Copy the commands"} onClick={copy} />
        </div>
      </div>
      <div className="home-install__needs">
        <span>{m.needs ?? NEEDS}</span>
        <a href={m.anchor ? docsHref("install", m.anchor) : docsHref("build-and-run")}>Full install steps</a>
      </div>
    </div>
  );
}
