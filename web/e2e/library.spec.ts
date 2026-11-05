import { expect, test } from "@playwright/test";
import type { Locator } from "@playwright/test";
import { login } from "./helpers";
import { emptyLibrary, fixture } from "./library-helpers";

/** Whether an <img> really loaded a picture. */
async function loaded(img: Locator) {
  await expect
    .poll(() => img.evaluate((el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth))
    .toBeGreaterThan(0);
}

test("the library takes sliced Print Files, shows their Plates and thumbnails, and refuses models", async ({
  page,
}) => {
  await login(page);
  await emptyLibrary(page);
  await page.getByRole("link", { name: "Library" }).click();
  await expect(page.getByRole("heading", { name: "Library" })).toBeVisible();
  const upload = page.getByLabel("Upload Print Files");

  // A multi-plate .gcode.3mf shows the right number of Plates.
  await upload.setInputFiles(fixture("three-plates.gcode.3mf"));
  await expect(page.getByRole("status")).toHaveText("Added three-plates.gcode.3mf: 3 Plates.");
  const tray = page.getByRole("article", { name: "three-plates.gcode.3mf" });
  await expect(tray.getByTestId("sliced-for")).toHaveText("Bambu Lab X1 Carbon");
  const plates = tray.getByRole("list", { name: "Plates of three-plates.gcode.3mf" });
  await expect(plates.getByRole("listitem")).toHaveCount(3);
  await expect(plates.getByText("Plate 1: Cubes")).toBeVisible();
  // Plates 1 and 2 have thumbnails; plate 3 falls back to a placeholder.
  await loaded(plates.getByRole("img", { name: "Plate 1 of three-plates.gcode.3mf" }));
  await expect(plates.getByRole("img", { name: "Plate 3 of three-plates.gcode.3mf: no thumbnail" })).toBeVisible();

  // A plain .gcode is one Plate, with its header thumbnail, and its model can be set by hand.
  await upload.setInputFiles(fixture("benchy-prusaslicer.gcode"));
  await expect(page.getByRole("status")).toHaveText("Added benchy-prusaslicer.gcode: 1 Plate.");
  const benchy = page.getByRole("article", { name: "benchy-prusaslicer.gcode" });
  await expect(benchy.getByRole("list", { name: "Plates of benchy-prusaslicer.gcode" }).getByRole("listitem")).toHaveCount(1);
  await loaded(benchy.getByRole("img", { name: "Plate 1 of benchy-prusaslicer.gcode" }));
  await expect(benchy.getByTestId("sliced-for")).toHaveText("MK4");
  await benchy.getByLabel("Model benchy-prusaslicer.gcode was sliced for").fill("MK4S");
  await benchy.getByRole("button", { name: "Set model" }).click();
  await expect(benchy.getByTestId("sliced-for")).toHaveText("MK4S");

  // Unsliced formats are refused with a clear message.
  await upload.setInputFiles(fixture("part.stl"));
  await expect(page.getByRole("alert")).toContainText("part.stl: A .stl file is a 3D model, not a Print File: it needs slicing first.");
  await upload.setInputFiles(fixture("unsliced.3mf"));
  await expect(page.getByRole("alert")).toContainText("has no sliced Plates");
  await expect(page.getByRole("article")).toHaveCount(2);

  // The library is stored: the same after a reload.
  await page.reload();
  await expect(page.getByRole("article")).toHaveCount(2);

  // The 3D model of a .gcode.3mf, coloured by filament.
  await tray.getByRole("link", { name: "3D model" }).click();
  await expect(page.getByRole("heading", { name: "three-plates.gcode.3mf: 3D model" })).toBeVisible();
  await expect(page.getByTestId("model-canvas")).toBeVisible();
  const legend = page.getByRole("list", { name: "Objects" });
  await expect(legend).toContainText("Cube");
  await expect(legend).toContainText("Pyramid");

  await emptyLibrary(page);
});
