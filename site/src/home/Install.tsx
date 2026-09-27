import { IconButton, Tabs } from "@ostra/design";
import { useEffect, useRef, useState } from "react";
import { docsHref } from "../shared/links";

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

export function Install() {
  const [method, setMethod] = useState("quick");
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const m = METHODS[method];

  const copy = () => {
    void navigator.clipboard?.writeText(m.lines.map((l) => l.cmd).join("\n")).catch(() => {});
    clearTimeout(timer.current);
    setCopied(true);
    timer.current = setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div className="home-install">
      <div className="home-install__method">
        <Tabs
          variant="segmented"
          label="Install method"
          tabs={Object.entries(METHODS).map(([id, x]) => ({ id, label: x.label }))}
          value={method}
          onChange={(id) => {
            setMethod(id);
            setCopied(false);
          }}
        />
        <span className="home-muted">{m.note}</span>
      </div>
      <div className="home-install__box">
        <div className="home-install__lines">
          {m.lines.map((l) => (
            <div key={l.cmd} className="home-install__line">
              <span className="home-install__prompt">{m.prompt ?? "$"}</span>
              <span>{l.cmd}</span>
              {l.note && <span className="home-install__note">{l.note}</span>}
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
