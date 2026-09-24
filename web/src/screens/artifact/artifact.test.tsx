import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as f from "../../api/mock/fixtures";
import { artifactFor, mockHeadings } from "../../api/mock/fixtures.execution";
import type { Heading } from "../../api/gen/Heading";
import { ConsoleContext, type ConsoleContextValue, type Nav } from "../../lib/nav";
import { ArtifactScreen } from "../ArtifactScreen";
import { ArtifactMarkdown } from "./ArtifactMarkdown";
import { approvalBadges, artifactKind } from "./kind";
import { slugify, Slugger } from "./outline";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;

async function render(node: ReactNode, rounds = 4) {
  host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    root = createRoot(host!);
    root.render(node);
  });
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  localStorage.clear();
});

const ids = () => Array.from(host!.querySelectorAll(".art-md h1, .art-md h2, .art-md h3, .art-md h4")).map((h) => h.id);

describe("heading slugs match the server", () => {
  // The cases of `slugs_follow_github` in crates/ostra-core/src/outline.rs.
  it("slugifies like github-slugger", () => {
    expect(slugify("Hello, World!")).toBe("hello-world");
    expect(slugify("Phase 1: greeting")).toBe("phase-1-greeting");
    expect(slugify("snake_case and kebab-case")).toBe("snake_case-and-kebab-case");
    expect(slugify("Two  spaces")).toBe("two--spaces");
    expect(slugify("Ünïcode Straße")).toBe("ünïcode-straße");
    expect(slugify("C++ & Rust (2024)")).toBe("c--rust-2024");
  });

  it("numbers repeats", () => {
    const s = new Slugger();
    expect(["Reqs", "Reqs", "Reqs-1", "Reqs"].map((t) => s.slug(t))).toEqual(["reqs", "reqs-1", "reqs-1-1", "reqs-2"]);
  });

  it("gives rendered headings the server's ids, and unlisted ones a fresh slug", async () => {
    // The server outline of this document, as `atx_and_setext_headings_outside_code` produces it.
    const md = "# Spec: `cancel` orders #\n\n## Requirements\n### Requirements\n\n> ## Quoted\n\nSetext title\n============\n\nSecond *level*\n---\n\n## [Link](http://x) ##\n";
    const server: Heading[] = [
      { level: 1, id: "spec-cancel-orders", title: "Spec: cancel orders" },
      { level: 2, id: "requirements", title: "Requirements" },
      { level: 3, id: "requirements-1", title: "Requirements" },
      { level: 1, id: "setext-title", title: "Setext title" },
      { level: 2, id: "second-level", title: "Second level" },
      { level: 2, id: "link", title: "Link" },
    ];
    await render(<ArtifactMarkdown text={md} headings={server} onAnchor={() => {}} />, 1);
    expect(ids()).toEqual(["spec-cancel-orders", "requirements", "requirements-1", "quoted", "setext-title", "second-level", "link"]);
  });

  it("slugs on its own when the server sent no outline", async () => {
    await render(<ArtifactMarkdown text={"## Reqs\n## Reqs\n## C++ & Rust (2024)"} headings={[]} onAnchor={() => {}} />, 1);
    expect(ids()).toEqual(["reqs", "reqs-1", "c--rust-2024"]);
  });

  it("matches the mock outline for every mock artifact", async () => {
    const art = artifactFor(`/x/.ostra/sessions/s/ostra-spec-a.md`);
    await render(<ArtifactMarkdown text={art.content} headings={art.headings} onAnchor={() => {}} />, 1);
    expect(ids()).toEqual(art.headings.map((h) => h.id));
    expect(mockHeadings("# A\n```\n# not\n```\n## A")).toEqual([
      { level: 1, title: "A", id: "a" },
      { level: 2, title: "A", id: "a-1" },
    ]);
  });

  it("follows in-document links through onAnchor", async () => {
    const onAnchor = vi.fn();
    await render(<ArtifactMarkdown text={"## Requirements\n\nSee [the list](#requirements)."} headings={[]} onAnchor={onAnchor} />, 1);
    await act(async () => host!.querySelector<HTMLAnchorElement>('a[href="#requirements"]')!.click());
    expect(onAnchor).toHaveBeenCalledWith("requirements");
  });
});

