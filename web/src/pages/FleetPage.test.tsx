import { render as rtlRender, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { printerView } from "../test-fixtures";
import { FleetCards } from "./FleetPage";

const render = (ui: ReactElement) => rtlRender(<MemoryRouter>{ui}</MemoryRouter>);

describe("FleetCards", () => {
  const simulated = printerView({ id: "s", name: "Simulated One", driver: "simulated" });
  const other = printerView({ id: "o", name: "Other One", driver: "bambu" });

  it("gives only Simulated Printers a simulation panel", () => {
    render(<FleetCards printers={[simulated, other]} role="admin" />);
    expect(within(screen.getByRole("article", { name: "Simulated One" })).getByText("Simulation")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "Other One" })).queryByText("Simulation")).not.toBeInTheDocument();
  });

  it("shows the panel only to Admins", () => {
    render(<FleetCards printers={[simulated]} role="operator" />);
    expect(screen.queryByText("Simulation")).not.toBeInTheDocument();
  });

  it("offers a Viewer no controls at all", () => {
    const printing = printerView(
      { id: "p", name: "Busy One" },
      { state: "printing", job: { key: "k", progress: 10 } },
    );
    const awaiting = printerView(
      {
        id: "a",
        name: "Waiting One",
        awaiting_clear: {
          id: 7,
          printer_id: "a",
          external: true,
          driver_key: "k",
          print_file: null,
          plate: null,
          started_at: "2026-11-04T10:00:00Z",
          ended_at: "2026-11-04T11:00:00Z",
          outcome: "succeeded",
          overruled: false,
        },
      },
      { state: "awaiting_clear" },
    );
    const { unmount } = render(<FleetCards printers={[printing, awaiting]} role="viewer" />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    unmount();
    render(<FleetCards printers={[printing, awaiting]} role="operator" />);
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mark Clear" })).toBeInTheDocument();
  });

  it("hides controls and says why when firmware withdraws them", () => {
    const blocked = printerView(
      { id: "b", name: "Blocked One", driver: "bambu" },
      {
        state: "printing",
        job: { key: "k", progress: 30 },
        capabilities: ["filament_slots"],
        control_withdrawn: "the printer's firmware refused local control. Turn on Developer Mode.",
      },
    );
    render(<FleetCards printers={[blocked]} role="admin" />);
    const card = screen.getByRole("article", { name: "Blocked One" });
    expect(within(card).queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(within(card).queryByText("Controls")).not.toBeInTheDocument();
    expect(within(card).getByTestId("control-withdrawn")).toHaveTextContent("Developer Mode");
    expect(within(card).getByTestId("state")).toHaveTextContent("Printing");
  });
});
