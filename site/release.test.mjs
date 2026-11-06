// `pnpm --filter @gantry/site test`: the placeholders the page is filled with.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fill, placeholders, repositoryUrl, siteUrl, sitemap, sitemapImages } from "./release.mjs";

const release = {
  tagName: "v1.2.0",
  url: `${repositoryUrl}/releases/tag/v1.2.0`,
  publishedAt: "2026-11-01T10:00:00Z",
  assets: [
    { name: "Gantry_1.2.0_x64-setup.exe", url: "https://example.test/setup.exe" },
    { name: "gantry-1.2.0-linux-arm64.tar.gz", url: "https://example.test/arm64.tar.gz" },
    { name: "SHA256SUMS", url: "https://example.test/SHA256SUMS" },
  ],
};

test("a release's downloads link straight to its files", () => {
  const values = placeholders(release);
  assert.equal(values.VERSION, "1.2.0");
  assert.equal(values.WIN_EXE_URL, "https://example.test/setup.exe");
  assert.equal(values.SERVER_LINUX_ARM64_URL, "https://example.test/arm64.tar.gz");
  assert.equal(values.CHECKSUMS_URL, "https://example.test/SHA256SUMS");
});

test("a download the release lacks links to the release instead", () => {
  assert.equal(placeholders(release).MAC_DMG_URL, release.url);
});

test("with no release yet, everything links to the latest release", () => {
  const values = placeholders(null);
  assert.equal(values.VERSION, "latest");
  assert.equal(values.WIN_EXE_URL, `${repositoryUrl}/releases/latest`);
});

test("an unknown placeholder fails the build", () => {
  assert.throws(() => fill("{{NOPE}}", placeholders(null), "index.html"), /unknown placeholder \{\{NOPE\}\}/);
  assert.equal(fill("v{{VERSION}}", placeholders(release), "index.html"), "v1.2.0");
});

test("the sitemap is dated by the release, or the build before one", () => {
  assert.match(sitemap(release), /<lastmod>2026-11-01<\/lastmod>/);
  assert.match(sitemap(null, new Date("2026-12-25T09:00:00Z")), /<lastmod>2026-12-25<\/lastmod>/);
});

test("the sitemap lists the page and its images by absolute URL", () => {
  const xml = sitemap(null);
  assert.match(xml, new RegExp(`<loc>${siteUrl}/</loc>`));
  for (const file of sitemapImages) assert.ok(xml.includes(`<image:loc>${siteUrl}/${file}</image:loc>`), file);
});

// What search results and link previews (Slack, Discord, iMessage, X…) read.
const page = fill(readFileSync(new URL("./index.html", import.meta.url), "utf8"), placeholders(release), "index.html");
const meta = (attribute, name) =>
  page.match(new RegExp(`<meta\\s+${attribute}="${name}"\\s+content="([^"]*)"`))?.[1];

test("the title and description fit in a search result", () => {
  const title = page.match(/<title>([^<]+)<\/title>/)[1];
  assert.ok(title.length <= 60, `title is ${title.length} characters`);
  const description = meta("name", "description");
  assert.ok(description.length >= 70 && description.length <= 160, `description is ${description.length} characters`);
});

test("every link preview tag is there, with absolute URLs", () => {
  for (const name of ["og:title", "og:description", "og:image", "og:image:alt", "og:url", "og:site_name"]) {
    assert.ok(meta("property", name), name);
  }
  for (const name of ["twitter:card", "twitter:title", "twitter:description", "twitter:image", "twitter:image:alt"]) {
    assert.ok(meta("name", name), name);
  }
  assert.equal(meta("property", "og:image"), `${siteUrl}/og-image.jpg`);
  assert.equal(meta("property", "og:url"), `${siteUrl}/`);
  assert.match(page, new RegExp(`<link rel="canonical" href="${siteUrl}/" />`));
});

test("the structured data is valid JSON describing the software", () => {
  const json = page.match(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/)[1];
  const software = JSON.parse(json)["@graph"].find((node) => node["@type"] === "SoftwareApplication");
  assert.equal(software.softwareVersion, "1.2.0");
  assert.equal(software.downloadUrl, release.url);
  assert.equal(software.offers.price, "0");
});