describe("report tables", () => {
  it("resizes a column from its header and resets on double-click", async () => {
    await render(<ArtifactMarkdown text={"| ID | Statement |\n| --- | --- |\n| R1 | THE SYSTEM SHALL x. |\n"} headings={[]} onAnchor={() => {}} />);
    const handles = host!.querySelectorAll<HTMLElement>('th [role="separator"]');
    expect(handles).toHaveLength(2);
    const table = host!.querySelector("table")!;
    const th = handles[0].closest("th")!;
    await act(async () => {
      handles[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    expect(table.style.tableLayout).toBe("fixed");
    expect(th.style.width).toMatch(/px$/);
    await act(async () => {
      handles[0].dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    });
    expect(table.style.tableLayout).toBe("");
    expect(th.style.width).toBe("");
  });
});

describe("sideways scrolling", () => {
  it("keeps the sticky scrollbar and the table in step", async () => {
    await render(<ArtifactMarkdown text={"| A | B |\n| --- | --- |\n| 1 | 2 |\n"} headings={[]} onAnchor={() => {}} />, 1);
    const box = host!.querySelector<HTMLDivElement>(".art-table")!;
    const bar = host!.querySelector<HTMLDivElement>(".art-hscroll")!;
    await act(async () => {
      box.scrollLeft = 120;
      box.dispatchEvent(new Event("scroll"));
    });
    expect(bar.scrollLeft).toBe(120);
    await act(async () => {
      bar.dispatchEvent(new Event("scroll"));
      bar.scrollLeft = 40;
      bar.dispatchEvent(new Event("scroll"));
    });
    expect(box.scrollLeft).toBe(40);
  });
});

describe("artifact kind and badges", () => {
  it("reads the kind from the ref, else from the file name", () => {
    expect(artifactKind("/s/ostra-spec-x.md")).toBe("spec");
    expect(artifactKind("/s/backend/ostra-review-ledger-phase-1.md")).toBe("ledger");
    expect(artifactKind("/s/ostra-plan-x-phase-2-service.md")).toBe("phase");
    expect(artifactKind("/s/ostra-plan-x.md")).toBe("plan");
    expect(artifactKind("/s/notes.md", "research")).toBe("research");
  });

  it("shows the spec's fact-check and approval", () => {
    expect(approvalBadges(f.sessionDetail, "spec").map((b) => b.label)).toEqual(["Fact-check PASS", "Approved by you"]);
    expect(approvalBadges(f.sessionDetail, "report")).toEqual([]);
  });
});

describe("artifact screen", () => {
  const spec = `/home/me/code/shop/.ostra/sessions/${f.SESSION}/ostra-spec-20260922-100700-order-cancel.md`;

  function withNav(node: ReactNode, open: Nav["open"]) {
    const noop = () => {};
    const ctx: ConsoleContextValue = {
      nav: { ws: f.WS, activeId: null, tabs: [], open, close: noop, pin: noop, href: (id) => id },
      shell: { openDock: noop, closeDock: noop, taskDraft: null, setTaskDraft: noop, newWorkspace: noop, addProject: noop, runSetup: noop, browseFiles: noop, theme: "dark", toggleTheme: noop },
      workspace: { detail: null, reload: noop },
    };
    return (
      <MemoryRouter>
        <ConsoleContext.Provider value={ctx}>{node}</ConsoleContext.Provider>
      </MemoryRouter>
    );
  }

  const plan = `/home/me/code/shop/.ostra/sessions/${f.SESSION}/ostra-plan-20260922-100500-order-cancel.md`;
  const research = `/home/me/code/shop/.ostra/sessions/${f.SESSION}/backend/ostra-research-20260922-100100-order-lifecycle.md`;
  const report = `/home/me/code/shop/.ostra/sessions/${f.SESSION}/backend/ostra-implementer-phase-1.md`;
  const chapters = () =>
    Array.from(host!.querySelectorAll<HTMLElement>('[aria-label="Outlines"] [role="treeitem"]')).map((r) => ({
      el: r,
      label: r.querySelector(".os-tree-item__label")?.textContent ?? r.textContent ?? "",
      meta: r.querySelector(".os-tree-item__meta")?.textContent ?? null,
    }));
  const chapter = (label: string) => chapters().find((c) => c.label.startsWith(label))!.el;
  const button = (text: string) => Array.from(host!.querySelectorAll<HTMLButtonElement>("button")).find((b) => b.textContent === text)!;

  it("lists the outline, shows the EARS note for a spec, and deep-links a heading", async () => {
    const open = vi.fn();
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, open));
    await act(async () => button("Markdown").click());
    const outline = Array.from(host!.querySelectorAll('[aria-label="Outline"] [role="treeitem"]')).map((r) => r.textContent);
    expect(outline).toContain("Requirements");
    expect(outline).toContain("External Evidence");
    expect(host!.textContent).toContain("How to read requirements and acceptance criteria");
    const reqs = Array.from(host!.querySelectorAll<HTMLElement>('[aria-label="Outline"] [role="treeitem"]')).find((r) => r.textContent === "Requirements")!;
    await act(async () => reqs.click());
    expect(open).toHaveBeenCalledWith(`artifact:${spec}`, { anchor: "requirements" });
    expect(host!.querySelector("#requirements")?.tagName).toBe("H2");
  });

  it("hides the EARS note once dismissed", async () => {
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, () => {}));
    await act(async () => host!.querySelector<HTMLButtonElement>('button[aria-label="Hide the note"]')!.click());
    expect(host!.textContent).not.toContain("How to read requirements and acceptance criteria");
    expect(host!.textContent).toContain("How to read requirements");
  });

  it("pages a typed spec by chapter, with counts", async () => {
    const open = vi.fn();
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, open));
    const list = chapters();
    expect(list.map((c) => c.label)).toEqual(expect.arrayContaining(["Overview", "Criteria", "Requirements", "D1: Cancellation endpoint", "External evidence", "Traceability", "Fact-check"]));
    expect(list.find((c) => c.label === "Requirements")!.meta).toBe("4");
    expect(host!.querySelector(".doc-chapter")!.getAttribute("data-chapter")).toBe("overview");
    expect(host!.textContent).toContain("4 of 4");
    await act(async () => chapter("External evidence").click());
    expect(open).toHaveBeenLastCalledWith(`artifact:${spec}`, { anchor: "evidence" });
    expect(host!.querySelector(".doc-chapter")!.getAttribute("data-chapter")).toBe("evidence");
    expect(host!.querySelector("#el-e1")).not.toBeNull();
    expect(host!.querySelector("#el-r1")).toBeNull();
  });

  it("follows a cross-reference chip to its element", async () => {
    const open = vi.fn();
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, open));
    await act(async () => chapter("External evidence").click());
    const r3 = Array.from(host!.querySelectorAll<HTMLButtonElement>("button.doc-ref")).find((b) => b.textContent === "R3")!;
    await act(async () => r3.click());
    expect(open).toHaveBeenLastCalledWith(`artifact:${spec}`, { anchor: "req-d1/r3" });
    expect(host!.querySelector(".doc-chapter")!.getAttribute("data-chapter")).toBe("req-d1");
    expect(host!.querySelector("#el-r3")!.className).toContain("doc-el--focus");
  });

  it("marks the element a fact-check finding names", async () => {
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, () => {}));
    expect(chapter("D1: Cancellation endpoint").querySelector(".doc-dot--info")).not.toBeNull();
    await act(async () => chapter("D1: Cancellation endpoint").click());
    const notes = host!.querySelector('#el-r3 [aria-label="Notes on R3"]');
    expect(notes?.textContent).toContain("Fact-check");
    expect(notes?.textContent).toContain("retry");
    await act(async () => chapter("Fact-check").click());
    expect(host!.textContent).toContain("PASS");
  });

  it("renders a plan's phases and a phase file", async () => {
    await render(withNav(<ArtifactScreen ws={f.WS} path={plan} />, () => {}));
    expect(chapters().map((c) => c.label)).toEqual(expect.arrayContaining(["Phases", "Phase 1: Cancel transition", "Phase 2: Cancel button"]));
    await act(async () => chapter("Phase 1: Cancel transition").click());
    expect(host!.querySelector("#el-step-1\\.1")?.textContent).toContain("src/orders/service.ts");
    expect(host!.querySelector('#el-step-1\\.2 [aria-label="Notes on step 1.2"]')).not.toBeNull();
    act(() => root?.unmount());
    host?.remove();
    await render(withNav(<ArtifactScreen ws={f.WS} path={plan.replace(".md", "-phase-2.md")} />, () => {}));
    expect(chapters().map((c) => c.label)).toEqual(["Overview", "Steps", "Requirements delivered", "External constraints"]);
  });

  it("shows a research document's chapters", async () => {
    await render(withNav(<ArtifactScreen ws={f.WS} path={research} />, () => {}));
    expect(chapters().map((c) => c.label)).toEqual(expect.arrayContaining(["Files", "Patterns", "Data flow", "Approaches", "Open questions", "Sources"]));
    await act(async () => chapter("Open questions").click());
    expect(host!.querySelector("#el-q1")?.textContent).toContain("Recommended");
    expect(host!.textContent).not.toContain("How to read requirements");
  });

  it("switches between the document and its markdown, and leaves reports as markdown", async () => {
    await render(withNav(<ArtifactScreen ws={f.WS} path={spec} />, () => {}));
    await act(async () => button("Markdown").click());
    expect(host!.querySelector('[aria-label="Outline"]')).not.toBeNull();
    await act(async () => button("Document").click());
    expect(host!.querySelector('[aria-label="Outlines"]')).not.toBeNull();
    act(() => root?.unmount());
    host?.remove();
    await render(withNav(<ArtifactScreen ws={f.WS} path={report} />, () => {}));
    expect(host!.querySelector('[aria-label="Outlines"]')).toBeNull();
    expect(button("Markdown")).toBeUndefined();
  });
});
