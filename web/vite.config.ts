import { readFileSync } from "node:fs";
import { defineConfig } from "vitest/config";
import type { Plugin } from "vite";
import react from "@vitejs/plugin-react";

// `pnpm dev` proxies the API to a locally running `gantry` (cargo run).
const backend = process.env.GANTRY_URL ?? "http://127.0.0.1:8080";

/**
 * Gantry's version, so the footer and the binary agree: a release's
 * GANTRY_VERSION (as crates/server/build.rs takes it), or else the
 * workspace's Cargo.toml. The Dockerfile's web stage copies Cargo.toml in for
 * this.
 */
function workspaceVersion(): string {
  const release = process.env.GANTRY_VERSION?.trim();
  if (release) return release;
  const cargo = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
  const section = cargo.slice(cargo.indexOf("[workspace.package]"));
  return /^version\s*=\s*"([^"]+)"/m.exec(section)?.[1] ?? "0.0.0";
}

/**
 * In `pnpm dev` there is no Gantry server to fill in `index.html`'s absolute
 * URLs (crates/server/src/assets.rs does that in production), so the dev
 * server's own address stands in for them.
 */
function devOrigin(): Plugin {
  return {
    name: "gantry-dev-origin",
    apply: "serve",
    transformIndexHtml: {
      order: "pre",
      handler(html, ctx) {
        const host = ctx.server?.resolvedUrls?.local[0]?.replace(/\/$/, "") ?? "";
        return html.replaceAll("__GANTRY_ORIGIN__", host);
      },
    },
  };
}

export default defineConfig({
  plugins: [react(), devOrigin()],
  define: {
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(workspaceVersion()),
    "import.meta.env.VITE_APP_AUTHOR": JSON.stringify("Adrian Eyre"),
  },
  // Three.js (the 3D viewer) is its own chunk, loaded only on a model page,
  // and is a little over Vite's default warning size.
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 600 },
  server: {
    proxy: {
      "/api": { target: backend, ws: true },
      "/healthz": backend,
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
