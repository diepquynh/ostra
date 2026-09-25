/// <reference types="vitest/config" />

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const target = process.env.OSTRA_DEV_SERVER ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": { target, changeOrigin: false },
      "/ws": { target: target.replace(/^http/, "ws"), ws: true },
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
