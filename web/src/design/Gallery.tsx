import { type ReactNode, useEffect, useState } from "react";
import {
  Banner,
  Breadcrumbs,
  Button,
  Checkbox,
  Chip,
  CodeView,
  CommandPalette,
  Decision,
  Dialog,
  DiffView,
  ExecutionGroup,
  type FolderLister,
  FolderPicker,
  type FsEntry,
  GateCard,
  ICON_NAMES,
  Icon,
  IconButton,
  type IconName,
  Input,
  Kbd,
  type LaneId,
  LaneStepper,
  Menu,
  type PaletteItem,
  Panel,
  PhaseDag,
  SectionLabel,
  Select,
  Spinner,
  StageRow,
  StatusChip,
  StatusDot,
  Stepper,
  Switch,
  Table,
  Tabs,
  Terminal,
  type TerminalLine,
  ToolCall,
  TreeItem,
  TreeSection,
} from "./index";

const GALLERY_CSS = `
.gx-page{padding:20px 28px 60px;display:flex;flex-direction:column;gap:28px;background:var(--surface-editor);min-height:100vh}
.gx-card{display:flex;flex-direction:column;gap:10px}
.gx-card__head{display:flex;align-items:baseline;gap:10px}
.gx-card__name{font:var(--type-heading)}
.gx-card__sub{font-size:var(--text-sm);color:var(--text-muted)}
.gx-frame{padding:20px;border:1px dashed var(--border-default);border-radius:var(--radius-md);background:var(--surface-editor)}
.gx-r{display:flex;flex-wrap:wrap;gap:10px;align-items:center}
.gx-c{display:flex;flex-direction:column;gap:14px}
.gx-l{font:var(--type-label);letter-spacing:var(--tracking-label);text-transform:uppercase;color:var(--text-muted);width:72px;flex:none}
`;

function Card({
  name,
  subtitle,
  width = 700,
  children,
}: {
  name: string;
  subtitle: string;
  width?: number;
  children: ReactNode;
}) {
  return (
    <section className="gx-card">
      <div className="gx-card__head">
        <span className="gx-card__name">{name}</span>
        <span className="gx-card__sub">{subtitle}</span>
      </div>
      <div className="gx-frame" style={{ maxWidth: width }}>
        {children}
      </div>
    </section>
  );
}

function CoreCard() {
  const glyphs: IconName[] = [
    "folder",
    "file-text",
    "git-pull-request",
    "terminal",
    "square-terminal",
    "shield-alert",
    "circle-check",
    "circle-alert",
    "brain",
    "coins",
    "message-square",
    "settings",
    "play",
    "square",
    "list-checks",
    "file-diff",
  ];
  return (
    <Card name="Core" subtitle="Button, IconButton, Kbd, Icon">
      <div className="gx-c">
        <div className="gx-r">
          <span className="gx-l">Variants</span>
          <Button variant="primary" icon="check" kbd="⌘↵">
            Approve the spec
          </Button>
          <Button>Request changes</Button>
          <Button variant="ghost" icon="git-branch">
            main
          </Button>
          <Button variant="danger">Deny</Button>
        </div>
        <div className="gx-r">
          <span className="gx-l">Sizes</span>
          <Button size="sm" variant="primary">
            Start
          </Button>
          <Button size="sm">Allow once</Button>
          <Button size="lg" variant="primary" icon="play">
            Start task
          </Button>
          <Button disabled>Disabled</Button>
        </div>
        <div className="gx-r">
          <span className="gx-l">Icon</span>
          <IconButton icon="panel-left" label="Sidebar" />
          <IconButton icon="panel-right" label="Quick question" active />
          <IconButton icon="search" label="Search" variant="default" />
          <IconButton icon="x" label="Close" size="sm" />
          <Kbd keys={["⌘", "K"]} />
          <Kbd>Esc</Kbd>
        </div>
        <div className="gx-r" style={{ color: "var(--text-secondary)" }}>
          <span className="gx-l">Glyphs</span>
          {glyphs.map((n) => (
            <Icon key={n} name={n} />
          ))}
        </div>
      </div>
    </Card>
  );
}

