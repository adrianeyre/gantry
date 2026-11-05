import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

test("cards are live, and hide what a Printer can't do", async ({ page }) => {
  await login(page);

  // A card appears and connects without a reload, over the WebSocket.
  const added = Date.now();
  const full = await addSimulated(page, "Live Card");
  await expect(card(page, "Live Card").getByTestId("state")).toHaveText("Idle", { timeout: 1500 });
  expect(Date.now() - added).toBeLessThan(1500);
  await expect(card(page, "Live Card").getByText("Connected")).toBeVisible();
  await expect(card(page, "Live Card").getByRole("list", { name: "Filament Slots" })).toBeVisible();
  await expect(
    card(page, "Live Card").getByLabel("Temperatures and fans").getByText("Nozzle", { exact: true }),
  ).toBeVisible();

  // A Printer without Filament Slots shows none.
  const bare = await addSimulated(page, "Bare Card", { filament_slots: 0, control: false });
  await expect(card(page, "Bare Card").getByTestId("state")).toHaveText("Idle");
  await expect(card(page, "Bare Card").getByRole("list", { name: "Filament Slots" })).toHaveCount(0);

  // Printing, a Printer with control offers Pause; one without hides it.
  for (const id of [full, bare]) {
    await simulate(page, id, { action: "start_print", duration_secs: 6000, layers: 10 });
  }
  await expect(card(page, "Live Card").getByRole("button", { name: "Pause" })).toBeVisible();
  await expect(card(page, "Bare Card").getByTestId("state")).toHaveText(/Preparing|Printing/);
  await expect(card(page, "Bare Card").getByRole("button", { name: "Pause" })).toHaveCount(0);
  await expect(card(page, "Bare Card").getByRole("button", { name: "Cancel Job" })).toHaveCount(0);

  // A reconnecting browser starts from a full snapshot.
  await page.reload();
  await expect(card(page, "Live Card").getByTestId("state")).toHaveText(/Preparing|Printing/);
  await expect(card(page, "Bare Card").getByTestId("state")).toHaveText(/Preparing|Printing/);

  // A removed Printer's card goes away live.
  await removePrinter(page, bare);
  await expect(card(page, "Bare Card")).toHaveCount(0);
  await removePrinter(page, full);
  await expect(card(page, "Live Card")).toHaveCount(0);
});
