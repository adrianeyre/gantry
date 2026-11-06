// What the page knows about the project and its latest release, as the
// `{{NAME}}` placeholders site/index.html is written with. vite.config.ts
// fills them in; `release.test.mjs` checks the matching.
//
// GANTRY_RELEASE names a file holding `gh release view --json
// tagName,url,publishedAt,assets`: the download buttons then link straight to
// that release's files. Without it they link to the Releases page.
import { existsSync, readFileSync } from "node:fs";

/** The repository, named once: every link to it on the page comes from here. */
export const repositoryUrl = "https://github.com/adrianeyre/gantry";

/** Where the site is published (site/public/CNAME says the same to GitHub Pages). */
export const siteUrl = "https://gantry.adrianeyre.co.uk";

/** The downloads the page offers, each matched against the release's assets. */
export const downloads = [
  { key: "WIN_EXE", pattern: /_x64-setup\.exe$/ },
  { key: "WIN_MSI", pattern: /_x64_en-US\.msi$/ },
  { key: "MAC_DMG", pattern: /_aarch64\.dmg$/ },
  { key: "LINUX_DEB", pattern: /_amd64\.deb$/ },
  { key: "LINUX_APPIMAGE", pattern: /_amd64\.AppImage$/ },
  { key: "SERVER_LINUX_AMD64", pattern: /-linux-amd64\.tar\.gz$/ },
  { key: "SERVER_LINUX_ARM64", pattern: /-linux-arm64\.tar\.gz$/ },
  { key: "SERVER_WINDOWS", pattern: /-windows-x64\.zip$/ },
  { key: "SERVER_MACOS", pattern: /-macos-arm64\.tar\.gz$/ },
  { key: "CHECKSUMS", pattern: /^SHA256SUMS$/ },
];

/** The release GANTRY_RELEASE names, or null when there is none yet. */
export function readRelease(file = process.env.GANTRY_RELEASE) {
  if (!file || !existsSync(file)) return null;
  const text = readFileSync(file, "utf8").trim();
  if (!text) return null;
  const release = JSON.parse(text);
  return release.tagName ? release : null;
}

/** Every placeholder's value, for a release or for none. */
export function placeholders(release, now = new Date()) {
  const releases = `${repositoryUrl}/releases`;
  const values = {
    SITE_URL: siteUrl,
    REPO_URL: repositoryUrl,
    RELEASES_URL: releases,
    DOCS_URL: `${repositoryUrl}/tree/main/docs`,
    BLOB_URL: `${repositoryUrl}/blob/main`,
    IMAGE: "ghcr.io/adrianeyre/gantry",
    VERSION: release ? release.tagName.replace(/^v/, "") : "latest",
    VERSION_LABEL: release ? `Latest release: ${release.tagName}` : "Latest release",
    RELEASE_URL: release?.url ?? `${releases}/latest`,
    YEAR: String(now.getFullYear()),
  };
  for (const { key, pattern } of downloads) {
    const asset = release?.assets?.find((a) => pattern.test(a.name));
    values[`${key}_URL`] = asset?.url ?? values.RELEASE_URL;
  }
  return values;
}

/** Fills `{{NAME}}`s, failing on one it doesn't know so a typo never ships. */
export function fill(text, values, file) {
  return text.replaceAll(/\{\{([A-Z0-9_]+)\}\}/g, (match, name) => {
    if (!(name in values)) throw new Error(`${file} names an unknown placeholder ${match}`);
    return values[name];
  });
}

/** The images the sitemap offers to image search, as paths in dist/. */
export const sitemapImages = [
  "og-image.jpg",
  "images/fleet-light.png",
  "images/fleet-dark.png",
  "images/queue.png",
  "images/cameras.png",
  "images/library.png",
  "images/model-viewer.png",
  "images/printer-detail.png",
  "images/wall.png",
];

/**
 * sitemap.xml, written into dist/ by the build: the page, dated by the
 * latest release (or the build, before there is one), with its images.
 */
export function sitemap(release, now = new Date()) {
  const lastmod = (release?.publishedAt ? new Date(release.publishedAt) : now).toISOString().slice(0, 10);
  const images = sitemapImages
    .map((file) => `    <image:image><image:loc>${siteUrl}/${file}</image:loc></image:image>`)
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
  <url>
    <loc>${siteUrl}/</loc>
    <lastmod>${lastmod}</lastmod>
${images}
  </url>
</urlset>
`;
}
