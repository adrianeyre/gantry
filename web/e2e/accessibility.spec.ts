import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { addSimulated, admin, card, login, removePrinter, simulate } from "./helpers";
import { emptyLibrary, removePrintFile, uploadFixture } from "./library-helpers";

// Automated WCAG 2.2 A and AA checks. axe finds about half of all possible
// failures; the rest (meaning, sensible order, wording) need a person, which
// the accessibility statement says.
const WCAG = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22a", "wcag22aa"];

async function expectNoViolations(page: Page, where: string) {
  const results = await new AxeBuilder({ page }).withTags(WCAG).analyze();
  const summary = results.violations.map(
    (v) => `${v.id} (${v.impact}): ${v.help}\n  ${v.nodes.map((n) => n.target.join(" ")).join("\n  ")}`,
  );
  expect(summary, `${where}\n${summary.join("\n")}`).toEqual([]);
}

/** Every page an Admin can reach, with a Simulated Printer printing so the cards show everything. */
function pages(id: string, fileId: number) {
  return [
    ["Fleet", "/"],
    ["Cameras", "/cameras"],
    ["Library", "/library"],
    ["3D model", `/library/${fileId}/model`],
    ["Toolpath", `/library/${fileId}/plates/1/toolpath`],
    ["Queue", "/queue"],
    ["Filament", "/filament"],
    ["Utilisation", "/analytics"],
    ["Printers", "/printers"],
    ["Printer detail", `/printers/${id}`],
    ["Add a Printer", "/printers/new"],
    ["Edit Printer", `/printers/${id}/edit`],
    ["Jobs", `/printers/${id}/jobs`],
    ["Users", "/users"],
    ["Alerts", "/alerts"],
    ["Webhooks", "/webhooks"],
    ["Account", "/account"],
    ["Audit log", "/audit"],
    ["Settings", "/settings"],
    ["Not found", "/nowhere"],
  ] as const;
}

for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`in ${colorScheme} mode (following the system)`, () => {
    test.use({ colorScheme });

    test("the login page has no WCAG 2.2 AA violations", async ({ page }) => {
      await page.goto("/login");
      await expect(page.locator("html")).toHaveAttribute("data-mode", colorScheme);
      await expectNoViolations(page, "login");
      await page.getByRole("button", { name: "Show password" }).click();
      await expect(page.getByLabel("Password")).toHaveAttribute("type", "text");
    });

    test("every page, a policy and a confirmation have no WCAG 2.2 AA violations", async ({ page }) => {
      // About twenty pages and a few states, each checked by axe.
      test.setTimeout(90_000);
      await login(page);
      const id = await addSimulated(page, "A11y Sim");
      await simulate(page, id, { action: "start_print", print_file: "a11y.gcode", duration_secs: 6000, layers: 10 });
      await expect(card(page, "A11y Sim").getByTestId("state")).toHaveText(/Preparing|Printing/);
      await emptyLibrary(page);
      const fileId = await uploadFixture(page, "three-plates.gcode.3mf");

      for (const [name, path] of pages(id, fileId)) {
        await page.goto(path);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
        await expectNoViolations(page, name);
      }

      // A new API token, shown once, and TOTP being set up, with its QR code.
      await page.goto("/account");
      await page.getByLabel("Token name").fill("A11y token");
      await page.getByRole("button", { name: "Make token" }).click();
      await expect(page.getByRole("region", { name: "New API token" })).toBeVisible();
      await expectNoViolations(page, "Account with a new API token");
      await page.getByRole("button", { name: "Set up two-factor authentication" }).click();
      await page.getByLabel("Your password").fill(admin.password);
      await page.getByRole("button", { name: "Set up", exact: true }).click();
      await expect(page.getByRole("img", { name: /QR code/ })).toBeVisible();
      await expectNoViolations(page, "Account setting up TOTP");
      const tokens = (await (await page.request.get("/api/tokens")).json()) as { id: number; name: string }[];
      for (const t of tokens.filter((t) => t.name === "A11y token")) await page.request.delete(`/api/tokens/${t.id}`);

      // A webhook in the list, and its edit dialog.
      const hook = await page.request.post("/api/webhooks", {
        data: { name: "A11y hook", url: "https://example.com/hook", events: ["job_ended"] },
      });
      expect(hook.status(), await hook.text()).toBe(201);
      const hookId = ((await hook.json()) as { id: number }).id;
      await page.goto("/webhooks");
      await page.getByRole("button", { name: "Edit A11y hook" }).click();
      await expect(page.getByRole("dialog", { name: "Edit A11y hook" })).toBeVisible();
      await expectNoViolations(page, "Webhook edit dialog");
      await page.keyboard.press("Escape");
      await page.request.delete(`/api/webhooks/${hookId}`);

      // The wall display: no page chrome, just the cards.
      await page.goto("/wall");
      await expect(page.getByRole("main", { name: "Wall display" }).getByRole("article").first()).toBeVisible();
      await expectNoViolations(page, "Wall display");

      // The card's panels open, a footer policy, and a confirmation.
      await page.goto("/");
      const sim = card(page, "A11y Sim");
      await sim.getByText("Controls").click();
      await sim.getByText("Simulation").click();
      await expectNoViolations(page, "Fleet with panels open");
      await page.getByRole("contentinfo").getByRole("button", { name: "Cookie Policy" }).click();
      await expectNoViolations(page, "Cookie Policy dialog");
      await page.keyboard.press("Escape");
      await sim.getByRole("button", { name: "Pause" }).click();
      await expect(page.getByRole("dialog").getByRole("button", { name: "Not now" })).toBeFocused();
      await expectNoViolations(page, "Pause confirmation");
      await page.keyboard.press("Escape");

      await removePrintFile(page, fileId);
      await removePrinter(page, id);
    });
  });
}

