import { createHmac } from "node:crypto";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
// @ts-expect-error -- a plain .mjs module shared with the launcher script.
import { BASE_URL } from "./config.mjs";
import { admin, login, logout } from "./helpers";

/** RFC 4648 base32, as authenticator apps take a TOTP key. */
function fromBase32(text: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of text.replace(/[\s=]/g, "").toUpperCase()) {
    const i = alphabet.indexOf(c);
    if (i < 0) throw new Error(`not base32: ${c}`);
    bits += i.toString(2).padStart(5, "0");
  }
  const bytes: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(parseInt(bits.slice(i, i + 8), 2));
  return Buffer.from(bytes);
}

/** RFC 6238 TOTP: HMAC-SHA-1, 30 s steps, 6 digits. */
function totp(secret: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const hash = createHmac("sha1", fromBase32(secret)).update(counter).digest();
  const offset = hash[hash.length - 1] & 0x0f;
  const value = (hash.readUInt32BE(offset) & 0x7fffffff) % 1_000_000;
  return value.toString().padStart(6, "0");
}

const step = () => Math.floor(Date.now() / 1000 / 30);

/** Makes a user through the API as the seeded Admin, then logs out. */
async function makeUser(page: Page, who: { username: string; password: string }, role = "operator") {
  await login(page);
  const response = await page.request.post("/api/users", { data: { ...who, role } });
  expect(response.status(), await response.text()).toBe(201);
  const id = ((await response.json()) as { id: number }).id;
  await logout(page);
  return id;
}

async function removeUser(page: Page, id: number) {
  await login(page);
  await page.request.delete(`/api/users/${id}`);
}

test("an API token made on the Account page lists the Printers with a Bearer header, until it is revoked", async ({
  page,
  request,
}) => {
  await login(page);
  await page.getByRole("link", { name: "Account" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  const form = page.getByRole("form", { name: "Make an API token" });
  await form.getByLabel("Token name").fill("e2e token");
  await form.getByLabel("Expires after").selectOption({ label: "30 days" });
  await form.getByRole("button", { name: "Make token" }).click();

  const shown = page.getByRole("region", { name: "New API token" });
  await expect(shown).toContainText("can't show it again");
  const token = (await shown.getByTestId("secret-value").textContent())!.trim();
  expect(token).toMatch(/^gantry_/);
  await expect(shown.getByTestId("curl-example")).toHaveText(
    `curl -H "Authorization: Bearer ${token}" ${BASE_URL as string}/api/printers`,
  );

  // `request` has no session cookie: only the token lets it in.
  expect((await request.get("/api/printers")).status()).toBe(401);
  const listed = await request.get("/api/printers", { headers: { Authorization: `Bearer ${token}` } });
  expect(listed.status(), await listed.text()).toBe(200);
  const names = ((await listed.json()) as { name: string }[]).map((p) => p.name);
  expect(names).toContain("Seed Simulated A");
  const session = await request.get("/api/session", { headers: { Authorization: `Bearer ${token}` } });
  expect(((await session.json()) as { api_token?: string }).api_token).toBe("e2e token");

  // The token is listed by its prefix, never in full.
  const table = page.getByRole("table", { name: "Your API tokens" });
  await expect(table.getByRole("cell", { name: "e2e token", exact: true })).toBeVisible();
  await expect(table).not.toContainText(token);

  await table.getByRole("button", { name: "Revoke the token e2e token" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Revoke" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Revoked e2e token." })).toBeVisible();
  const refused = await request.get("/api/printers", { headers: { Authorization: `Bearer ${token}` } });
  expect(refused.status()).toBe(401);
});

test("TOTP set up on the Account page is asked for at the next login", async ({ page }) => {
  const who = { username: "e2e-totp", password: "a totp user password" };
  const id = await makeUser(page, who);

  await login(page, who);
  await page.getByRole("link", { name: "Account" }).click();
  await page.getByRole("button", { name: "Set up two-factor authentication" }).click();
  await page.getByLabel("Your password").fill(who.password);
  await page.getByRole("button", { name: "Set up", exact: true }).click();
  await expect(page.getByRole("img", { name: "A QR code that adds Gantry to an authenticator app" })).toBeVisible();
  const secret = (await page.getByTestId("totp-secret").textContent())!.replace(/\s/g, "");
  await page.getByLabel("The 6-digit code the app shows").fill(totp(secret, step()));
  await page.getByRole("button", { name: "Turn on two-factor authentication" }).click();
  const codes = page.getByRole("region", { name: "New recovery codes" });
  await expect(codes).toContainText("Gantry shows them only now");
  await page.getByRole("button", { name: "I've saved my recovery codes" }).click();
  await expect(page.getByTestId("totp-state")).toContainText("On since");
  await expect(page.getByTestId("totp-state")).toContainText("10 recovery codes left.");
  await logout(page);

  // The password alone isn't enough now.
  await page.getByLabel("Username").fill(who.username);
  await page.getByLabel("Password").fill(who.password);
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  const code = page.getByLabel("Code");
  await expect(code).toBeFocused();
  await code.fill("000000");
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("that code isn't right");
  // The code just used to turn it on can't be used again, so this is the
  // next step's: the server accepts one step either side of now.
  await page.getByLabel("Code").fill(totp(secret, step() + 1));
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  // Back on the page they logged out from.
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  await expect(page.getByTestId("totp-state")).toContainText("On since");
  await logout(page);

  await removeUser(page, id);
});

test("an Admin requiring TOTP sends a user without it to set it up, and they can log out", async ({ page }) => {
  const who = { username: "e2e-enrol", password: "an enrolling user password" };
  const id = await makeUser(page, who, "viewer");
  await login(page);
  expect((await page.request.put("/api/settings", { data: { require_totp: true } })).status()).toBe(200);
  // The Admin hasn't set it up either, so the next page is the setup.
  await page.goto("/printers");
  await expect(page.getByRole("heading", { level: 1, name: "Set up two-factor authentication" })).toBeVisible();
  await expect(page.getByText(/An Admin requires two-factor authentication for every account/)).toBeVisible();
  // Until then even turning the requirement off is refused, so log out.
  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login/);

  await page.getByLabel("Username").fill(who.username);
  await page.getByLabel("Password").fill(who.password);
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Set up two-factor authentication" })).toBeVisible();
  expect((await page.request.get("/api/printers")).status()).toBe(403);
  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login/);

  // Tidy up: the Admin sets TOTP up, turns the requirement off, then TOTP off again.
  await page.getByLabel("Username").fill(admin.username);
  await page.getByLabel("Password").fill(admin.password);
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await page.getByLabel("Your password").fill(admin.password);
  await page.getByRole("button", { name: "Set up", exact: true }).click();
  const secret = (await page.getByTestId("totp-secret").textContent())!.replace(/\s/g, "");
  const used = step();
  await page.getByLabel("The 6-digit code the app shows").fill(totp(secret, used));
  await page.getByRole("button", { name: "Turn on two-factor authentication" }).click();
  await page.getByRole("button", { name: "I've saved my recovery codes" }).click();
  // Through to the page asked for before.
  await expect(page.getByRole("heading", { level: 1, name: "Printers" })).toBeVisible();
  expect((await page.request.put("/api/settings", { data: { require_totp: false } })).status()).toBe(200);
  const off = await page.request.post("/api/account/totp/disable", {
    data: { password: admin.password, code: totp(secret, used + 1) },
  });
  expect(off.status(), await off.text()).toBe(204);
  await page.request.delete(`/api/users/${id}`);
});

