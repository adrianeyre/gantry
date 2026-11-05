import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import type { PrintFile } from "../library";
import { printerView } from "../test-fixtures";
import { stubApi, stubFleet } from "../test-server";
import { LibraryPage } from "./LibraryPage";

afterEach(() => vi.unstubAllGlobals());

const tray: PrintFile = {
  id: 7,
  name: "tray.gcode.3mf",
  kind: "gcode_3mf",
  printer_model: "Bambu Lab X1 Carbon",
  model_source: "file",
  slicer: "BambuStudio 01.09.00.70",
  size_bytes: 4346,
  uploaded_at: "2026-11-04T10:00:00Z",
  uploaded_by: "adrian",
  filament_kinds: ["PLA", "PETG"],
  plates: [
    { number: 1, name: "Cubes", estimated_secs: 1200, layers: 50, filaments: [{ kind: "PLA", colour: "#FFFFFF" }], has_thumbnail: true },
    { number: 2, name: null, estimated_secs: null, layers: null, filaments: [], has_thumbnail: false },
  ],
};

function renderPage() {
  stubFleet([printerView({ id: "x1", name: "X1", model: "X1C" })]);
  render(
    <MemoryRouter>
      <AuthProvider>
        <LibraryPage />
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("LibraryPage", () => {
  it("shows each Plate with its thumbnail, or a placeholder when it has none", async () => {
    stubApi({ "GET /api/session": { id: 1, username: "op", role: "operator" }, "GET /api/print-files": [tray] });
    renderPage();
    const file = await screen.findByRole("article", { name: "tray.gcode.3mf" });
    expect(within(file).getByTestId("sliced-for")).toHaveTextContent("Bambu Lab X1 Carbon");
    const plates = within(file).getByRole("list", { name: "Plates of tray.gcode.3mf" });
    expect(within(plates).getAllByRole("listitem")).toHaveLength(2);
    expect(within(plates).getByRole("img", { name: "Plate 1 of tray.gcode.3mf" })).toHaveAttribute(
      "src",
      "/api/print-files/7/plates/1/thumbnail",
    );
    expect(within(plates).getByRole("img", { name: "Plate 2 of tray.gcode.3mf: no thumbnail" })).toBeInTheDocument();
    expect(within(file).getByRole("link", { name: "3D model" })).toHaveAttribute("href", "/library/7/model");
  });

  it("uploads a file and shows why an unsliced one is refused", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "op", role: "operator" },
      "GET /api/print-files": [],
      "POST /api/print-files": [
        422,
        { error: "Some fields need attention", fields: [{ field: "file", message: "A .stl file is a 3D model, not a Print File: it needs slicing first." }] },
      ],
    });
    renderPage();
    const input = await screen.findByLabelText("Upload Print Files");
    await userEvent.upload(input, new File(["solid x"], "part.stl"));
    expect(await screen.findByRole("alert")).toHaveTextContent("part.stl: A .stl file is a 3D model");
    const call = fetch.mock.calls.find(([path]) => String(path).startsWith("/api/print-files?"));
    expect(call?.[0]).toBe("/api/print-files?name=part.stl");
  });

  it("sends a Plate to a Printer", async () => {
    const fetch = stubApi({
      "GET /api/session": { id: 1, username: "op", role: "operator" },
      "GET /api/print-files": [tray],
      "POST /api/printers/x1/start": [201, { id: 42 }],
    });
    renderPage();
    await screen.findByRole("article", { name: "tray.gcode.3mf" });
    await screen.findByRole("option", { name: /X1 \(X1C\)/ });
    await userEvent.selectOptions(screen.getByLabelText("Printer for tray.gcode.3mf"), "x1");
    await userEvent.selectOptions(screen.getByLabelText("Plate of tray.gcode.3mf"), "1");
    await userEvent.click(screen.getByRole("button", { name: "Start now" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Started Job #42 on X1.");
    const start = fetch.mock.calls.find(([path]) => path === "/api/printers/x1/start");
    expect(JSON.parse(String(start?.[1]?.body))).toEqual({ print_file_id: 7, plate: 1 });
  });

  it("offers a Viewer nothing to change", async () => {
    stubApi({ "GET /api/session": { id: 1, username: "v", role: "viewer" }, "GET /api/print-files": [tray] });
    renderPage();
    await screen.findByRole("article", { name: "tray.gcode.3mf" });
    expect(screen.queryByLabelText("Upload Print Files")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start now" })).not.toBeInTheDocument();
  });
});