test("each of the six themes passes in light and dark", async ({ page }) => {
  await login(page);
  for (const mode of ["light", "dark"]) {
    for (const theme of ["cobalt", "violet", "teal", "rose", "copper", "graphite"]) {
      await page.evaluate(
        ([m, t]) => {
          localStorage.setItem("gantry.mode", m);
          localStorage.setItem("gantry.theme", t);
        },
        [mode, theme],
      );
      await page.goto("/settings");
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await expect(page.locator("html")).toHaveAttribute("data-mode", mode);
      await expectNoViolations(page, `${mode} ${theme}`);
    }
  }
});

test("a chosen mode and theme last across reloads, and apply before the first paint", async ({ page }) => {
  await login(page);
  await page.getByRole("link", { name: "Settings" }).click();
  await page.getByRole("group", { name: "Mode" }).getByRole("radio", { name: /Dark/ }).check();
  await page.getByRole("group", { name: "Theme" }).getByRole("radio", { name: /Teal/ }).check();
  await page.reload();
  // index.html sets these before React runs; check them on the raw document.
  const before = await page.evaluate(() => [document.documentElement.dataset.mode, document.documentElement.dataset.theme]);
  expect(before).toEqual(["dark", "teal"]);
});

test("the page reflows at 320 pixels wide with no sideways scrolling (400% zoom)", async ({ page }) => {
  await login(page);
  await page.setViewportSize({ width: 320, height: 256 });
  for (const path of ["/", "/printers", "/settings", "/audit", "/account", "/webhooks"]) {
    await page.goto(path);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow, path).toBeLessThanOrEqual(0);
  }
});

test("the header and footer never hide what has keyboard focus", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Focus Sim");
  await page.setViewportSize({ width: 1024, height: 640 });
  await page.goto("/");
  await expect(card(page, "Focus Sim")).toBeVisible();
  for (let i = 0; i < 40; i++) {
    await page.keyboard.press("Tab");
    const hidden = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body) return null;
      const box = el.getBoundingClientRect();
      const header = document.querySelector(".topbar")!.getBoundingClientRect();
      const footer = document.querySelector(".site-footer")!.getBoundingClientRect();
      if (header.bottom >= footer.top) return null;
      // The skip link is drawn over the header on purpose, while it has focus.
      const inHeaderOrFooter = el.closest(".topbar, .site-footer, .cookie-banner, .skip-link");
      if (inHeaderOrFooter) return null;
      // Entirely covered: above the header's bottom edge or below the footer's top.
      return box.bottom <= header.bottom || box.top >= footer.top ? el.outerHTML.slice(0, 120) : null;
    });
    expect(hidden).toBeNull();
  }
  await removePrinter(page, id);
});

test("the skip link is first, and takes focus to the main content", async ({ page }) => {
  await login(page);
  await page.goto("/printers");
  // Start from a rendered page, with focus on the document itself.
  await expect(page.getByRole("heading", { level: 1, name: "Printers" })).toBeVisible();
  await page.locator("body").evaluate((body) => (body as HTMLElement).focus());
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: "Skip to main content" });
  await expect(skip).toBeFocused();
  await expect(skip).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("main")).toBeFocused();
});
