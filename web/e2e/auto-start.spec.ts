import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";
import { emptyLibrary, emptyQueue, uploadFixture } from "./library-helpers";

async function markClear(page: import("@playwright/test").Page, name: string) {
  const sim = card(page, name);
  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Mark Clear" }).click();
}

test("with Auto-start on, Mark Clear starts the next Job, audited as Gantry; with it off, the Job is only suggested", async ({
  page,
}) => {
  await login(page);
  await emptyQueue(page);
  await emptyLibrary(page);
  const id = await addSimulated(page, "Auto X1C", { model: "X1C" });
  const fileId = await uploadFixture(page, "three-plates.gcode.3mf");
  await simulate(page, id, { action: "set_speed", speed: 1000 });
  const queued = await page.request.post("/api/queue", {
    data: { printer_id: id, print_file_id: fileId, plates: [3, 1, 2] },
  });
  expect(queued.status(), await queued.text()).toBe(201);

  // Auto-start is on by default, as the Printer's detail page says.
  await page.goto(`/printers/${id}`);
  await expect(page.getByRole("checkbox", { name: /Auto-start/ })).toBeChecked();
  await expect(page.getByRole("checkbox", { name: /Auto-Clear/ })).not.toBeChecked();

  // The first Job is started by hand, then prints and ends.
  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  const sim = card(page, "Auto X1C");
  await sim.getByRole("group", { name: "Next in the Queue" }).getByRole("button", { name: "Start next Job" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 15_000 });

  // Mark Clear: the Queue starts Plate 1 by itself.
  await markClear(page, "Auto X1C");
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 15_000 });
  await page.getByRole("link", { name: "Queue", exact: true }).click();
  const items = page.getByRole("list", { name: "Queued Jobs" }).getByRole("listitem");
  await expect(items).toHaveCount(1);
  await expect(items.first()).toHaveAttribute("aria-label", "three-plates.gcode.3mf Plate 2");

  // The audit log says Gantry started it.
  await page.getByRole("link", { name: "Audit log" }).click();
  const started = page
    .getByRole("table", { name: "Audit entries" })
    .getByRole("row")
    .filter({ hasText: "job.started" })
    .filter({ hasText: "Gantry" });
  await expect(started).toHaveCount(1);
  await expect(started.first()).toContainText("Auto X1C");

  // Turn Auto-start off: after Mark Clear the last Job is only suggested.
  await page.goto(`/printers/${id}`);
  await page.getByRole("checkbox", { name: /Auto-start/ }).uncheck();
  await expect(page.getByRole("checkbox", { name: /Auto-start/ })).not.toBeChecked();
  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  await markClear(page, "Auto X1C");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  const next = sim.getByRole("group", { name: "Next in the Queue" });
  await expect(next).toContainText("three-plates.gcode.3mf · Plate 2");
  // It stays suggested, not started.
  await page.waitForTimeout(1500);
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await expect(next).toBeVisible();

  await emptyQueue(page);
  await removePrinter(page, id);
  await emptyLibrary(page);
});
