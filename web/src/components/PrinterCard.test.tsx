import { render as rtlRender, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { applyMessage } from "../fleet";
import { printerView } from "../test-fixtures";
import { PrinterCard } from "./PrinterCard";

const render = (ui: ReactElement) => rtlRender(<MemoryRouter>{ui}</MemoryRouter>);

describe("PrinterCard", () => {
  it("shows a printing Job with its progress, layer and time remaining", () => {
    render(
      <PrinterCard
        printer={printerView(
          {},
          {
            state: "printing",
            job: {
              key: "k",
              print_file: "benchy.gcode.3mf",
              progress: 43.2,
              layer: 12,
              total_layers: 40,
              remaining_secs: 3900,
            },
            temperatures: { nozzle: { current: 219.6, target: 220 }, bed: { current: 60, target: 60 } },
            faults: [{ code: "door", severity: "warning", message: "The door is open" }],
          },
        )}
      />,
    );
    expect(screen.getByTestId("state")).toHaveTextContent("Printing · 43%");
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "43");
    expect(screen.getByText("benchy.gcode.3mf")).toBeInTheDocument();
    expect(screen.getByText("12 of 40")).toBeInTheDocument();
    expect(screen.getByText("1 h 05 min")).toBeInTheDocument();
    expect(screen.getByText("220 °C → 220 °C")).toBeInTheDocument();
    const faults = screen.getByRole("list", { name: "Faults" });
    expect(within(faults).getByText(/The door is open/)).toBeInTheDocument();
  });

  it("shows a disconnected Printer's reason and last known state", () => {
    render(
      <PrinterCard
        printer={printerView(
          {
            last_seen: {
              state: "printing",
              progress: 43,
              faults: [{ code: "door", severity: "error", message: "The door is open" }],
              at: "2026-11-03T12:00:00Z",
            },
          },
          { connection: { state: "disconnected", reason: "unreachable" }, state: null },
        )}
      />,
    );
    expect(screen.getByTestId("state")).toHaveTextContent(
      "Disconnected: unreachable (last seen: printing at 43%)",
    );
    expect(screen.getByText("Disconnected")).toHaveClass("badge", "disconnected");
    expect(screen.queryByText("Nozzle")).not.toBeInTheDocument();
    const faults = screen.getByRole("list", { name: "Last known Faults" });
    expect(within(faults).getByText(/The door is open/)).toBeInTheDocument();
  });

  it("hides what a Printer's capabilities don't include", () => {
    render(<PrinterCard printer={printerView({}, { capabilities: [] })} />);
    expect(screen.queryByRole("list", { name: "Filament Slots" })).not.toBeInTheDocument();
    expect(screen.queryByText("Chamber")).not.toBeInTheDocument();
    expect(screen.getByText("Nozzle")).toBeInTheDocument();
  });

  it("shows Filament Slots when the capability is there", () => {
    render(<PrinterCard printer={printerView()} />);
    const slots = screen.getByRole("list", { name: "Filament Slots" });
    expect(within(slots).getByText("PLA 80%")).toBeInTheDocument();
    expect(within(slots).getByText("Empty")).toBeInTheDocument();
  });
});

describe("applyMessage", () => {
  it("replaces everything on a snapshot, then applies deltas", () => {
    const a = printerView({ id: "a", name: "A" });
    const b = printerView({ id: "b", name: "B" });
    let fleet = applyMessage(new Map([["stale", printerView({ id: "stale" })]]), {
      type: "snapshot",
      printers: [a, b],
    });
    expect([...fleet.keys()]).toEqual(["a", "b"]);
    fleet = applyMessage(fleet, { type: "printer", printer: { ...a, name: "A2" } });
    expect(fleet.get("a")?.name).toBe("A2");
    fleet = applyMessage(fleet, { type: "removed", id: "b" });
    expect([...fleet.keys()]).toEqual(["a"]);
  });
});

describe("PrinterCard Jobs", () => {
  it("shows the running Job as an External Job", () => {
    render(
      <PrinterCard
        printer={printerView(
          {
            job: {
              id: 7, printer_id: "p1", external: true, driver_key: "k", print_file: null, plate: null,
              started_at: "2026-11-03T12:00:00Z", ended_at: null, outcome: null, overruled: false,
            },
          },
          { state: "printing", job: { key: "k", progress: 10 } },
        )}
      />,
    );
    expect(screen.getByText("#7 · External Job")).toBeInTheDocument();
    expect(screen.getByText("Unknown")).toBeInTheDocument();
  });

  it("shows the Outcome a Printer is Awaiting Clear after", () => {
    render(
      <PrinterCard
        printer={printerView(
          {
            awaiting_clear: {
              id: 7, printer_id: "p1", external: true, driver_key: "k", print_file: null, plate: null,
              started_at: "2026-11-03T12:00:00Z", ended_at: "2026-11-03T13:00:00Z", outcome: "cancelled", overruled: false,
            },
          },
          { state: "awaiting_clear" },
        )}
      />,
    );
    expect(screen.getByTestId("state")).toHaveTextContent("Awaiting Clear");
    expect(screen.getByTestId("last-outcome")).toHaveTextContent("Last Job #7: Cancelled");
    expect(screen.getByRole("link", { name: "Jobs" })).toHaveAttribute("href", "/printers/p1/jobs");
  });
});

describe("PrinterCard camera", () => {
  it("opens no camera stream until asked", async () => {
    const { default: userEvent } = await import("@testing-library/user-event");
    render(<PrinterCard printer={printerView()} />);
    expect(screen.queryByRole("img", { name: "Bench camera" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByText("Camera"));
    expect(screen.getByRole("img", { name: "Bench camera" })).toHaveAttribute("src", "/api/printers/p1/camera");
  });

  it("has no camera without the capability", () => {
    render(<PrinterCard printer={printerView({}, { capabilities: [] })} />);
    expect(screen.queryByText("Camera")).not.toBeInTheDocument();
  });
});
