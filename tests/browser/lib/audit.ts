import type { Page } from "@playwright/test";
import { expect, type Guard } from "./guard";

/**
 * Static checks on what a render path left in the DOM: no event-handler attributes, no script
 * URLs, no frames or plugins, no inline or foreign scripts, no images from another origin.
 */
export async function domProblems(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const out: string[] = [];
    const urlAttrs = ["href", "src", "action", "formaction", "xlink:href", "data", "poster", "srcset", "background"];
    for (const el of Array.from(document.querySelectorAll("*"))) {
      for (const a of Array.from(el.attributes)) {
        const n = a.name.toLowerCase();
        if (n.startsWith("on")) out.push(`<${el.tagName.toLowerCase()} ${n}="${a.value.slice(0, 80)}">`);
        if (urlAttrs.includes(n) && /^\s*(javascript|vbscript|data:(?!image\/))/i.test(a.value))
          out.push(`<${el.tagName.toLowerCase()} ${n}="${a.value.slice(0, 80)}">`);
      }
      const tag = el.tagName;
      if (["IFRAME", "FRAME", "OBJECT", "EMBED", "BASE"].includes(tag)) out.push(`<${tag.toLowerCase()}> element`);
      if (tag === "FORM" && el.getAttribute("action")) out.push(`<form action="${el.getAttribute("action")}">`);
      if (tag === "SCRIPT") {
        const s = el as HTMLScriptElement;
        if (!s.src) out.push(`inline <script>: ${s.text.slice(0, 80)}`);
        else if (new URL(s.src).origin !== location.origin) out.push(`foreign script ${s.src}`);
      }
      if (tag === "IMG" || tag === "image") {
        const src = (el as HTMLImageElement).currentSrc || el.getAttribute("src") || "";
        if (src && !src.startsWith("data:") && new URL(src, location.href).origin !== location.origin)
          out.push(`image from another origin: ${src}`);
      }
    }
    return out;
  });
}

/** Link texts the payloads use; clicking each must neither run script nor leave the app. */
const PAYLOAD_LINKS = /^(js link|data link|vbscript link|ref link|entity link|raw anchor|cell)\b/;

/**
 * Wait for `marker`, assert the DOM is clean, then click every payload link in view. A click on a
 * sanitized link (an empty href) may open the app itself in a new tab, which is closed again.
 */
export async function audit(page: Page, guard: Guard, marker: string, where: string) {
  await expect(page.getByText(marker, { exact: false }).first(), `${where}: ${marker} rendered`).toBeVisible();
  expect(await domProblems(page), `${where}: DOM`).toEqual([]);
  const links = page.locator("a").filter({ hasText: PAYLOAD_LINKS });
  const n = await links.count();
  guard.allowPopups = true;
  for (let i = 0; i < n; i++) {
    const a = links.nth(i);
    if (!(await a.isVisible())) continue;
    const href = await a.getAttribute("href");
    expect(href ?? "", `${where}: payload link href`).not.toMatch(/^\s*(javascript|vbscript|data):/i);
    await a.click({ timeout: 5000 }).catch(() => {});
  }
  await page.waitForTimeout(300);
  for (const p of guard.popups.splice(0)) {
    const url = p.url();
    expect(
      url === "about:blank" || url.startsWith(`${guard.state.app}/`),
      `${where}: a link opened ${url}`,
    ).toBe(true);
    await p.close();
  }
  guard.allowPopups = false;
  expect(new URL(page.url()).origin, `${where}: still on the app`).toBe(guard.state.app);
}
