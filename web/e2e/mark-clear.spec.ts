import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

async function finishAPrint(page: Page, id: string, sim: Locator) {
  await simulate(page, id, { action: "set_speed", speed: 100 });
  await simulate(page, id, { action: "start_print", print_file: "clear.gcode", duration_secs: 30, layers: 3 });
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 5000 });
}

test("Mark Clear returns the Printer to Idle, and can overrule Succeeded to Failed", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Clear Sim");
  const sim = card(page, "Clear Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  // Not offered outside Awaiting Clear.
  await expect(sim.getByRole("button", { name: "Mark Clear" })).toHaveCount(0);

  await finishAPrint(page, id, sim);
  await expect(sim.getByTestId("last-outcome")).toContainText("Succeeded");

  // Declining leaves it Awaiting Clear.
  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Not yet" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear");

  // Plain Mark Clear: back to Idle, Outcome untouched.
  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Mark Clear" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await expect(sim.getByRole("button", { name: "Mark Clear" })).toHaveCount(0);

  // Again, overruling Succeeded to Failed.
  await finishAPrint(page, id, sim);
  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await page.getByRole("dialog").getByRole("checkbox").check();
  await page.getByRole("dialog").getByRole("button", { name: "Mark Clear" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Idle");

  await sim.getByRole("link", { name: "Jobs" }).click();
  await expect(page.getByTestId("outcome")).toHaveText(["Failed (overruled)", "Succeeded"]);
  await removePrinter(page, id);
});

test("only a Succeeded Job can be overruled", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Failed Sim");
  const sim = card(page, "Failed Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await simulate(page, id, { action: "start_print", duration_secs: 6000, layers: 3 });
  await expect(sim.getByRole("button", { name: "Pause" })).toBeVisible();
  await simulate(page, id, { action: "fail_print" });
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear");

  await sim.getByRole("button", { name: "Mark Clear" }).click();
  await expect(page.getByRole("dialog").getByRole("checkbox")).toHaveCount(0);
  await page.getByRole("dialog").getByRole("button", { name: "Not yet" }).click();

  const refused = await page.request.post(`/api/printers/${id}/mark-clear`, { data: { overrule_to_failed: true } });
  expect(refused.status()).toBe(409);
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear");
  await removePrinter(page, id);
});
