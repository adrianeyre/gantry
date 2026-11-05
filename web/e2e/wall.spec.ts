import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { addSimulated, login, logout, removePrinter, simulate } from "./helpers";

const kiosk = { username: "e2e-wall-viewer", password: "a kiosk viewer password" };

async function cardsFit(page: Page) {
  // No scrolling, and every card is wholly on the screen.
  const fit = await page.evaluate(() => {
    const doc = document.documentElement;
    const inside = [...document.querySelectorAll("article.wall-card")].every((el) => {
      const r = el.getBoundingClientRect();
      return r.top >= 0 && r.left >= 0 && r.bottom <= window.innerHeight + 0.5 && r.right <= window.innerWidth + 0.5;
    });
    return {
      inside,
      scrolls: doc.scrollHeight > window.innerHeight + 1 || doc.scrollWidth > window.innerWidth + 1,
    };
  });
  expect(fit).toEqual({ inside: true, scrolls: false });
}

test("the wall display fits 1 to 20+ Printers on the screen, shows no controls, and works for a Viewer", async ({
  page,
}) => {
  await login(page);
  // Other tests' Printers may be in the Fleet too: count them in.
  const before = ((await (await page.request.get("/api/printers")).json()) as unknown[]).length;
  const created = await page.request.post("/api/users", { data: { ...kiosk, role: "viewer" } });
  expect([201, 409, 422]).toContain(created.status());
  const ids: string[] = [];
  ids.push(await addSimulated(page, "Wall 01"));
  await simulate(page, ids[0], { action: "start_print", print_file: "wall.gcode", duration_secs: 6000, layers: 60 });
  await logout(page);

  await page.setViewportSize({ width: 1920, height: 1080 });
  await login(page, kiosk);
  await page.goto("/wall");
  const wall = page.getByRole("main", { name: "Wall display" });
  await expect(wall.getByRole("article")).toHaveCount(before + 1);
  await expect(wall.getByRole("article", { name: "Wall 01" }).getByTestId("state")).toHaveText(/Preparing|Printing/);
  await expect(page.getByRole("button")).toHaveCount(0);
  await expect(page.getByRole("link")).toHaveCount(0);
  await cardsFit(page);
  // The wall has no controls, so not even a log-out button: leave first.
  await page.goto("/");
  await logout(page);

  // Grow the Fleet past 20.
  await login(page);
  for (let i = 2; i <= 22; i++) ids.push(await addSimulated(page, `Wall ${String(i).padStart(2, "0")}`));
  await logout(page);
  await login(page, kiosk);
  await page.goto("/wall");
  await expect(wall.getByRole("article")).toHaveCount(before + 22);
  await expect(page.locator("button, a, input, select, textarea")).toHaveCount(0);
  await cardsFit(page);
  // And on a smaller, portrait screen.
  await page.setViewportSize({ width: 800, height: 1280 });
  await cardsFit(page);
  // Each card stays readable: its name is shown, not clipped away.
  await expect(wall.getByRole("article", { name: "Wall 22" }).getByRole("heading", { name: "Wall 22" })).toBeVisible();

  // Escape leaves for the dashboard.
  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { name: "Fleet" })).toBeVisible();
  await logout(page);
  await login(page);
  for (const id of ids) await removePrinter(page, id);
});
