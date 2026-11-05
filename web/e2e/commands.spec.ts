import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

test("pause, resume and Cancel each ask first, and Cancel leaves Awaiting Clear", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Commands Sim");
  const sim = card(page, "Commands Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await simulate(page, id, { action: "start_print", print_file: "commands.gcode", duration_secs: 6000, layers: 10 });
  await expect(sim.getByRole("button", { name: "Pause" })).toBeVisible();

  // Declining does nothing.
  await sim.getByRole("button", { name: "Pause" }).click();
  await expect(page.getByRole("dialog")).toContainText("Pause this Job?");
  await page.getByRole("dialog").getByRole("button", { name: "Not now" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);

  // Confirmed, it pauses, and the result is shown.
  await sim.getByRole("button", { name: "Pause" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Pause" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Paused/);
  await expect(sim.getByRole("status")).toHaveText("Paused");

  await sim.getByRole("button", { name: "Resume" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Resume" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);
  await expect(sim.getByRole("status")).toHaveText("Resumed");

  await sim.getByRole("button", { name: "Cancel Job" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Keep printing" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);
  await sim.getByRole("button", { name: "Cancel Job" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel Job" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear");
  await expect(sim.getByTestId("last-outcome")).toContainText("Cancelled");

  await removePrinter(page, id);
});

test("a command to a disconnected Printer is refused at once and never queued", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Offline Sim");
  const sim = card(page, "Offline Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await simulate(page, id, { action: "start_print", duration_secs: 6000, layers: 10 });
  await expect(sim.getByRole("button", { name: "Pause" })).toBeVisible();
  await simulate(page, id, { action: "drop_connection", reason: "unreachable" });
  await expect(sim.getByTestId("state")).toHaveText(/Disconnected: unreachable/);
  // No controls are offered while disconnected.
  await expect(sim.getByRole("button", { name: "Pause" })).toHaveCount(0);

  const started = Date.now();
  const refused = await page.request.post(`/api/printers/${id}/commands`, { data: { command: "pause" } });
  expect(Date.now() - started).toBeLessThan(1000);
  expect(refused.status()).toBe(409);
  expect((await refused.json()).error).toContain("disconnected (unreachable)");

  await simulate(page, id, { action: "restore_connection" });
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);
  await page.waitForTimeout(500);
  await expect(sim.getByTestId("state")).not.toHaveText(/Paused/);

  await removePrinter(page, id);
});
