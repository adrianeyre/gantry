import { expect, test } from "@playwright/test";
import { login } from "./helpers";

test("an Admin adds, edits and removes a Simulated Printer", async ({ page }) => {
  await login(page);
  await page.getByRole("link", { name: "Printers" }).click();
  await page.getByRole("link", { name: "Add Printer" }).click();
  await page.getByLabel("Driver").selectOption({ label: "Simulated" });

  // Validation errors are shown inline.
  await page.getByRole("button", { name: "Add Printer" }).click();
  await expect(page.getByLabel("Name")).toHaveAttribute("aria-invalid", "true");
  await page.getByLabel("Filament Slots").fill("40");
  await page.getByLabel("Name").fill("E2E Bench");
  await page.getByRole("button", { name: "Add Printer" }).click();
  await expect(page.getByText("Must be between 0 and 16")).toBeVisible();

  await page.getByLabel("Filament Slots").fill("4");
  await page.getByLabel("Access code", { exact: true }).fill("E2E-DUMMY-CODE");
  await page.getByRole("button", { name: "Add Printer" }).click();
  await expect(page.getByRole("cell", { name: "E2E Bench", exact: true })).toBeVisible();

  // It is stored, so it is still there after a reload, and the credential
  // never comes back to the browser.
  await page.reload();
  await expect(page.getByRole("cell", { name: "E2E Bench", exact: true })).toBeVisible();
  const listed = await (await page.request.get("/api/printers")).text();
  expect(listed).toContain("E2E Bench");
  expect(listed).not.toContain("E2E-DUMMY-CODE");

  // A name another Printer has is refused inline.
  await page.getByRole("link", { name: "Add Printer" }).click();
  await page.getByLabel("Driver").selectOption({ label: "Simulated" });
  await page.getByLabel("Name").fill("e2e bench");
  await page.getByRole("button", { name: "Add Printer" }).click();
  await expect(page.getByText("Another Printer already has this name")).toBeVisible();
  await page.getByRole("button", { name: "Back" }).click();

  // Edit: the saved credential is kept unless replaced.
  await page.getByRole("link", { name: "Edit E2E Bench" }).click();
  await expect(page.getByLabel("Access code", { exact: true })).toHaveAttribute("placeholder", /Saved/);
  await page.getByLabel("Name").fill("E2E Bench Renamed");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("cell", { name: "E2E Bench Renamed", exact: true })).toBeVisible();

  // Remove, after confirming.
  await page.getByRole("button", { name: "Remove E2E Bench Renamed" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("cell", { name: "E2E Bench Renamed", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Remove E2E Bench Renamed" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("cell", { name: "E2E Bench Renamed", exact: true })).toHaveCount(0);
});

test("the Bambu Driver asks for its address, serial number and access code", async ({ page }) => {
  await login(page);
  await page.goto("/printers/new");
  await page.getByLabel("Driver").selectOption({ label: "Bambu Lab (LAN Mode)" });
  await page.getByLabel("Name").fill("E2E Bambu");
  await page.getByRole("button", { name: "Add Printer" }).click();
  for (const field of ["IP address", "Serial number", "Access code"]) {
    await expect(page.getByLabel(field, { exact: true })).toHaveAttribute("aria-invalid", "true");
  }
  await expect(page.getByLabel("Serial number", { exact: true })).toHaveAttribute("type", "password");
  await expect(page.getByLabel("Access code", { exact: true })).toHaveAttribute("type", "password");
});
