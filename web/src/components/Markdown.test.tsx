import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown, markdownUrl } from "./Markdown";

describe("agent-written Markdown", () => {
  it("loads images only from data: URLs", () => {
    const node = { type: "element", tagName: "img", properties: {}, children: [] } as never;
    expect(markdownUrl("http://127.0.0.1:9/x.png", "src", node)).toBe("");
    expect(markdownUrl("//evil.example/x.png", "src", node)).toBe("");
    expect(markdownUrl("/api/auth/sessions", "src", node)).toBe("");
    expect(markdownUrl("data:image/png;base64,AAAA", "src", node)).toBe("data:image/png;base64,AAAA");
    expect(markdownUrl("javascript:alert(1)", "href", node)).toBe("");
    expect(markdownUrl("https://example.com/", "href", node)).toBe("https://example.com/");
  });

  it("shows a dropped image's alt text", () => {
    const { container } = render(<Markdown text="![a chart](http://127.0.0.1:9/c.png)" />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("a chart");
  });
});