function IconsCard() {
  return (
    <Card name="Icon map" subtitle={`Every mapped Lucide name (${ICON_NAMES.length})`} width={900}>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(150px, 1fr))",
          gap: 8,
          color: "var(--text-secondary)",
        }}
      >
        {ICON_NAMES.map((n) => (
          <div key={n} style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0 }}>
            <Icon name={n} />
            <span
              style={{
                font: "var(--text-2xs)/1.2 var(--font-mono)",
                color: "var(--text-muted)",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {n}
            </span>
          </div>
        ))}
      </div>
    </Card>
  );
}

const FS: Record<string, FsEntry[]> = {
  "/": [{ name: "home" }, { name: "usr" }, { name: "tmp" }],
  "/home": [{ name: "me" }],
  "/home/me": [{ name: "code" }, { name: "notes" }, { name: "Downloads" }],
  "/home/me/code": [
    { name: "shop-backend", is_git: true, is_ostra_project: true },
    { name: "shop-web", is_git: true, is_ostra_project: true },
    { name: "shop-admin", is_git: true },
    { name: "notes" },
  ],
  "/home/me/code/shop-backend": [{ name: "src" }, { name: "migrations" }, { name: "tests" }],
};

const parentOf = (p: string) => (p === "/" ? null : p.slice(0, p.lastIndexOf("/")) || "/");

const fakeList: FolderLister = async (raw) => {
  const path = raw.startsWith("~") ? "/home/me" + raw.slice(1) : raw;
  await new Promise((r) => setTimeout(r, 60));
  const entries = FS[path];
  if (!entries) {
    let nearest: string | null = parentOf(path);
    while (nearest && !FS[nearest]) nearest = parentOf(nearest);
    return { path, parent: parentOf(path), entries: [], exists: false, nearest, home: "/home/me" };
  }
  return { path, parent: parentOf(path), entries: entries.map((e) => ({ is_dir: true, ...e })), home: "/home/me" };
};

function FormsCard() {
  const [v, setV] = useState("/home/me/code/sh");
  const [browse, setBrowse] = useState("/home/me/code");
  const [live, setLive] = useState("~/code/");
  const [yolo, setYolo] = useState(true);
  const [tests, setTests] = useState(true);
  const [answer, setAnswer] = useState("soft");
  return (
    <Card name="Forms" subtitle="Input, Select, Checkbox, Switch, FolderPicker">
      <div className="gx-c">
        <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1fr) minmax(0, 1fr)", gap: 16 }}>
          <div className="gx-c">
            <Input icon="search" placeholder="Search lessons" />
            <Input label="Project key" mono defaultValue="shop-backend" hint="Lowercase letters, digits and dashes." />
            <Input
              label="Allow rule"
              mono
              defaultValue="Bash(rm -rf *)"
              error="Deny beats allow: this pattern is already denied globally."
            />
          </div>
          <div className="gx-c">
            <Input
              multiline
              rows={2}
              placeholder="Add order cancellation: customers can cancel until the order ships."
            />
            <div className="gx-r">
              <Select size="sm" mono options={["native", "harness:claude", "harness:codex"]} />
              <Select size="sm" options={["fast", "balanced", "advanced", "frontier"]} defaultValue="advanced" />
            </div>
            <div className="gx-r">
              <Checkbox label="Write tests" checked={tests} onChange={(e) => setTests(e.target.checked)} />
              <Checkbox label="Update docs" />
              <Switch label="YOLO" tone="warn" checked={yolo} onChange={(e) => setYolo(e.target.checked)} />
              <Switch label="Push" />
            </div>
            <Checkbox
              radio
              name="q"
              checked={answer === "soft"}
              onChange={() => setAnswer("soft")}
              label="Soft delete"
              description="Keep the row and set cancelled_at. Recommended."
            />
            <Checkbox
              radio
              name="q"
              checked={answer === "hard"}
              onChange={() => setAnswer("hard")}
              label="Hard delete"
              description="Remove the row."
            />
          </div>
        </div>
        <SectionLabel>Controlled browsing (the design card)</SectionLabel>
        <FolderPicker
          value={v}
          onChange={setV}
          browsePath={browse}
          parent={parentOf(browse)}
          home="/home/me"
          onBrowse={setBrowse}
          height={150}
          entries={FS[browse] ?? []}
          missing={!FS[browse]}
        />
        <SectionLabel>With a list function (type ~/co, Tab, sh, Tab)</SectionLabel>
        <FolderPicker value={live} onChange={setLive} list={fakeList} height={150} />
      </div>
    </Card>
  );
}

