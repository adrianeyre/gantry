import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";
import { emptyLibrary, emptyQueue, uploadFixture } from "./library-helpers";

test("queueing a 3-Plate Print File makes 3 Jobs, reordered by hand, never suggested onto an uncleared bed", async ({
  page,
}) => {
  await login(page);
  await emptyQueue(page);
  await emptyLibrary(page);
  const id = await addSimulated(page, "Queue X1C", { model: "X1C" });
  // This test is about suggestions: with Auto-start on, Mark Clear would start the next Job itself.
  const off = await page.request.put(`/api/printers/${id}/automation`, { data: { auto_start: false } });
  expect(off.status(), await off.text()).toBe(200);
  await uploadFixture(page, "three-plates.gcode.3mf");

  // Queue every Plate for the Printer.
  await page.getByRole("link", { name: "Library" }).click();
  const tray = page.getByRole("article", { name: "three-plates.gcode.3mf" });
  await tray.getByLabel("Printer for three-plates.gcode.3mf").selectOption({ label: "Queue X1C (X1C) · Idle" });
  await tray.getByLabel("Plate of three-plates.gcode.3mf").selectOption("all");
  await tray.getByRole("button", { name: "Add to Queue" }).click();
  await expect(tray.getByRole("status")).toHaveText("Queued 3 Jobs for Queue X1C.");

  // First in, first out, and the first is suggested.
  await page.getByRole("link", { name: "Queue", exact: true }).click();
  const items = page.getByRole("list", { name: "Queued Jobs" }).getByRole("listitem");
  const order = () => items.evaluateAll((els) => els.map((e) => e.getAttribute("aria-label")));
  await expect(items).toHaveCount(3);
  expect(await order()).toEqual([
    "three-plates.gcode.3mf Plate 1",
    "three-plates.gcode.3mf Plate 2",
    "three-plates.gcode.3mf Plate 3",
  ]);
  await expect(page.getByRole("button", { name: "Start three-plates.gcode.3mf Plate 1" })).toBeVisible();

  // Move Plate 3 to the front; the order survives a reload.
  await page.getByRole("button", { name: "Move three-plates.gcode.3mf Plate 3 up" }).click();
  await expect(items.nth(1)).toHaveAttribute("aria-label", "three-plates.gcode.3mf Plate 3");
  await page.getByRole("button", { name: "Move three-plates.gcode.3mf Plate 3 up" }).click();
  await expect(items.nth(0)).toHaveAttribute("aria-label", "three-plates.gcode.3mf Plate 3");
  await page.reload();
  await expect(items).toHaveCount(3);
  expect(await order()).toEqual([
    "three-plates.gcode.3mf Plate 3",
    "three-plates.gcode.3mf Plate 1",
    "three-plates.gcode.3mf Plate 2",
  ]);

  // The Idle Printer's card suggests the next Job, and an Operator starts it.
  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  const sim = card(page, "Queue X1C");
  const next = sim.getByRole("group", { name: "Next in the Queue" });
  await expect(next).toContainText("three-plates.gcode.3mf · Plate 3");
  await simulate(page, id, { action: "set_speed", speed: 1000 });
  await next.getByRole("button", { name: "Start next Job" }).click();

  // It prints and ends: Awaiting Clear, and no Job is suggested.
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 15_000 });
  await expect(sim.getByRole("group", { name: "Next in the Queue" })).toHaveCount(0);
  await page.getByRole("link", { name: "Queue", exact: true }).click();
  await expect(items).toHaveCount(2);
  await expect(page.getByRole("button", { name: /^Start / })).toHaveCount(0);

  // After Mark Clear, the next one is suggested.
  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Mark Clear" }).click();
  await expect(sim.getByRole("group", { name: "Next in the Queue" })).toContainText("Plate 1");
  await page.getByRole("link", { name: "Queue", exact: true }).click();
  await expect(page.getByRole("button", { name: "Start three-plates.gcode.3mf Plate 1" })).toBeVisible();

  await emptyQueue(page);
  await removePrinter(page, id);
  await emptyLibrary(page);
});
