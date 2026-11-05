import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter } from "./helpers";

test("the camera grid shows every Printer with a camera, streaming live", async ({ page, browser }) => {
  await login(page);
  const withCamera = await addSimulated(page, "Cam Sim");
  const without = await addSimulated(page, "No Cam Sim", { camera: false });
  await expect(card(page, "No Cam Sim").getByTestId("state")).toHaveText("Idle");
  await expect(card(page, "Cam Sim").getByTestId("state")).toHaveText("Idle");

  // On a card, the camera is off until asked for.
  const sim = card(page, "Cam Sim");
  await expect(sim.getByRole("img")).toHaveCount(0);
  await sim.getByText("Camera").click();
  const cardImage = sim.getByRole("img", { name: "Cam Sim camera" });
  await expect.poll(() => cardImage.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(320);
  await expect(card(page, "No Cam Sim").getByText("Camera")).toHaveCount(0);

  await page.getByRole("link", { name: "Cameras" }).click();
  await expect(page.getByRole("heading", { name: "Cameras" })).toBeVisible();
  for (const name of ["Cam Sim", "Seed Simulated A", "Seed Simulated B"]) {
    const image = page.getByRole("figure", { name }).getByRole("img");
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(320);
  }
  await expect(page.getByRole("figure", { name: "No Cam Sim" })).toHaveCount(0);

  // From this machine the server picks full quality. Picking Low asks for
  // it on every camera, and the server says it sent it.
  const camImage = page.getByRole("figure", { name: "Cam Sim" }).getByRole("img");
  const picker = page.getByRole("combobox", { name: "Cam Sim camera quality" });
  await expect(picker).toHaveValue("auto");
  await expect(camImage).toHaveAttribute("src", `/api/printers/${withCamera}/camera`);
  const lowSent = page.waitForResponse((r) => r.url().endsWith(`/api/printers/${withCamera}/camera?quality=low`));
  await picker.selectOption({ label: "Low" });
  expect((await lowSent).headers()["x-gantry-camera-quality"]).toBe("low");
  await expect(camImage).toHaveAttribute("src", `/api/printers/${withCamera}/camera?quality=low`);
  await expect(page.getByRole("figure", { name: "Seed Simulated A" }).getByRole("img")).toHaveAttribute(
    "src",
    /\?quality=low$/,
  );
  // The simulated camera is already small, so it isn't upscaled.
  await expect.poll(() => camImage.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(320);

  // The choice is remembered in this browser.
  // The streams never finish loading, so only wait for the document.
  await page.reload({ waitUntil: "domcontentloaded" });
  await expect(picker).toHaveValue("low");
  await expect(camImage).toHaveAttribute("src", `/api/printers/${withCamera}/camera?quality=low`);
  await picker.selectOption({ label: "Medium" });
  await expect(camImage).toHaveAttribute("src", `/api/printers/${withCamera}/camera?quality=medium`);
  await expect.poll(() => camImage.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(320);
  await picker.selectOption({ label: "Auto" });
  await expect(camImage).toHaveAttribute("src", `/api/printers/${withCamera}/camera`);

  // An unknown quality is refused.
  const unknown = await page.request.get(`/api/printers/${withCamera}/camera?quality=ultra`);
  expect(unknown.status()).toBe(422);

  // Camera endpoints need a login.
  const anonymous = await browser.newContext();
  const refused = await anonymous.request.get(`/api/printers/${withCamera}/camera`);
  expect(refused.status()).toBe(401);
  await anonymous.close();

  await removePrinter(page, withCamera);
  await removePrinter(page, without);
});
