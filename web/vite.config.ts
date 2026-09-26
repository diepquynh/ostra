/// <reference types="vitest/config" />

import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const target = process.env.OSTRA_DEV_SERVER ?? "http://127.0.0.1:7878";

// The homepage's console shot (VITE_SHOT=1) is served by a static host, so the policy the server sends as a header
// (crates/ostra-server/src/api.rs) travels in the page instead. The mock socket opens no connection.
const shotCsp: Plugin = {
  name: "ostra-shot-csp",
  apply: "build",
  transformIndexHtml: {
    order: "pre",
    handler: (html) =>
      process.env.VITE_SHOT === "1"
        ? html.replace(
            "<head>",
            `<head>\n    <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; manifest-src 'self'" />`,
          )
        : html,
  },
};

export default defineConfig({
  plugins: [react(), shotCsp],
  server: {
    port: 5173,
    proxy: {
      "/api": { target, changeOrigin: false },
      "/ws": { target: target.replace(/^http/, "ws"), ws: true },
      // The OAuth redirect for MCP servers returns to the page's own origin.
      "/mcp/oauth": { target, changeOrigin: false },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    chunkSizeWarningLimit: 4000,
    rolldownOptions: {
      output: {
        // Libraries that change less often than the app get their own chunks, so a UI change keeps them cached.
        // Monaco and xterm are not listed: they load with lazy() and stay in their own chunks.
        codeSplitting: {
          groups: [
            { name: "react", test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/, priority: 40 },
            { name: "router", test: /node_modules[\\/]react-router[\\/]/, priority: 30 },
            { name: "icons", test: /node_modules[\\/]lucide-react[\\/]/, priority: 20 },
            { name: "markdown-lib", test: /node_modules[\\/](react-markdown|remark-gfm)[\\/]/, priority: 10 },
          ],
        },
      },
    },
  },
  test: {
    environment: "jsdom",
    env: { VITE_MOCK: "1" },
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
