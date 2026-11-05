import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
// @ts-expect-error -- a plain .mjs module shared with the launcher script.
import { ADMIN } from "./config.mjs";

export const admin = ADMIN as { username: string; password: string };

/** Logs in through the login page, as the seeded Admin unless told otherwise. */
export async function login(page: Page, who: { username: string; password: string } = admin) {
  await page.goto("/");
  await expect(page).toHaveURL(/\/login/);
  await page.getByLabel("Username").fill(who.username);
  await page.getByLabel("Password").fill(who.password);
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Fleet" })).toBeVisible();
}

/** Logs out through the top bar. */
export async function logout(page: Page) {
  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login/);
}

/** Adds a Simulated Printer through the API (as the logged-in page) and returns its id. */
export async function addSimulated(page: Page, name: string, settings: Record<string, unknown> = {}) {
  const response = await page.request.post("/api/printers", {
    data: { name, driver: "simulated", settings },
  });
  expect(response.status(), await response.text()).toBe(201);
  return ((await response.json()) as { id: string }).id;
}

/** Removes a Printer through the API. */
export async function removePrinter(page: Page, id: string) {
  await page.request.delete(`/api/printers/${id}`);
}

/** A Printer's card on the Fleet page. */
export function card(page: Page, name: string) {
  return page.getByRole("article", { name, exact: true });
}

/** Drives a Simulated Printer through the simulation endpoint. */
export async function simulate(page: Page, id: string, action: Record<string, unknown>) {
  const response = await page.request.post(`/api/printers/${id}/simulation`, { data: action });
  expect(response.status(), await response.text()).toBe(204);
}