function FeedbackCard() {
  const [open, setOpen] = useState(false);
  return (
    <Card name="Feedback" subtitle="Chip, StatusChip, StatusDot, Spinner, Banner, Dialog">
      <div className="gx-c">
        <div className="gx-r">
          <span className="gx-l">Status</span>
          <StatusChip status="running" />
          <StatusChip status="waiting" />
          <StatusChip status="completed" />
          <StatusChip status="failed" />
          <StatusChip kind="execution" status="stuck" />
          <StatusChip kind="execution" status="handoff" />
          <StatusChip kind="phase" status="queued" />
          <StatusChip kind="init" status="not_initialized" />
        </div>
        <div className="gx-r">
          <span className="gx-l">Meta</span>
          <Chip tone="info">Init</Chip>
          <Chip>Implement</Chip>
          <Chip tone="warn">YOLO</Chip>
          <Chip mono>harness:codex</Chip>
          <Chip mono outline>
            claude-sonnet-5
          </Chip>
          <Chip tone="bad" icon="shield-alert">
            security block
          </Chip>
        </div>
        <div className="gx-r">
          <span className="gx-l">Dots</span>
          <StatusDot tone="accent" pulse />
          <StatusDot tone="warn" />
          <StatusDot tone="ok" />
          <StatusDot tone="bad" />
          <StatusDot hollow />
          <Spinner />
          <span style={{ color: "var(--text-muted)" }}>Live</span>
        </div>
        <Banner tone="warn" title="YOLO is on">
          Ostra answers gates and permission asks itself and records each decision. Guards and deny rules still apply.
        </Banner>
        <Banner tone="bad">Settings have 2 problems. Open Settings to fix them before starting work.</Banner>
        <Banner tone="info" actions={<Button size="sm">Retry</Button>}>
          The server closed the connection.
        </Banner>
        <Banner tone="neutral">Nothing is running in this workspace.</Banner>
        <Dialog
          inline
          width={520}
          title="Remove admin from shop?"
          subtitle="Nothing on disk is deleted."
          onClose={() => {}}
          footer={
            <>
              <span style={{ flex: 1 }} />
              <Button>Cancel</Button>
              <Button variant="danger">Remove from workspace</Button>
            </>
          }
        >
          Sessions that targeted admin keep their history. You can import the folder again later.
        </Dialog>
        <div className="gx-r">
          <Button onClick={() => setOpen(true)}>Open a modal dialog</Button>
        </div>
        <Dialog
          open={open}
          title="Add a project"
          subtitle="to shop"
          onClose={() => setOpen(false)}
          footer={
            <>
              <span style={{ flex: 1 }} />
              <Button onClick={() => setOpen(false)}>Cancel</Button>
              <Button variant="primary" onClick={() => setOpen(false)}>
                Import project
              </Button>
            </>
          }
        >
          <Input
            label="Project key"
            mono
            defaultValue="shop-admin"
            hint="Names the project in every stage and session folder."
          />
        </Dialog>
      </div>
    </Card>
  );
}

const PALETTE: PaletteItem[] = [
  { id: "s1", group: "Sessions", icon: "git-pull-request", label: "Add order cancellation", hint: "waiting" },
  { id: "s2", group: "Sessions", icon: "git-pull-request", label: "Refund webhook retries", hint: "completed" },
  {
    id: "x1",
    group: "Executions",
    icon: "square-terminal",
    label: "Implementer · Phase 2 in backend",
    hint: "harness:codex",
  },
  { id: "x2", group: "Executions", icon: "activity", label: "Explore · Research in backend", hint: "native" },
  { id: "a1", group: "Artifacts", icon: "file-text", label: "Order cancellation spec", hint: "spec.md" },
  { id: "w1", group: "Workspace", icon: "coins", label: "Cost" },
  { id: "w2", group: "Workspace", icon: "settings", label: "Settings" },
  { id: "t1", group: "Actions", icon: "sun-moon", label: "Toggle light and dark theme" },
];

