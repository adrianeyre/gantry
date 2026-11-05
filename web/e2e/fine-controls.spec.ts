import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter } from "./helpers";

test("each fine control reaches the Simulated Printer and its state shows on the card", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Fine Sim");
  const sim = card(page, "Fine Sim");
  await expect(sim.getByTestId("state")).toHaveText("Idle");

  await sim.getByText("Controls").click();
  const result = sim.getByTestId("fine-result");

  await sim.getByLabel("Speed profile").selectOption("sport");
  await expect(sim.getByTestId("speed-profile")).toHaveText("Sport");

  await sim.getByLabel("Nozzle target °C").fill("210");
  await sim.getByRole("button", { name: "Set nozzle" }).click();
  await expect(sim.getByLabel("Temperatures and fans").getByText(/→ 210 °C/)).toBeVisible();
  await sim.getByLabel("Bed target °C").fill("55");
  await sim.getByRole("button", { name: "Set bed" }).click();
  await expect(sim.getByLabel("Temperatures and fans").getByText(/→ 55 °C/)).toBeVisible();

  // The box follows what the Printer reports, so it ticks once the Printer says so.
  await sim.getByRole("checkbox", { name: "Work light" }).click();
  await expect(sim.getByRole("checkbox", { name: "Work light" })).toBeChecked();
  await expect(result).toHaveText("Work light on");

  await sim.getByLabel("Auxiliary fan %").fill("65");
  await sim.getByRole("button", { name: "Set Auxiliary fan" }).click();
  await expect(sim.getByLabel("Temperatures and fans").getByText("65%")).toBeVisible();

  // Homing makes it Busy for a moment, then it knows where it is.
  await sim.getByRole("button", { name: "Home" }).click();
  await expect(sim.getByTestId("position")).toHaveText("X 0.0 · Y 0.0 · Z 0.0");
  await expect(sim.getByTestId("state")).toHaveText("Idle", { timeout: 10_000 });

  await sim.getByRole("button", { name: "Jog X plus" }).click();
  await expect(sim.getByTestId("position")).toHaveText("X 10.0 · Y 0.0 · Z 0.0");
  await sim.getByLabel("Jog step").selectOption("1");
  await sim.getByRole("button", { name: "Jog Z plus" }).click();
  await expect(sim.getByTestId("position")).toHaveText("X 10.0 · Y 0.0 · Z 1.0");
  // A refused move says why.
  await sim.getByRole("button", { name: "Jog Y minus" }).click();
  await expect(result).toContainText("outside 0 to 256 mm");

  await sim.getByLabel("Slot to load").selectOption("1");
  await sim.getByRole("button", { name: "Load", exact: true }).click();
  await expect(sim.getByTestId("loaded-slot")).toHaveText("Slot 2");
  await expect(sim.getByTestId("state")).toHaveText("Idle", { timeout: 15_000 });
  await sim.getByRole("button", { name: "Unload" }).click();
  await expect(sim.getByTestId("loaded-slot")).toHaveCount(0);

  await removePrinter(page, id);
});
