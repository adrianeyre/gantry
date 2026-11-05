import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

test("the endpoint raises a Fault and drops the Connection, and the card shows both", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Sim Endpoint");
  const sim = card(page, "Sim Endpoint");
  await expect(sim.getByTestId("state")).toHaveText("Idle");

  await simulate(page, id, { action: "raise_fault", code: "nozzle_clog", severity: "error", message: "Nozzle clogged" });
  await expect(sim.getByRole("list", { name: "Faults" })).toContainText("Nozzle clogged");

  await simulate(page, id, { action: "drop_connection", reason: "unreachable" });
  await expect(sim.getByTestId("state")).toHaveText("Disconnected: unreachable (last seen: idle)");
  await expect(sim.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(sim.getByRole("list", { name: "Last known Faults" })).toContainText("Nozzle clogged");

  await simulate(page, id, { action: "restore_connection" });
  await expect(sim.getByTestId("state")).toHaveText("Idle");

  // The endpoint needs the same login as the rest of the API.
  const anonymous = await page.request.post(`/api/printers/${id}/simulation`, {
    data: { action: "drop_connection" },
    headers: { cookie: "" },
  });
  expect([401]).toContain(anonymous.status());

  await removePrinter(page, id);
});

test("the panel drives a Simulated Printer, and cards follow within a second", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Sim Panel");
  const sim = card(page, "Sim Panel");
  await expect(sim.getByTestId("state")).toHaveText("Idle");
  await sim.getByText("Simulation").click();

  const panel = sim.getByRole("form", { name: "Print" });
  await panel.getByLabel("Print File").fill("panel-test.gcode");
  await panel.getByLabel("Duration (s)").fill("600");
  await panel.getByLabel("Layers").fill("30");
  const started = Date.now();
  await panel.getByRole("button", { name: "Start print" }).click();
  await expect(sim.getByText("panel-test.gcode")).toBeVisible({ timeout: 1000 });
  expect(Date.now() - started).toBeLessThan(1000);
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);

  await sim.getByLabel("Speed (×)").fill("50");
  await sim.getByRole("button", { name: "Set speed" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Printing · \d+%/);

  await sim.getByLabel("Code").fill("runout");
  await sim.getByLabel("Severity").selectOption("warning");
  await sim.getByLabel("Message").fill("Filament ran out");
  await sim.getByRole("button", { name: "Raise Fault" }).click();
  await expect(sim.getByRole("list", { name: "Faults" })).toContainText("Filament ran out");
  await sim.getByRole("button", { name: "Clear Fault" }).click();
  await expect(sim.getByRole("list", { name: "Faults" })).toHaveCount(0);

  await sim.getByLabel("Type").fill("ASA");
  await sim.getByRole("button", { name: "Load filament" }).click();
  await expect(sim.getByRole("list", { name: "Filament Slots" })).toContainText("ASA 100%");
  await sim.getByRole("button", { name: "Empty slot" }).click();
  await expect(sim.getByRole("list", { name: "Filament Slots" })).not.toContainText("ASA");

  await sim.getByRole("button", { name: "Drop Connection" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Disconnected: unreachable \(last seen: printing at \d+%\)/);
  await sim.getByRole("button", { name: "Restore Connection" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Printing/);

  await sim.getByRole("button", { name: "Fail print" }).click();
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear");
  await expect(sim.getByTestId("last-outcome")).toContainText("Failed");

  await removePrinter(page, id);
});

test("only Simulated Printers have the panel and endpoint", async ({ page }) => {
  await login(page);
  const response = await page.request.post("/api/printers/not-a-printer/simulation", {
    data: { action: "restore_connection" },
  });
  expect(response.status()).toBe(404);
});
