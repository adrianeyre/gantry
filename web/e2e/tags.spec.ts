import { expect, test } from "@playwright/test";
import { addSimulated, card, login, logout, removePrinter } from "./helpers";

const viewer = { username: "tags-viewer", password: "a viewer password for tags" };

test("Admins tag Printers, everyone filters the Fleet by Tag, and Tags travel in an export", async ({ page }) => {
  await login(page);
  const garage = await addSimulated(page, "Tag Garage Sim");
  const office = await addSimulated(page, "Tag Office Sim");

  // Admins create Tags and attach them under Printers.
  await page.getByRole("link", { name: "Printers" }).click();
  const tags = page.getByRole("region", { name: "Tags" });
  for (const name of ["E2E Garage", "E2E Office"]) {
    await page.getByLabel("New Tag").fill(name);
    await page.getByRole("button", { name: "Add Tag" }).click();
    await expect(tags.getByRole("button", { name: `Remove the Tag ${name}` })).toBeVisible();
  }
  await page.getByLabel("E2E Garage on Tag Garage Sim").check();
  await page.getByLabel("E2E Office on Tag Office Sim").check();
  await page.reload();
  await expect(page.getByLabel("E2E Garage on Tag Garage Sim")).toBeChecked();
  await expect(page.getByLabel("E2E Office on Tag Garage Sim")).not.toBeChecked();

  // The Fleet filters by one or more Tags.
  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  const filter = page.getByRole("group", { name: "Filter by Tag" });
  await expect(card(page, "Tag Garage Sim")).toBeVisible();
  await filter.getByRole("button", { name: "E2E Garage" }).click();
  await expect(card(page, "Tag Garage Sim")).toBeVisible();
  await expect(card(page, "Tag Office Sim")).toHaveCount(0);
  await expect(card(page, "Seed Simulated A")).toHaveCount(0);
  await filter.getByRole("button", { name: "E2E Office" }).click();
  await expect(card(page, "Tag Office Sim")).toBeVisible();
  await expect(card(page, "Tag Garage Sim")).toBeVisible();
  await expect(card(page, "Tag Garage Sim").getByRole("list", { name: "Tags" })).toHaveText("E2E Garage");

  // Tags are in the config export.
  const exported = await (await page.request.get("/api/config/export")).text();
  expect(exported).toContain('"E2E Garage"');
  expect(exported).toMatch(/name = "Tag Office Sim"[\s\S]*?tags = \["E2E Office"\]/);

  // A Viewer filters too, but can't change Tags, even through the API.
  await page.goto("/users");
  const form = page.getByRole("form", { name: "Add a user" });
  await form.getByLabel("Username").fill(viewer.username);
  await form.getByLabel("Password").fill(viewer.password);
  await form.getByLabel("Role").selectOption("viewer");
  await form.getByRole("button", { name: "Add user" }).click();
  await expect(page.getByLabel(`Role of ${viewer.username}`)).toHaveValue("viewer");
  await logout(page);
  await login(page, viewer);
  await page.getByRole("group", { name: "Filter by Tag" }).getByRole("button", { name: "E2E Office" }).click();
  await expect(card(page, "Tag Office Sim")).toBeVisible();
  await expect(card(page, "Tag Garage Sim")).toHaveCount(0);
  await page.getByRole("link", { name: "Printers" }).click();
  await expect(page.getByLabel("New Tag")).toHaveCount(0);
  await expect(page.getByLabel("E2E Garage on Tag Garage Sim")).toHaveCount(0);
  expect((await page.request.post("/api/tags", { data: { name: "Nope" } })).status()).toBe(403);
  await logout(page);

  // Clean up as the Admin.
  await login(page);
  const all = (await (await page.request.get("/api/tags")).json()) as { id: number; name: string }[];
  for (const t of all.filter((t) => t.name.startsWith("E2E "))) await page.request.delete(`/api/tags/${t.id}`);
  const users = (await (await page.request.get("/api/users")).json()) as { id: number; username: string }[];
  const added = users.find((u) => u.username === viewer.username);
  if (added) await page.request.delete(`/api/users/${added.id}`);
  await removePrinter(page, garage);
  await removePrinter(page, office);
});