function NavigationCard() {
  const [t, setT] = useState("s1");
  const [u, setU] = useState("activity");
  const [seg, setSeg] = useState("all");
  const [bar, setBar] = useState([
    { id: "s1", icon: "git-pull-request" as IconName, label: "Order cancellation" },
    { id: "x1", icon: "square-terminal" as IconName, label: "Implementer · 2", italic: true },
    { id: "a1", icon: "file-text" as IconName, label: "spec.md" },
  ]);
  const [expanded, setExpanded] = useState({ s1: true, s2: false });
  const [sel, setSel] = useState("s1");
  const [menu, setMenu] = useState(false);
  const [step, setStep] = useState(2);
  const [palette, setPalette] = useState(false);
  const [picked, setPicked] = useState<string | null>(null);
  return (
    <Card name="Navigation" subtitle="Tabs, TreeItem, Breadcrumbs, Menu, Stepper, CommandPalette">
      <div style={{ display: "grid", gridTemplateColumns: "230px 1fr", gap: 16 }}>
        <div
          role="tree"
          aria-label="Sessions"
          style={{
            background: "var(--surface-panel)",
            border: "1px solid var(--border-default)",
            borderRadius: 6,
            padding: 4,
          }}
        >
          <TreeSection label="Sessions" actions={<IconButton size="sm" icon="plus" label="New task" />}>
            <TreeItem
              label="Add order cancellation"
              icon={<StatusDot tone="warn" />}
              expanded={expanded.s1}
              selected={sel === "s1"}
              meta="$1.84"
              onClick={() => setSel("s1")}
              onToggle={() => setExpanded((x) => ({ ...x, s1: !x.s1 }))}
            />
            {expanded.s1 && (
              <>
                <TreeItem
                  depth={1}
                  icon="file-text"
                  label="spec.md"
                  selected={sel === "spec"}
                  onClick={() => setSel("spec")}
                />
                <TreeItem
                  depth={1}
                  icon="square-terminal"
                  label="Implementer · phase 2"
                  meta="run"
                  selected={sel === "impl"}
                  onClick={() => setSel("impl")}
                />
              </>
            )}
            <TreeItem
              label="Refund webhook retries"
              icon={<StatusDot tone="ok" />}
              expanded={expanded.s2}
              meta="$0.62"
              onToggle={() => setExpanded((x) => ({ ...x, s2: !x.s2 }))}
            />
            {expanded.s2 && <TreeItem depth={1} icon="file-text" label="plan.md" />}
          </TreeSection>
        </div>
        <div className="gx-c">
          <Tabs
            variant="bar"
            value={t}
            onChange={setT}
            onClose={(id) => setBar((b) => b.filter((x) => x.id !== id))}
            tabs={bar}
          />
          <Breadcrumbs
            items={[
              { label: "shop", icon: "box" },
              { label: "Order cancellation" },
              { label: "Implementer · phase 2" },
            ]}
            onNavigate={() => {}}
          />
          <Tabs
            value={u}
            onChange={setU}
            tabs={[
              { id: "activity", label: "Activity" },
              { id: "terminal", label: "Terminal" },
              { id: "spawn", label: "Spawn parameters" },
            ]}
          />
          <Tabs
            variant="segmented"
            value={seg}
            onChange={setSeg}
            tabs={[
              { id: "all", label: "All" },
              { id: "waiting", label: "Waiting", count: 2 },
              { id: "done", label: "Done" },
            ]}
          />
        </div>
        <div style={{ position: "relative", height: 210 }}>
          <Button size="sm" variant="ghost" iconRight="chevrons-up-down" onClick={() => setMenu(true)}>
            shop
          </Button>
          <Menu
            open={menu}
            onClose={() => setMenu(false)}
            width={280}
            items={[
              { type: "heading", label: "Workspaces" },
              { id: "a", icon: "box", label: "shop", sub: "3 projects · 2 active", checked: true },
              { id: "ov", icon: "layout-dashboard", label: "Overview", indent: 1, active: true },
              { id: "co", icon: "coins", label: "Cost", indent: 1, hint: "$4.18" },
              { id: "b", icon: "box", label: "internal-tools", sub: "2 projects", checked: false },
              { type: "divider" },
              { id: "c", icon: "plus", label: "New workspace…" },
              { id: "d", icon: "folder-plus", label: "Add project to shop…" },
            ]}
          />
          {!menu && (
            <div style={{ marginTop: 8, fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>
              Click the trigger to open the menu.
            </div>
          )}
        </div>
        <div className="gx-c" style={{ paddingTop: 4 }}>
          <Stepper
            current={step}
            onSelect={setStep}
            steps={[
              { label: "Check this machine", hint: "Keys and harnesses" },
              { label: "Name and folder", hint: "Where .ostra/ lives" },
              { label: "Add projects", hint: "Folders to work on" },
              { label: "Defaults" },
            ]}
          />
          <Stepper
            orientation="horizontal"
            current={1}
            steps={[
              { label: "Detect" },
              { label: "Scout" },
              { label: "Propose" },
              { label: "Approve" },
              { label: "Generate" },
            ]}
          />
          <div className="gx-r">
            <Button size="sm" icon="search" kbd="⌘K" onClick={() => setPalette(true)}>
              Go to anything
            </Button>
            {picked && <span style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>Picked: {picked}</span>}
          </div>
        </div>
      </div>
      <CommandPalette
        open={palette}
        items={PALETTE}
        onClose={() => setPalette(false)}
        onSelect={(it) => setPicked(it.label)}
      />
    </Card>
  );
}

const SRC = `    pub async fn cancel(&self, id: OrderId, by: Actor) -> Result<Order> {
        let order = self.get(id).await?;
        // Rule R3: shipped orders cannot be cancelled.
        match order.status {
            OrderStatus::Shipped | OrderStatus::Delivered => Err(Error::CannotCancel),
            _ => self.repo.mark_cancelled(id, by, Utc::now()).await,
        }
    }`;

interface ExecRow {
  id: number;
  agent: string;
  project: string;
  executor: string;
  status: string;
  cost: string;
}

const ROWS: ExecRow[] = [
  { id: 1, agent: "Explore", project: "backend", executor: "native", status: "ok", cost: "$0.41" },
  { id: 2, agent: "Fact check", project: "backend", executor: "native", status: "ok", cost: "$0.22" },
  { id: 3, agent: "Implementer", project: "backend", executor: "harness:codex", status: "running", cost: "$0.87" },
];

function DataCard() {
  const [s, setS] = useState(3);
  return (
    <Card name="Data" subtitle="Panel, SectionLabel, Table, CodeView">
      <div className="gx-c">
        <SectionLabel>Executions</SectionLabel>
        <Panel
          title="Executions"
          subtitle="3 runs"
          icon="list"
          bodyFlush
          actions={
            <Button size="sm" variant="ghost">
              Cost
            </Button>
          }
        >
          <Table
            dense
            selectedKey={s}
            onRowClick={(r) => setS(r.id)}
            columns={[
              { key: "agent", label: "Agent" },
              { key: "project", label: "Project" },
              {
                key: "executor",
                label: "Executor",
                render: (r) => (
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, color: "var(--text-secondary)" }}>
                    {r.executor}
                  </span>
                ),
              },
              { key: "status", label: "Status", render: (r) => <StatusChip kind="execution" status={r.status} /> },
              { key: "cost", label: "Cost", num: true },
            ]}
            rows={ROWS}
          />
        </Panel>
        <div className="gx-r">
          <Panel tone="highlight" title="Highlight" style={{ flex: 1 }}>
            The current item.
          </Panel>
          <Panel tone="warn" title="Warn" style={{ flex: 1 }}>
            Needs attention.
          </Panel>
          <Panel tone="bad" flush title="Bad, flush" style={{ flex: 1 }}>
            Failed.
          </Panel>
        </div>
        <Table columns={[{ key: "a", label: "Lesson" }]} rows={[]} empty="No lessons yet." />
        <CodeView language="rs" startLine={23} highlight={[25]} added={[26, 27, 28, 29]} code={SRC} />
        <CodeView
          language="toml"
          code={'[project]\nkey = "shop-backend" # the project key\nstack = "rust-axum"\nmax = 3'}
        />
      </div>
    </Card>
  );
}