test("a passkey added on the Account page logs in (a virtual authenticator)", async ({ page }) => {
  // Browsers only use passkeys on a host name, not an IP address.
  const origin = (BASE_URL as string).replace("127.0.0.1", "localhost");
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
    },
  });

  await page.goto(`${origin}/login`);
  await page.getByLabel("Username").fill(admin.username);
  await page.getByLabel("Password").fill(admin.password);
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Fleet" })).toBeVisible();
  await page.getByRole("link", { name: "Account" }).click();
  await page.getByLabel("Passkey name").fill("e2e passkey");
  await page.getByRole("button", { name: "Add a passkey" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Added the passkey e2e passkey." })).toBeVisible();
  await expect(page.getByRole("table", { name: "Passkeys" }).getByRole("cell", { name: "e2e passkey", exact: true })).toBeVisible();
  const { credentials } = await cdp.send("WebAuthn.getCredentials", { authenticatorId });
  expect(credentials).toHaveLength(1);
  expect(credentials[0].isResidentCredential).toBe(true);

  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login/);
  // No username typed: the passkey says who it is.
  await page.getByRole("button", { name: "Log in with a passkey" }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Account" })).toBeVisible();
  await expect(page.getByRole("banner").getByText(admin.username)).toBeVisible();

  // Removed, it no longer logs in.
  await page.getByRole("button", { name: "Remove the passkey e2e passkey" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove" }).click();
  await expect(page.getByText("No passkeys yet.")).toBeVisible();
  await page.getByRole("button", { name: "Log out" }).click();
  await page.getByRole("button", { name: "Log in with a passkey" }).click();
  await expect(page.getByRole("alert")).toContainText("Gantry doesn't know this passkey");

  await cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId });
});
