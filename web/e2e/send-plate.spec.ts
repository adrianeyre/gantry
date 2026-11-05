import { expect, test } from "@playwright/test";
import { addSimulated, card, login, removePrinter, simulate } from "./helpers";
import { emptyLibrary, uploadFixture } from "./library-helpers";

test("an Operator sends a Plate to a Simulated Printer, which prints it as one Job", async ({ page }) => {
  await login(page);
  await emptyLibrary(page);
  const x1 = await addSimulated(page, "Send X1C", { model: "X1C" });
  const p1 = await addSimulated(page, "Send P1S", { model: "P1S" });
  await uploadFixture(page, "three-plates.gcode.3mf");

  await page.getByRole("link", { name: "Library" }).click();
  const tray = page.getByRole("article", { name: "three-plates.gcode.3mf" });
  const printer = tray.getByLabel("Printer for three-plates.gcode.3mf");
  const plate = tray.getByLabel("Plate of three-plates.gcode.3mf");

  // The wrong model is refused, and nothing starts.
  await printer.selectOption({ label: "Send P1S (P1S) · Idle" });
  await plate.selectOption("1");
  await tray.getByRole("button", { name: "Start now" }).click();
  await expect(tray.getByRole("alert")).toContainText("was sliced for Bambu Lab X1 Carbon, but Send P1S's model is P1S");

  // The right model starts one Job, linked to the Print File and Plate.
  await printer.selectOption({ label: "Send X1C (X1C) · Idle" });
  await plate.selectOption("2");
  await tray.getByRole("button", { name: "Start now" }).click();
  await expect(tray.getByRole("status")).toContainText("Started Job #");

  await page.getByRole("link", { name: "Fleet", exact: true }).click();
  const sim = card(page, "Send X1C");
  await expect(sim.getByTestId("state")).toContainText(/Preparing|Printing/);
  await expect(sim.getByText("three-plates.gcode.3mf")).toBeVisible();
  await expect(sim.getByText("External Job")).toHaveCount(0);
  // The card shows the running Plate's thumbnail.
  const thumbnail = sim.getByRole("img", { name: "The Plate Send X1C is printing" });
  await expect(thumbnail).toHaveAttribute("src", /\/plates\/2\/thumbnail$/);

  // It finishes, and a Printer Awaiting Clear refuses a new Job.
  await simulate(page, x1, { action: "set_speed", speed: 1000 });
  await expect(sim.getByTestId("state")).toHaveText("Awaiting Clear", { timeout: 15_000 });
  await page.getByRole("link", { name: "Library" }).click();
  await printer.selectOption({ label: "Send X1C (X1C) · Awaiting Clear" });
  await plate.selectOption("1");
  await tray.getByRole("button", { name: "Start now" }).click();
  await expect(tray.getByRole("alert")).toContainText("Awaiting Clear");

  // Exactly one Job, which isn't External.
  const jobs = (await (await page.request.get(`/api/printers/${x1}/jobs`)).json()) as {
    external: boolean;
    plate: number;
    print_file_id: number;
  }[];
  expect(jobs).toHaveLength(1);
  expect(jobs[0].external).toBe(false);
  expect(jobs[0].plate).toBe(2);

  await removePrinter(page, x1);
  await removePrinter(page, p1);
  await emptyLibrary(page);
});
