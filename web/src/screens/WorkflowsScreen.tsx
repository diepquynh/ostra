import { Banner, Button, Chip, Input, Panel, Select } from "@ostra/design";
import { useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { api } from "../api";
import type { FunctionDoc, WorkflowDoc } from "../api/types";
import { TransformEditor } from "../features/builder/TransformEditor";
import { WorkflowBuilder } from "../features/builder/WorkflowBuilder";
import { useAsync } from "../lib/hooks";
import { useNav, useWorkspace } from "../lib/nav";
import { LoadError, Loading, Page } from "./workspace/Page";

const BASES = ["implement", "research", "spec", "plan", "verify", "test", "docs", "prompt", "quick-change"];

/**
 * Rules WB1, WB7, and WF9: the workspace's workflows and transform functions. `ws:workflows#<name>` opens the
 * Workflow builder, and `ws:workflows#transform:<name>` the editor of a composite transform.
 */
export function WorkflowsScreen({ ws }: { ws: string }) {
  const { hash } = useLocation();
  const open = decodeURIComponent(hash.replace(/^#/, ""));
  if (open.startsWith("transform:")) return <TransformPage ws={ws} anchor={open.slice("transform:".length)} />;
  return open ? <BuilderPage ws={ws} name={open} /> : <WorkflowList ws={ws} />;
}

function WorkflowList({ ws }: { ws: string }) {
  const { detail, reload } = useWorkspace();
  const nav = useNav();
  const [name, setName] = useState("");
  const [base, setBase] = useState("implement");
  const [error, setError] = useState<string | null>(null);
  const [fnName, setFnName] = useState("");
  const palette = useAsync(() => api.builderPalette(ws), [ws]);
  const custom = (palette.data?.transforms ?? []).filter((t) => t.custom || t.plugin);

  if (!detail) {
    return (
      <Page title="Workflows">
        <Loading>Reading the workflows…</Loading>
      </Page>
    );
  }

  const restore = async () => {
    setError(null);
    try {
      await api.restoreWorkflows(ws);
      reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const create = () => {
    const n = name.trim();
    if (!/^[a-z][a-z0-9-]*$/.test(n)) {
      setError("Name the workflow in lowercase kebab-case, starting with a letter.");
      return;
    }
    nav.open("ws:workflows", { anchor: `${encodeURIComponent(n)}?from=${base}` });
  };

  const remove = async (n: string) => {
    setError(null);
    try {
      await api.deleteWorkflow(ws, n);
      reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <Page
      title="Workflows"
      sub="Each workflow is a graph of nodes: Ostra's stages, agents, plugin stages, transforms, and prompts. A session runs the one you pick, or the one for its category."
    >
      {detail.missing_workflows.length > 0 && (
        <Banner
          tone="warn"
          actions={
            <Button size="sm" onClick={() => void restore()}>
              Add them
            </Button>
          }
        >
          This workspace has no copy of Ostra's default {detail.missing_workflows.join(", ")} workflow
          {detail.missing_workflows.length === 1 ? "" : "s"}, so sessions run Ostra's own. Add the copies to edit them.
        </Banner>
      )}
      {error && <Banner tone="bad">{error}</Banner>}
      <Panel title="New workflow" icon="plus">
        <div className="wp-row" style={{ gap: 8 }}>
          <Input
            aria-label="Workflow name"
            placeholder="secure-implement"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <Select
            aria-label="Start from"
            value={base}
            onChange={(e) => setBase(e.target.value)}
            options={[
              ...BASES.map((b) => ({ value: b, label: `Start from ${b}` })),
              ...detail.workflows
                .filter((w) => !BASES.includes(w.name))
                .map((w) => ({ value: w.name, label: `Start from ${w.name}` })),
            ]}
          />
          <Button variant="primary" icon="workflow" onClick={create}>
            Open in the builder
          </Button>
        </div>
      </Panel>
      <Panel
        title="Transforms"
        icon="square-function"
        subtitle="Your own transform functions, built from Ostra's and each other. Any workflow node can run one."
      >
        <div className="wp-stack" style={{ gap: 6 }}>
          {custom.map((t) => (
            <div key={t.name} className="wp-row" style={{ gap: 8 }}>
              {t.plugin ? (
                <span
                  style={{ fontFamily: "var(--font-mono)", padding: "0 8px" }}
                  title={`Takes ${t.inputs.map((p) => `${p.name} (${p.kind})`).join(", ") || "no inputs"}`}
                >
                  {t.name}
                </span>
              ) : (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => nav.open("ws:workflows", { anchor: `transform:${encodeURIComponent(t.name)}` })}
                  style={{ fontFamily: "var(--font-mono)" }}
                >
                  {t.name}
                </Button>
              )}
              <Chip>gives {t.output}</Chip>
              {t.plugin && <Chip tone="info">plugin {t.plugin}, runs in code</Chip>}
              <span className="wp-muted">{t.description}</span>
            </div>
          ))}
          <div className="wp-row" style={{ gap: 8 }}>
            <Input
              aria-label="Transform name"
              placeholder="high-risk-files"
              value={fnName}
              onChange={(e) => setFnName(e.target.value)}
            />
            <Button
              icon="plus"
              disabled={!/^[a-z][a-z0-9-]*$/.test(fnName.trim())}
              onClick={() => nav.open("ws:workflows", { anchor: `transform:${encodeURIComponent(fnName.trim())}?new` })}
            >
              New transform
            </Button>
          </div>
        </div>
      </Panel>
      <Panel title="Workflows" icon="workflow">
        <div className="wp-stack" style={{ gap: 6 }}>
          {detail.workflows.map((w) => (
            <div key={w.name} className="wp-row" style={{ gap: 8 }}>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => nav.open("ws:workflows", { anchor: encodeURIComponent(w.name) })}
                style={{ fontFamily: "var(--font-mono)" }}
              >
                {w.name}
              </Button>
              <Chip>base {w.base.toLowerCase().replace("_", "-")}</Chip>
              {w.builtin && <Chip tone="warn">Ostra's default, no copy</Chip>}
              {w.plugin && <Chip tone="info">plugin {w.plugin}, read only</Chip>}
              {w.default_for.length > 0 && (
                <Chip tone="accent">default for {w.default_for.join(", ").toLowerCase()}</Chip>
              )}
              <span className="wp-muted" style={{ flex: 1 }}>
                {w.description} {w.stages.length} nodes.
              </span>
              {!w.builtin && !w.plugin && (
                <Button
                  size="sm"
                  variant="ghost"
                  icon="trash-2"
                  aria-label={`Delete ${w.name}`}
                  onClick={() => void remove(w.name)}
                />
              )}
            </div>
          ))}
        </div>
      </Panel>
    </Page>
  );
}

function BuilderPage({ ws, name: anchor }: { ws: string; name: string }) {
  const { detail, reload } = useWorkspace();
  const nav = useNav();
  // Opening the tab that is already active without an anchor changes nothing, so leave the builder by
  // dropping the hash from the URL.
  const navigate = useNavigate();
  const [name, query] = anchor.split("?");
  const from = new URLSearchParams(query ?? "").get("from");
  const palette = useAsync(() => api.builderPalette(ws), [ws]);
  const doc = useAsync<WorkflowDoc>(async () => {
    if (!from) return api.workflow(ws, name);
    const start = await api.workflow(ws, from);
    return {
      ...start,
      name,
      builtin: false,
      flattened: false,
      plugin: null,
      file: { ...start.file, description: "", default_for: [], layout: start.file.layout },
    };
  }, [ws, name, from]);

  const back = (
    <Button size="sm" icon="arrow-left" variant="ghost" onClick={() => navigate(nav.href("ws:workflows"))}>
      All workflows
    </Button>
  );
  if (doc.error) {
    return (
      <Page title="Workflow builder" actions={back}>
        <LoadError error={doc.error} onRetry={doc.reload} />
      </Page>
    );
  }
  if (!doc.data || !palette.data || !detail) {
    return (
      <Page title="Workflow builder" actions={back}>
        <Loading>Reading the workflow…</Loading>
      </Page>
    );
  }
  return (
    <Page title="Workflow builder" actions={back}>
      {doc.data.builtin && (
        <Banner tone="info">
          This is Ostra's default. Saving writes the workspace's own copy, which sessions then run.
        </Banner>
      )}
      {doc.data.plugin && (
        <Banner tone="info">
          Plugin {doc.data.plugin} builds this workflow in code, so it is read only here. Start a new workflow from it
          to change a copy.
        </Banner>
      )}
      {doc.data.flattened && !doc.data.plugin && (
        <Banner tone="info">The file uses `extends` or `remove`. Saving writes every node into it.</Banner>
      )}
      {doc.data.issues.length > 0 && <Banner tone="bad">{doc.data.issues.join(" ")}</Banner>}
      <WorkflowBuilder
        key={`${name}|${from ?? ""}`}
        ws={ws}
        name={name}
        initial={doc.data.file}
        palette={palette.data}
        agents={detail.agents}
        readOnly={!!doc.data.plugin}
        onSaved={(n) => {
          reload();
          if (from) nav.open("ws:workflows", { anchor: encodeURIComponent(n) });
        }}
      />
    </Page>
  );
}

function TransformPage({ ws, anchor }: { ws: string; anchor: string }) {
  const nav = useNav();
  const navigate = useNavigate();
  const { reload } = useWorkspace();
  const [name, query] = anchor.split("?");
  const fresh = query === "new";
  const palette = useAsync(() => api.builderPalette(ws), [ws]);
  const doc = useAsync<FunctionDoc>(
    () =>
      fresh
        ? Promise.resolve({
            name,
            file: { description: "", output: "", input: [], arg: [], step: [] },
            info: null,
            issues: [],
            used_by: [],
          })
        : api.transformFunction(ws, name),
    [ws, name, fresh],
  );
  const back = (
    <Button size="sm" icon="arrow-left" variant="ghost" onClick={() => navigate(nav.href("ws:workflows"))}>
      All workflows
    </Button>
  );
  if (doc.error) {
    return (
      <Page title="Transform" actions={back}>
        <LoadError error={doc.error} onRetry={doc.reload} />
      </Page>
    );
  }
  if (!doc.data || !palette.data) {
    return (
      <Page title="Transform" actions={back}>
        <Loading>Reading the function…</Loading>
      </Page>
    );
  }
  return (
    <Page title="Transform" actions={back}>
      {doc.data.issues.length > 0 && <Banner tone="bad">{doc.data.issues.join(" ")}</Banner>}
      <TransformEditor
        key={anchor}
        ws={ws}
        name={name}
        initial={doc.data.file}
        transforms={palette.data.transforms}
        usedBy={doc.data.used_by}
        onSaved={() => {
          reload();
          if (fresh) nav.open("ws:workflows", { anchor: `transform:${encodeURIComponent(name)}` });
        }}
        onDeleted={() => {
          reload();
          navigate(nav.href("ws:workflows"));
        }}
      />
    </Page>
  );
}
