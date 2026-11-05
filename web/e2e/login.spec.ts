import { expect, test } from "@playwright/test";
import { admin, login } from "./helpers";

test("a visitor without a session is sent to the login page", async ({ page, request }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);
  expect((await request.get("/api/session")).status()).toBe(401);
});

test("a wrong password is refused", async ({ page }) => {
  await page.goto("/login");
  await page.getByLabel("Username").fill(admin.username);
  await page.getByLabel("Password").fill("not the password");
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText("wrong username or password");
});

test("logs in and out", async ({ page, context }) => {
  await login(page);
  await expect(page.getByText(admin.username)).toBeVisible();

  const [cookie] = await context.cookies();
  expect(cookie.name).toBe("gantry_session");
  expect(cookie.httpOnly).toBe(true);
  expect(cookie.sameSite).toBe("Strict");

  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login$/);

  // The session ended server side: the old cookie no longer works.
  const stale = await page.request.get("/api/session", {
    headers: { cookie: `${cookie.name}=${cookie.value}` },
  });
  expect(stale.status()).toBe(401);
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);
});

test("the Fleet already has Simulated Printers in it", async ({ page }) => {
  await login(page);
  await expect(page.getByRole("article", { name: "Seed Simulated A" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Seed Simulated B" })).toBeVisible();
});
