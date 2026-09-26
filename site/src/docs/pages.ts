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
      { id: "build-and-run", title: "Build and run", file: R, section: "Build and run" },
      { id: "model-access", title: "Model access", file: R, section: "Model access" },
      { id: "configuration", title: "Configuration", file: R, section: "Configuration" },
      { id: "tests", title: "Tests", file: R, section: "Tests" },
    ],
  },
  {
    label: "Concepts",
    pages: [
      { id: "what-ostra-is", title: "What Ostra is", file: H, section: "1" },
      { id: "glossary", title: "Glossary", file: H, section: "3" },
      { id: "architecture", title: "Architecture", file: H, section: "4" },
      { id: "decisions", title: "Decisions", file: H, section: "2" },
    ],
  },
  {
    label: "How Ostra works",
    pages: [
      { id: "workspaces-and-projects", title: "Workspaces and projects", file: H, section: "6" },
      { id: "settings", title: "Settings", file: H, section: "7" },
      { id: "engine", title: "The engine", file: H, section: "8" },
      { id: "agents", title: "Agents and prompts", file: H, section: "9" },
      { id: "executors-tools-policy", title: "Executors, tools, and policy", file: H, section: "10" },
      { id: "sessions-and-resume", title: "Sessions and resume", file: H, section: "11" },
      { id: "user-interface", title: "User interface", file: H, section: "12" },
    ],
  },
  {
    label: "Security",
    pages: [
      { id: "security", title: "Security", file: H, section: "15" },
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
    label: "Reference",
    pages: [
      { id: "api", title: "API", file: H, section: "13" },
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
