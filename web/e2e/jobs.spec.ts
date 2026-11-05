import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

test("a simulated print becomes one External Job, then Awaiting Clear", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Jobs Sim");
  const sim = card(page, "Jobs Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");

  await simulate(page, id, { action: "set_speed", speed: 100 });
  await simulate(page, id, { action: "start_print", print_file: "jobs-e2e.gcode", duration_secs: 60, layers: 6 });
  await expect(sim.getByText("External Job")).toBeVisible();
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 5000 });
  await expect(sim.getByTestId("last-outcome")).toContainText("Succeeded");

  // A failed print records Failed. (Starting it clears the bed.)
  await simulate(page, id, { action: "start_print", print_file: "jobs-e2e-2.gcode", duration_secs: 6000, layers: 6 });
  await expect(sim.getByText("jobs-e2e-2.gcode")).toBeVisible();
  await simulate(page, id, { action: "fail_print" });
  await expect(sim.getByTestId("last-outcome")).toContainText("Failed");

  await sim.getByRole("link", { name: "Jobs" }).click();
  await expect(page.getByRole("heading", { name: "Jobs on Jobs Sim" })).toBeVisible();
  const rows = page.getByRole("row");
  await expect(rows).toHaveCount(3);
  await expect(page.getByTestId("outcome")).toHaveText(["Failed", "Succeeded"]);

  // Jobs are stored: the list is the same after a reload.
  await page.reload();
  await expect(page.getByTestId("outcome")).toHaveText(["Failed", "Succeeded"]);

  await removePrinter(page, id);
});
