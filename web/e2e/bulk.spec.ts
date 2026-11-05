import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";

test("a bulk pause asks once, and each Printer gets its own result and audit entry", async ({ page }) => {
  await login(page);
  const printing = await addSimulated(page, "Bulk Printing");
  const idle = await addSimulated(page, "Bulk Idle");
  const offline = await addSimulated(page, "Bulk Offline");
  for (const name of ["Bulk Printing", "Bulk Idle", "Bulk Offline"]) {
    await expect(card(page, name).getByTestId("state")).toHaveText("Idle");
  }
  await simulate(page, printing, { action: "start_print", duration_secs: 6000, layers: 10 });
  await simulate(page, offline, { action: "start_print", duration_secs: 6000, layers: 10 });
  await simulate(page, offline, { action: "drop_connection", reason: "unreachable" });
  await expect(card(page, "Bulk Offline").getByTestId("state")).toHaveText(/Disconnected/);

  const auditBefore = await page.request.get("/api/audit?action=printer.command");
  const before = ((await auditBefore.json()) as { entries: unknown[] }).entries.length;

  for (const name of ["Bulk Printing", "Bulk Idle", "Bulk Offline"]) {
    await page.getByLabel(`Select ${name}`).check();
  }
  const bar = page.getByRole("group", { name: "Bulk actions" });
  await expect(bar).toContainText("For 3 selected:");
  await bar.getByRole("button", { name: "Pause" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Pause on 3 Printers?");
  await dialog.getByRole("button", { name: "Pause" }).click();

  const results = page.getByRole("list", { name: "Bulk results" });
  await expect(results).toContainText("Bulk Printing: Done");
  await expect(results).toContainText("Bulk Idle: Skipped (not printing)");
  await expect(results).toContainText(/Bulk Offline: Rejected \(the Printer is disconnected/);
  // One Printer's failure didn't stop another.
  await expect(card(page, "Bulk Printing").getByTestId("state")).toHaveText(/Paused/);

  const auditAfter = await page.request.get("/api/audit?action=printer.command");
  const entries = ((await auditAfter.json()) as { entries: { detail: { bulk?: boolean } | null }[] }).entries;
  expect(entries.length - before).toBe(3);
  expect(entries.slice(0, 3).every((e) => e.detail?.bulk === true)).toBe(true);

  await bar.getByRole("button", { name: "Clear selection" }).click();
  await expect(bar).toContainText("For every Printer:");
  for (const id of [printing, idle, offline]) await removePrinter(page, id);
});
