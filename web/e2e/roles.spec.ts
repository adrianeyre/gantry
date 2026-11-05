import { expect, test } from "@playwright/test";
import { addSimulated, admin, card, login, logout, removePrinter, simulate } from "./helpers";

const viewer = { username: "e2e-viewer", password: "a viewer password" };
const operator = { username: "e2e-operator", password: "an operator password" };

test("Admins manage users, and each role sees and may do only its own share", async ({ page }) => {
  await login(page);
  const id = await addSimulated(page, "Roles Sim");
  await expect(card(page, "Roles Sim").getByTestId("state")).toHaveText("Idle");
  await simulate(page, id, { action: "start_print", print_file: "roles.gcode", duration_secs: 6000, layers: 10 });

  // An Admin adds a Viewer and an Operator through the Users page.
  await page.getByRole("link", { name: "Users" }).click();
  const form = page.getByRole("form", { name: "Add a user" });
  for (const [who, role] of [
    [viewer, "viewer"],
    [operator, "operator"],
  ] as const) {
    await form.getByLabel("Username").fill(who.username);
    await form.getByLabel("Password").fill(who.password);
    await form.getByLabel("Role").selectOption(role);
    await form.getByRole("button", { name: "Add user" }).click();
    await expect(page.getByLabel(`Role of ${who.username}`)).toHaveValue(role);
  }

  // The last Admin can't demote themselves.
  await page.getByLabel(`Role of ${admin.username}`).selectOption("operator");
  await expect(page.getByRole("alert")).toContainText("last Admin");
  await expect(page.getByLabel(`Role of ${admin.username}`)).toHaveValue("admin");
  await logout(page);

  // A Viewer sees the Printer, but no controls, and the API refuses them too.
  await login(page, viewer);
  const sim = card(page, "Roles Sim");
  await expect(sim.getByTestId("state")).toHaveText(/Preparing|Printing/);
  await expect(sim.getByRole("button")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Users" })).toHaveCount(0);
  const refused = await page.request.post(`/api/printers/${id}/commands`, { data: { command: "pause" } });
  expect(refused.status()).toBe(403);
  await page.getByRole("link", { name: "Printers" }).click();
  await expect(page.getByRole("cell", { name: "Roles Sim" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Add Printer" })).toHaveCount(0);
  await logout(page);

  // An Operator may control the Printer, but not manage Printers or users.
  await login(page, operator);
  await sim.getByRole("button", { name: "Pause" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Pause" }).click();
  await expect(sim.getByTestId("state")).toHaveText(/Paused/);
  await expect(sim.getByText("Simulation")).toHaveCount(0);
  const add = await page.request.post("/api/printers", { data: { name: "Nope", driver: "simulated", settings: {} } });
  expect(add.status()).toBe(403);
  expect((await page.request.get("/api/users")).status()).toBe(403);
  await logout(page);

  // Tidy up as the Admin.
  await login(page);
  await page.getByRole("link", { name: "Users" }).click();
  for (const who of [viewer, operator]) {
    await page.getByRole("button", { name: `Remove ${who.username}` }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Remove" }).click();
    await expect(page.getByLabel(`Role of ${who.username}`)).toHaveCount(0);
  }
  await removePrinter(page, id);
});
