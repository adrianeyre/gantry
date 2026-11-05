// `pnpm --filter @gantry/site test`: the placeholders the page is filled with.
import { test } from "node:test";
import assert from "node:assert/strict";
import { fill, placeholders, repositoryUrl } from "./release.mjs";

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
