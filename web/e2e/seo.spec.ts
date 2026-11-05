import { expect, test } from "@playwright/test";
// @ts-expect-error -- a plain .mjs module shared with the launcher script.
import { BASE_URL } from "./config.mjs";

// What a social site or search engine sees: they fetch without a session, and
// follow the redirect from / to the login page.

test("a shared link previews with an absolute title, description and image", async ({ request }) => {
  const response = await request.get("/");
  expect(response.ok()).toBe(true);
  const html = await response.text();
  expect(html).not.toContain("__GANTRY_ORIGIN__");

  const meta = (attr: "property" | "name", key: string) =>
    // Attributes may be wrapped onto their own lines, as they are in index.html.
    new RegExp(`<meta\\s+${attr}="${key.replace(/[:.]/g, "\\$&")}"\\s+content="([^"]*)"`).exec(html)?.[1];
  expect(/<title>([^<]+)<\/title>/.exec(html)?.[1]).toBe("Gantry · 3D printer fleet dashboard");
  expect(meta("name", "description")?.length).toBeGreaterThan(70);
  expect(meta("property", "og:title")).toBe("Gantry · Every 3D printer, one screen");
  expect(meta("property", "og:url")).toBe(`${BASE_URL}/`);
  expect(meta("property", "og:image")).toBe(`${BASE_URL}/og-image.jpg`);
  expect(meta("name", "twitter:card")).toBe("summary_large_image");
  expect(meta("name", "twitter:image")).toBe(`${BASE_URL}/og-image.jpg`);
  expect(html).toContain(`<link rel="canonical" href="${BASE_URL}/" />`);
  expect(html).toContain('"@type": "SoftwareApplication"');
});

test("the preview image, icons, manifest and robots.txt are public", async ({ request }) => {
  for (const [path, type] of [
    ["/og-image.jpg", "image/jpeg"],
    ["/favicon.svg", "image/svg+xml"],
    ["/favicon.ico", "image/"],
    ["/apple-touch-icon.png", "image/png"],
    ["/icon-512.png", "image/png"],
    ["/site.webmanifest", ""],
    ["/robots.txt", "text/plain"],
  ] as const) {
    const response = await request.get(path, { maxRedirects: 0 });
    expect(response.status(), path).toBe(200);
    expect(response.headers()["content-type"], path).toContain(type);
  }
  const card = await request.get("/og-image.jpg");
  expect((await card.body()).length).toBeLessThan(300 * 1024);
});

test("each page has its own title", async ({ page }) => {
  await page.goto("/login");
  await expect(page).toHaveTitle("Log in · Gantry");
});
