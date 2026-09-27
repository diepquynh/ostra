/// <reference types="vitest/config" />

import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

// Vite compares paths as strings. Started from a miscased Windows path (c:\users\...), the workspace symlinks resolve
// to the real casing, the optimizer sees two Reacts, and the page renders blank. The real path gives one casing.
const root = realpathSync.native(fileURLToPath(new URL(".", import.meta.url)));
const repo = realpathSync.native(fileURLToPath(new URL("..", import.meta.url)));

// A static host sends no headers, so the policy the console's server sends (crates/ostra-server/src/api.rs)
// travels as a meta tag: scripts from this origin only, no remote images, and only the homepage frames the console.
// frame-ancestors has no meta form. Build only, because the dev server injects an inline script.
const policy = (frames: boolean) =>
  "default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline'; " +
  "img-src 'self' data:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; " +
  `form-action 'none'; frame-src ${frames ? "'self'" : "'none'"}; manifest-src 'self'`;

const csp: Plugin = {
  name: "ostra-site-csp",
  apply: "build",
  transformIndexHtml: {
    order: "pre",
    handler: (html, ctx) => {
      const tag = `<meta http-equiv="Content-Security-Policy" content="${policy(!ctx.path.includes("docs/"))}" />`;
      return html.replace("<head>", `<head>\n    ${tag}`);
    },
  },
};

export default defineConfig({
  root,
  plugins: [react(), csp],
  // Relative asset paths, so the site works from any folder of a static host.
  base: "./",
  // No SPA fallback: a missing console/ build must 404 in the iframe, not load the homepage inside itself.
  appType: "mpa",
  // Docs pages import the repository's Markdown, which sits above this package.
  server: { port: 5174, fs: { allow: [repo] } },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rolldownOptions: {
      input: {
        home: `${root}/index.html`,
        docs: `${root}/docs/index.html`,
      },
    },
  },
  test: { include: ["src/**/*.test.ts", "src/**/*.test.tsx"] },
});
