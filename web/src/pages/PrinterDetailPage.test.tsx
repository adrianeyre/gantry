import { render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import { printerView } from "../test-fixtures";
import { stubApi, stubFleet } from "../test-server";
import { PrinterDetailPage } from "./PrinterDetailPage";

afterEach(() => vi.unstubAllGlobals());

function renderPage() {
  render(
    <MemoryRouter initialEntries={["/printers/x1"]}>
      <AuthProvider>
        <Routes>
          <Route path="/printers/:id" element={<PrinterDetailPage />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>,
  );
}

describe("PrinterDetailPage", () => {
  it("groups Faults by severity and marks low Filament Slots", async () => {
    stubApi({
      "GET /api/settings": { low_filament_percent: 15 },
      "GET /api/session": { id: 1, username: "v", role: "viewer" },
      "GET /api/printers/x1": { id: "x1", auto_start: true, auto_clear: false },
    });
    stubFleet([
      printerView(
        { id: "x1", name: "X1", model: "X1C" },
        {
          faults: [
            { code: "door", severity: "warning", message: "The door is open" },
            { code: "clog", severity: "error", message: "Nozzle clogged" },
            { code: "fan", severity: "warning", message: "Fan speed low" },
          ],
          filament_slots: [
            { id: "0", label: "Slot 1", filament: { kind: "PLA", colour: "#FFFFFF", remaining_percent: 80 } },
            { id: "1", label: "Slot 2", filament: { kind: "PETG", colour: "#D33A2C", remaining_percent: 10 }, low: true },
            { id: "2", label: "Slot 3" },
          ],
        },
      ),
    ]);
    renderPage();
    expect(await screen.findByRole("heading", { name: "Errors (1)" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Warnings (2)" })).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Errors" })).getByText("Nozzle clogged")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Warnings" })).getAllByRole("listitem")).toHaveLength(2);
    // Errors come before warnings.
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual(["Errors (1)", "Warnings (2)"]);

    const table = screen.getByRole("table", { name: "Filament Slots" });
    expect(within(screen.getByTestId("slot-1")).getByText("Low")).toBeInTheDocument();
    expect(within(screen.getByTestId("slot-0")).queryByText("Low")).not.toBeInTheDocument();
    expect(within(table).getByText("Empty")).toBeInTheDocument();
    expect(await screen.findByText("A slot with 15% or less left is marked low.")).toBeInTheDocument();
    expect(screen.getByTestId("model")).toHaveTextContent("X1C");
  });

  it("lets an Admin turn Auto-start and Auto-Clear on and off, and shows a Viewer how they are set", async () => {
    const fetch = stubApi({
      "GET /api/settings": { low_filament_percent: 15 },
      "GET /api/session": { id: 1, username: "a", role: "admin" },
      "GET /api/printers/x1": { id: "x1", auto_start: true, auto_clear: false },
      "PUT /api/printers/x1/automation": { auto_start: true, auto_clear: true },
    });
    stubFleet([printerView({ id: "x1", name: "X1" })]);
    renderPage();
    const autoStart = await screen.findByRole("checkbox", { name: /Auto-start/ });
    const autoClear = screen.getByRole("checkbox", { name: /Auto-Clear/ });
    expect(autoStart).toBeChecked();
    expect(autoClear).not.toBeChecked();
    await userEvent.click(autoClear);
    expect(await screen.findByRole("checkbox", { name: /Auto-Clear/, checked: true })).toBeInTheDocument();
    const put = fetch.mock.calls.find(([url, init]) => String(url).endsWith("/automation") && init?.method === "PUT");
    expect(JSON.parse(String(put?.[1]?.body))).toEqual({ auto_clear: true });
  });

  it("shows a Viewer the automation but not to change", async () => {
    stubApi({
      "GET /api/settings": { low_filament_percent: 15 },
      "GET /api/session": { id: 1, username: "v", role: "viewer" },
      "GET /api/printers/x1": { id: "x1", auto_start: false, auto_clear: true },
    });
    stubFleet([printerView({ id: "x1", name: "X1" })]);
    renderPage();
    const autoClear = await screen.findByRole("checkbox", { name: /Auto-Clear/ });
    expect(autoClear).toBeChecked();
    expect(autoClear).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: /Auto-start/ })).not.toBeChecked();
  });
});