function PipelineCard() {
  const [l, setL] = useState<LaneId>("build");
  const [phase, setPhase] = useState<string | number>(2);
  const [stage, setStage] = useState("fc");
  const [run, setRun] = useState("c");
  const [lines, setLines] = useState<TerminalLine[]>([
    ["fn", "› Implement phase 2: POST /orders/:id/cancel"],
    ["ok", "  ✓ cargo check -p orders  (8.4s)"],
    ["bad", "  ✗ Write tests/orders/cancel_test.rs  denied by guard"],
    ["warn", "  ⏸ Waiting for approval in Ostra"],
  ]);
  return (
    <Card
      name="Pipeline"
      subtitle="LaneStepper, StageRow, PhaseDag, ToolCall, GateCard, Decision, ExecutionGroup, Terminal"
      width={900}
    >
      <div className="gx-c">
        <LaneStepper
          selected={l}
          onSelect={setL}
          lanes={{
            research: {
              status: "done",
              detail: "2 docs",
              why: "Research gathers context before anything is specified.",
            },
            requirements: { status: "done", detail: "6 reqs" },
            verification: { status: "done", detail: "PASS" },
            design: { status: "done", detail: "4 phases" },
            build: { status: "current", detail: "Phase 2 of 4" },
            review: { status: "waiting", detail: "Waiting for you" },
            test: { status: "pending" },
            docs: { status: "skipped", detail: "Not asked" },
            done: { status: "pending" },
          }}
        />
        <div style={{ display: "grid", gridTemplateColumns: "minmax(0, 1fr) minmax(0, 1fr)", gap: 14 }}>
          <div className="gx-c">
            <PhaseDag
              selected={phase}
              onOpen={(p) => setPhase(p.id)}
              layers={[
                [
                  {
                    id: 1,
                    title: "Cancellation model",
                    project: "backend",
                    complexity: "low",
                    testPolicy: "Required",
                    status: "passed",
                  },
                ],
                [
                  {
                    id: 2,
                    title: "Cancel endpoint",
                    project: "backend",
                    complexity: "medium",
                    testPolicy: "Required",
                    status: "reviewing",
                    reviewPass: 2,
                    dependsOn: [1],
                  },
                ],
              ]}
            />
            <ToolCall tool="Edit" summary="src/orders/service.rs" duration="0.2s" defaultOpen>
              <DiffView
                lines={[
                  { type: "ctx", text: "match order.status {" },
                  { type: "del", text: "    Shipped => Err(CannotCancel)," },
                  { type: "add", text: "    Shipped | Delivered => Err(CannotCancel)," },
                ]}
              />
            </ToolCall>
            <ToolCall
              tool="Write"
              summary="tests/cancel_test.rs"
              policy={{
                decision: "deny",
                layer: "guard",
                rule: "no-tests-from-implementer",
                reason: "The implementer may not write test paths.",
                advice: "Leave tests to the write-test stage.",
              }}
            />
            <ToolCall
              tool="Bash"
              summary="sqlx migrate run"
              state="running"
              policy={{ decision: "ask", layer: "permission", rule: "Bash(*)" }}
            />
            <ToolCall
              tool="Read"
              summary="src/orders/model.rs"
              duration="0.1s"
              policy={{ decision: "allow", layer: "permission", rule: "Read(**)" }}
            >
              <pre>pub struct Order {"{ … }"}</pre>
            </ToolCall>
            <ToolCall tool="Grep" summary="cancelled_at" state="error" duration="0.3s" />
            <Panel bodyFlush>
              <StageRow
                label="Fact-check the spec"
                status="done"
                meta="PASS · 2 passes"
                selected={stage === "fc"}
                onClick={() => setStage("fc")}
              />
              <StageRow
                label="Implement phase 2"
                status="running"
                selected={stage === "im"}
                onClick={() => setStage("im")}
              />
              <StageRow
                label="Review phase 2"
                status="waiting"
                meta="Waiting for you"
                selected={stage === "rv"}
                onClick={() => setStage("rv")}
              />
              <StageRow label="Write docs" status="skipped" meta="Not asked" />
              <StageRow label="Run tests" status="pending" />
            </Panel>
          </div>
          <div className="gx-c">
            <GateCard
              kind="permission"
              title="Allow a command in backend"
              explanation="The implementer wants to run a command that no rule allows."
              actions={
                <>
                  <Button variant="primary" size="sm">
                    Allow once
                  </Button>
                  <Button size="sm">Always in this workspace</Button>
                  <Button size="sm" variant="danger">
                    Deny
                  </Button>
                </>
              }
            >
              <pre>$ sqlx migrate run</pre>
            </GateCard>
            <GateCard
              kind="spec_approval"
              title="Approve the spec"
              answered
              answeredBy="yolo"
              answer="Approved"
              reason="Fact-check PASS, no open questions."
            />
            <Panel bodyFlush>
              <ExecutionGroup
                agent="Implementer"
                project="backend"
                selected={run}
                onOpen={(r) => setRun(r.id)}
                runs={[
                  { id: "a", label: "Phase 1", status: "ok", executor: "harness:codex", cost: "$0.18" },
                  { id: "b", label: "Phase 1 · fix pass", status: "ok", executor: "harness:codex", cost: "$0.06" },
                  { id: "c", label: "Phase 2", status: "running", executor: "harness:codex", cost: "$0.41" },
                ]}
              />
              <ExecutionGroup
                agent="Implementer"
                project="web"
                selected={run}
                onOpen={(r) => setRun(r.id)}
                runs={[{ id: "d", label: "Phase 3", status: "running", executor: "native", cost: "$0.07" }]}
              />
            </Panel>
            <Terminal
              height={180}
              title="codex · gpt-5.6-terra"
              meta="~/code/shop-backend"
              live
              interactive
              onInput={(text) => setLines((ls) => [...ls, ["", "› " + text]])}
              lines={lines}
            />
            <Terminal height={120} title="claude · claude-sonnet-5" meta="xterm.js slot">
              <div
                style={{
                  flex: 1,
                  minHeight: 0,
                  display: "grid",
                  placeItems: "center",
                  color: "var(--term-muted)",
                  border: "1px dashed var(--term-border)",
                }}
              >
                The live xterm.js host renders here.
              </div>
            </Terminal>
            <Panel bodyFlush>
              <Decision
                judge="Stakes"
                choice="medium"
                reason="the change touches the order state machine and adds one migration."
                basis="spec.md, 6 requirements"
                at="14:02"
              />
              <Decision
                judge="Classify"
                choice="feature"
                reason="the request adds a new endpoint."
                overridden
                at="13:58"
              />
              <Decision
                judge="Route answer"
                choice="the spec agent"
                reason="the answer changes a requirement."
                canOverride={false}
              />
            </Panel>
          </div>
        </div>
      </div>
    </Card>
  );
}

/** Dev-only gallery of every design-system component in its variants, translated from the design's cards. */
export default function Gallery() {
  const [theme, setTheme] = useState<"dark" | "light">(() =>
    document.documentElement.dataset.theme === "light" ? "light" : "dark",
  );
  useEffect(() => {
    const root = document.documentElement;
    if (theme === "light") root.dataset.theme = "light";
    else delete root.dataset.theme;
  }, [theme]);
  return (
    <div className="gx-page">
      <style>{GALLERY_CSS}</style>
      <div className="gx-r" style={{ justifyContent: "space-between", maxWidth: 900 }}>
        <h1 style={{ margin: 0, font: "var(--type-title)" }}>Ostra design system</h1>
        <Tabs
          variant="segmented"
          label="Theme"
          value={theme}
          onChange={(id) => setTheme(id as "dark" | "light")}
          tabs={[
            { id: "dark", label: "Dark", icon: "moon" },
            { id: "light", label: "Light", icon: "sun" },
          ]}
        />
      </div>
      <CoreCard />
      <FormsCard />
      <FeedbackCard />
      <NavigationCard />
      <DataCard />
      <PipelineCard />
      <IconsCard />
    </div>
  );
}
