// Builds the project website in site/dist/, which pages.yml publishes to
// GitHub Pages:
//
//   pnpm --filter @gantry/site dev       # http://localhost:5174
//   pnpm --filter @gantry/site build     # then `preview`, or open dist/
//   GANTRY_RELEASE=release.json pnpm --filter @gantry/site build
//
// index.html is the page, written as static HTML so it reads without
// JavaScript; src/main.tsx mounts the Stageshow demos into it. Its
// `{{NAME}}` placeholders come from release.mjs. The screenshots (docs/images/)
// and the icons (web/public/) are served from where they already live, so
// neither is kept twice in Git. sitemap.xml is written from release.mjs. The
// build fails on an unknown placeholder or on a link to a file that wasn't built.
import { cpSync, createReadStream, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { extname, join } from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { fill, placeholders, readRelease, sitemap, sitemapImages } from "./release.mjs";

const site = import.meta.dirname;
const root = join(site, "..");
const output = join(site, "dist");

/** Files the page uses that live elsewhere in the repository: site path → source. */
const borrowed: Record<string, string> = {
  images: join(root, "docs/images"),
  "favicon.ico": join(root, "web/public/favicon.ico"),
  "favicon.svg": join(root, "web/public/favicon.svg"),
  "apple-touch-icon.png": join(root, "web/public/apple-touch-icon.png"),
  "og-image.jpg": join(root, "web/public/og-image.jpg"),
};

const types: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
};

/** The borrowed file a site path names, if any. */
function borrowedFile(path: string): string | null {
  const clean = decodeURIComponent(path.split("?")[0]).replace(/^\/+/, "");
  for (const [at, from] of Object.entries(borrowed)) {
    if (clean === at) return from;
    if (clean.startsWith(`${at}/`)) {
      const file = join(from, clean.slice(at.length + 1));
      if (file.startsWith(from)) return file;
    }
  }
  return null;
}

/** Every relative href and src in the built page must name a file in dist/. */
function checkLinks(html: string) {
  const missing: string[] = [];
  for (const [, url] of html.matchAll(/\s(?:href|src|srcset)="([^"#]+)"/g)) {
    if (/^[a-z]+:/i.test(url) || url.startsWith("//")) continue;
    const path = join(output, decodeURIComponent(url.split("?")[0]));
    if (!existsSync(path)) missing.push(url);
  }
  if (missing.length) throw new Error(`site/index.html links to files that weren't built: ${missing.join(", ")}`);
}

function gantrySite(): Plugin {
  const release = readRelease();
  const values = placeholders(release);
  return {
    name: "gantry-site",
    transformIndexHtml: {
      order: "pre",
      handler: (html) => fill(html, values, "site/index.html"),
    },
    // Vite leaves the page's links to borrowed files alone; in dev they are
    // served from where they live.
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const file = req.url ? borrowedFile(req.url) : null;
        if (!file || !existsSync(file)) return next();
        res.setHeader("Content-Type", types[extname(file)] ?? "application/octet-stream");
        createReadStream(file).pipe(res);
      });
    },
    closeBundle() {
      for (const [at, from] of Object.entries(borrowed)) {
        const to = join(output, at);
        rmSync(to, { recursive: true, force: true });
        mkdirSync(join(to, ".."), { recursive: true });
        cpSync(from, to, { recursive: true });
      }
      const missingImages = sitemapImages.filter((file) => !existsSync(join(output, file)));
      if (missingImages.length) throw new Error(`The sitemap names images that weren't built: ${missingImages.join(", ")}`);
      writeFileSync(join(output, "sitemap.xml"), sitemap(release));
      checkLinks(readFileSync(join(output, "index.html"), "utf8"));
      const count = readdirSync(output, { recursive: true }).length;
      console.log(`Built the website in ${output} (${count} files, ${release ? release.tagName : "no release yet"})`);
    },
  };
}

export default defineConfig({
  root: site,
  // Relative, so the same build works at the custom domain's root and at
  // github.io/<repository>/ before its DNS exists.
  base: "./",
  plugins: [react(), gantrySite()],
  server: { port: 5174, strictPort: true },
  preview: { port: 5174, strictPort: true },
  build: {
    outDir: output,
    emptyOutDir: true,
    target: "es2022",
  },
});
