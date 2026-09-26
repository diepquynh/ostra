/**
 * Test strings. Each one, if it ever ran or loaded, sets `window.__pwned` or requests
 * `<fake>/canary/<id>`, and the fixture fails the test on either.
 */

/** JS that marks a hit both ways. Written without spaces or slashes, so it fits in file and ref names. */
export const js = (id: string) => `window.__pwned='${id}'`;

/** Markdown and HTML that must render as inert text or safe links. `tag` names the render path. */
export function markdown(tag: string, fake: string): string {
  const c = (id: string) => `${fake}/canary/${tag}-${id}`;
  return [
    `# Heading ${tag} <img src=x onerror="${js(`${tag}-h1`)}">`,
    "",
    `[js link ${tag}](javascript:${js(`${tag}-mdjs`)})`,
    `[data link ${tag}](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)`,
    `[vbscript link ${tag}](vbscript:msgbox)`,
    `[ref link ${tag}][evil-${tag}]`,
    `<javascript:${js(`${tag}-auto`)}>`,
    `[entity link ${tag}](jav&#x61;script:${js(`${tag}-ent`)})`,
    "",
    `[evil-${tag}]: javascript:${js(`${tag}-ref`)}`,
    "",
    `<a href="javascript:${js(`${tag}-rawa`)}">raw anchor ${tag}</a>`,
    `<img src=x onerror="${js(`${tag}-rawimg`)}">`,
    `<script>${js(`${tag}-script`)}</script>`,
    `<iframe src="javascript:parent.${js(`${tag}-iframe`)}"></iframe>`,
    `<svg onload="${js(`${tag}-svg`)}"></svg>`,
    `<style>body{background:url(${c("css")})}</style>`,
    `<form action="${c("form")}"><button>form ${tag}</button></form>`,
    "",
    // Images: a remote canary (CSP must block it; better, the renderer drops it), an API route
    // (the guard must refuse an image request), and a javascript: source.
    `![canary image ${tag}](${c("img")})`,
    `![api image ${tag}](/api/auth/sessions)`,
    `![js image ${tag}](javascript:${js(`${tag}-jsimg`)})`,
    "",
    "```html",
    `<script>${js(`${tag}-fenced`)}</script>`,
    "```",
    "",
    `| col | <img src=x onerror="${js(`${tag}-table`)}"> |`,
    "| --- | --- |",
    `| [cell](javascript:${js(`${tag}-cell`)}) | ok |`,
    "",
    `PW-MARKER-${tag}`,
  ].join("\n");
}

/** A one-line string for titles, names, and labels. */
export const inline = (tag: string) => `PW-${tag} <img src=x onerror="${js(`${tag}-inline`)}"> "'><svg onload=${js(`${tag}-svg`)}>`;

/** File contents that a viewer must show as text. */
export const htmlFile = (tag: string, fake: string) =>
  `<!doctype html><title>x</title><script>${js(`${tag}-html`)};fetch('${fake}/canary/${tag}-html')</script><img src=x onerror="${js(`${tag}-htmlimg`)}">\n`;

export const svgFile = (tag: string, fake: string) =>
  `<svg xmlns="http://www.w3.org/2000/svg" onload="${js(`${tag}-svgload`)}"><script>${js(`${tag}-svg`)};fetch('${fake}/canary/${tag}-svg')</script><image href="${fake}/canary/${tag}-svgimg"/></svg>\n`;
