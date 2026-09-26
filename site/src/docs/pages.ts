export type PageDef = {
  /** The page's address: `docs/#<id>`. */
  id: string;
  title: string;
  /** The Markdown file, as a path from the repository root. Files in docs/ need no other registration. */
  file: string;
  /**
   * One `##` section of the file: its number ("8" matches "## 8. The engine") or its exact heading text. "" is the
   * text before the first `##`. Omit it to show the whole file.
   */
  section?: string;
};

export type NavGroup = { label: string; pages: PageDef[] };

const R = "README.md";

export const TEST_DOC_FILE = "test-doc.md";
// The browser security suite builds the site with a page of attack strings in VITE_TEST_DOC (tests/browser).
const TEST_PAGES: NavGroup[] = import.meta.env.VITE_TEST_DOC
  ? [{ label: "Test", pages: [{ id: "test-doc", title: "Test document", file: TEST_DOC_FILE }] }]
  : [];
const H = "HANDOVER.md";

/** The docs sidebar, in reading order. Previous and Next follow this order. */
export const NAV: NavGroup[] = [
  {
    label: "Get started",
    pages: [
      { id: "overview", title: "Overview", file: R, section: "" },
      { id: "quick-start", title: "Quick start", file: "docs/start/quick-start.md" },
      { id: "install", title: "Install and run", file: "docs/start/install.md" },
      { id: "first-session", title: "Your first session", file: "docs/start/first-session.md" },
      { id: "model-access", title: "Model access", file: R, section: "Model access" },
      { id: "configuration", title: "Configuration", file: R, section: "Configuration" },
      { id: "os-compatibility", title: "OS compatibility", file: "docs/platforms/os-compatibility.md" },
      { id: "troubleshooting", title: "Troubleshooting", file: "docs/start/troubleshooting.md" },
    ],
  },
  {
    label: "How Ostra works",
    pages: [
      { id: "pipeline", title: "The pipeline", file: "docs/internals/pipeline.md" },
      { id: "gates-and-judges", title: "Gates, YOLO, and judges", file: "docs/internals/gates-and-judges.md" },
      { id: "agents", title: "Agents", file: "docs/internals/agents.md" },
      { id: "executors", title: "Executors", file: "docs/internals/executors.md" },
      { id: "tools", title: "Tools", file: "docs/internals/tools.md" },
      { id: "code-index", title: "Code index", file: "docs/internals/code-index.md" },
      { id: "mcp", title: "MCP servers", file: "docs/internals/mcp.md" },
      { id: "project-memory", title: "Project memory", file: "docs/internals/project-memory.md" },
      { id: "settings-and-routing", title: "Settings and routing", file: "docs/internals/settings-and-routing.md" },
      { id: "spend-and-limits", title: "Spend and limits", file: "docs/internals/spend-and-limits.md" },
    ],
  },
  {
    label: "Architecture",
    pages: [
      { id: "architecture-overview", title: "Overview", file: "docs/architecture/overview.md" },
      { id: "event-log", title: "The event log", file: "docs/internals/event-log.md" },
      { id: "planner", title: "The planner", file: "docs/internals/planner.md" },
      { id: "storage", title: "Storage", file: "docs/architecture/storage.md" },
      { id: "server", title: "The server", file: "docs/architecture/server.md" },
    ],
  },
  {
    label: "Security",
    pages: [
      { id: "threat-model", title: "Threat model", file: "docs/security/threat-model.md" },
      { id: "agent-containment", title: "Agent containment", file: "docs/security/agent-containment.md" },
      { id: "server-and-browser", title: "Server and browser", file: "docs/security/server-and-browser.md" },
      { id: "secrets-and-data", title: "Secrets and data", file: "docs/security/secrets-and-data.md" },
      { id: "browser-security-suite", title: "Browser security suite", file: "tests/browser/README.md" },
    ],
  },
  {
    label: "Providers",
    pages: [
      { id: "providers", title: "Provider usage", file: "docs/providers/README.md" },
      { id: "anthropic", title: "Anthropic", file: "docs/providers/anthropic.md" },
      { id: "openai", title: "OpenAI", file: "docs/providers/openai.md" },
      { id: "google", title: "Google", file: "docs/providers/google.md" },
      { id: "xai", title: "xAI", file: "docs/providers/xai.md" },
      { id: "gateways", title: "Gateways and base URLs", file: "docs/providers/gateways.md" },
      { id: "provider-code-rules", title: "Rules for provider code", file: "docs/providers/contributing.md" },
    ],
  },
  {
    label: "Design brief",
    pages: [
      { id: "what-ostra-is", title: "What Ostra is", file: H, section: "1" },
      { id: "decisions", title: "Decisions", file: H, section: "2" },
      { id: "glossary", title: "Glossary", file: H, section: "3" },
      { id: "architecture", title: "Architecture", file: H, section: "4" },
      { id: "workspaces-and-projects", title: "Workspaces and projects", file: H, section: "6" },
      { id: "settings", title: "Settings", file: H, section: "7" },
      { id: "engine", title: "The engine", file: H, section: "8" },
      { id: "agents-and-prompts", title: "Agents and prompts", file: H, section: "9" },
      { id: "executors-tools-policy", title: "Executors, tools, and policy", file: H, section: "10" },
      { id: "sessions-and-resume", title: "Sessions and resume", file: H, section: "11" },
      { id: "user-interface", title: "User interface", file: H, section: "12" },
      { id: "security", title: "Security", file: H, section: "15" },
    ],
  },
  {
    label: "Reference",
    pages: [
      { id: "api", title: "API", file: H, section: "13" },
      { id: "build-and-run", title: "Build and run", file: R, section: "Build and run" },
      { id: "tests", title: "Tests", file: R, section: "Tests" },
      { id: "repository-layout", title: "Repository layout", file: R, section: "Repository layout" },
      { id: "source-tree", title: "Source tree", file: H, section: "14" },
    ],
  },
  {
    label: "Project",
    pages: [
      { id: "milestones", title: "Milestones", file: H, section: "16" },
      { id: "testing-strategy", title: "Testing strategy", file: H, section: "17" },
      { id: "open-items", title: "Open items", file: H, section: "18" },
      { id: "lessons", title: "Lessons from Ultracode", file: H, section: "19" },
      { id: "writing-rules", title: "Writing rules", file: H, section: "20" },
      { id: "ultracode-sources", title: "Ultracode sources", file: H, section: "5" },
      { id: "contributor-notes", title: "Contributor notes", file: "CLAUDE.md" },
    ],
  },
  ...TEST_PAGES,
];
