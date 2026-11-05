import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "../auth";
import { PrinterCard } from "../components/PrinterCard";
import { printerView } from "../test-fixtures";
import { stubApi, stubFleet } from "../test-server";
import { FleetPage } from "./FleetPage";

afterEach(() => vi.unstubAllGlobals());

describe("the Fleet page's Tag filter", () => {
  it("shows only Printers with any chosen Tag, and shows each card's Tags", async () => {
    stubApi({
      "GET /api/session": { id: 1, username: "v", role: "viewer" },
      "GET /api/tags": [
        { id: 1, name: "Garage", printers: ["a", "b"] },
        { id: 2, name: "Office", printers: ["c"] },
        { id: 3, name: "Spare", printers: [] },
      ],
    });
    stubFleet([
      printerView({ id: "a", name: "Alpha" }),
      printerView({ id: "b", name: "Bravo" }),
      printerView({ id: "c", name: "Charlie" }),
    ]);
    render(
      <MemoryRouter>
        <AuthProvider>
          <FleetPage />
        </AuthProvider>
      </MemoryRouter>,
    );
    const names = () => screen.getAllByRole("article").map((a) => a.getAttribute("aria-label"));
    await screen.findByRole("article", { name: "Charlie" });
    const filter = await screen.findByRole("group", { name: "Filter by Tag" });
    expect(names()).toEqual(["Alpha", "Bravo", "Charlie"]);
    expect(within(screen.getByRole("article", { name: "Alpha" })).getByRole("list", { name: "Tags" })).toHaveTextContent(
      "Garage",
    );

    await userEvent.click(within(filter).getByRole("button", { name: "Office" }));
    expect(names()).toEqual(["Charlie"]);
    await userEvent.click(within(filter).getByRole("button", { name: "Garage" }));
    expect(names()).toEqual(["Alpha", "Bravo", "Charlie"]);
    await userEvent.click(within(filter).getByRole("button", { name: "Office" }));
    expect(names()).toEqual(["Alpha", "Bravo"]);
    expect(within(filter).getByRole("button", { name: "Garage" })).toHaveAttribute("aria-pressed", "true");

    await userEvent.click(within(filter).getByRole("button", { name: "Garage" }));
    await userEvent.click(within(filter).getByRole("button", { name: "Spare" }));
    expect(screen.queryAllByRole("article")).toHaveLength(0);
    expect(screen.getByText("No Printer has those Tags.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show all" }));
    expect(names()).toHaveLength(3);
  });
});

describe("the card", () => {
  it("shows the running Plate's thumbnail, low slots, and a link to the detail", () => {
    const printer = printerView(
      {
        job: {
          id: 5,
          printer_id: "p1",
          external: false,
          driver_key: "k",
          print_file: "tray.gcode.3mf",
          print_file_id: 7,
          plate: 2,
          started_at: "2026-11-04T10:00:00Z",
          ended_at: null,
          outcome: null,
          overruled: false,
        },
      },
      {
        state: "printing",
        job: { key: "k", progress: 30, plate: 2, print_file: "tray.gcode.3mf" },
        filament_slots: [
          { id: "0", label: "Slot 1", filament: { kind: "PLA", colour: "#FFFFFF", remaining_percent: 9 }, low: true },
        ],
      },
    );
    render(
      <MemoryRouter>
        <PrinterCard printer={printer} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("img", { name: "The Plate Bench is printing" })).toHaveAttribute(
      "src",
      "/api/print-files/7/plates/2/thumbnail",
    );
    expect(within(screen.getByRole("list", { name: "Filament Slots" })).getByText("Low")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Details" })).toHaveAttribute("href", "/printers/p1");
  });

  it("falls back to a placeholder for an External Job", () => {
    const printer = printerView({}, { state: "printing", job: { key: "k", progress: 30 } });
    render(
      <MemoryRouter>
        <PrinterCard printer={printer} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("img", { name: "The Plate Bench is printing: no thumbnail" })).toBeInTheDocument();
  });
});
